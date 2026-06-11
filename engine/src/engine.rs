//! The `Engine`: the object behind the opaque `*mut Engine` FFI handle (plan §6).
//!
//! Threading model (plan §6): one **render thread** owns the `wezterm-term` `Terminal`, the
//! ConPTY, and the wgpu surface (DX12 surfaces are not freely `Send`). One **PTY reader
//! thread** does blocking reads and forwards byte chunks to the render thread over a channel.
//! The C# FFI calls (input/resize/etc.) post [`RenderCmd`] messages to the render thread; the
//! `Engine` itself is just the channel endpoint plus the worker-thread handle.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, Once};
use std::thread::JoinHandle;

use termwiz::input::{KeyCode, Modifiers};
use wezterm_term::{Terminal, TerminalSize};
use windows::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE};
use windows::Win32::System::Threading::GetCurrentProcess;

use crate::ffi::events::{event_kind, EngineOptions, EventSink};
use crate::pty::{default_shell, ConPty};
use crate::render::terminal::{SelectionSpan, TerminalRenderer};
use crate::vt::{build_terminal, Osc99Sniffer, SharedSink};

/// Messages posted to the render thread. Variants carrying a `SyncSender` are synchronous:
/// the FFI caller blocks until the render thread replies (attach/resize/selection are rare and
/// must report success/values back across the boundary).
enum RenderCmd {
    /// Hand the render thread a renderer already bound to the panel. The renderer is built on
    /// the **UI thread** (in [`Engine::attach_swapchain_panel`]) because the first
    /// `surface.configure` calls `ISwapChainPanelNative::SetSwapChain`, which has UI-thread
    /// affinity. Everything afterward (resize → `ResizeBuffers`, present) is thread-agnostic, so
    /// the render thread owns the renderer from here on.
    AttachRenderer {
        renderer: Box<TerminalRenderer>,
        width: u32,
        height: u32,
        reply: SyncSender<Result<(), String>>,
    },
    Detach,
    SpawnShell {
        cmdline: String,
        cwd: Option<String>,
        reply: SyncSender<Result<(), String>>,
    },
    PtyBytes(Vec<u8>),
    PtyEof,
    SendText(Vec<u8>),
    SendKey {
        vk: u32,
        modifiers: u32,
        down: bool,
    },
    // Fields consumed by U6/U8 (scrollback viewport + drag-selection); routed now so the FFI
    // and channel plumbing is complete and exercised end-to-end in U4.
    #[allow(dead_code)]
    SendMouse {
        x: f32,
        y: f32,
        button: u32,
        kind: u32,
        modifiers: u32,
    },
    #[allow(dead_code)]
    SendScroll {
        delta_lines: f32,
    },
    Resize {
        cols: u16,
        rows: u16,
        pixel_width: u32,
        pixel_height: u32,
        dpi_scale: f32,
        reply: SyncSender<Result<(), String>>,
    },
    SelectionText(SyncSender<Option<String>>),
    /// Debug/diagnostic: snapshot the visible grid as plain text (used by tests).
    ScreenText(SyncSender<String>),
    Shutdown,
}

/// Owns the terminal core, PTY, renderer, and worker threads for one terminal surface.
///
/// Opaque to C#: created with [`Engine::new`] (boxed into `*mut Engine` by the FFI layer)
/// and torn down on `optimus_engine_destroy`, which drops this and joins the render thread.
pub struct Engine {
    options: EngineOptions,
    tx: Sender<RenderCmd>,
    render_thread: Option<JoinHandle<()>>,
    /// Shared with the render thread's `AlertHandler`; updated by `set_event_callback`.
    sink: SharedSink,
    /// PID of the spawned ConPTY child, written by the render thread on a successful
    /// `SpawnShell` (before the synchronous reply), 0 until then; reset to 0 on child exit
    /// (PTY reader EOF) and on teardown, honoring the "0 when unavailable" contract. Read by
    /// `optimus_engine_child_pid` — diagnostics/measurement only; Job Object enrollment must
    /// use [`Engine::child_process_handle`] (PID reuse, see below).
    child_pid: Arc<AtomicU32>,
    /// Raw HANDLE value of an **engine-owned duplicate** of the ConPTY child's process handle,
    /// 0 when unavailable. Written together with `child_pid`; closed + zeroed on child exit
    /// and on teardown (the render thread owns the stored duplicate). `optimus_engine_child_process_handle`
    /// re-duplicates it per call so the host can `AssignProcessToJobObject` without an
    /// OpenProcess(pid) — immune to PID recycling.
    child_process_handle: Arc<AtomicUsize>,
}

