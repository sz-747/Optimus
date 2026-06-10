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
    let mut terminal = Terminal::new(size, config, "optimus", env!("CARGO_PKG_VERSION"), writer);
    terminal.set_notification_handler(Box::new(EngineAlertHandler::new(sink)));
    terminal
}

// ===========================================================================================
// OSC 99 (Kitty desktop notifications) sniffer — plan Phase 3 U1.
//
// The pinned `wezterm-term` (tag 20240203-110809-5046fc22) parses OSC 9 / OSC 777 into
// `Alert::ToastNotification` but has no OSC-99 parser. Rather than fork the terminal core we
// observe the *same* byte stream that feeds `Terminal::advance_bytes`, recognize OSC 99, and
// surface it through the existing `event_kind::TOAST` host path (KTD2) — so a Kitty
// notification is indistinguishable downstream from OSC 9/777 (R1, AE3).
//
// The sniffer is a pure observer: it never modifies the stream (wezterm still sees the OSC 99
// and harmlessly ignores it). It is a small byte-at-a-time state machine so a sequence split
// across PTY read bursts (transport chunking) reassembles, and it accumulates `d=0` protocol
// chunks per identifier until the final `d=1` chunk arrives.
// ===========================================================================================

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Max bytes buffered for a single in-progress OSC 99 sequence before it is abandoned. Guards
/// a malformed/never-terminated sequence from growing unbounded (plan U1 risk #2).
const MAX_OSC99_PAYLOAD: usize = 64 * 1024;

/// Max combined title+body bytes accumulated across `d=0` chunks for one identifier.
const MAX_OSC99_ACCUM: usize = 64 * 1024;

/// Max number of distinct in-flight (chunked) notifications tracked at once.
const MAX_PENDING_NOTIFICATIONS: usize = 64;

/// A completed Kitty notification, ready to surface through the TOAST host-event path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc99Notification {
    pub title: String,
    pub body: String,
}

/// Per-identifier accumulator holding partial title/body across `d=0` protocol chunks.
struct Pending {
    id: String,
    title: String,
    body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Scanning ordinary output for the next `ESC`.
    Ground,
    /// Saw `ESC`; expecting `]` to begin an OSC.
    Escape,
    /// Saw `ESC ]`; reading the numeric command id (we only care if it is `99`).
    OscNum,
    /// Confirmed `ESC ] 99 ;`; accumulating the raw `<metadata>;<payload>` until a terminator.
    Payload,
    /// Saw `ESC` inside the payload; a following `\` is the ST terminator.
    PayloadEsc,
}

/// Stateful OSC-99 scanner. Held in the render thread's `RenderState` so state survives across
/// PTY read bursts. Feed it the same bytes handed to `Terminal::advance_bytes`.
pub struct Osc99Sniffer {
    state: State,
    num: Vec<u8>,
    buf: Vec<u8>,
    pending: Vec<Pending>,
}

impl Default for Osc99Sniffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Osc99Sniffer {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            num: Vec::new(),
            buf: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Observe `bytes`, returning any notifications that completed within them. Returns an empty
    /// (non-allocating) vec for the common pass-through case.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Osc99Notification> {
        let mut out = Vec::new();
        for &b in bytes {
            self.step(b, &mut out);
        }
        out
    }

    fn step(&mut self, b: u8, out: &mut Vec<Osc99Notification>) {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Escape;
                }
            }
            State::Escape => match b {
                b']' => {
                    self.num.clear();
                    self.state = State::OscNum;
                }
                ESC => {} // consecutive ESC: stay, wait for the dispatch byte
                _ => self.state = State::Ground,
            },
            State::OscNum => match b {
                b'0'..=b'9' => {
                    if self.num.len() < 4 {
                        self.num.push(b);
                    } else {
                        self.reset_scan(); // implausibly long command id
                    }
                }
                b';' => {
                    if self.num == b"99" {
                        self.buf.clear();
                        self.state = State::Payload;
                    } else {
                        // Some other OSC (9, 777, 1337, ...) — leave it to wezterm-term.
                        self.state = State::Ground;
                    }
                }
                ESC => self.state = State::Escape,
                _ => self.state = State::Ground,
            },
            State::Payload => match b {
                BEL => self.complete(out),
                ESC => self.state = State::PayloadEsc,
                _ => {
                    if self.buf.len() >= MAX_OSC99_PAYLOAD {
                        self.reset_scan(); // runaway: abandon, no emit
                    } else {
                        self.buf.push(b);
                    }
                }
            },
            State::PayloadEsc => match b {
                b'\\' => self.complete(out), // ST terminator (ESC \)
                ESC => {}                    // another ESC: keep waiting for the `\`
                _ => self.reset_scan(),      // ESC not part of ST: malformed, abandon
            },
        }
    }

    fn reset_scan(&mut self) {
        self.num.clear();
        self.buf.clear();
        self.state = State::Ground;
    }

    fn complete(&mut self, out: &mut Vec<Osc99Notification>) {
        let raw = std::mem::take(&mut self.buf);
        self.num.clear();
        self.state = State::Ground;
        self.process(&raw, out);
    }

    fn process(&mut self, raw: &[u8], out: &mut Vec<Osc99Notification>) {
        // raw is `<metadata> ; <payload>`; split on the FIRST ';' (the payload may contain more).
        let (meta_bytes, payload): (&[u8], &[u8]) = match raw.iter().position(|&c| c == b';') {
            Some(i) => (&raw[..i], &raw[i + 1..]),
            None => (raw, &b""[..]),
        };
        let meta = String::from_utf8_lossy(meta_bytes);

        // Defaults matter (the protocol is easy to mis-default): p=title, d=1 (done), e=0 (raw).
        let mut p_is_body = false;
        let mut done = true;
        let mut base64 = false;
        let mut id = String::new();
        for pair in meta.split(':') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k {
                "p" => p_is_body = v == "body",
                "d" => done = v != "0",
                "e" => base64 = v == "1",
                "i" => id = v.to_string(),
                _ => {} // ignore unknown keys (e.g. urgency)
            }
        }

        let text = if base64 {
            match decode_base64(payload) {
                Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                None => return, // malformed base64: drop the sequence
            }
        } else {
            String::from_utf8_lossy(payload).into_owned()
        };

        let idx = match self.pending.iter().position(|p| p.id == id) {
            Some(i) => i,
            None => {
                if self.pending.len() >= MAX_PENDING_NOTIFICATIONS {
                    self.pending.remove(0); // drop the oldest in-flight notification
                }
                self.pending.push(Pending {
                    id: id.clone(),
                    title: String::new(),
                    body: String::new(),
                });
                self.pending.len() - 1
            }
        };

        let over = {
            let acc = &mut self.pending[idx];
            let target = if p_is_body { &mut acc.body } else { &mut acc.title };
            if target.len() + text.len() > MAX_OSC99_ACCUM {
                true
            } else {
                target.push_str(&text);
                false
            }
        };
        if over {
            self.pending.remove(idx); // runaway accumulation: drop it
            return;
        }

        if done {
            let acc = self.pending.remove(idx);
            out.push(Osc99Notification {
                title: acc.title,
                body: acc.body,
            });
        }
    }
}

