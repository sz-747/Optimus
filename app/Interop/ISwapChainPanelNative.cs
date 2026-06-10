using System;
using System.Runtime.InteropServices;
using Microsoft.UI.Xaml.Controls;

namespace Optimus.Interop;

/// <summary>
/// Helper for obtaining the native <c>ISwapChainPanelNative*</c> behind a WinUI 3
/// <see cref="SwapChainPanel"/> (plan §5.2 / §6). The engine's wgpu surface binds to this
/// pointer via <c>SurfaceTargetUnsafe::SwapChainPanel</c>; wgpu calls
/// <c>ISwapChainPanelNative::SetSwapChain</c> itself, so we only need the QI'd pointer —
/// no vtable declaration is required.
///
/// This is the pattern proven in Spike 1 (the GATE): QI the panel's <c>IInspectable*</c> for
/// the documented IID, hand the raw pointer to Rust on the UI thread.
/// </summary>
internal static class SwapChainPanelNativeInterop
{
    // ISwapChainPanelNative — documented (Windows SDK) IID.
    private static readonly Guid IID_ISwapChainPanelNative = new("63aad0b8-7c24-40ff-85a8-640d944cc325");

    /// <summary>
    /// QI <paramref name="panel"/> for <c>ISwapChainPanelNative</c> and return the raw,
    /// <b>AddRef'd</b> pointer. The caller owns one reference and MUST release it with
    /// <see cref="System.Runtime.InteropServices.Marshal.Release(IntPtr)"/> once the engine has
    /// detached (after <c>optimus_engine_destroy</c>, which drops wgpu's own ref on the panel).
    /// </summary>
    /// <exception cref="InvalidOperationException">QueryInterface failed.</exception>
    public static IntPtr GetNativePointer(SwapChainPanel panel)
    {
        // Owning IInspectable* for the panel; QI it for ISwapChainPanelNative.
        IntPtr inspectable = WinRT.MarshalInspectable<SwapChainPanel>.FromManaged(panel);
        try
        {
            Guid iid = IID_ISwapChainPanelNative;
            int hr = Marshal.QueryInterface(inspectable, in iid, out IntPtr native);
            if (hr < 0)
            {
                throw new InvalidOperationException(
                    $"QueryInterface(ISwapChainPanelNative) failed: 0x{hr:X8}");
            }
            return native;
        }
        finally
        {
            Marshal.Release(inspectable);
        }
    }
}
