using System;
using System.Runtime.InteropServices;
using Cmux.Core;
using Cmux.Interop;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;
using Windows.UI.Core;

namespace Cmux.Controls;

/// <summary>
/// One terminal surface (plan §8, Phase 1; Phase 2 U2): hosts the <see cref="SwapChainPanel"/> the
/// Rust engine renders into, owns the <see cref="EngineHandle"/> lifecycle, and forwards
/// keyboard/pointer input across the FFI. Implements <see cref="ISurface"/> so the Phase-2
/// <c>SurfaceManager</c> can own its create/activate/focus/teardown lifecycle from the model plane.
///
/// Responsibilities by unit:
/// <list type="bullet">
///   <item>Lifecycle — create the engine, attach the panel <b>exactly once</b> (KTD9/R10),
///         spawn the shell, and tear down cleanly and <b>idempotently</b>, driven explicitly by
///         the surface manager rather than by XAML <c>Unloaded</c> (which fires on re-parent).</item>
///   <item>U7 input — KeyDown/KeyUp → <c>send_key</c> (control keys), CharacterReceived →
///         <c>send_text</c> (layout/IME/dead-key text), pointer/wheel → <c>send_mouse</c>/<c>send_scroll</c>.</item>
///   <item>U9 resize/DPI — SizeChanged + CompositionScaleChanged → physical pixels → <c>resize</c>.</item>
/// </list>
/// </summary>
public sealed partial class TerminalPane : UserControl, ISurface
{
    private EngineHandle? _engine;

    // The AddRef'd ISwapChainPanelNative* handed to the engine. Released after the engine is
    // destroyed (which drops wgpu's own ref) — see <see cref="Shutdown"/>.
    private IntPtr _panelNative;

    // Attach-once / shutdown-once guard (KTD9/R10): re-parenting fires Loaded/Unloaded again, but
    // the panel must attach exactly once and the engine tear down exactly once.
    private readonly SurfaceLifecycleGuard _lifecycle = new();
    private bool _started;

    private readonly SurfaceId _id;
    private readonly string? _cwd;
    private readonly string? _cmdline;

    /// <summary>This surface's stable id (<see cref="ISurface.Id"/>).</summary>
    public SurfaceId Id => _id;

    /// <summary>Raised (on the UI thread) when the shell sets the window/tab title (OSC 0/2).</summary>
    public event Action<string>? TitleChanged;

    /// <summary>Raised (on the UI thread) when the shell process exits, with its exit code.</summary>
    public event Action<long>? ChildExited;

    /// <summary>Parameterless ctor kept for the XAML loader; real surfaces use the id ctor.</summary>
    public TerminalPane() : this(default, null, null)
    {
    }

    /// <summary>
    /// Create a surface bound to <paramref name="id"/>, optionally spawning in <paramref name="cwd"/>
    /// with a specific <paramref name="cmdline"/> (empty → the engine's default shell).
    /// </summary>
    public TerminalPane(SurfaceId id, string? cwd = null, string? cmdline = null)
    {
        _id = id;
        _cwd = cwd;
        _cmdline = cmdline;

        this.InitializeComponent();
        this.Loaded += OnLoaded;
        // NOTE: deliberately NOT wiring Unloaded → Shutdown. Tree restructuring re-parents this
        // control (Unloaded then Loaded); teardown is driven explicitly by the SurfaceManager so a
        // re-parent never destroys a live shell (KTD9/R10).

        Panel.SizeChanged += OnPanelSizeChanged;
        Panel.CompositionScaleChanged += OnCompositionScaleChanged;

        Panel.KeyDown += OnKeyDown;
        Panel.KeyUp += OnKeyUp;
        Panel.CharacterReceived += OnCharacterReceived;

        Panel.PointerPressed += OnPointerPressed;
        Panel.PointerMoved += OnPointerMoved;
        Panel.PointerReleased += OnPointerReleased;
        Panel.PointerWheelChanged += OnPointerWheelChanged;
    }

    // ---- Lifecycle ---------------------------------------------------------------------------

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        // Attach exactly once: a re-parent's second Loaded (or a post-shutdown one) is a no-op
        // so we neither recreate the engine nor re-bind the wgpu surface (KTD9/R10).
        if (!_lifecycle.TryAttach())
        {
            return;
        }

        _engine = EngineHandle.CreateDefault();
        _engine.SetEventCallback(this.DispatcherQueue, OnHostEvent);

