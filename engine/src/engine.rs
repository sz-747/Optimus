//! The `Engine`: one terminal surface = one ConPTY + one worker thread + one reader thread.
//!
//! Threading model (migration plan §3): one **worker thread** per engine owns the ConPTY
//! lifetime — the single proven owner of PTY spawn/teardown ordering. One **PTY reader
//! thread** does blocking reads and forwards byte chunks to the worker over a channel; the
//! worker sniffs OSC 99 and hands the same bytes to the caller's `on_bytes` hook (the Tauri
//! shell forwards them to xterm.js). Public methods post [`Cmd`] messages; `spawn_shell` and
//! `resize` are synchronous (the caller needs the result).

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Once};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, WaitForSingleObject, INFINITE,
};

use crate::job::JobObject;
use crate::pty::{default_shell, ConPty};
use crate::vt::Osc99Sniffer;

/// Engine construction options.
#[derive(Clone, Copy, Debug)]
pub struct EngineOptions {
    /// Initial grid width in columns (before the first resize). 0 → default (80).
    pub initial_cols: u16,
    /// Initial grid height in rows. 0 → default (24).
    pub initial_rows: u16,
    /// Per-process hard memory cap for the ConPTY child's Job Object, in bytes (plan §3.3 /
    /// U4; typically 2× the capacity model's per-terminal budget). 0 → no cap (the job still
    /// carries `KILL_ON_JOB_CLOSE`, just unbounded memory). The child always gets a job when
    /// creation succeeds; this only sets `JOB_OBJECT_LIMIT_PROCESS_MEMORY`.
    pub job_memory_limit_bytes: usize,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            initial_cols: 80,
            initial_rows: 24,
            job_memory_limit_bytes: 0,
        }
    }
}

impl EngineOptions {
    /// Normalize zero fields to their defaults (zero = "unset").
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
            job_memory_limit_bytes: self.job_memory_limit_bytes,
        }
    }
}

/// Engine failures. Replaces the FFI thread-local last-error channel with an ordinary error
/// type carried on `Result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

fn err(msg: impl Into<String>) -> EngineError {
    EngineError(msg.into())
}

/// Out-of-band events surfaced by the engine (everything else — title, bell, cwd — is
/// parsed frontend-side by xterm.js from the same byte stream).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// A completed OSC-99 (Kitty) desktop notification.
    Toast { title: String, body: String },
    /// The ConPTY child exited (PTY reader hit EOF).
    ChildExit { code: i32 },
}

/// Raw PTY output hook — called on the worker thread with each byte burst.
pub type BytesFn = Box<dyn FnMut(&[u8]) + Send>;
/// Engine-event hook — called on the worker thread.
pub type EventFn = Box<dyn FnMut(EngineEvent) + Send>;

/// Messages posted to the worker thread. Variants carrying a `SyncSender` are synchronous:
/// the caller blocks until the worker replies.
enum Cmd {
    SpawnShell {
        cmdline: String,
        cwd: Option<String>,
        environment: Vec<(String, String)>,
        reply: SyncSender<Result<(), EngineError>>,
    },
    PtyBytes(Vec<u8>),
    PtyEof,
    /// The child process exited (waiter thread observed the process handle signal).
    /// Carries the authoritative exit code. Sent independently of `PtyEof` — conhost keeps
    /// the output pipe open until `ClosePseudoConsole`, so EOF alone never signals exit.
    ChildExited(i32),
    SendText(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
        reply: SyncSender<Result<(), EngineError>>,
    },
    Shutdown,
}

/// Owns the ConPTY and worker/reader threads for one terminal surface.
pub struct Engine {
    options: EngineOptions,
    tx: Sender<Cmd>,
    worker: Option<JoinHandle<()>>,
    /// PID of the spawned ConPTY child, written by the worker on a successful `SpawnShell`
    /// (before the synchronous reply), 0 until then; reset to 0 on child exit (PTY reader
    /// EOF) and on teardown — the "0 when unavailable" contract. Diagnostics/measurement
    /// only; Job Object enrollment must use [`Engine::child_process_handle`] (PID reuse).
    child_pid: Arc<AtomicU32>,
    /// Raw HANDLE value of an **engine-owned duplicate** of the ConPTY child's process
    /// handle, 0 when unavailable. Written together with `child_pid`; closed + zeroed on
    /// child exit and on teardown. [`Engine::child_process_handle`] re-duplicates it per
    /// call so the caller can `AssignProcessToJobObject` without an `OpenProcess(pid)` —
    /// immune to PID recycling.
    child_process_handle: Arc<AtomicUsize>,
}