impl Engine {
    pub fn new(options: EngineOptions) -> Self {
        let options = options.normalized();
        let sink: SharedSink = Arc::new(Mutex::new(None));
        let child_pid = Arc::new(AtomicU32::new(0));
        let child_process_handle = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel::<RenderCmd>();

        let render_sink = Arc::clone(&sink);
        let render_child_pid = Arc::clone(&child_pid);
        let render_child_handle = Arc::clone(&child_process_handle);
        let render_tx = tx.clone();
        let render_thread = std::thread::Builder::new()
            .name("optimus-render".into())
            .spawn(move || {
                render_loop(
                    options,
                    rx,
                    render_tx,
                    render_sink,
                    render_child_pid,
                    render_child_handle,
                )
            })
            .expect("spawn render thread");

        Self {
            options,
            tx,
            render_thread: Some(render_thread),
            sink,
            child_pid,
            child_process_handle,
        }
    }

    /// The Windows process id of the spawned ConPTY child, or 0 if no shell has been
    /// spawned (or the spawn failed). Set by the render thread before `spawn_shell`'s
    /// synchronous reply, so it is valid as soon as `spawn_shell` returns `Ok`.
    pub fn child_pid(&self) -> u32 {
        self.child_pid.load(Ordering::SeqCst)
    }

    /// A **fresh duplicate** of the ConPTY child's process handle (raw HANDLE value), or 0
    /// when unavailable. The caller owns the returned duplicate and must close it; the engine
    /// keeps its own internal handle (closed on child exit / teardown), so closing the
    /// returned value never invalidates engine state. Use this — not [`Engine::child_pid`] +
    /// `OpenProcess` — for Job Object enrollment: a handle cannot suffer PID reuse.
    pub fn child_process_handle(&self) -> usize {
        duplicate_handle_value(self.child_process_handle.load(Ordering::SeqCst))
    }

    /// Register the host-event callback (notifications/title/bell/cwd/exit).
    pub fn set_event_callback(&mut self, sink: EventSink) {
        if let Ok(mut guard) = self.sink.lock() {
            *guard = Some(sink);
        }
    }

    /// Bind the wgpu renderer to the WinUI 3 `SwapChainPanel` behind `panel`.
    ///
    /// **Must be called on the UI thread** (plan §5.2). wgpu's first `surface.configure` calls
    /// `ISwapChainPanelNative::SetSwapChain`, which has UI-thread affinity — so the renderer is
    /// constructed *here*, on the caller's thread, and only then handed to the render thread.
    /// (Building it on the render thread fails surface configuration with "Invalid surface".)
    ///
    /// # Safety
    /// `panel` must be a valid, live `ISwapChainPanel*` for the engine's lifetime, and this must
    /// be called on the thread that owns the `SwapChainPanel`.
    pub unsafe fn attach_swapchain_panel(&mut self, panel: *mut c_void) -> Result<(), String> {
        // Bind at a placeholder size; the C# resize/DPI loop (U9) reconfigures immediately with
        // real physical pixels. Derive a non-degenerate guess from the initial grid. DPI scale is
        // a placeholder 1.0 — the first resize sets the real scale via `renderer.set_scale`.
        let width = (self.options.initial_cols as u32 * 8).max(1);
        let height = (self.options.initial_rows as u32 * 16).max(1);

        // SAFETY: `panel` is a valid ISwapChainPanel* (caller's contract) and we are on the UI
        // thread, satisfying SetSwapChain's affinity requirement.
        let renderer = unsafe { TerminalRenderer::new(panel, width, height, 1.0) }
            .map_err(|e| format!("attach failed: {e}"))?;

        let (reply, wait) = std::sync::mpsc::sync_channel(0);
        self.send(RenderCmd::AttachRenderer {
            renderer: Box::new(renderer),
            width,
            height,
            reply,
        })?;
        wait.recv().map_err(|_| "render thread gone".to_string())?
    }

    /// Tear down the renderer surface (panel going away).
    pub fn detach(&mut self) {
        let _ = self.send(RenderCmd::Detach);
    }

