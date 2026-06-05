//! FFI support layer (plan §6): panic guard, last-error channel, and string helpers
//! shared by the `#[no_mangle]` entry points in `lib.rs`.
//!
//! The `#[repr(C)]` POD types and the host-event callback live in [`events`] (the file
//! csbindgen scans). This module holds the *non*-ABI helpers, so it is deliberately kept
//! out of the csbindgen input set.

pub mod events;

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::slice;

use self::events::ByteBuffer;

thread_local! {
    /// Last error message for the current thread, surfaced via `cmux_last_error_message`.
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Record a human-readable error for the calling thread.
pub fn set_last_error(msg: impl Into<String>) {
    let msg = msg.into();
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(msg));
}

/// Take (and clear) the last error for the calling thread.
pub fn take_last_error() -> Option<String> {
    LAST_ERROR.with(|e| e.borrow_mut().take())
}

/// Run an FFI body, trapping any panic at the boundary (unwinding across `extern "C"`
/// into the CLR is UB). On panic, record an error and return `default`.
pub fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(payload) => {
            let msg = panic_message(&payload);
            set_last_error(format!("panic at FFI boundary: {msg}"));
            default
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// View a borrowed UTF-8 `ptr + len` argument as a `&str` (lossless: returns `None` if the
/// bytes are not valid UTF-8). A null/zero-length pointer yields `Some("")`.
///
/// # Safety
/// `ptr` must be valid for `len` bytes (or null when `len == 0`).
pub unsafe fn str_arg<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if len == 0 {
        return Some("");
    }
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `ptr` is valid for `len` bytes.
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes).ok()
}

/// Write `s` (UTF-8) into `*out` as an owned [`ByteBuffer`] the caller must free with
/// `cmux_buffer_free`. Returns `0` on success, `-1` if `out` is null.
///
/// # Safety
/// `out` must be a valid, writable `*mut ByteBuffer` (or null, handled as an error).
pub unsafe fn write_string_out(out: *mut ByteBuffer, s: &str) -> i32 {
    if out.is_null() {
        return -1;
    }
    let buf = ByteBuffer::from_vec(s.as_bytes().to_vec());
    // SAFETY: `out` is non-null and writable per the caller's contract.
    unsafe { out.write(buf) };
    0
}
