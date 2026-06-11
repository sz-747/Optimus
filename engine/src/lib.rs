//! optimus for Windows — terminal engine.
//!
//! This crate is built as a `cdylib` (`optimus_engine.dll`) consumed by the C# WinUI 3
//! app over a C ABI (plan §6), and as an `rlib` so the in-tree spikes/tests can link the
//! engine directly.
//!
//! This file is the **`#[no_mangle] extern "C"` ABI surface** (plan §4). Each entry point:
//!   - wraps its body in [`ffi::guard`] so a panic never unwinds across FFI (plan §9 #5);
//!   - takes/returns plain ints, pointers, UTF-8 `ptr+len` strings, and `#[repr(C)]` PODs;
//!   - delegates real work to [`engine::Engine`].
//!
//! csbindgen (`build.rs`) scans this file + `ffi/events.rs` to generate the C# bindings at
//! `app/Interop/NativeMethods.g.cs` (checked in).

use std::ffi::c_void;

pub mod engine;
pub mod ffi;
pub mod pty;
pub mod render;
pub mod vt;

use crate::engine::Engine;
use crate::ffi::events::{ByteBuffer, EngineOptions, EventSink, HostEvent, HostEventFn};
use crate::ffi::{str_arg, take_last_error, write_string_out};

// Re-export the FFI POD types + kind constants at the crate root for downstream users.
pub use crate::ffi::events::event_kind;

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle
// ─────────────────────────────────────────────────────────────────────────────

/// Create an engine. `opts` may be null (selects defaults). Returns an opaque handle, or
/// null on failure (see `optimus_last_error_message`).
///
/// # Safety
/// `opts`, if non-null, must point to a valid [`EngineOptions`]. The returned handle must
/// be freed exactly once with [`optimus_engine_destroy`].
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_create(opts: *const EngineOptions) -> *mut Engine {
    ffi::guard(std::ptr::null_mut(), || {
        let options = if opts.is_null() {
            EngineOptions::default()
        } else {
            // SAFETY: caller guarantees a valid pointer when non-null.
            unsafe { *opts }
        };
        Box::into_raw(Box::new(Engine::new(options)))
    })
}