    /// Spawn the shell inside a fresh ConPTY sized to the current grid. An empty `cmdline`
    /// selects the default shell (pwsh → powershell → cmd).
    pub fn spawn_shell(&mut self, cmdline: &str, cwd: Option<&str>) -> Result<(), String> {
        let cmdline = if cmdline.trim().is_empty() {
            default_shell()
        } else {
            cmdline.to_string()
        };
        let (reply, wait) = std::sync::mpsc::sync_channel(0);
        self.send(RenderCmd::SpawnShell {
            cmdline,
            cwd: cwd.map(str::to_string),
            reply,
        })?;
        wait.recv().map_err(|_| "render thread gone".to_string())?
    }

    /// Forward already-resolved text (layout/IME/paste) to the shell input.
    pub fn send_text(&mut self, text: &str) {
        let _ = self.send(RenderCmd::SendText(text.as_bytes().to_vec()));
    }

    /// Forward a key event (control keys, modifiers) to the terminal/PTY.
    pub fn send_key(&mut self, vk: u32, modifiers: u32, down: bool) {
        let _ = self.send(RenderCmd::SendKey {
            vk,
            modifiers,
            down,
        });
    }

    /// Forward a mouse event in physical pixels (mapped to cells on the render thread).
    pub fn send_mouse(&mut self, x: f32, y: f32, button: u32, kind: u32, modifiers: u32) {
        let _ = self.send(RenderCmd::SendMouse {
            x,
            y,
            button,
            kind,
            modifiers,
        });
    }

    /// Forward a scroll event (lines; +up / -down) to the scrollback viewport.
    pub fn send_scroll(&mut self, delta_lines: f32) {
        let _ = self.send(RenderCmd::SendScroll { delta_lines });
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
        let (reply, wait) = std::sync::mpsc::sync_channel(0);
        self.send(RenderCmd::Resize {
            cols,
            rows,
            pixel_width,
            pixel_height,
            dpi_scale,
            reply,
        })?;
        wait.recv().map_err(|_| "render thread gone".to_string())?
    }

    /// The current selection as plain text, or `None` if nothing is selected.
    pub fn selection_text(&self) -> Option<String> {
        let (reply, wait) = std::sync::mpsc::sync_channel(0);
        if self.tx.send(RenderCmd::SelectionText(reply)).is_err() {
            return None;
        }
        wait.recv().ok().flatten()
    }

    /// Snapshot the visible grid as plain text. Diagnostic helper (used by tests) — the GPU
    /// renderer (U6) reads the same `Screen` for drawing.
    pub fn screen_text(&self) -> String {
        let (reply, wait) = std::sync::mpsc::sync_channel(0);
        if self.tx.send(RenderCmd::ScreenText(reply)).is_err() {
            return String::new();
        }
        wait.recv().unwrap_or_default()
    }

    /// Read-only access to the engine's options (initial grid size, scrollback).
    pub fn options(&self) -> EngineOptions {
        self.options
    }