impl Engine {
    /// Create an engine. `on_bytes` receives every PTY output burst; `on_event` receives
    /// [`EngineEvent`]s. Both are invoked on the worker thread — hop threads yourself if
    /// you need to (a Tauri `Channel` send is fine as-is).
    pub fn new(
        options: EngineOptions,
        on_bytes: BytesFn,
        on_event: EventFn,
    ) -> Result<Self, EngineError> {
        let options = options.normalized();
        let child_pid = Arc::new(AtomicU32::new(0));
        let child_process_handle = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel::<Cmd>();

        let worker_child_pid = Arc::clone(&child_pid);
        let worker_child_handle = Arc::clone(&child_process_handle);
        let worker_tx = tx.clone();
        let worker = std::thread::Builder::new()
            .name("optimus-worker".into())
            .spawn(move || {
                worker_loop(
                    options,
                    rx,
                    worker_tx,
                    on_bytes,
                    on_event,
                    worker_child_pid,
                    worker_child_handle,
                )
            })
            .map_err(|e| err(format!("spawn worker thread failed: {e}")))?;

        Ok(Self {
            options,
            tx,
            worker: Some(worker),
            child_pid,
            child_process_handle,
        })
    }

    /// The Windows process id of the spawned ConPTY child, or 0 if no shell has been
    /// spawned (or the spawn failed / the child exited). Valid as soon as `spawn_shell`
    /// returns `Ok` (the worker publishes it before the synchronous reply).
    pub fn child_pid(&self) -> u32 {
        self.child_pid.load(Ordering::SeqCst)
    }

    /// A **fresh duplicate** of the ConPTY child's process handle (raw HANDLE value), or 0
    /// when unavailable. The caller owns the returned duplicate and must close it; the
    /// engine keeps its own internal duplicate (closed on child exit / teardown). Use this
    /// — not [`Engine::child_pid`] + `OpenProcess` — for Job Object enrollment: a handle
    /// cannot suffer PID reuse.
    pub fn child_process_handle(&self) -> usize {
        duplicate_handle_value(self.child_process_handle.load(Ordering::SeqCst))
    }

    /// Spawn the shell inside a fresh ConPTY sized to the current grid. An empty `cmdline`
    /// selects the default shell (pwsh → powershell → cmd).
    pub fn spawn_shell(&mut self, cmdline: &str, cwd: Option<&str>) -> Result<(), EngineError> {
        self.spawn_shell_with_environment(cmdline, cwd, &[])
    }

    /// Spawn a shell with child-only environment overrides. Overrides are merged with the
    /// inherited process environment without mutating it, so parallel panes cannot exchange
    /// caller identity or socket routing.
    pub fn spawn_shell_with_environment(
        &mut self,
        cmdline: &str,
        cwd: Option<&str>,
        environment: &[(String, String)],
    ) -> Result<(), EngineError> {
        let cmdline = if cmdline.trim().is_empty() {
            default_shell()
        } else {
            cmdline.to_string()
        };
        let (reply, wait) = std::sync::mpsc::sync_channel(0);
        self.send(Cmd::SpawnShell {
            cmdline,
            cwd: cwd.map(str::to_string),
            environment: environment.to_vec(),
            reply,
        })?;
        wait.recv().map_err(|_| err("worker thread gone"))?
    }

    /// Forward input (keystrokes as encoded by xterm.js `onData`, or paste) to the shell.
    pub fn send_text(&mut self, text: &str) {
        let _ = self.send(Cmd::SendText(text.as_bytes().to_vec()));
    }