        // QI the panel for ISwapChainPanelNative and hand the raw pointer to the engine's wgpu
        // surface (plan §5.2). We own one ref until teardown.
        _panelNative = SwapChainPanelNativeInterop.GetNativePointer(Panel);
        _engine.AttachSwapChainPanel(_panelNative);

        Configure();
    }

    /// <summary>
    /// Tear down the engine and release the panel COM ref. Idempotent (<see cref="ISurface.Shutdown"/>).
    /// Driven explicitly by the surface manager / window <c>Closed</c> handler — never by
    /// <c>Unloaded</c> — so the render thread stops before the panel is disposed (plan §7.2) and a
    /// re-parent cannot tear down a live shell (KTD9/R10).
    /// </summary>
    public void Shutdown()
    {
        if (!_lifecycle.TryShutdown())
        {
            return;
        }

        try
        {
            // Destroys the engine: closes the pseudoconsole, joins the reader, drops the wgpu
            // surface (releasing wgpu's ref on the panel) and the render thread.
            _engine?.Dispose();
        }
        finally
        {
            _engine = null;
            if (_panelNative != IntPtr.Zero)
            {
                Marshal.Release(_panelNative);
                _panelNative = IntPtr.Zero;
            }
        }
    }

    /// <summary>
    /// Composite this surface or collapse it out of the composition tree (<see cref="ISurface.SetActive"/>).
    /// Inactive surfaces are <see cref="Visibility.Collapsed"/> so they cost no composition while
    /// their engine keeps the shell alive in the background (R3/R11, KTD2).
    /// </summary>
    public void SetActive(bool active) =>
        this.Visibility = active ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Give the GPU panel programmatic keyboard focus (<see cref="ISurface.FocusSurface"/>, R8).</summary>
    public void FocusSurface() => Panel.Focus(FocusState.Programmatic);

    /// <summary>
    /// Push the current surface geometry to the engine and, on the first valid size, spawn the
    /// shell. The engine is authoritative for cols/rows (plan §6 decision): we pass physical
    /// pixels + DPI and 0/0 for cols/rows, and it derives the grid from its own cell metrics.
    /// </summary>
    private void Configure()
    {
        if (!_lifecycle.IsAttached || _engine is null)
        {
            return;
        }

        (uint pw, uint ph, float scale) = PhysicalSize();
        if (pw == 0 || ph == 0)
        {
            return; // layout not settled yet; a SizeChanged will call back.
        }

        try
        {
            _engine.Resize(0, 0, pw, ph, scale);

            if (!_started)
            {
                // Empty cmdline → engine's default shell (pwsh → powershell → cmd); cwd null → inherit.
                _engine.SpawnShell(_cmdline ?? string.Empty, _cwd);
                _started = true; // only after a successful spawn, so a failure retries next call
                Panel.Focus(FocusState.Programmatic);
            }
        }
        catch (Exception ex)
        {
            // A resize/DPI reconfigure (or first spawn) can transiently fail — e.g. the GPU
            // surface churns while the window crosses monitors or changes scale. Log it and keep
            // the app alive; the next SizeChanged/CompositionScaleChanged retries. Letting this
            // throw out of a UI-thread event handler would crash the process (STATUS_STOWED_EXCEPTION).
            App.LogError("TerminalPane.Configure", ex);
        }
    }

    /// <summary>Panel size in physical pixels (effective DIPs × composition scale) + the scale.</summary>
    private (uint widthPx, uint heightPx, float scale) PhysicalSize()
    {
        float sx = Panel.CompositionScaleX > 0 ? Panel.CompositionScaleX : 1.0f;
        float sy = Panel.CompositionScaleY > 0 ? Panel.CompositionScaleY : 1.0f;
        uint pw = (uint)Math.Round(Panel.ActualWidth * sx);
        uint ph = (uint)Math.Round(Panel.ActualHeight * sy);
        return (pw, ph, sx);
    }

    // ---- U9: resize / DPI --------------------------------------------------------------------

    private void OnPanelSizeChanged(object sender, SizeChangedEventArgs e) => Configure();

    private void OnCompositionScaleChanged(SwapChainPanel sender, object args) => Configure();

    // ---- U7: keyboard ------------------------------------------------------------------------

    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (_engine is null)
        {
            return;
        }
        KeyModifiers mods = CurrentModifiers();

        // Clipboard chords are handled by the host, not forwarded to the shell (plan §8 U8).
        const KeyModifiers ctrlShift = KeyModifiers.Ctrl | KeyModifiers.Shift;
        if ((mods & ctrlShift) == ctrlShift)
        {
            if (e.Key == VirtualKey.C)
            {
                CopySelection();
                e.Handled = true;
                return;
            }
            if (e.Key == VirtualKey.V)
            {
                PasteClipboard();
                e.Handled = true;
                return;
            }
        }

        _engine.SendKey((uint)e.Key, mods, down: true);

        // Mark handled for keys the engine encodes itself (named control keys, or anything with
        // Ctrl/Alt held). Plain printable keys are left unhandled so they flow to
        // CharacterReceived (correct layout/IME/dead-key resolution) — see plan §8 U7.
        if (EngineConsumesKey(e.Key, mods))
        {
            e.Handled = true;
        }
    }

    private void OnKeyUp(object sender, KeyRoutedEventArgs e)
    {
        if (_engine is null)
        {
            return;
        }
        KeyModifiers mods = CurrentModifiers();
        _engine.SendKey((uint)e.Key, mods, down: false);
        if (EngineConsumesKey(e.Key, mods))
        {
            e.Handled = true;
        }
    }

    private void OnCharacterReceived(UIElement sender, CharacterReceivedRoutedEventArgs e)
    {
        if (_engine is null)
        {
            return;
        }
        // Control characters (Enter/Tab/Backspace/Esc, etc.) are delivered via send_key, so drop
        // them here to avoid double-input. Everything else is resolved text (incl. IME/dead keys).
        char c = e.Character;
        if (c < ' ' || c == '\u007f')
        {
            return;
        }
        _engine.SendText(c.ToString());
    }

    /// <summary>
    /// True when the engine will encode this key on its own (so we suppress the bubbling default
    /// and the corresponding CharacterReceived). Mirrors the engine's <c>map_key</c> contract.
    /// </summary>
    private static bool EngineConsumesKey(VirtualKey key, KeyModifiers mods)
    {
        if ((mods & (KeyModifiers.Ctrl | KeyModifiers.Alt | KeyModifiers.Super)) != 0)
        {
            return true;
        }
        switch (key)
        {
            case VirtualKey.Enter:
            case VirtualKey.Tab:
            case VirtualKey.Escape:
            case VirtualKey.Back:
            case VirtualKey.Delete:
            case VirtualKey.Insert:
            case VirtualKey.Left:
            case VirtualKey.Up:
            case VirtualKey.Right:
            case VirtualKey.Down:
            case VirtualKey.Home:
            case VirtualKey.End:
            case VirtualKey.PageUp:
            case VirtualKey.PageDown:
                return true;
            default:
                return key >= VirtualKey.F1 && key <= VirtualKey.F12;
        }
    }

    /// <summary>Snapshot the live modifier state (plan §8 U7: keyboard source, not event args).</summary>
    private static KeyModifiers CurrentModifiers()
    {
        KeyModifiers mods = KeyModifiers.None;
        if (IsDown(VirtualKey.Shift))
        {
            mods |= KeyModifiers.Shift;
        }
        if (IsDown(VirtualKey.Control))
        {
            mods |= KeyModifiers.Ctrl;
        }
        if (IsDown(VirtualKey.Menu))
        {
            mods |= KeyModifiers.Alt;
        }
        if (IsDown(VirtualKey.LeftWindows) || IsDown(VirtualKey.RightWindows))
        {
            mods |= KeyModifiers.Super;
        }
        return mods;
    }

    private static bool IsDown(VirtualKey key) =>
        (InputKeyboardSource.GetKeyStateForCurrentThread(key) & CoreVirtualKeyStates.Down)
            == CoreVirtualKeyStates.Down;

    // ---- U7: pointer -------------------------------------------------------------------------

    private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
    {
        if (_engine is null)
        {
            return;
        }
        Panel.Focus(FocusState.Pointer);
        PointerPoint pp = e.GetCurrentPoint(Panel);
        Panel.CapturePointer(e.Pointer);
        (float x, float y) = ToPhysical(pp);
        _engine.SendMouse(x, y, (uint)ButtonOf(pp), (uint)MouseKind.Down, CurrentModifiers());
        e.Handled = true;
    }

    private void OnPointerMoved(object sender, PointerRoutedEventArgs e)
    {
        if (_engine is null)
        {
            return;
        }
        PointerPoint pp = e.GetCurrentPoint(Panel);
        // Only forward drags (a button held) — Phase 1 uses moves for drag-selection (U8).
        if (!pp.Properties.IsLeftButtonPressed
            && !pp.Properties.IsRightButtonPressed
            && !pp.Properties.IsMiddleButtonPressed)
        {
            return;
        }
        (float x, float y) = ToPhysical(pp);
        _engine.SendMouse(x, y, (uint)ButtonOf(pp), (uint)MouseKind.Move, CurrentModifiers());
    }

    private void OnPointerReleased(object sender, PointerRoutedEventArgs e)
    {
        if (_engine is null)
        {
            return;
        }
        PointerPoint pp = e.GetCurrentPoint(Panel);
        if (pp.Properties.PointerUpdateKind == PointerUpdateKind.RightButtonReleased)
        {
            // Right-click copies the current selection (plan §8 U8).
            CopySelection();
        }
        else
        {
            // Finish a left-drag selection (the released button is no longer "pressed",
            // so address the engine's left-button selection explicitly).
            (float x, float y) = ToPhysical(pp);
            _engine.SendMouse(x, y, (uint)MouseButton.Left, (uint)MouseKind.Up, CurrentModifiers());
        }
        Panel.ReleasePointerCapture(e.Pointer);
        e.Handled = true;
    }

    private void OnPointerWheelChanged(object sender, PointerRoutedEventArgs e)
    {
        if (_engine is null)
        {
            return;
        }
        PointerPoint pp = e.GetCurrentPoint(Panel);
        int delta = pp.Properties.MouseWheelDelta;
        if (delta != 0)
        {
            // One wheel notch (120) = 3 lines; positive delta scrolls toward history.
            _engine.SendScroll(delta / 120f * 3f);
            e.Handled = true;
        }
    }

    private (float x, float y) ToPhysical(PointerPoint pp)
    {
        float sx = Panel.CompositionScaleX > 0 ? Panel.CompositionScaleX : 1.0f;
        float sy = Panel.CompositionScaleY > 0 ? Panel.CompositionScaleY : 1.0f;
        return ((float)(pp.Position.X * sx), (float)(pp.Position.Y * sy));
    }

    private static MouseButton ButtonOf(PointerPoint pp)
    {
        if (pp.Properties.IsRightButtonPressed)
        {
            return MouseButton.Right;
        }
        if (pp.Properties.IsMiddleButtonPressed)
        {
            return MouseButton.Middle;
        }
        return MouseButton.Left;
    }

    // ---- U8: clipboard -----------------------------------------------------------------------

    /// <summary>Copy the engine's current selection to the Windows clipboard.</summary>
    private void CopySelection()
    {
        string? text = _engine?.SelectionText();
        if (string.IsNullOrEmpty(text))
        {
            return;
        }
        var package = new DataPackage { RequestedOperation = DataPackageOperation.Copy };
        package.SetText(text);
        Clipboard.SetContent(package);
    }

    /// <summary>Paste clipboard text into the shell as resolved input.</summary>
    private async void PasteClipboard()
    {
        try
        {
            DataPackageView view = Clipboard.GetContent();
            if (!view.Contains(StandardDataFormats.Text))
            {
                return;
            }
            string text = await view.GetTextAsync();
            if (!string.IsNullOrEmpty(text))
            {
                _engine?.SendText(text);
            }
        }
        catch
        {
            // Clipboard access can transiently fail (locked by another process); ignore.
        }
    }

    // ---- Host events -------------------------------------------------------------------------

    private void OnHostEvent(HostEventArgs e)
    {
        switch (e.Kind)
        {
            case HostEventKind.Title when e.Title is not null:
                TitleChanged?.Invoke(e.Title);
                break;
            case HostEventKind.ChildExit:
                ChildExited?.Invoke(e.Arg0);
                break;
        }
    }

    /// <summary>Mouse button mirrored across the FFI (matches the engine's decoding in U8).</summary>
    private enum MouseButton : uint
    {
        Left = 0,
        Right = 1,
        Middle = 2,
    }

    /// <summary>Mouse event kind mirrored across the FFI (matches the engine's decoding in U8).</summary>
    private enum MouseKind : uint
    {
        Down = 0,
        Move = 1,
        Up = 2,
    }
}