/// Destroy an engine handle (stops threads, drops the PTY + GPU resources). Null-safe.
///
/// # Safety
/// `engine` must be a handle from [`optimus_engine_create`] not already destroyed.
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_destroy(engine: *mut Engine) {
    if engine.is_null() {
        return;
    }
    let _ = ffi::guard((), || {
        // SAFETY: reclaim the Box created in `optimus_engine_create`.
        drop(unsafe { Box::from_raw(engine) });
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Renderer attach
// ─────────────────────────────────────────────────────────────────────────────

/// Bind the wgpu renderer to the WinUI 3 `SwapChainPanel` whose native `ISwapChainPanel*`
/// is `panel`. Call on the UI thread (plan §5.2). Returns `0` on success, `<0` on error.
///
/// # Safety
/// `engine` must be a live handle; `panel` a valid, live `ISwapChainPanel*`.
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_attach_swapchain_panel(
    engine: *mut Engine,
    panel: *mut c_void,
) -> i32 {
    ffi::guard(-1, || {
        let Some(engine) = (unsafe { engine.as_mut() }) else {
            ffi::set_last_error("attach_swapchain_panel: null engine");
            return -1;
        };
        // SAFETY: panel validity is the caller's contract.
        match unsafe { engine.attach_swapchain_panel(panel) } {
            Ok(()) => 0,
            Err(e) => {
                ffi::set_last_error(e);
                -1
            }
        }
    })
}

/// Detach the renderer surface (panel going away). Null-safe.
///
/// # Safety
/// `engine` must be a live handle from [`optimus_engine_create`].
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_detach(engine: *mut Engine) {
    let _ = ffi::guard((), || {
        if let Some(engine) = unsafe { engine.as_mut() } {
            engine.detach();
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// PTY / shell
// ─────────────────────────────────────────────────────────────────────────────

/// Spawn the shell. `cmdline`/`cwd` are UTF-8 `ptr + len` (cwd may be empty → inherit).
/// Returns `0` on success, `<0` on error.
///
/// # Safety
/// `engine` live; `cmdline_utf8`/`cwd_utf8` valid for their lengths (or null when len 0).
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_spawn_shell(
    engine: *mut Engine,
    cmdline_utf8: *const u8,
    cmdline_len: usize,
    cwd_utf8: *const u8,
    cwd_len: usize,
) -> i32 {
    ffi::guard(-1, || {
        let Some(engine) = (unsafe { engine.as_mut() }) else {
            ffi::set_last_error("spawn_shell: null engine");
            return -1;
        };
        let Some(cmdline) = (unsafe { str_arg(cmdline_utf8, cmdline_len) }) else {
            ffi::set_last_error("spawn_shell: cmdline is not valid UTF-8");
            return -1;
        };
        let cwd = unsafe { str_arg(cwd_utf8, cwd_len) }.filter(|s| !s.is_empty());
        match engine.spawn_shell(cmdline, cwd) {
            Ok(()) => 0,
            Err(e) => {
                ffi::set_last_error(e);
                -1
            }
        }
    })
}

/// The Windows process id of the spawned ConPTY child, or `0` when unavailable (no shell
/// spawned yet, spawn failed, child exited, or null engine). Valid as soon as
/// [`optimus_engine_spawn_shell`] returns `0`; reset to 0 when the child exits or the engine
/// tears down. **Diagnostics/measurement only** (e.g. `GetProcessMemoryInfo` calibration where
/// PID reuse is low-stakes) — Job Object enrollment must use
/// [`optimus_engine_child_process_handle`] instead, which cannot suffer PID recycling.
///
/// # Safety
/// `engine` must be a live handle from [`optimus_engine_create`] (or null → returns 0).
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_child_pid(engine: *mut Engine) -> u32 {
    ffi::guard(0, || {
        let Some(engine) = (unsafe { engine.as_ref() }) else {
            return 0;
        };
        engine.child_pid()
    })
}

/// A Windows HANDLE to the spawned ConPTY child process, or `0` when unavailable (no live
/// child, duplication failed, or null engine). Returned as `usize` (the raw HANDLE value);
/// the engine DLL runs in-process with the host, so the value is directly usable.
///
/// **Ownership:** every call returns a **fresh duplicate** (`DuplicateHandle`) that the
/// **caller owns** and must close (`CloseHandle` / a C# `SafeProcessHandle`). The engine keeps
/// its own internal handle (closed on child exit / engine destroy), so disposing the returned
/// duplicate never invalidates engine state. The host uses this — never `OpenProcess(pid)` —
/// to `AssignProcessToJobObject`, eliminating the PID-reuse TOCTOU between reading the PID and
/// assigning the job (plan U4 review fix).
///
/// # Safety
/// `engine` must be a live handle from [`optimus_engine_create`] (or null → returns 0).
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_child_process_handle(engine: *mut Engine) -> usize {
    ffi::guard(0, || {
        let Some(engine) = (unsafe { engine.as_ref() }) else {
            return 0;
        };
        engine.child_process_handle()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Input
// ─────────────────────────────────────────────────────────────────────────────

/// Send already-resolved text (layout/IME/paste) to the shell. UTF-8 `ptr + len`.
///
/// # Safety
/// `engine` live; `utf8` valid for `len` bytes (or null when `len == 0`).
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_send_text(engine: *mut Engine, utf8: *const u8, len: usize) {
    let _ = ffi::guard((), || {
        let Some(engine) = (unsafe { engine.as_mut() }) else {
            return;
        };
        if let Some(text) = unsafe { str_arg(utf8, len) } {
            engine.send_text(text);
        }
    });
}

/// Send a key event. `vk` is a Windows virtual-key code; `modifiers` is a bitmask
/// (see the C# `KeyModifiers`); `down` is press vs release.
///
/// # Safety
/// `engine` must be a live handle from [`optimus_engine_create`].
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_send_key(
    engine: *mut Engine,
    vk: u32,
    modifiers: u32,
    down: bool,
) {
    let _ = ffi::guard((), || {
        if let Some(engine) = unsafe { engine.as_mut() } {
            engine.send_key(vk, modifiers, down);
        }
    });
}

/// Send a mouse event in physical pixels relative to the panel. `button`/`kind`/`modifiers`
/// are enums mirrored on the C# side.
///
/// # Safety
/// `engine` must be a live handle from [`optimus_engine_create`].
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_send_mouse(
    engine: *mut Engine,
    x: f32,
    y: f32,
    button: u32,
    kind: u32,
    modifiers: u32,
) {
    let _ = ffi::guard((), || {
        if let Some(engine) = unsafe { engine.as_mut() } {
            engine.send_mouse(x, y, button, kind, modifiers);
        }
    });
}

/// Send a scroll event (`delta_lines`: +up / -down).
///
/// # Safety
/// `engine` must be a live handle from [`optimus_engine_create`].
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_send_scroll(engine: *mut Engine, delta_lines: f32) {
    let _ = ffi::guard((), || {
        if let Some(engine) = unsafe { engine.as_mut() } {
            engine.send_scroll(delta_lines);
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Resize (cols/rows + pixels + DPI)
// ─────────────────────────────────────────────────────────────────────────────

/// Resize the grid, GPU surface, and pseudoconsole together. Returns `0` / `<0`.
///
/// # Safety
/// `engine` must be a live handle from [`optimus_engine_create`].
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_resize(
    engine: *mut Engine,
    cols: u16,
    rows: u16,
    pixel_width: u32,
    pixel_height: u32,
    dpi_scale: f32,
) -> i32 {
    ffi::guard(-1, || {
        let Some(engine) = (unsafe { engine.as_mut() }) else {
            ffi::set_last_error("resize: null engine");
            return -1;
        };
        match engine.resize(cols, rows, pixel_width, pixel_height, dpi_scale) {
            Ok(()) => 0,
            Err(e) => {
                ffi::set_last_error(e);
                -1
            }
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Selection / clipboard
// ─────────────────────────────────────────────────────────────────────────────

/// Write the current selection (UTF-8) into `*out`. Returns `0` if there is a selection,
/// `1` if there is none (and `*out` is set to an empty buffer), `<0` on error. Free `*out`
/// with [`optimus_buffer_free`].
///
/// # Safety
/// `engine` live; `out` a valid writable `*mut ByteBuffer`.
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_selection_text(
    engine: *mut Engine,
    out: *mut ByteBuffer,
) -> i32 {
    ffi::guard(-1, || {
        if out.is_null() {
            ffi::set_last_error("selection_text: null out");
            return -1;
        }
        let Some(engine) = (unsafe { engine.as_ref() }) else {
            ffi::set_last_error("selection_text: null engine");
            unsafe { out.write(ByteBuffer::EMPTY) };
            return -1;
        };
        match engine.selection_text() {
            Some(text) => {
                unsafe { write_string_out(out, &text) };
                0
            }
            None => {
                unsafe { out.write(ByteBuffer::EMPTY) };
                1
            }
        }
    })
}

/// Free a [`ByteBuffer`] produced by this library. Null/empty-safe.
///
/// # Safety
/// `buf` must be a buffer returned by this library and not already freed.
#[no_mangle]
pub unsafe extern "C" fn optimus_buffer_free(buf: ByteBuffer) {
    let _ = ffi::guard((), || {
        // SAFETY: buffer originated from `ByteBuffer::from_vec` in this library.
        unsafe { buf.free() };
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Host event callback
// ─────────────────────────────────────────────────────────────────────────────

/// Register the host-event callback. `cb` is invoked from the engine's worker threads with
/// borrowed string pointers; the C# side must `DispatcherQueue.TryEnqueue` and copy strings
/// immediately (plan §6). `user_data` is an opaque token (a pinned `GCHandle`).
///
/// # Safety
/// `engine` live; `cb` a valid C-ABI function pointer; `user_data` valid for the engine's
/// lifetime.
#[no_mangle]
pub unsafe extern "C" fn optimus_engine_set_event_callback(
    engine: *mut Engine,
    cb: HostEventFn,
    user_data: *mut c_void,
) {
    let _ = ffi::guard((), || {
        if let Some(engine) = unsafe { engine.as_mut() } {
            engine.set_event_callback(EventSink::new(cb, user_data));
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Error detail
// ─────────────────────────────────────────────────────────────────────────────

/// Write the calling thread's last error message (UTF-8) into `*out`. Returns `0` if a
/// message was available, `1` if none (empty buffer written), `<0` on error. Clears the
/// stored error. Free `*out` with [`optimus_buffer_free`].
///
/// # Safety
/// `out` must be a valid writable `*mut ByteBuffer`.
#[no_mangle]
pub unsafe extern "C" fn optimus_last_error_message(out: *mut ByteBuffer) -> i32 {
    ffi::guard(-1, || {
        if out.is_null() {
            return -1;
        }
        match take_last_error() {
            Some(msg) => {
                unsafe { write_string_out(out, &msg) };
                0
            }
            None => {
                unsafe { out.write(ByteBuffer::EMPTY) };
                1
            }
        }
    })
}

// `HostEvent` is part of the ABI (csbindgen emits it from `ffi/events.rs`) and is delivered
// via `EventSink`; anchor the type here so the ABI surface references it in one place.
#[allow(dead_code)]
fn _abi_type_anchors(_e: *const HostEvent) {}
