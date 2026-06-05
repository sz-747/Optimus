using System;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Spike1;

/// <summary>
/// Spike 1 (plan §7.1, GATE): prove wgpu (DX12) composites into a WinUI 3
/// <see cref="SwapChainPanel"/> — create, animate a clear color, resize, and track DPI.
///
/// Flow: QI the panel for <c>ISwapChainPanelNative</c>, hand the raw pointer to the Rust
/// engine (<c>cmux_engine.dll</c>), which binds a wgpu surface to it via
/// <c>SurfaceTargetUnsafe::SwapChainPanel</c> and drives present. wgpu internally calls
/// <c>ISwapChainPanelNative::SetSwapChain</c>, so all surface work happens on the UI thread.
/// </summary>
public sealed partial class MainWindow : Window
{
    private const string Engine = "cmux_engine.dll";

    [DllImport(Engine, CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr cmux_spike_renderer_create(IntPtr panel, uint width, uint height);

    [DllImport(Engine, CallingConvention = CallingConvention.Cdecl)]
    private static extern int cmux_spike_renderer_render(IntPtr renderer, double r, double g, double b);

    [DllImport(Engine, CallingConvention = CallingConvention.Cdecl)]
    private static extern void cmux_spike_renderer_resize(IntPtr renderer, uint width, uint height);

    [DllImport(Engine, CallingConvention = CallingConvention.Cdecl)]
    private static extern void cmux_spike_renderer_destroy(IntPtr renderer);

    // ISwapChainPanelNative — documented IID. We only need the QI'd pointer to hand to wgpu
    // (wgpu calls SetSwapChain itself), so no vtable/method declaration is required here.
    private static readonly Guid IID_ISwapChainPanelNative = new("63aad0b8-7c24-40ff-85a8-640d944cc325");

    private IntPtr _renderer = IntPtr.Zero;    // opaque *mut PanelRenderer
    private IntPtr _panelNative = IntPtr.Zero;  // ISwapChainPanelNative* we hold a ref on
    private long _frame;
    private long _presented;   // render() returned 0
    private long _unavailable; // render() returned 1 (dropped frame)

    // Evidence sink for headless verification of the GATE (created next to the exe).
    private static readonly string LogPath = Path.Combine(AppContext.BaseDirectory, "spike1.log");

    private static void Log(string message)
    {
        try
        {
            File.AppendAllText(LogPath, $"{DateTime.Now:HH:mm:ss.fff}  {message}{Environment.NewLine}");
        }
        catch
        {
            // logging must never take down the spike
        }
    }

    public MainWindow()
    {
        this.InitializeComponent();
        this.Title = "cmux spike 1 — SwapChainPanel";
        Log("=== spike1 start ===");

        Panel.Loaded += OnPanelLoaded;
        Panel.SizeChanged += OnPanelSizeChanged;
        Panel.CompositionScaleChanged += OnCompositionScaleChanged;
        this.Closed += OnClosed;
    }

    /// Panel size in *physical* pixels = DIP size × composition (DPI) scale. wgpu's swapchain
    /// is sized in device pixels; the panel applies the inverse transform when compositing.
    private (uint w, uint h) PhysicalSize()
    {
        double w = Panel.ActualWidth * Panel.CompositionScaleX;
        double h = Panel.ActualHeight * Panel.CompositionScaleY;
        return ((uint)Math.Max(1, Math.Round(w)), (uint)Math.Max(1, Math.Round(h)));
    }

    private void OnPanelLoaded(object sender, RoutedEventArgs e)
    {
        try
        {
            // Owning IInspectable* for the panel; QI it for ISwapChainPanelNative.
            IntPtr inspectable = WinRT.MarshalInspectable<SwapChainPanel>.FromManaged(Panel);
            try
            {
                Guid iid = IID_ISwapChainPanelNative;
                int hr = Marshal.QueryInterface(inspectable, in iid, out _panelNative);
                if (hr < 0)
                {
                    Status.Text = $"QI ISwapChainPanelNative failed: 0x{hr:X8}";
                    Log($"QI ISwapChainPanelNative FAILED: 0x{hr:X8}");
                    return;
                }
                Log($"QI ISwapChainPanelNative ok -> 0x{_panelNative.ToInt64():X}");
            }
            finally
            {
                Marshal.Release(inspectable);
            }

            var (w, h) = PhysicalSize();
            _renderer = cmux_spike_renderer_create(_panelNative, w, h);
            if (_renderer == IntPtr.Zero)
            {
                Status.Text = "renderer create FAILED (no DX12 adapter / surface bind?)";
                Log($"renderer create FAILED at {w}x{h}");
                return;
            }

            Status.Text = $"renderer up — {w}x{h} @ scale {Panel.CompositionScaleX:0.##} — animating";
            Log($"renderer create ok at {w}x{h} @ scale {Panel.CompositionScaleX:0.##}");
            // Per-frame on the UI thread; the compositor throttles this to display refresh.
            CompositionTarget.Rendering += OnRendering;
        }
        catch (Exception ex)
        {
            Status.Text = "exception: " + ex.Message;
            Log("EXCEPTION in OnPanelLoaded: " + ex);
        }
    }

    private void OnRendering(object? sender, object e)
    {
        if (_renderer == IntPtr.Zero)
        {
            return;
        }

        _frame++;
        // Cycle hue so continuous compositing is unmistakable (not a single static clear).
        double t = _frame / 120.0;
        double r = 0.5 + 0.5 * Math.Sin(t);
        double g = 0.5 + 0.5 * Math.Sin(t + 2.0944); // +120°
        double b = 0.5 + 0.5 * Math.Sin(t + 4.1888); // +240°
        int status = cmux_spike_renderer_render(_renderer, r, g, b);
        if (status == 0)
        {
            _presented++;
        }
        else
        {
            _unavailable++;
        }

        // First frame and every ~2s of frames: record evidence of a live present loop.
        if (_frame == 1 || _frame % 120 == 0)
        {
            Log($"frame {_frame}: presented={_presented} unavailable={_unavailable} (last status={status})");
        }
    }

    private void OnPanelSizeChanged(object sender, SizeChangedEventArgs e)
    {
        if (_renderer == IntPtr.Zero)
        {
            return;
        }

        var (w, h) = PhysicalSize();
        cmux_spike_renderer_resize(_renderer, w, h);
        Status.Text = $"resize {w}x{h} @ scale {Panel.CompositionScaleX:0.##}";
        Log($"resize -> {w}x{h} @ scale {Panel.CompositionScaleX:0.##}");
    }

    private void OnCompositionScaleChanged(SwapChainPanel sender, object args)
    {
        if (_renderer == IntPtr.Zero)
        {
            return;
        }

        var (w, h) = PhysicalSize();
        cmux_spike_renderer_resize(_renderer, w, h);
        Status.Text = $"DPI scale {sender.CompositionScaleX:0.##} → {w}x{h}";
        Log($"DPI scale {sender.CompositionScaleX:0.##} -> {w}x{h}");
    }

    private void OnClosed(object sender, WindowEventArgs args)
    {
        CompositionTarget.Rendering -= OnRendering;
        Log($"=== closing === total frames={_frame} presented={_presented} unavailable={_unavailable}");

        // Destroy the renderer first (wgpu drops its surface + its ref on the panel),
        // then release the ref we took in the QueryInterface above.
        if (_renderer != IntPtr.Zero)
        {
            cmux_spike_renderer_destroy(_renderer);
            _renderer = IntPtr.Zero;
        }
        if (_panelNative != IntPtr.Zero)
        {
            Marshal.Release(_panelNative);
            _panelNative = IntPtr.Zero;
        }
    }
}
