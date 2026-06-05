//! The `Engine`: the object behind the opaque `*mut Engine` FFI handle (plan §6).
//!
//! **U3 status: stub.** This file establishes the type and method surface the FFI layer
//! (`lib.rs`) calls into, so the full C ABI + csbindgen bindings can be stood up and
//! exercised. U4 replaces the bodies with the real implementation: a `wezterm-term`
//! `Terminal` + render thread + PTY reader thread + channels (plan §6 threading).

use std::ffi::c_void;

use crate::ffi::events::{EngineOptions, EventSink};

/// Owns the terminal core, PTY, renderer, and worker threads for one terminal surface.
///
/// Opaque to C#: created with [`Engine::new`] (boxed into `*mut Engine` by the FFI layer)
/// and torn down on `cmux_engine_destroy`.
pub struct Engine {
    options: EngineOptions,
    event_sink: Option<EventSink>,

    // --- U3 stub bookkeeping (replaced by real state in U4) ---
    panel_attached: bool,
    shell_spawned: bool,
}

impl Engine {
    pub fn new(options: EngineOptions) -> Self {
        Self {
            options: options.normalized(),
            event_sink: None,
            panel_attached: false,
            shell_spawned: false,
        }
    }

    /// Register the host-event callback (notifications/title/bell/cwd/exit/progress).
    pub fn set_event_callback(&mut self, sink: EventSink) {
        self.event_sink = Some(sink);
    }

    /// Bind the wgpu renderer to the WinUI 3 `SwapChainPanel` behind `panel`.
    ///
    /// Called on the UI thread (plan §5.2). U4/U6 hand the pointer to the render thread.
    ///
    /// # Safety
    /// `panel` must be a valid, live `ISwapChainPanel*` for the engine's lifetime.
    pub unsafe fn attach_swapchain_panel(&mut self, panel: *mut c_void) -> Result<(), String> {
        let _ = panel; // U6: create the wgpu surface on the render thread.
        self.panel_attached = true;
        Ok(())
    }

    /// Tear down the renderer surface (panel going away). U6 stops presents first.
    pub fn detach(&mut self) {
        self.panel_attached = false;
    }

    /// Spawn the shell inside a fresh ConPTY sized to the current grid.
    pub fn spawn_shell(&mut self, cmdline: &str, cwd: Option<&str>) -> Result<(), String> {
        let _ = (cmdline, cwd); // U4/U5: create ConPty + reader thread + advance_bytes loop.
        self.shell_spawned = true;
        Ok(())
    }

    /// Forward already-resolved text (layout/IME/paste) to the shell input.
    pub fn send_text(&mut self, text: &str) {
        let _ = text; // U4/U7: write to the ConPTY input pipe.
    }

    /// Forward a key event (control keys, modifiers) to the terminal/PTY.
    pub fn send_key(&mut self, vk: u32, modifiers: u32, down: bool) {
        let _ = (vk, modifiers, down); // U4/U7.
    }

    /// Forward a mouse event in physical pixels (mapped to cells in U7).
    pub fn send_mouse(&mut self, x: f32, y: f32, button: u32, kind: u32, modifiers: u32) {
        let _ = (x, y, button, kind, modifiers); // U4/U7.
    }

    /// Forward a scroll event (lines; +up / -down) to the scrollback viewport / PTY.
    pub fn send_scroll(&mut self, delta_lines: f32) {
        let _ = delta_lines; // U4/U7.
    }

    /// Resize: grid (cols/rows), surface (physical px), and pseudoconsole, with DPI scale.
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u32,
        pixel_height: u32,
        dpi_scale: f32,
    ) -> Result<(), String> {
        let _ = (cols, rows, pixel_width, pixel_height, dpi_scale); // U4/U6/U9.
        Ok(())
    }

    /// The current selection as plain text, or `None` if nothing is selected.
    pub fn selection_text(&self) -> Option<String> {
        None // U4/U8.
    }

    /// Read-only access to the engine's options (initial grid size, scrollback).
    pub fn options(&self) -> EngineOptions {
        self.options
    }

    /// Whether a host-event callback is currently registered (used by U4 wiring/tests).
    pub fn has_event_sink(&self) -> bool {
        self.event_sink.is_some()
    }
}
