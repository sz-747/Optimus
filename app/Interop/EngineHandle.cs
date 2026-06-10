using System;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.UI.Dispatching;

namespace Optimus.Interop;

/// <summary>
/// The opaque native engine type. The C# side never dereferences this — it only holds and
/// passes <c>Engine*</c> across the FFI. It is defined here (not in the generated
/// <c>NativeMethods.g.cs</c>) because csbindgen references <c>Engine*</c> but deliberately
/// does not emit the type: <c>Engine</c> lives outside the csbindgen input files so the
/// pointer stays opaque (plan §6).
/// </summary>
internal readonly struct Engine
{
}

/// <summary>Keyboard modifier bitmask handed to <c>optimus_engine_send_key</c> (plan §6).</summary>
[Flags]
internal enum KeyModifiers : uint
{
    None = 0,
    Shift = 1 << 0,
    Ctrl = 1 << 1,
    Alt = 1 << 2,
    Super = 1 << 3,
}

/// <summary>Host-event kind mirrored from the Rust <c>event_kind</c> module (plan §6).</summary>
internal enum HostEventKind : uint
{
    Toast = 0,
    Title = 1,
    Bell = 2,
    SetUserVar = 3,
    Cwd = 4,
    ChildExit = 5,
    Progress = 6,
}

/// <summary>
/// A host event copied into managed memory. The native <see cref="HostEvent"/> carries
/// <b>borrowed</b> UTF-8 pointers valid only for the duration of the callback; this record
/// holds owned copies safe to use after the callback returns and after the UI-thread hop.
/// </summary>
internal readonly record struct HostEventArgs(HostEventKind Kind, string? Title, string? Body, long Arg0);

/// <summary>
/// Safe, owning wrapper over the native <c>Engine*</c> handle (plan §6). Centralizes the
/// unsafe FFI surface: handle lifetime, UTF-8 string marshalling, the host-event callback
/// (rooted via a <see cref="GCHandle"/> and marshalled to the UI thread), and last-error
/// retrieval. All public methods are safe to call from the UI thread.
/// </summary>
internal sealed unsafe class EngineHandle : IDisposable
{
    private Engine* _engine;

    // Keeps `this` alive and gives the native side a stable opaque token (`user_data`) it can
    // round-trip back to us in the event callback. Not pinned: the native side never reads the
    // managed object's memory, only echoes the token, so a normal handle is correct (and avoids
    // pinning a managed object for the engine's whole lifetime).
    private GCHandle _selfHandle;

    private DispatcherQueue? _dispatcher;
    private Action<HostEventArgs>? _onEvent;

    private EngineHandle(Engine* engine)
    {
        _engine = engine;
    }

    /// <summary>
    /// Create an engine with the given options (pass zero-valued fields to take the engine's
    /// defaults — Rust normalizes 0 → 80×24, 10000-line scrollback).
    /// </summary>
    /// <exception cref="EngineException">The native create failed.</exception>
    public static EngineHandle Create(EngineOptions options)
    {
        Engine* engine = NativeMethods.optimus_engine_create(&options);
        if (engine == null)
        {
            throw new EngineException("optimus_engine_create returned null: " + LastError());
        }
        return new EngineHandle(engine);
    }

    /// <summary>Create an engine with the engine's built-in defaults.</summary>
    public static EngineHandle CreateDefault() => Create(default);

    /// <summary>
    /// Register the host-event callback. <paramref name="dispatcher"/> is the UI-thread
    /// dispatcher; <paramref name="onEvent"/> runs there. The native callback fires on the
    /// engine's worker threads, copies the borrowed strings immediately, then enqueues onto the
    /// dispatcher (plan §6).
    /// </summary>
    public void SetEventCallback(DispatcherQueue dispatcher, Action<HostEventArgs> onEvent)
    {
        ThrowIfDisposed();
        _dispatcher = dispatcher;
        _onEvent = onEvent;
        if (!_selfHandle.IsAllocated)
        {
            _selfHandle = GCHandle.Alloc(this);
        }
        delegate* unmanaged[Cdecl]<void*, HostEvent*, void> cb = &OnNativeEvent;
        NativeMethods.optimus_engine_set_event_callback(_engine, cb, (void*)GCHandle.ToIntPtr(_selfHandle));
    }

    /// <summary>
    /// Bind the wgpu renderer to <paramref name="panelNative"/> (an <c>ISwapChainPanelNative*</c>
    /// from <see cref="SwapChainPanelNativeInterop.GetNativePointer"/>). Call on the UI thread.
    /// </summary>
    public void AttachSwapChainPanel(IntPtr panelNative)
    {
        ThrowIfDisposed();
        int rc = NativeMethods.optimus_engine_attach_swapchain_panel(_engine, (void*)panelNative);
        if (rc < 0)
        {
            throw new EngineException("optimus_engine_attach_swapchain_panel failed: " + LastError());
        }
    }

    /// <summary>Detach the renderer surface (panel going away).</summary>
    public void Detach()
    {
        ThrowIfDisposed();
        NativeMethods.optimus_engine_detach(_engine);
    }

    /// <summary>Spawn the shell. <paramref name="cwd"/> null/empty → inherit the parent's cwd.</summary>
    public void SpawnShell(string cmdline, string? cwd = null)
    {
        ThrowIfDisposed();
        byte[] cmd = Encoding.UTF8.GetBytes(cmdline);
        byte[] dir = string.IsNullOrEmpty(cwd) ? Array.Empty<byte>() : Encoding.UTF8.GetBytes(cwd);
        fixed (byte* pc = cmd)
        fixed (byte* pd = dir)
        {
            // `fixed` on an empty array yields a null pointer; Rust's str_arg treats (null, 0) as "".
            int rc = NativeMethods.optimus_engine_spawn_shell(_engine, pc, (nuint)cmd.Length, pd, (nuint)dir.Length);
            if (rc < 0)
            {
                throw new EngineException("optimus_engine_spawn_shell failed: " + LastError());
            }
        }
    }

