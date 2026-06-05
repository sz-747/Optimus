//! FFI plain-old-data types and the host-event sink (plan §6).
//!
//! Everything here is `#[repr(C)]` and is consumed by csbindgen to generate
//! `app/Interop/NativeMethods.g.cs`. Keep this file free of non-FFI dependencies so the
//! generated C# stays a faithful mirror of the C ABI.

use std::ffi::c_void;

/// Options passed to [`crate::cmux_engine_create`]. A null pointer selects the defaults.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EngineOptions {
    /// Initial grid width in columns (before the first resize). 0 → default (80).
    pub initial_cols: u16,
    /// Initial grid height in rows. 0 → default (24).
    pub initial_rows: u16,
    /// Scrollback capacity in lines. 0 → default (10000).
    pub scrollback_lines: u32,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            initial_cols: 80,
            initial_rows: 24,
            scrollback_lines: 10_000,
        }
    }
}

impl EngineOptions {
    /// Normalize zero fields to their defaults (zero is the C "unset" sentinel).
    pub fn normalized(self) -> Self {
        let d = Self::default();
        Self {
            initial_cols: if self.initial_cols == 0 {
                d.initial_cols
            } else {
                self.initial_cols
            },
            initial_rows: if self.initial_rows == 0 {
                d.initial_rows
            } else {
                self.initial_rows
            },
            scrollback_lines: if self.scrollback_lines == 0 {
                d.scrollback_lines
            } else {
                self.scrollback_lines
            },
        }
    }
}

/// An owned byte buffer handed across FFI (e.g. selection text, error messages).
///
/// Allocated by Rust; the C# side copies the bytes immediately and then calls
/// [`crate::cmux_buffer_free`]. Each side frees only its own allocations (plan §6).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ByteBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
}

impl ByteBuffer {
    /// An empty buffer (null ptr, zero len/cap). Safe to `cmux_buffer_free`.
    pub const EMPTY: ByteBuffer = ByteBuffer {
        ptr: std::ptr::null_mut(),
        len: 0,
        cap: 0,
    };

    /// Move a `Vec<u8>` into a `ByteBuffer`, transferring ownership to the caller.
    pub fn from_vec(mut v: Vec<u8>) -> ByteBuffer {
        let ptr = v.as_mut_ptr();
        let len = v.len();
        let cap = v.capacity();
        std::mem::forget(v);
        ByteBuffer { ptr, len, cap }
    }

    /// Reclaim the `Vec<u8>` this buffer owns so it can be dropped. Null → no-op.
    ///
    /// # Safety
    /// Must be called at most once per buffer produced by [`ByteBuffer::from_vec`].
    pub unsafe fn free(self) {
        if !self.ptr.is_null() {
            // SAFETY: ptr/len/cap came from a `Vec<u8>` via `from_vec`.
            drop(unsafe { Vec::from_raw_parts(self.ptr, self.len, self.cap) });
        }
    }
}

/// `HostEvent.kind` discriminants (plan §6). Mirrored as plain `u32` on the C# side.
pub mod event_kind {
    pub const TOAST: u32 = 0;
    pub const TITLE: u32 = 1;
    pub const BELL: u32 = 2;
    pub const SET_USER_VAR: u32 = 3;
    pub const CWD: u32 = 4;
    pub const CHILD_EXIT: u32 = 5;
    pub const PROGRESS: u32 = 6;
}

/// A notification delivered from the engine (PTY reader / render thread) to the C# host
/// (plan §6). String fields are **borrowed** — valid only for the duration of the
/// callback; C# must copy them into managed `string`s immediately.
#[repr(C)]
pub struct HostEvent {
    pub kind: u32,
    pub title_utf8: *const u8,
    pub title_len: usize,
    pub body_utf8: *const u8,
    pub body_len: usize,
    /// Kind-specific scalar (e.g. child exit code, progress percent).
    pub arg0: i64,
}

/// The host-event callback signature (C ABI). `user_data` is the opaque pointer the host
/// registered (a pinned C# `GCHandle`).
pub type HostEventFn = extern "C" fn(user_data: *mut c_void, ev: *const HostEvent);

/// A `Send` bundle of the host callback + its `user_data`, owned by the engine and
/// invoked from the render/reader thread.
///
/// `user_data` is an opaque token the engine never dereferences; the C# side guarantees it
/// stays valid (rooted `GCHandle`) until the engine is destroyed.
#[derive(Clone, Copy)]
pub struct EventSink {
    cb: HostEventFn,
    user_data: *mut c_void,
}

// SAFETY: the callback is a plain C function pointer and `user_data` is an opaque token;
// moving them to the render/reader thread is exactly the intended use. The C# side keeps
// the GCHandle rooted for the engine's lifetime.
unsafe impl Send for EventSink {}

impl EventSink {
    pub fn new(cb: HostEventFn, user_data: *mut c_void) -> Self {
        Self { cb, user_data }
    }

    /// Invoke the host callback with a fully-formed event. Borrowed string pointers in
    /// `ev` must outlive this call (they do — the caller owns the backing storage).
    pub fn emit(&self, ev: &HostEvent) {
        (self.cb)(self.user_data, ev as *const HostEvent);
    }

    /// Convenience: emit a title/body event (Toast/Title/Cwd/SetUserVar) from Rust `str`s.
    /// The strings are borrowed for the duration of the callback only.
    pub fn emit_text(&self, kind: u32, title: &str, body: &str, arg0: i64) {
        let ev = HostEvent {
            kind,
            title_utf8: title.as_ptr(),
            title_len: title.len(),
            body_utf8: body.as_ptr(),
            body_len: body.len(),
            arg0,
        };
        self.emit(&ev);
    }

    /// Convenience: emit a scalar-only event (Bell/ChildExit/Progress) with no strings.
    pub fn emit_scalar(&self, kind: u32, arg0: i64) {
        let ev = HostEvent {
            kind,
            title_utf8: std::ptr::null(),
            title_len: 0,
            body_utf8: std::ptr::null(),
            body_len: 0,
            arg0,
        };
        self.emit(&ev);
    }
}