    fn send(&self, cmd: RenderCmd) -> Result<(), String> {
        self.tx
            .send(cmd)
            .map_err(|_| "render thread is not running".to_string())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Ask the render thread to tear down (close the pseudoconsole, join the reader, drop the
        // GPU surface) and wait for it, so no worker outlives the freed callback `user_data`.
        let _ = self.tx.send(RenderCmd::Shutdown);
        if let Some(handle) = self.render_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Per-engine state owned exclusively by the render thread.
///
/// Field order matters for teardown: `terminal` (holding the PTY input writer) is declared
/// before `pty` (which closes that handle on drop), so the writer is dropped first.
struct RenderState {
    options: EngineOptions,
    sink: SharedSink,
    self_tx: Sender<RenderCmd>,
    /// Shared with the owning [`Engine`]; stores the ConPTY child's PID on spawn (U4).
    child_pid: Arc<AtomicU32>,
    /// Shared with the owning [`Engine`]; stores an engine-owned duplicate of the child's
    /// process handle on spawn, closed + zeroed on child exit / teardown.
    child_process_handle: Arc<AtomicUsize>,

    terminal: Option<Terminal>,
    reader: Option<JoinHandle<()>>,
    pty: Option<ConPty>,
    renderer: Option<TerminalRenderer>,

    // Current grid + surface geometry (updated by Resize; seeded from options).
    cols: u16,
    rows: u16,
    pixel_width: u32,
    pixel_height: u32,
    dpi: u32,

    /// Lines scrolled back from the live bottom (0 = following output). Driven by SendScroll;
    /// reset to 0 when fresh PTY output arrives so new text is always visible.
    scroll_offset: usize,

    /// Active drag-selection (anchored in stable-row coordinates so it survives scrolling).
    selection: Option<Selection>,

    needs_present: bool,

    /// OSC-99 (Kitty notification) scanner (plan §8 U1). Observes the same PTY bytes as
    /// `Terminal::advance_bytes`; held here so a sequence split across read bursts reassembles.
    osc99: Osc99Sniffer,
}

/// A drag-selection. Endpoints are `(stable_row, column)`; `stable_row` is wezterm's
/// scrollback-stable index so the selection tracks the same text as content scrolls.
#[derive(Clone, Copy)]
struct Selection {
    anchor: (i64, usize),
    head: (i64, usize),
    /// True while the pointer button is held (drag in progress).
    active: bool,
}

/// Order two `(row, col)` points so the earlier (top-left) comes first.
fn order_points(a: (i64, usize), b: (i64, usize)) -> ((i64, usize), (i64, usize)) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The render thread's main loop: drain commands (coalescing redraws), present once per burst.
///
/// Every command and present is run under [`catch_unwind`] so a panic from the GPU/driver
/// (e.g. a wgpu surface error while a window crosses monitors / changes DPI) cannot tear down
/// the render thread and orphan a pending reply channel — which the C# host would otherwise see
/// as "render thread gone" and rethrow as a fatal unhandled exception. The panic location is
/// recorded to `optimus_engine.log` by the installed hook; the thread keeps running so a transient
/// fault recovers on the next frame and a persistent one merely leaves the panel blank.
fn render_loop(
    options: EngineOptions,
    rx: Receiver<RenderCmd>,
    self_tx: Sender<RenderCmd>,
    sink: SharedSink,
    child_pid: Arc<AtomicU32>,
    child_process_handle: Arc<AtomicUsize>,
) {
    install_render_panic_logger();

    let mut state = RenderState {
        options,
        sink,
        self_tx,
        child_pid,
        child_process_handle,
        terminal: None,
        reader: None,
        pty: None,
        renderer: None,
        cols: options.initial_cols,
        rows: options.initial_rows,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
        scroll_offset: 0,
        selection: None,
        needs_present: false,
        osc99: Osc99Sniffer::new(),
    };

    while let Ok(cmd) = rx.recv() {
        if state.handle_guarded(cmd) {
            break;
        }
        // Coalesce: drain anything already queued before presenting once.
        loop {
            match rx.try_recv() {
                Ok(cmd) => {
                    if state.handle_guarded(cmd) {
                        state.teardown();
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        if state.needs_present {
            state.present_guarded();
            state.needs_present = false;
        }
    }

    state.teardown();
}

/// Install (once, process-wide) a panic hook that appends the panic location + message to
/// `optimus_engine.log`. The render thread's [`crate::ffi::set_last_error`] is thread-local and so
/// can't reach the C# host (which reads it from the UI thread), so this file is the only durable
/// record of *why* a render-thread panic happened. The previous hook's behavior is preserved.
fn install_render_panic_logger() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown location>".to_string());
            let msg = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic>".to_string());
            append_engine_log(&format!("panic: {msg}  @ {loc}"));
            default(info);
        }));
    });
}

/// Best-effort append of a diagnostic line to `optimus_engine.log` (next to the running executable,
/// falling back to the system temp dir). Used for events that must survive the thread/FFI
/// boundary, where the thread-local last-error channel can't reach the host.
fn append_engine_log(line: &str) {
    use std::io::Write;
    let when = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let entry = format!("[{when}] {line}\n");

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let candidates = [
        exe_dir.map(|d| d.join("optimus_engine.log")),
        Some(std::env::temp_dir().join("optimus_engine.log")),
    ];
    for path in candidates.into_iter().flatten() {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(entry.as_bytes());
            return;
        }
    }
}

/// Record that the render thread trapped a panic. The hook has already logged the location +
/// message to `optimus_engine.log`; we keep the renderer (a transient GPU fault recovers next frame)
/// and leave a breadcrumb on this thread's last-error channel.
fn note_render_panic() {
    crate::ffi::set_last_error("render thread trapped a panic (see optimus_engine.log next to the exe)");
}

impl RenderState {
    /// Run [`Self::handle`] under a panic guard. Returns the stop flag (or `false` if a panic was
    /// trapped — keep the thread alive). On a trapped panic the command's reply channel (if any)
    /// has already been dropped during unwinding, so the FFI caller observes a soft error.
    fn handle_guarded(&mut self, cmd: RenderCmd) -> bool {
        match catch_unwind(AssertUnwindSafe(|| self.handle(cmd))) {
            Ok(stop) => stop,
            Err(_) => {
                note_render_panic();
                false
            }
        }
    }