    /// <summary>Send already-resolved input text (layout/IME/paste) to the shell.</summary>
    public void SendText(string text)
    {
        ThrowIfDisposed();
        byte[] bytes = Encoding.UTF8.GetBytes(text);
        fixed (byte* p = bytes)
        {
            NativeMethods.optimus_engine_send_text(_engine, p, (nuint)bytes.Length);
        }
    }

    /// <summary>Forward a key press/release (Windows virtual-key code + modifier bitmask).</summary>
    public void SendKey(uint virtualKey, KeyModifiers modifiers, bool down)
    {
        ThrowIfDisposed();
        NativeMethods.optimus_engine_send_key(_engine, virtualKey, (uint)modifiers, down);
    }

    /// <summary>Forward a mouse event in physical pixels relative to the panel.</summary>
    public void SendMouse(float x, float y, uint button, uint kind, KeyModifiers modifiers)
    {
        ThrowIfDisposed();
        NativeMethods.optimus_engine_send_mouse(_engine, x, y, button, kind, (uint)modifiers);
    }

    /// <summary>Forward a scroll event (<paramref name="deltaLines"/>: +up / -down).</summary>
    public void SendScroll(float deltaLines)
    {
        ThrowIfDisposed();
        NativeMethods.optimus_engine_send_scroll(_engine, deltaLines);
    }

    /// <summary>Resize the grid, GPU surface, and pseudoconsole together.</summary>
    public void Resize(ushort cols, ushort rows, uint pixelWidth, uint pixelHeight, float dpiScale)
    {
        ThrowIfDisposed();
        int rc = NativeMethods.optimus_engine_resize(_engine, cols, rows, pixelWidth, pixelHeight, dpiScale);
        if (rc < 0)
        {
            throw new EngineException("optimus_engine_resize failed: " + LastError());
        }
    }

    /// <summary>The current selection as text, or <c>null</c> when nothing is selected.</summary>
    public string? SelectionText()
    {
        ThrowIfDisposed();
        ByteBuffer buf;
        int rc = NativeMethods.optimus_engine_selection_text(_engine, &buf);
        if (rc < 0)
        {
            throw new EngineException("optimus_engine_selection_text failed: " + LastError());
        }
        try
        {
            return rc == 1 ? null : Utf8ToString(buf.ptr, buf.len);
        }
        finally
        {
            NativeMethods.optimus_buffer_free(buf);
        }
    }

    /// <summary>The calling thread's last engine error message (empty string if none).</summary>
    public static string LastError()
    {
        ByteBuffer buf;
        int rc = NativeMethods.optimus_last_error_message(&buf);
        if (rc != 0)
        {
            return rc == 1 ? string.Empty : "(failed to read last error)";
        }
        try
        {
            return Utf8ToString(buf.ptr, buf.len) ?? string.Empty;
        }
        finally
        {
            NativeMethods.optimus_buffer_free(buf);
        }
    }

    public void Dispose()
    {
        // Destroy the engine FIRST (stops worker threads, so no callback can fire afterward),
        // THEN release the GCHandle that rooted us as the callback's user_data.
        if (_engine != null)
        {
            NativeMethods.optimus_engine_destroy(_engine);
            _engine = null;
        }
        if (_selfHandle.IsAllocated)
        {
            _selfHandle.Free();
        }
        _dispatcher = null;
        _onEvent = null;
    }

    private void ThrowIfDisposed()
    {
        if (_engine == null)
        {
            throw new ObjectDisposedException(nameof(EngineHandle));
        }
    }

    /// <summary>
    /// The native host-event entry point. Runs on an engine worker thread: it must not throw
    /// across the boundary, and the <see cref="HostEvent"/> string pointers are borrowed, so it
    /// copies them immediately before hopping to the UI thread (plan §6).
    /// </summary>
    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnNativeEvent(void* userData, HostEvent* ev)
    {
        try
        {
            if (userData == null || ev == null)
            {
                return;
            }
            if (GCHandle.FromIntPtr((IntPtr)userData).Target is not EngineHandle self)
            {
                return;
            }

            HostEvent e = *ev;
            var args = new HostEventArgs(
                (HostEventKind)e.kind,
                Utf8ToString(e.title_utf8, e.title_len),
                Utf8ToString(e.body_utf8, e.body_len),
                e.arg0);

            DispatcherQueue? dispatcher = self._dispatcher;
            Action<HostEventArgs>? onEvent = self._onEvent;
            if (dispatcher is null || onEvent is null)
            {
                return;
            }
            dispatcher.TryEnqueue(() => onEvent(args));
        }
        catch
        {
            // A panic/exception unwinding across the native boundary is undefined behavior.
            // Host events are best-effort notifications; swallow and move on.
        }
    }

    /// <summary>Copy a borrowed UTF-8 <c>ptr + len</c> into a managed string.</summary>
    private static string? Utf8ToString(byte* ptr, nuint len)
    {
        if (len == 0)
        {
            return string.Empty;
        }
        if (ptr == null)
        {
            return null;
        }
        return Encoding.UTF8.GetString(ptr, checked((int)len));
    }
}

/// <summary>An error surfaced from the native engine across the FFI boundary.</summary>
internal sealed class EngineException : Exception
{
    public EngineException(string message) : base(message)
    {
    }
}