    /// Resize the grid + pseudoconsole. Pixel geometry and DPI are the frontend's problem
    /// now — xterm.js fit computes cols/rows and this just forwards them.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError> {
        let (reply, wait) = std::sync::mpsc::sync_channel(0);
        self.send(Cmd::Resize { cols, rows, reply })?;
        wait.recv().map_err(|_| err("worker thread gone"))?
    }

    /// Read-only access to the engine's options (initial grid size).
    pub fn options(&self) -> EngineOptions {
        self.options
    }

    fn send(&self, cmd: Cmd) -> Result<(), EngineError> {
        self.tx
            .send(cmd)
            .map_err(|_| err("worker thread is not running"))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Ask the worker to tear down (close the pseudoconsole, join the reader) and wait
        // for it, so no thread outlives the hooks' captured state.
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

/// Per-engine state owned exclusively by the worker thread.
struct WorkerState {
    self_tx: Sender<Cmd>,
    on_bytes: BytesFn,
    on_event: EventFn,
    /// Shared with the owning [`Engine`]; stores the ConPTY child's PID on spawn.
    child_pid: Arc<AtomicU32>,
    /// Shared with the owning [`Engine`]; stores an engine-owned duplicate of the child's
    /// process handle on spawn, closed + zeroed on child exit / teardown.
    child_process_handle: Arc<AtomicUsize>,

    reader: Option<JoinHandle<()>>,
    pty: Option<ConPty>,
    /// The ConPTY child's Job Object (tier-2 memory backstop / KILL_ON_JOB_CLOSE reap). `None`
    /// before spawn or when job creation failed (best-effort). Dropped in `teardown` **after**
    /// `pty.shutdown()`, so a cleanly-exiting shell is never job-killed first.
    job: Option<JobObject>,
    /// Per-process job memory cap in bytes (from [`EngineOptions`]); 0 = uncapped.
    job_memory_limit_bytes: usize,
    /// Ensures `ChildExit` is emitted exactly once per spawn — the waiter thread
    /// (`ChildExited`) and PTY-reader EOF (`PtyEof`) can both observe the same exit.
    exit_emitted: bool,

    // Current grid (updated by Resize; seeded from options).
    cols: u16,
    rows: u16,

    /// OSC-99 (Kitty notification) scanner. Observes the same PTY bytes handed to
    /// `on_bytes`; held here so a sequence split across read bursts reassembles.
    osc99: Osc99Sniffer,
}

/// The worker thread's main loop.
///
/// Every command runs under [`catch_unwind`] so a panic (ours or a hook's) cannot tear down
/// the worker and orphan a pending reply channel — the caller would see that as "worker
/// thread gone". The panic location is logged by the installed hook; the thread keeps
/// running (plan §3 item 7: a panicking surface reports and the process survives).
fn worker_loop(
    options: EngineOptions,
    rx: Receiver<Cmd>,
    self_tx: Sender<Cmd>,
    on_bytes: BytesFn,
    on_event: EventFn,
    child_pid: Arc<AtomicU32>,
    child_process_handle: Arc<AtomicUsize>,
) {
    install_panic_logger();

    let mut state = WorkerState {
        self_tx,
        on_bytes,
        on_event,
        child_pid,
        child_process_handle,
        reader: None,
        pty: None,
        job: None,
        job_memory_limit_bytes: options.job_memory_limit_bytes,
        exit_emitted: false,
        cols: options.initial_cols,
        rows: options.initial_rows,
        osc99: Osc99Sniffer::new(),
    };

    while let Ok(cmd) = rx.recv() {
        if state.handle_guarded(cmd) {
            break;
        }
    }

    state.teardown();
}

/// Install (once, process-wide) a panic hook that appends the panic location + message to
/// `optimus_engine.log`. The worker's panics are trapped (not propagated), so this file is
/// the durable record of *why* one happened. The previous hook's behavior is preserved.
fn install_panic_logger() {
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

/// Best-effort append of a diagnostic line to `optimus_engine.log` (next to the running
/// executable, falling back to the system temp dir).
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

impl WorkerState {
    /// Run [`Self::handle`] under a panic guard. Returns the stop flag (or `false` if a
    /// panic was trapped — keep the thread alive). On a trapped panic the command's reply
    /// channel (if any) was dropped during unwinding, so the caller observes a soft error.
    fn handle_guarded(&mut self, cmd: Cmd) -> bool {
        match catch_unwind(AssertUnwindSafe(|| self.handle(cmd))) {
            Ok(stop) => stop,
            Err(_) => {
                append_engine_log("worker thread trapped a panic (command dropped)");
                false
            }
        }
    }

    /// Handle one command. Returns `true` if the loop should stop (Shutdown).
    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::SpawnShell {
                cmdline,
                cwd,
                environment,
                reply,
            } => {
                let _ = reply.send(self.spawn_shell(&cmdline, cwd.as_deref(), &environment));
            }
            Cmd::PtyBytes(buf) => {
                // OSC 99 (Kitty notifications): xterm.js has no OSC-99 handler and agents
                // emit it from arbitrary children — the backend sniff is authoritative.
                for t in self.osc99.feed(&buf) {
                    (self.on_event)(EngineEvent::Toast {
                        title: t.title,
                        body: t.body,
                    });
                }
                (self.on_bytes)(&buf);
            }
            Cmd::PtyEof => {
                let code = self.pty.as_ref().and_then(ConPty::exit_code).unwrap_or(0);
                self.emit_child_exit(code);
            }
            Cmd::ChildExited(code) => {
                self.emit_child_exit(code);
            }
            Cmd::SendText(bytes) => {
                if let Some(pty) = self.pty.as_ref() {
                    let _ = pty.write(&bytes);
                }
            }
            Cmd::Resize { cols, rows, reply } => {
                let _ = reply.send(self.resize(cols, rows));
            }
            Cmd::Shutdown => return true,
        }
        false
    }

    fn spawn_shell(
        &mut self,
        cmdline: &str,
        cwd: Option<&str>,
        environment: &[(String, String)],
    ) -> Result<(), EngineError> {
        let pty = ConPty::spawn_with_environment(cmdline, cwd, self.cols, self.rows, environment)
            .map_err(|e| err(format!("spawn shell failed: {e}")))?;
        // Publish the child PID + an engine-owned duplicate of its process handle before
        // the synchronous reply unblocks the caller, so both are valid the moment
        // `spawn_shell` returns Ok.
        self.publish_child(&pty);

        // Enroll the child in a Job Object (plan §3.3): KILL_ON_JOB_CLOSE ties every shell's
        // lifetime to this process, plus an optional hard per-process memory cap. Best-effort —
        // `create_for` returns None on failure and the terminal simply runs unbackstopped.
        // Uses the ConPTY's own handle (valid now); the assignment outlives it.
        self.job = JobObject::create_for(pty.child_process_handle(), self.job_memory_limit_bytes);

        // PTY reader thread: blocking reads → bytes forwarded to this worker thread.
        let mut reader = pty.output_reader();
        let tx = self.self_tx.clone();
        let handle = std::thread::Builder::new()
            .name("optimus-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            let _ = tx.send(Cmd::PtyEof);
                            break;
                        }
                        Ok(n) => {
                            if tx.send(Cmd::PtyBytes(buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            let _ = tx.send(Cmd::PtyEof);
                            break;
                        }
                    }
                }
            })
            .map_err(|e| err(format!("spawn reader thread failed: {e}")))?;

        // Child-exit waiter: conhost keeps the output pipe open until ClosePseudoConsole,
        // so PTY EOF never fires on a plain child exit — wait on the process handle
        // instead. The waiter owns its own duplicate; after teardown (which terminates the
        // child via ClosePseudoConsole) the wait completes and the thread exits.
        let wait_handle = duplicate_handle_value(pty.child_process_handle().0 as usize);
        if wait_handle != 0 {
            let tx = self.self_tx.clone();
            let spawned = std::thread::Builder::new()
                .name("optimus-child-wait".into())
                .spawn(move || {
                    let h = HANDLE(wait_handle as *mut c_void);
                    // SAFETY: `h` is an owned, live duplicate; closed below on this thread.
                    unsafe {
                        WaitForSingleObject(h, INFINITE);
                        let mut code: u32 = 0;
                        let _ = GetExitCodeProcess(h, &mut code);
                        let _ = CloseHandle(h);
                        let _ = tx.send(Cmd::ChildExited(code as i32));
                    }
                });
            if spawned.is_err() {
                close_handle_value(wait_handle);
            }
        }

        self.pty = Some(pty);
        self.reader = Some(handle);
        self.exit_emitted = false;
        Ok(())
    }

    /// Emit `ChildExit` exactly once per spawn and clear the published child identity (a
    /// stale PID could be recycled by the OS under a caller's feet).
    fn emit_child_exit(&mut self, code: i32) {
        if self.exit_emitted {
            return;
        }
        self.exit_emitted = true;
        self.clear_child();
        (self.on_event)(EngineEvent::ChildExit { code });
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError> {
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        if let Some(pty) = self.pty.as_ref() {
            pty.resize(self.cols, self.rows)
                .map_err(|e| err(format!("resize pseudoconsole failed: {e}")))?;
        }
        Ok(())
    }

    /// Ordered teardown (crash-critical, plan §3 item 6): close the pseudoconsole first so
    /// the blocked reader hits EOF, then join the reader, then drop the PTY (handle close).
    fn teardown(&mut self) {
        // Clear the published child identity before the PTY (and its original process
        // handle) goes away.
        self.clear_child();
        if let Some(pty) = self.pty.as_ref() {
            pty.shutdown();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.pty = None;
        // Dispose the job last: ClosePseudoConsole above already gave the shell a clean exit,
        // so closing the job here (KILL_ON_JOB_CLOSE) only reaps stragglers — never job-first.
        self.job = None;
    }

    /// Publish the freshly spawned child's PID and an **engine-owned duplicate** of its
    /// process handle (the duplicate outlives the `ConPty`'s own handle, so the caller can
    /// re-duplicate it at any time without racing PTY teardown). Any previously stored
    /// duplicate is closed.
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