    /// Run [`Self::present`] under a panic guard so a GPU error never tears down the thread.
    fn present_guarded(&mut self) {
        if catch_unwind(AssertUnwindSafe(|| self.present())).is_err() {
            note_render_panic();
        }
    }

    /// Handle one command. Returns `true` if the loop should stop (Shutdown).
    fn handle(&mut self, cmd: RenderCmd) -> bool {
        match cmd {
            RenderCmd::AttachRenderer {
                renderer,
                width,
                height,
                reply,
            } => {
                // The renderer was already built + bound on the UI thread (SetSwapChain affinity);
                // we just take ownership so all subsequent drawing/resizing happens here.
                self.renderer = Some(*renderer);
                self.pixel_width = width;
                self.pixel_height = height;
                self.needs_present = true;
                let _ = reply.send(Ok(()));
            }
            RenderCmd::Detach => {
                self.renderer = None;
            }
            RenderCmd::SpawnShell {
                cmdline,
                cwd,
                reply,
            } => {
                let _ = reply.send(self.spawn_shell(&cmdline, cwd.as_deref()));
            }
            RenderCmd::PtyBytes(buf) => {
                if let Some(term) = self.terminal.as_mut() {
                    term.advance_bytes(&buf);
                    // Snap the viewport to the live bottom on fresh output.
                    self.scroll_offset = 0;
                    self.needs_present = true;
                }
                // OSC 99 (Kitty notifications): wezterm-term doesn't parse them, so sniff the same
                // bytes and surface through the existing TOAST path (plan §8 U1, KTD2).
                let toasts = self.osc99.feed(&buf);
                if !toasts.is_empty() {
                    if let Ok(guard) = self.sink.lock() {
                        if let Some(sink) = guard.as_ref() {
                            for t in &toasts {
                                sink.emit_text(event_kind::TOAST, &t.title, &t.body, 0);
                            }
                        }
                    }
                }
            }
            RenderCmd::PtyEof => {
                let code = self.pty.as_ref().and_then(ConPty::exit_code).unwrap_or(0);
                // The child exited: clear the published PID + handle so the FFI honors its
                // "0 when unavailable" contract (a stale PID could be recycled by the OS).
                self.clear_child();
                if let Ok(guard) = self.sink.lock() {
                    if let Some(sink) = guard.as_ref() {
                        sink.emit_scalar(event_kind::CHILD_EXIT, code as i64);
                    }
                }
            }
            RenderCmd::SendText(bytes) => {
                if let Some(pty) = self.pty.as_ref() {
                    let _ = pty.write(&bytes);
                }
            }
            RenderCmd::SendKey {
                vk,
                modifiers,
                down,
            } => {
                // Phase 1 acts on key-down only (key-up matters for kitty-keyboard, deferred).
                if down {
                    if let Some((key, mods)) = map_key(vk, modifiers) {
                        if let Some(term) = self.terminal.as_mut() {
                            // key_down encodes per the terminal's modes (e.g. DECCKM for arrows)
                            // and writes through the Terminal's PTY writer.
                            let _ = term.key_down(key, mods);
                        }
                    }
                }
            }
            RenderCmd::SendMouse {
                x,
                y,
                button,
                kind,
                ..
            } => {
                self.handle_mouse(x, y, button, kind);
            }
            RenderCmd::SendScroll { delta_lines } => {
                // Positive delta scrolls toward history (older), negative toward the bottom.
                let max_offset = self
                    .terminal
                    .as_ref()
                    .map(|t| {
                        let s = t.screen();
                        s.scrollback_rows().saturating_sub(s.physical_rows)
                    })
                    .unwrap_or(0);
                let delta = delta_lines.round() as i64;
                let next = (self.scroll_offset as i64 + delta).clamp(0, max_offset as i64);
                if next as usize != self.scroll_offset {
                    self.scroll_offset = next as usize;
                    self.needs_present = true;
                }
            }
            RenderCmd::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
                dpi_scale,
                reply,
            } => {
                let _ = reply.send(self.resize(cols, rows, pixel_width, pixel_height, dpi_scale));
            }
            RenderCmd::SelectionText(reply) => {
                let _ = reply.send(self.compute_selection_text());
            }
            RenderCmd::ScreenText(reply) => {
                let text = self.terminal.as_ref().map(dump_screen).unwrap_or_default();
                let _ = reply.send(text);
            }
            RenderCmd::Shutdown => return true,
        }
        false
    }

    fn spawn_shell(&mut self, cmdline: &str, cwd: Option<&str>) -> Result<(), String> {
        let pty = ConPty::spawn(cmdline, cwd, self.cols, self.rows)
            .map_err(|e| format!("spawn shell failed: {e}"))?;
        // Publish the child PID + an engine-owned duplicate of its process handle before the
        // synchronous reply unblocks the FFI caller, so `optimus_engine_child_pid` /
        // `optimus_engine_child_process_handle` are valid the moment `spawn_shell` returns Ok (U4).
        self.publish_child(&pty);

        let size = TerminalSize {
            rows: self.rows as usize,
            cols: self.cols as usize,
            pixel_width: self.pixel_width as usize,
            pixel_height: self.pixel_height as usize,
            dpi: self.dpi,
        };
        let writer = Box::new(pty.input_writer());
        let terminal = build_terminal(
            size,
            self.options.scrollback_lines as usize,
            writer,
            Arc::clone(&self.sink),
        );

        // PTY reader thread: blocking reads → bytes forwarded to this render thread.
        let mut reader = pty.output_reader();
        let tx = self.self_tx.clone();
        let handle = std::thread::Builder::new()
            .name("optimus-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            let _ = tx.send(RenderCmd::PtyEof);
                            break;
                        }
                        Ok(n) => {
                            if tx.send(RenderCmd::PtyBytes(buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            let _ = tx.send(RenderCmd::PtyEof);
                            break;
                        }
                    }
                }
            })
            .map_err(|e| format!("spawn reader thread failed: {e}"))?;

        self.pty = Some(pty);
        self.terminal = Some(terminal);
        self.reader = Some(handle);
        self.selection = None;
        self.needs_present = true;
        Ok(())
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u32,
        pixel_height: u32,
        dpi_scale: f32,
    ) -> Result<(), String> {
        self.pixel_width = pixel_width.max(1);
        self.pixel_height = pixel_height.max(1);
        self.dpi = ((96.0 * dpi_scale).round() as u32).max(1);

        // Update the renderer's DPI + surface first so cell metrics reflect the new scale before
        // we derive the grid from pixels.
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_scale(dpi_scale);
            renderer.resize(self.pixel_width, self.pixel_height);
        }

        // Engine-authoritative grid: cols/rows == 0 means "derive from pixels using the engine's
        // own cell metrics" (the C# side knows panel pixels + DPI but not the font's cell size).
        let (cols, rows) = if cols == 0 || rows == 0 {
            match self.renderer.as_ref() {
                Some(renderer) => {
                    let (cw, ch) = renderer.cell_size();
                    let c = (self.pixel_width as f32 / cw).floor().max(1.0) as u16;
                    let r = (self.pixel_height as f32 / ch).floor().max(1.0) as u16;
                    (c, r)
                }
                // No renderer yet (headless/tests): keep the previously configured grid.
                None => (self.cols, self.rows),
            }
        } else {
            (cols, rows)
        };
        self.cols = cols.max(1);
        self.rows = rows.max(1);

        if let Some(term) = self.terminal.as_mut() {
            term.resize(TerminalSize {
                rows: self.rows as usize,
                cols: self.cols as usize,
                pixel_width: self.pixel_width as usize,
                pixel_height: self.pixel_height as usize,
                dpi: self.dpi,
            });
        }
        if let Some(pty) = self.pty.as_ref() {
            pty.resize(self.cols, self.rows)
                .map_err(|e| format!("resize pseudoconsole failed: {e}"))?;
        }
        self.needs_present = true;
        Ok(())
    }

    /// Present one frame: the grid + glyph passes driven by the `Terminal`'s `Screen` (U6).
    /// Without a terminal yet, there is nothing to draw (the surface keeps its cleared contents).
    fn present(&mut self) {
        let scroll_offset = self.scroll_offset;
        let selection = self.normalized_selection_span();
        if let (Some(renderer), Some(term)) = (self.renderer.as_mut(), self.terminal.as_ref()) {
            // Ignore transient FrameUnavailable (resize/DPI churn); the next burst re-presents.
            let _ = renderer.render(term, scroll_offset, true, selection);
        }
    }

    /// Update the drag-selection from a pointer event (physical pixels, left button only).
    fn handle_mouse(&mut self, x: f32, y: f32, button: u32, kind: u32) {
        // 0 = left button; only the left button drives selection in Phase 1.
        if button != 0 {
            return;
        }
        let (cell_w, cell_h) = match self.renderer.as_ref() {
            Some(r) => r.cell_size(),
            None => return,
        };
        let col = if cell_w > 0.0 { (x / cell_w).max(0.0) as usize } else { 0 };
        let rendered_row = if cell_h > 0.0 { (y / cell_h).max(0.0) as usize } else { 0 };
        let stable = match self.rendered_row_to_stable(rendered_row) {
            Some(s) => s,
            None => return,
        };

        match kind {
            0 => {
                // Down: start a new selection anchored here.
                self.selection = Some(Selection {
                    anchor: (stable, col),
                    head: (stable, col),
                    active: true,
                });
                self.needs_present = true;
            }
            1 => {
                // Move: extend the active selection.
                if let Some(sel) = self.selection.as_mut() {
                    if sel.active {
                        sel.head = (stable, col);
                        self.needs_present = true;
                    }
                }
            }
            2 => {
                // Up: finish the drag (keep the selection for copy).
                if let Some(sel) = self.selection.as_mut() {
                    sel.active = false;
                }
            }
            _ => {}
        }
    }

    /// Map a row index within the currently-rendered viewport (0 = top visible row) to a stable
    /// scrollback row index.
    fn rendered_row_to_stable(&self, rendered_row: usize) -> Option<i64> {
        let term = self.terminal.as_ref()?;
        let screen = term.screen();
        let visible = screen.physical_rows;
        if visible == 0 {
            return None;
        }
        let total = screen.scrollback_rows();
        let bottom = total.saturating_sub(self.scroll_offset);
        let start = bottom.saturating_sub(visible);
        let phys = (start + rendered_row.min(visible - 1)).min(total.saturating_sub(1));
        Some(screen.phys_to_stable_row_index(phys) as i64)
    }

    /// The current selection normalized to a top-left → bottom-right span, or `None` when there
    /// is no selection or it is empty (a click without a drag).
    fn normalized_selection_span(&self) -> Option<SelectionSpan> {
        let sel = self.selection?;
        let (a, b) = order_points(sel.anchor, sel.head);
        if a == b {
            return None;
        }
        Some(SelectionSpan {
            start_row: a.0,
            start_col: a.1,
            end_row: b.0,
            end_col: b.1,
        })
    }

    /// Extract the selected text (one `\n`-joined line per row, trailing spaces trimmed).
    fn compute_selection_text(&self) -> Option<String> {
        let span = self.normalized_selection_span()?;
        let term = self.terminal.as_ref()?;
        let screen = term.screen();
        let cols = screen.physical_cols;

        let mut out = String::new();
        let mut first = true;
        for stable in span.start_row..=span.end_row {
            let phys = match screen.stable_row_to_phys(stable as isize) {
                Some(p) => p,
                None => continue, // scrolled off the top of the buffer
            };
            let lines = screen.lines_in_phys_range(phys..phys + 1);
            let line = match lines.first() {
                Some(l) => l,
                None => continue,
            };
            let (c0, c1) = span.column_range(stable, cols);
            let c1 = c1.min(cols);
            let segment = if c1 > c0 {
                line.columns_as_str(c0..c1)
            } else {
                String::new()
            };
            if !first {
                out.push('\n');
            }
            out.push_str(segment.trim_end());
            first = false;
        }

        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Ordered teardown (plan §7.2): close the pseudoconsole first so the blocked reader hits
    /// EOF, then join the reader, then drop the terminal/PTY/surface.
    fn teardown(&mut self) {
        // Clear the published child identity before the PTY (and its original process handle)
        // goes away, honoring the FFI's "0 when unavailable" contract on engine drop.
        self.clear_child();
        if let Some(pty) = self.pty.as_ref() {
            pty.shutdown();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.terminal = None;
        self.pty = None;
        self.renderer = None;
    }

    /// Publish the freshly spawned child's PID and an **engine-owned duplicate** of its process
    /// handle (the duplicate outlives the `ConPty`'s own handle, so the FFI can re-duplicate it
    /// at any time without racing PTY teardown). Any previously stored duplicate is closed.
    fn publish_child(&self, pty: &ConPty) {
        self.child_pid.store(pty.child_pid(), Ordering::SeqCst);
        let dup = duplicate_handle_value(pty.child_process_handle().0 as usize);
        close_handle_value(self.child_process_handle.swap(dup, Ordering::SeqCst));
    }

    /// Reset the published child identity to "unavailable" (PID 0 / handle 0) and close the
    /// engine-owned duplicate. Called on child exit (PTY reader EOF) and on teardown.
    fn clear_child(&self) {
        self.child_pid.store(0, Ordering::SeqCst);
        close_handle_value(self.child_process_handle.swap(0, Ordering::SeqCst));
    }
}

/// Duplicate a raw process-HANDLE value within the current process (`DUPLICATE_SAME_ACCESS`).
/// Returns the duplicate's raw value, or 0 on failure / when `raw` is 0. The caller owns the
/// returned handle and must close it via [`close_handle_value`] (or hand ownership on).
fn duplicate_handle_value(raw: usize) -> usize {
    if raw == 0 {
        return 0;
    }
    let mut dup = HANDLE::default();
    // SAFETY: `raw` is a live handle value owned by this process; GetCurrentProcess is a
    // pseudo-handle that needs no closing.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(raw as *mut c_void),
            GetCurrentProcess(),
            &mut dup,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    };
    match ok {
        Ok(()) => dup.0 as usize,
        Err(_) => 0,
    }
}

