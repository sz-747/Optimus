//! `wezterm-term` wiring (plan §2.2 / §8 U4): build the VT `Terminal` and route its parsed
//! OSC notifications to the host via the FFI event sink.
//!
//! We pick `wezterm-term` over `alacritty_terminal` specifically for its `AlertHandler`
//! trait, which surfaces `Alert::ToastNotification` (OSC 9 / OSC 777) and `Alert::SetUserVar`
//! (iTerm2 OSC 1337) as typed events — the foundation of the notification feature (Phase 3).
//! Phase 1 wires the path (and exercises it in spike 5) but does not yet surface events.

use std::sync::{Arc, Mutex};

use wezterm_term::color::ColorPalette;
use wezterm_term::{Alert, AlertHandler, Terminal, TerminalConfiguration, TerminalSize};

use crate::ffi::events::{event_kind, EventSink};

/// Shared, late-bound host-event sink. The render thread's [`EngineAlertHandler`] reads it;
/// the FFI `set_event_callback` (UI thread) writes it. `None` until the host registers.
pub type SharedSink = Arc<Mutex<Option<EventSink>>>;

/// Minimal [`TerminalConfiguration`]. Only `color_palette` is required; we also set the
/// scrollback depth and opt into title reporting so `Alert::WindowTitleChanged` fires.
#[derive(Debug)]
pub struct TermConfig {
    scrollback: usize,
}

impl TermConfig {
    pub fn new(scrollback: usize) -> Self {
        Self { scrollback }
    }
}

impl TerminalConfiguration for TermConfig {
    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }

    fn scrollback_size(&self) -> usize {
        self.scrollback
    }

    fn enable_title_reporting(&self) -> bool {
        true
    }
}

/// Forwards `wezterm-term` alerts to the host-event sink. Runs on the render thread (alerts
/// fire synchronously inside `advance_bytes`); the C# callback hops to the UI thread itself.
pub struct EngineAlertHandler {
    sink: SharedSink,
}

impl EngineAlertHandler {
    pub fn new(sink: SharedSink) -> Self {
        Self { sink }
    }
}

impl AlertHandler for EngineAlertHandler {
    fn alert(&mut self, alert: Alert) {
        let guard = match self.sink.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(sink) = guard.as_ref() else {
            return;
        };
        match alert {
            Alert::Bell => sink.emit_scalar(event_kind::BELL, 0),
            Alert::ToastNotification { title, body, .. } => {
                sink.emit_text(event_kind::TOAST, title.as_deref().unwrap_or(""), &body, 0)
            }
            Alert::WindowTitleChanged(title) => sink.emit_text(event_kind::TITLE, &title, "", 0),
            Alert::SetUserVar { name, value } => {
                sink.emit_text(event_kind::SET_USER_VAR, &name, &value, 0)
            }
            // CurrentWorkingDirectoryChanged carries no path on the alert; the render loop polls
            // `Terminal::get_current_dir()` instead. Remaining variants are not surfaced in Phase 1.
            _ => {}
        }
    }
}

/// Construct a `wezterm-term` `Terminal` wired to `writer` (the PTY input) and the host sink.
pub fn build_terminal(size: TerminalSize, scrollback: usize, writer: Box<dyn std::io::Write + Send>, sink: SharedSink) -> Terminal {
    let config = Arc::new(TermConfig::new(scrollback));
    let mut terminal = Terminal::new(size, config, "cmux", env!("CARGO_PKG_VERSION"), writer);
    terminal.set_notification_handler(Box::new(EngineAlertHandler::new(sink)));
    terminal
}