/// Minimal standard-alphabet base64 decoder (no external crate). Tolerates embedded CR/LF,
/// stops at the first `=` padding, and returns `None` on an invalid character.
fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &c in input {
        if c == b'=' {
            break;
        }
        if c == b'\r' || c == b'\n' {
            continue;
        }
        acc = (acc << 6) | val(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, body: &str) -> Osc99Notification {
        Osc99Notification {
            title: title.to_string(),
            body: body.to_string(),
        }
    }

    fn feed_all(input: &[u8]) -> Vec<Osc99Notification> {
        Osc99Sniffer::new().feed(input)
    }

    #[test]
    fn bare_payload_defaults_to_title() {
        // ESC ] 99 ; ; hello BEL — no metadata; default p=title (NOT body). Covers AE3.
        assert_eq!(feed_all(b"\x1b]99;;hello\x07"), vec![note("hello", "")]);
    }

    #[test]
    fn explicit_body_payload() {
        assert_eq!(feed_all(b"\x1b]99;p=body;the body\x1b\\"), vec![note("", "the body")]);
    }

    #[test]
    fn protocol_chunking_combines_title_and_body() {
        let mut s = Osc99Sniffer::new();
        // First chunk: title, more coming (d=0).
        assert!(s.feed(b"\x1b]99;p=title:i=7:d=0;Build\x07").is_empty());
        // Final chunk: body, done (d=1) — combine into one emit.
        assert_eq!(s.feed(b"\x1b]99;p=body:i=7:d=1;passed\x07"), vec![note("Build", "passed")]);
    }

    #[test]
    fn base64_payload_is_decoded() {
        // base64("hi") = "aGk=".
        assert_eq!(feed_all(b"\x1b]99;e=1;aGk=\x07"), vec![note("hi", "")]);
    }

    #[test]
    fn transport_split_assembles_to_single_emit() {
        let mut s = Osc99Sniffer::new();
        assert!(s.feed(b"\x1b]99;;hel").is_empty());
        assert_eq!(s.feed(b"lo\x07"), vec![note("hello", "")]);
    }

    #[test]
    fn st_terminator_accepted() {
        assert_eq!(feed_all(b"\x1b]99;;hi\x1b\\"), vec![note("hi", "")]);
    }

    #[test]
    fn osc9_and_osc777_are_ignored() {
        // These are wezterm-term's job; the sniffer must not double-handle them.
        assert!(feed_all(b"\x1b]9;hello\x07").is_empty());
        assert!(feed_all(b"\x1b]777;title;body\x1b\\").is_empty());
    }

    #[test]
    fn runaway_sequence_is_capped_without_emit() {
        let mut input = b"\x1b]99;".to_vec();
        input.extend(std::iter::repeat(b'a').take(70_000));
        assert!(feed_all(&input).is_empty());
        // And the sniffer recovers: a well-formed sequence afterwards still parses.
        let mut s = Osc99Sniffer::new();
        let _ = s.feed(&input);
        assert_eq!(s.feed(b"\x1b]99;;ok\x07"), vec![note("ok", "")]);
    }

    #[test]
    fn interleaved_text_yields_exactly_one_emit() {
        assert_eq!(feed_all(b"hello \x1b]99;;world\x07 bye"), vec![note("world", "")]);
    }

    #[test]
    fn unknown_metadata_key_is_ignored() {
        assert_eq!(feed_all(b"\x1b]99;u=2:p=body;text\x07"), vec![note("", "text")]);
    }

    #[test]
    fn payload_may_contain_semicolons() {
        // Only the first ';' separates metadata from payload.
        assert_eq!(feed_all(b"\x1b]99;;a;b;c\x07"), vec![note("a;b;c", "")]);
    }
}