/// Close a raw HANDLE value previously produced by [`duplicate_handle_value`]. No-op on 0.
fn close_handle_value(raw: usize) {
    if raw != 0 {
        // SAFETY: `raw` is an owned, live handle value (our own duplicate).
        let _ = unsafe { CloseHandle(HANDLE(raw as *mut c_void)) };
    }
}

/// Snapshot the visible grid (the tail `physical_rows` of the line buffer) as plain text,
/// trailing whitespace trimmed per line. Diagnostic only.
fn dump_screen(terminal: &Terminal) -> String {
    let screen = terminal.screen();
    let total = screen.scrollback_rows();
    let visible = screen.physical_rows;
    let start = total.saturating_sub(visible);
    screen
        .lines_in_phys_range(start..total)
        .iter()
        .map(|line| line.as_str().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map a Windows virtual-key code + modifier bitmask to a `wezterm-term` key.
///
/// Returns `None` for plain printable keys (no Ctrl/Alt) — those arrive as resolved text via
/// `optimus_engine_send_text` (`CharacterReceived`), so encoding them here would double-type.
fn map_key(vk: u32, modifiers: u32) -> Option<(KeyCode, Modifiers)> {
    let mods = map_modifiers(modifiers);

    // Named control keys: always forwarded.
    let named = match vk {
        0x0D => Some(KeyCode::Enter),
        0x09 => Some(KeyCode::Tab),
        0x1B => Some(KeyCode::Escape),
        0x08 => Some(KeyCode::Backspace),
        0x2E => Some(KeyCode::Delete),
        0x2D => Some(KeyCode::Insert),
        0x25 => Some(KeyCode::LeftArrow),
        0x26 => Some(KeyCode::UpArrow),
        0x27 => Some(KeyCode::RightArrow),
        0x28 => Some(KeyCode::DownArrow),
        0x24 => Some(KeyCode::Home),
        0x23 => Some(KeyCode::End),
        0x21 => Some(KeyCode::PageUp),
        0x22 => Some(KeyCode::PageDown),
        0x70..=0x7B => Some(KeyCode::Function((vk - 0x70 + 1) as u8)),
        _ => None,
    };
    if let Some(key) = named {
        return Some((key, mods));
    }

    // Printable keys only when Ctrl or Alt is held (e.g. Ctrl-C). Plain text goes via send_text.
    let has_ctrl_or_alt = mods.intersects(Modifiers::CTRL | Modifiers::ALT);
    if !has_ctrl_or_alt {
        return None;
    }
    let ch = match vk {
        0x41..=0x5A => Some((b'a' + (vk - 0x41) as u8) as char), // A-Z → lowercase; mods carry shift
        0x30..=0x39 => Some((b'0' + (vk - 0x30) as u8) as char), // 0-9
        0x20 => Some(' '),
        _ => None,
    }?;
    Some((KeyCode::Char(ch), mods))
}

/// Map the C# `KeyModifiers` bitmask (Shift=1, Ctrl=2, Alt=4, Super=8) to termwiz `Modifiers`.
fn map_modifiers(modifiers: u32) -> Modifiers {
    let mut mods = Modifiers::NONE;
    if modifiers & 0x1 != 0 {
        mods |= Modifiers::SHIFT;
    }
    if modifiers & 0x2 != 0 {
        mods |= Modifiers::CTRL;
    }
    if modifiers & 0x4 != 0 {
        mods |= Modifiers::ALT;
    }
    if modifiers & 0x8 != 0 {
        mods |= Modifiers::SUPER;
    }
    mods
}
