//! Spike 1 (plan §7.1) C ABI for the SwapChainPanel renderer.
//!
//! This is the *minimal* hand-rolled FFI that lets the WinUI 3 host drive the wgpu
//! renderer: create a [`PanelRenderer`] from an `ISwapChainPanel*`, clear+present a
//! frame, resize, and destroy. The full csbindgen-generated ABI is U3; these few
//! `cmux_spike_*` entry points exist only to prove the wgpu↔SwapChainPanel path
//! end-to-end before that scaffolding is built.
//!
//! Every entry point catches panics at the boundary — a Rust panic unwinding across
//! the C ABI into the CLR is undefined behavior, so we trap it and return a safe value
//! (plan §6 / §9 "FFI memory/callback safety").

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::PanelRenderer;

/// Create a renderer bound to the WinUI 3 `SwapChainPanel` behind `panel`, configured
/// at `width`×`height` physical pixels. Returns an opaque handle, or null on failure.
///
/// # Safety
/// `panel` must be a valid `ISwapChainPanel*` (from `ISwapChainPanelNative`). The
/// returned pointer must be released exactly once with [`cmux_spike_renderer_destroy`].
#[no_mangle]
pub unsafe extern "C" fn cmux_spike_renderer_create(
    panel: *mut c_void,
    width: u32,
    height: u32,
) -> *mut PanelRenderer {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller guarantees `panel` is a valid ISwapChainPanel*.
        // Scale 1.0: the spike only clears to a solid color, which fills the surface
        // regardless of the composition transform.
        match unsafe { PanelRenderer::new(panel, width, height, 1.0) } {
            Ok(r) => Box::into_raw(Box::new(r)),
            Err(_) => std::ptr::null_mut(),
        }
    }));
    result.unwrap_or(std::ptr::null_mut())
}

/// Clear the panel to the given color (components in 0.0..=1.0) and present one frame.
///
/// Returns a status so the host can confirm the present loop actually runs:
/// `0` = frame presented, `1` = frame unavailable this tick (non-fatal: timeout/occluded/
/// outdated — surface was reconfigured), `-1` = null handle or panic at the boundary.
///
/// # Safety
/// `renderer` must be a live handle from [`cmux_spike_renderer_create`].
#[no_mangle]
pub unsafe extern "C" fn cmux_spike_renderer_render(
    renderer: *mut PanelRenderer,
    r: f64,
    g: f64,
    b: f64,
) -> i32 {
    if renderer.is_null() {
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: non-null handle owned by the caller for the duration of this call.
        let renderer = unsafe { &mut *renderer };
        match renderer.clear_and_present(r, g, b, 1.0) {
            Ok(()) => 0,
            // Frame-unavailable (timeout/occluded/outdated) is non-fatal — drop the frame.
            Err(_) => 1,
        }
    }));
    result.unwrap_or(-1)
}

/// Resize the panel's swapchain to `width`×`height` physical pixels.
///
/// # Safety
/// `renderer` must be a live handle from [`cmux_spike_renderer_create`].
#[no_mangle]
pub unsafe extern "C" fn cmux_spike_renderer_resize(
    renderer: *mut PanelRenderer,
    width: u32,
    height: u32,
) {
    if renderer.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: non-null handle owned by the caller for the duration of this call.
        let renderer = unsafe { &mut *renderer };
        renderer.resize(width, height);
    }));
}

/// Destroy a renderer handle. Safe to call with null (no-op).
///
/// # Safety
/// `renderer` must be a handle from [`cmux_spike_renderer_create`] not already destroyed.
#[no_mangle]
pub unsafe extern "C" fn cmux_spike_renderer_destroy(renderer: *mut PanelRenderer) {
    if renderer.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: reclaim the Box created in `cmux_spike_renderer_create`.
        drop(unsafe { Box::from_raw(renderer) });
    }));
}
