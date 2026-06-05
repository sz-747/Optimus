using System;
using System.Runtime.InteropServices;
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
/// One terminal pane (plan §8, Phase 1): hosts the <see cref="SwapChainPanel"/> the Rust engine
/// renders into, owns the <see cref="EngineHandle"/> lifecycle, and forwards keyboard/pointer
/// input across the FFI.
///
/// Responsibilities by unit:
/// <list type="bullet">
///   <item>Lifecycle — create the engine, attach the panel, spawn the shell, tear down cleanly.</item>
///   <item>U7 input — KeyDown/KeyUp → <c>send_key</c> (control keys), CharacterReceived →
///         <c>send_text</c> (layout/IME/dead-key text), pointer/wheel → <c>send_mouse</c>/<c>send_scroll</c>.</item>
///   <item>U9 resize/DPI — SizeChanged + CompositionScaleChanged → physical pixels → <c>resize</c>.</item>
/// </list>
/// </summary>
public sealed partial class TerminalPane : UserControl
{
    private EngineHandle? _engine;

    // The AddRef'd ISwapChainPanelNative* handed to the engine. Released after the engine is
    // destroyed (which drops wgpu's own ref) — see <see cref="Shutdown"/>.
    private IntPtr _panelNative;

    private bool _attached;
    private bool _started;
    private bool _disposed;

    /// <summary>Raised (on the UI thread) when the shell sets the window/tab title (OSC 0/2).</summary>
    public event Action<string>? TitleChanged;

    /// <summary>Raised (on the UI thread) when the shell process exits, with its exit code.</summary>
    public event Action<long>? ChildExited;

    public TerminalPane()
    {
        this.InitializeComponent();
        this.Loaded += OnLoaded;
        this.Unloaded += OnUnloaded;

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
        if (_disposed || _attached)
        {
            return;
        }

        _engine = EngineHandle.CreateDefault();
        _engine.SetEventCallback(this.DispatcherQueue, OnHostEvent);

        // QI the panel for ISwapChainPanelNative and hand the raw pointer to the engine's wgpu
        // surface (plan §5.2). We own one ref until teardown.
        _panelNative = SwapChainPanelNativeInterop.GetNativePointer(Panel);
        _engine.AttachSwapChainPanel(_panelNative);
        _attached = true;

        Configure();
    }

    private void OnUnloaded(object sender, RoutedEventArgs e) => Shutdown();

    /// <summary>
    /// Tear down the engine and release the panel COM ref. Idempotent. Called from
    /// <c>Unloaded</c> and from the window's <c>Closed</c> handler (plan §7.2: render thread
    /// stopped before panel disposal).
    /// </summary>
    public void Shutdown()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;

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
    /// Push the current surface geometry to the engine and, on the first valid size, spawn the
    /// shell. The engine is authoritative for cols/rows (plan §6 decision): we pass physical
    /// pixels + DPI and 0/0 for cols/rows, and it derives the grid from its own cell metrics.
    /// </summary>
    private void Configure()
    {
        if (!_attached || _engine is null)
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
                _engine.SpawnShell(string.Empty); // default shell: pwsh → powershell → cmd
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
