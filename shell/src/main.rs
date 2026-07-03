// No console window in release — hard requirement (plan §2, GUI launch).
// Debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use optimus_engine::{Engine, EngineEvent, EngineOptions};
use tauri::ipc::Channel;
use tauri::Manager;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use optimus_shell::domain::capacity::{CapacityModel, CapacityProvider};
use optimus_shell::domain::ids::{BranchId, PaneId, SurfaceId};
use optimus_shell::domain::notifications::{NotificationId, TerminalNotification};
use optimus_shell::domain::split_tree::{Direction, Orientation};
use optimus_shell::domain::surface_manager::{Surface, SurfaceFactory};
use optimus_shell::domain::workspace::WorkspaceId;
use optimus_shell::host::DomainHost;
use optimus_shell::ipc::access::{self, AuthState};
use optimus_shell::ipc::dpapi::Win32SecretProtector;
use optimus_shell::ipc::naming;
use optimus_shell::ipc::password::PasswordStore;
use optimus_shell::ipc::pipe_server::{DispatchFn, PipeServer, PipeServerConfig};
use optimus_shell::ipc::router::{self, SocketEffects};
use optimus_shell::view::{CapacityView, SidebarRowView, ToastView, TreeView};

/// The live engine-backed [`Surface`]: owns one [`Engine`] behind a `Mutex<Option<_>>`. The
/// `Option` lets [`shutdown`](Surface::shutdown) drop the engine exactly once (idempotent — R2);
/// the `Mutex` gives the `&self` trait methods the `&mut Engine` they need and makes the
/// `Send`-but-`!Sync` engine `Sync`, so the manager can hold it as `Arc<dyn Surface>`.
struct EngineSurface {
    id: SurfaceId,
    engine: Mutex<Option<Engine>>,
}

impl EngineSurface {
    fn new(id: SurfaceId, engine: Engine) -> Self {
        Self {
            id,
            engine: Mutex::new(Some(engine)),
        }
    }

    /// Run `f` against the live engine; nothing if it has already been shut down.
    fn with_engine<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
        self.engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .map(f)
    }
}

impl Surface for EngineSurface {
    fn id(&self) -> SurfaceId {
        self.id
    }

    // Compositing + OS focus are the WebView/xterm.js's job now — the engine surface owns no
    // window, so these levers no-op (the C# GPU-panel activate/focus has no analog here).
    fn set_active(&self, _active: bool) {}
    fn focus_surface(&self) {}

    fn send_text(&self, text: &str) {
        self.with_engine(|e| e.send_text(text));
    }

    fn send_key(&self, virtual_key: u32, modifiers: u32) {
        if let Some(bytes) = encode_key(virtual_key, modifiers) {
            self.with_engine(|e| e.send_text(&bytes));
        }
    }

    fn resize(&self, cols: u16, rows: u16) {
        // Resize failure is non-fatal (the child may have exited); the grid just stays as-is.
        self.with_engine(|e| {
            let _ = e.resize(cols, rows);
        });
    }

    fn shutdown(&self) {
        // Drop the engine — its Drop drains + tears down the PTY and joins the worker. take()
        // makes a second shutdown a no-op (R2/R9).
        let _ = self
            .engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

/// Windows virtual-key + modifier bitmask → the VT byte sequence a terminal expects. The frontend
/// encodes its own keys via xterm.js `onData`; this is the external-agent `surface.send_key` path.
/// Modifier bits match `ChordModifiers` (Ctrl=1, Shift=2, Alt=4, Super=8).
///
/// ponytail: covers Enter/Tab/Esc/Backspace/arrows/Home/End/Delete + Ctrl-letter; returns `None`
/// for anything unmapped so an unknown key is dropped rather than mis-encoded. Grow the table (or
/// move to xterm's CSI-u model) when agents need function/keypad keys or Shift+arrow selection.
fn encode_key(virtual_key: u32, modifiers: u32) -> Option<String> {
    const CTRL: u32 = 1;
    const ALT: u32 = 4;
    let alt = modifiers & ALT != 0;

    // Ctrl + A..Z → control byte 0x01..0x1A (Ctrl-A = 0x01, Ctrl-C = 0x03).
    if modifiers & CTRL != 0 && (0x41..=0x5A).contains(&virtual_key) {
        return Some(char::from((virtual_key - 0x40) as u8).to_string());
    }

    let seq = match virtual_key {
        0x0D => "\r",      // VK_RETURN
        0x09 => "\t",      // VK_TAB
        0x1B => "\x1b",    // VK_ESCAPE
        0x08 => "\x7f",    // VK_BACK → DEL (terminal convention)
        0x25 => "\x1b[D",  // VK_LEFT
        0x26 => "\x1b[A",  // VK_UP
        0x27 => "\x1b[C",  // VK_RIGHT
        0x28 => "\x1b[B",  // VK_DOWN
        0x24 => "\x1b[H",  // VK_HOME
        0x23 => "\x1b[F",  // VK_END
        0x2E => "\x1b[3~", // VK_DELETE
        _ => return None,
    };
    // Alt is meta: prefix ESC (the common xterm convention).
    Some(if alt {
        format!("\x1b{seq}")
    } else {
        seq.to_string()
    })
}

/// A per-surface output sink staged by the spawn command just before creation. The factory runs
/// inside `try_create_surface` on the domain thread, so it cannot receive the invoke's `Channel`
/// through the [`SurfaceFactory::create`] signature — the command stashes it here by id first.
struct StagedSink {
    on_output: Channel<Vec<u8>>,
    on_event: Channel<String>,
}

/// Builds [`EngineSurface`]s for the domain's `SurfaceManager` — the single owner of live engines
/// now (KTD9; the flat `Vec<Engine>` dev-plumbing is gone). Each spawn stages its per-surface
/// [`Channel`]s (via [`Self::stage`]); `create` pops them and wires a fresh engine's byte + event
/// streams to that invoke's channels, then spawns the shell.
struct EngineSurfaceFactory {
    /// Per-surface sinks awaiting creation, keyed by the id the spawn command will create.
    pending: Mutex<HashMap<SurfaceId, StagedSink>>,
    job_memory_limit_bytes: usize,
}

impl EngineSurfaceFactory {
    fn new(job_memory_limit_bytes: usize) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            job_memory_limit_bytes,
        }
    }

    /// Stash this invoke's output/event channels under `id` so the imminent `create(id, …)` wires
    /// the engine to them.
    fn stage(&self, id: SurfaceId, on_output: Channel<Vec<u8>>, on_event: Channel<String>) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id,
                StagedSink {
                    on_output,
                    on_event,
                },
            );
    }

    /// Drop a staged sink whose creation never happened (refused at cap, or a dead domain thread).
    /// Idempotent — a sink already consumed by `create` is simply absent.
    fn unstage(&self, id: SurfaceId) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}

impl SurfaceFactory for EngineSurfaceFactory {
    fn create(
        &self,
        id: SurfaceId,
        cwd: Option<&str>,
        cmdline: Option<&str>,
    ) -> Result<Arc<dyn Surface>, String> {
        let StagedSink {
            on_output,
            on_event,
        } = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
            .ok_or_else(|| {
                format!("surface {id}: no staged output channel (spawn command must stage first)")
            })?;

        let mut engine = Engine::new(
            EngineOptions {
                job_memory_limit_bytes: self.job_memory_limit_bytes,
                ..EngineOptions::default()
            },
            Box::new(move |bytes| {
                let _ = on_output.send(bytes.to_vec());
            }),
            Box::new(move |ev| {
                let msg = match ev {
                    EngineEvent::Toast { title, body } => format!("toast: {title} — {body}"),
                    EngineEvent::ChildExit { code } => format!("child exit: {code}"),
                };
                let _ = on_event.send(msg);
            }),
        )
        .map_err(|e| e.to_string())?;
        engine
            .spawn_shell(cmdline.unwrap_or(""), cwd)
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(EngineSurface::new(id, engine)))
    }
}

/// Production capacity facts from `GlobalMemoryStatusEx`. ponytail: a static safe-zone cap computed
/// once at startup from available RAM + commit headroom — the hard crown-jewel guarantee (spawns
/// refuse at the cap). The adaptive tier (OS low-memory notifications, per-process calibration via
/// `GetPerformanceInfo` / `PrivateUsage`) is deferred; wire it (plan U3) when the cap must tighten
/// under live pressure.
struct Win32CapacityProvider;

impl Win32CapacityProvider {
    fn status() -> MEMORYSTATUSEX {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is a live, correctly-sized MEMORYSTATUSEX; the call only writes into it.
        let _ = unsafe { GlobalMemoryStatusEx(&mut status) };
        status
    }
}

impl CapacityProvider for Win32CapacityProvider {
    fn total_phys_bytes(&self) -> u64 {
        Self::status().ullTotalPhys
    }
    fn available_phys_bytes(&self) -> u64 {
        Self::status().ullAvailPhys
    }
    fn commit_headroom_bytes(&self) -> u64 {
        // ullAvailPageFile = CommitLimit − CommitTotal (how much more can be committed).
        // GetPerformanceInfo would give the exact pages; this is close enough for the cap.
        Self::status().ullAvailPageFile
    }
    fn is_low_memory_signaled(&self) -> bool {
        false // ponytail: static cap — no live tightening yet (see struct doc)
    }
    fn subscribe_low_memory(&self, _listener: Box<dyn Fn() + Send + Sync>) {}
    fn measure_process_private_bytes(&self, _pid: i32) -> Option<u64> {
        None // calibration refinement deferred (the seed budget governs the cap)
    }
}

/// P4 exit-gate command: back the selected workspace's focused surface with a live shell and stream
/// its raw VT bytes to the frontend over a per-surface `Channel` (backpressured — plan §3 item 4;
/// never `AppHandle::emit` for PTY output). Creation runs through the domain's capacity-governed
/// `SurfaceManager`, so a spawn past the RAM safe-zone cap is refused. Returns the [`SurfaceId`] the
/// frontend echoes back for input/resize — the same id external agents name over the pipe.
///
/// ponytail: one live terminal backing the model's seeded pane. Multi-pane spawn (allocating a new
/// tree tab + setting `OPTIMUS_SURFACE_ID` in the child env) is future work; today the agent path
/// resolves to this one surface via `selected_focused_surface`.
#[tauri::command]
fn dev_spawn_shell(
    factory: tauri::State<'_, Arc<EngineSurfaceFactory>>,
    host: tauri::State<'_, DomainHost>,
    on_output: Channel<Vec<u8>>,
    on_event: Channel<String>,
) -> Result<i32, String> {
    let surface = host
        .query(|d| d.selected_focused_surface())
        .ok_or("no focused surface to back")?;

    factory.stage(surface, on_output, on_event);

    // Option<_> so `query` has a Default for a dead domain thread (None → the error arm below).
    let outcome: Option<Result<i32, String>> = host.query(move |d| {
        Some(match d.try_create_surface(surface, None, None) {
            Ok(Some(id)) => Ok(id.0),
            Ok(None) => Err("safe-zone full — close a workspace to spawn more".to_string()),
            Err(e) => Err(e.to_string()),
        })
    });

    match outcome {
        Some(Ok(id)) => Ok(id),
        Some(Err(msg)) => {
            factory.unstage(surface); // refused at cap: the staged sink was never consumed
            Err(msg)
        }
        None => {
            factory.unstage(surface);
            Err("domain thread unavailable".to_string())
        }
    }
}

#[tauri::command]
fn dev_send_text(host: tauri::State<'_, DomainHost>, id: i32, text: String) -> Result<(), String> {
    host.run(move |d| d.send_text(SurfaceId(id), &text));
    Ok(())
}

#[tauri::command]
fn dev_resize(
    host: tauri::State<'_, DomainHost>,
    id: i32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    host.run(move |d| d.resize(SurfaceId(id), cols, rows));
    Ok(())
}

// ---- P4.2 split-tree command surface -----------------------------------------------------------
// The frontend drives structural ops (spawn/split/new-tab/close/focus/zoom/divider) through these
// and re-renders from the [`TreeView`] each returns — the tree only changes on these calls, so no
// separate event bus is needed. Spawn/split/new-tab additionally stage a per-surface `Channel`
// (like `dev_spawn_shell`) and back the new model pane with an engine through the capacity gate.

/// The reply to a spawn/split/new-tab: the id the frontend echoes back for input/resize, plus the
/// fresh layout to render the newly-created pane into.
#[derive(serde::Serialize)]
struct SpawnResult {
    surface_id: i32,
    tree: TreeView,
}

/// A close op's result: the surface ids whose engines were torn down (so the frontend disposes
/// exactly those terminals) plus the fresh layout.
#[derive(serde::Serialize, Default)]
struct CloseResult {
    closed: Vec<i32>,
    tree: TreeView,
}

/// Split direction → the controller's (orientation, insert-first) pair. Vertical = side-by-side.
fn parse_split(direction: &str) -> Result<(Orientation, bool), String> {
    Ok(match direction {
        "right" => (Orientation::Vertical, false),
        "left" => (Orientation::Vertical, true),
        "down" => (Orientation::Horizontal, false),
        "up" => (Orientation::Horizontal, true),
        other => return Err(format!("unknown split direction: {other}")),
    })
}

/// Focus-move direction string → [`Direction`].
fn parse_direction(direction: &str) -> Result<Direction, String> {
    Ok(match direction {
        "left" => Direction::Left,
        "right" => Direction::Right,
        "up" => Direction::Up,
        "down" => Direction::Down,
        other => return Err(format!("unknown direction: {other}")),
    })
}

/// Back a freshly-allocated model surface with an engine through the capacity gate. On refusal (or
/// a dead domain thread) it unstages the sink and closes the ghost pane so the tree never keeps a
/// pane with no terminal. Returns the created id + the fresh layout on success.
fn back_surface(
    factory: &EngineSurfaceFactory,
    host: &DomainHost,
    surface: SurfaceId,
) -> Result<SpawnResult, String> {
    // Option<_> so `query` has a Default if the domain thread is gone (None → the error arm).
    let outcome: Option<Result<i32, String>> = host.query(move |d| {
        Some(match d.try_create_surface(surface, None, None) {
            Ok(Some(id)) => Ok(id.0),
            Ok(None) => Err("safe-zone full — close a pane to spawn more".to_string()),
            Err(e) => Err(e.to_string()),
        })
    });

    match outcome {
        Some(Ok(id)) => {
            let tree = host.query(|d| d.tree_view());
            Ok(SpawnResult {
                surface_id: id,
                tree,
            })
        }
        Some(Err(msg)) => {
            factory.unstage(surface);
            host.run(move |d| {
                d.close_surface(surface); // roll back the ghost pane
            });
            Err(msg)
        }
        None => {
            factory.unstage(surface);
            Err("domain thread unavailable".to_string())
        }
    }
}

/// Back the selected workspace's seeded focused pane with a live shell (the boot path). Returns the
/// surface id + the initial layout.
#[tauri::command]
fn spawn(
    factory: tauri::State<'_, Arc<EngineSurfaceFactory>>,
    host: tauri::State<'_, DomainHost>,
    on_output: Channel<Vec<u8>>,
    on_event: Channel<String>,
) -> Result<SpawnResult, String> {
    let surface = host
        .query(|d| d.selected_focused_surface())
        .ok_or("no focused surface to back")?;
    factory.stage(surface, on_output, on_event);
    back_surface(&factory, &host, surface)
}

/// Split the focused pane in `direction`, spawning a shell in the new pane.
#[tauri::command]
fn split(
    factory: tauri::State<'_, Arc<EngineSurfaceFactory>>,
    host: tauri::State<'_, DomainHost>,
    direction: String,
    on_output: Channel<Vec<u8>>,
    on_event: Channel<String>,
) -> Result<SpawnResult, String> {
    let (orientation, insert_first) = parse_split(&direction)?;
    let surface = host
        .query(move |d| d.split_focused(orientation, insert_first))
        .ok_or("no pane to split")?;
    factory.stage(surface, on_output, on_event);
    back_surface(&factory, &host, surface)
}

/// Add a tab (new shell) to the focused pane.
#[tauri::command]
fn new_tab(
    factory: tauri::State<'_, Arc<EngineSurfaceFactory>>,
    host: tauri::State<'_, DomainHost>,
    on_output: Channel<Vec<u8>>,
    on_event: Channel<String>,
) -> Result<SpawnResult, String> {
    let surface = host
        .query(|d| d.new_tab_focused())
        .ok_or("no pane for a new tab")?;
    factory.stage(surface, on_output, on_event);
    back_surface(&factory, &host, surface)
}

#[tauri::command]
fn tree_view(host: tauri::State<'_, DomainHost>) -> TreeView {
    host.query(|d| d.tree_view())
}

/// The always-visible capacity meter snapshot (CLAUDE.md thesis). The frontend polls this after
/// spawn/close and on a slow timer — the safe-zone ledger changes on reserve/release.
#[tauri::command]
fn capacity_state(host: tauri::State<'_, DomainHost>) -> CapacityView {
    host.query(|d| d.capacity_view())
}

/// The sidebar row projection (workspace identity + git/PR/status).
#[tauri::command]
fn sidebar_state(host: tauri::State<'_, DomainHost>) -> Vec<SidebarRowView> {
    host.query(|d| d.sidebar_view())
}

/// Subscribe the frontend's persistent toast channel. Surfaced notifications (policy `show_toast`)
/// are pushed here as they're recorded on the domain thread — no polling.
#[tauri::command]
fn listen_notifications(host: tauri::State<'_, DomainHost>, on_toast: Channel<Vec<ToastView>>) {
    host.run(move |d| {
        d.set_toast_sink(Box::new(move |toasts| {
            let _ = on_toast.send(toasts); // frontend gone → drop; it re-subscribes on reload
        }));
    });
}

/// Create a new workspace and back its seeded pane with a shell (same capacity-gated path as spawn).
#[tauri::command]
fn new_workspace(
    factory: tauri::State<'_, Arc<EngineSurfaceFactory>>,
    host: tauri::State<'_, DomainHost>,
    on_output: Channel<Vec<u8>>,
    on_event: Channel<String>,
) -> Result<SpawnResult, String> {
    let surface = host
        .query(|d| d.new_workspace())
        .ok_or("could not create a workspace")?;
    factory.stage(surface, on_output, on_event);
    back_surface(&factory, &host, surface)
}

/// Close a workspace, tearing down its engines; returns the freed surface ids + the new layout.
#[tauri::command]
fn close_workspace(host: tauri::State<'_, DomainHost>, id: i32) -> CloseResult {
    host.query(move |d| {
        let closed = d
            .close_workspace(WorkspaceId(id))
            .iter()
            .map(|s| s.0)
            .collect();
        CloseResult {
            closed,
            tree: d.tree_view(),
        }
    })
}

/// Select a workspace (its tree becomes the live layout).
#[tauri::command]
fn select_workspace(host: tauri::State<'_, DomainHost>, id: i32) -> TreeView {
    host.query(move |d| {
        d.select_workspace(WorkspaceId(id));
        d.tree_view()
    })
}

#[tauri::command]
fn close_surface(host: tauri::State<'_, DomainHost>, id: i32) -> CloseResult {
    host.query(move |d| {
        let closed = d.close_surface(SurfaceId(id)).iter().map(|s| s.0).collect();
        CloseResult {
            closed,
            tree: d.tree_view(),
        }
    })
}

#[tauri::command]
fn close_focused(host: tauri::State<'_, DomainHost>) -> CloseResult {
    host.query(|d| {
        let closed = d.close_focused().iter().map(|s| s.0).collect();
        CloseResult {
            closed,
            tree: d.tree_view(),
        }
    })
}

#[tauri::command]
fn focus_pane(host: tauri::State<'_, DomainHost>, pane: i32) -> TreeView {
    host.query(move |d| {
        d.focus_pane(PaneId(pane));
        d.tree_view()
    })
}

#[tauri::command]
fn move_focus(host: tauri::State<'_, DomainHost>, direction: String) -> Result<TreeView, String> {
    let dir = parse_direction(&direction)?;
    Ok(host.query(move |d| {
        d.move_focus(dir);
        d.tree_view()
    }))
}

#[tauri::command]
fn select_tab(host: tauri::State<'_, DomainHost>, pane: i32, surface: i32) -> TreeView {
    host.query(move |d| {
        d.select_tab(PaneId(pane), SurfaceId(surface));
        d.tree_view()
    })
}

#[tauri::command]
fn select_next_tab(host: tauri::State<'_, DomainHost>) -> TreeView {
    host.query(|d| {
        d.select_next_tab();
        d.tree_view()
    })
}

#[tauri::command]
fn select_previous_tab(host: tauri::State<'_, DomainHost>) -> TreeView {
    host.query(|d| {
        d.select_previous_tab();
        d.tree_view()
    })
}

#[tauri::command]
fn toggle_zoom(host: tauri::State<'_, DomainHost>) -> TreeView {
    host.query(|d| {
        d.toggle_zoom();
        d.tree_view()
    })
}

#[tauri::command]
fn set_divider(host: tauri::State<'_, DomainHost>, branch: i32, fraction: f64) -> TreeView {
    host.query(move |d| {
        d.set_divider(BranchId(branch), fraction);
        d.tree_view()
    })
}

#[tauri::command]
fn equalize(host: tauri::State<'_, DomainHost>) -> TreeView {
    host.query(|d| {
        d.equalize();
        d.tree_view()
    })
}

/// App-side implementation of the socket command surface. Auth is live (DPAPI/env password store);
/// every domain verb marshals to the [`DomainHost`] thread, which owns the real workspace +
/// notification model — the port of C# `PipeServerEffects` routing to `WorkspaceHost`.
struct PipeEffects {
    host: DomainHost,
}

impl SocketEffects for PipeEffects {
    fn capabilities(&self) -> String {
        "v1,v2".to_string()
    }

    fn authenticate(&self, credential: &str) -> bool {
        // Auth is independent of the domain (C# keeps it a separate `Func`): verify against the
        // env/DPAPI password store directly on the calling thread.
        password_store().verify(credential)
    }

    fn focus_surface(&self, surface: SurfaceId) {
        self.host.run(move |d| d.focus_surface(surface));
    }

    // Routed to the domain's SurfaceManager. Live once the frontend spawn command creates
    // engine-backed surfaces through the host; until then the id resolves to no engine and the
    // send is dropped (Domain::send_text/send_key no-op on an unbacked id).
    fn send_text(&self, surface: SurfaceId, text: &str) {
        let text = text.to_string();
        self.host.run(move |d| d.send_text(surface, &text));
    }
    fn send_key(&self, surface: SurfaceId, virtual_key: u32, modifiers: u32) {
        self.host
            .run(move |d| d.send_key(surface, virtual_key, modifiers));
    }

    fn create_notification_for_target(
        &self,
        workspace_id: &str,
        surface: SurfaceId,
        title: &str,
        subtitle: &str,
        body: &str,
    ) {
        let (workspace_id, title, subtitle, body) = (
            workspace_id.to_string(),
            title.to_string(),
            subtitle.to_string(),
            body.to_string(),
        );
        self.host
            .run(move |d| d.create_for_target(&workspace_id, surface, &title, &subtitle, &body));
    }

    fn create_notification_for_caller(
        &self,
        preferred_surface_id: Option<&str>,
        title: &str,
        subtitle: &str,
        body: &str,
    ) {
        let (preferred, title, subtitle, body) = (
            preferred_surface_id.map(str::to_string),
            title.to_string(),
            subtitle.to_string(),
            body.to_string(),
        );
        self.host
            .run(move |d| d.create_for_caller(preferred.as_deref(), &title, &subtitle, &body));
    }

    fn notification_list(&self) -> Vec<TerminalNotification> {
        self.host.query(|d| d.notification_list())
    }
    fn notification_dismiss(&self, id: NotificationId) {
        self.host.run(move |d| d.notification_dismiss(id));
    }
    fn notification_dismiss_for_surface(&self, surface: SurfaceId) {
        self.host
            .run(move |d| d.notification_dismiss_for_surface(surface));
    }
    fn notification_dismiss_all_read(&self) {
        self.host.run(|d| d.notification_dismiss_all_read());
    }
    fn notification_clear(&self) {
        self.host.run(|d| d.notification_clear());
    }
    fn notification_mark_read(&self, id: NotificationId) {
        self.host.run(move |d| d.notification_mark_read(id));
    }
    fn notification_mark_read_for_surface(&self, surface: SurfaceId) {
        self.host
            .run(move |d| d.notification_mark_read_for_surface(surface));
    }
    fn notification_mark_all_read(&self) {
        self.host.run(|d| d.notification_mark_all_read());
    }
    fn notification_open(&self, id: NotificationId) {
        self.host.run(move |d| d.notification_open(id));
    }
    fn jump_to_unread(&self) -> bool {
        self.host.query(|d| d.jump_to_unread())
    }

    fn set_status(&self, status: &str) {
        let status = status.to_string();
        self.host.run(move |d| d.set_status(&status));
    }
    fn set_progress(&self, progress: &str) {
        let progress = progress.to_string();
        self.host.run(move |d| d.set_progress(&progress));
    }
    // C# `PipeServerEffects.LogLine` / `SidebarState` are deliberate no-ops.
    fn log_line(&self, _line: &str) {}
    fn sidebar_state(&self, _payload: &str) {}

    fn report_git_branch(&self, surface: SurfaceId, branch: &str, is_dirty: bool) {
        let branch = branch.to_string();
        self.host
            .run(move |d| d.report_git_branch(surface, &branch, is_dirty));
    }
    fn report_pr(
        &self,
        surface: SurfaceId,
        number: &str,
        label: &str,
        status: &str,
        branch: Option<&str>,
        is_stale: bool,
    ) {
        let (number, label, status, branch) = (
            number.to_string(),
            label.to_string(),
            status.to_string(),
            branch.map(str::to_string),
        );
        self.host.run(move |d| {
            d.report_pr(
                surface,
                &number,
                &label,
                &status,
                branch.as_deref(),
                is_stale,
            )
        });
    }
    fn report_pwd(&self, surface: SurfaceId, path: &str) {
        let path = path.to_string();
        self.host.run(move |d| d.report_pwd(surface, &path));
    }
}

/// Real password source: `OPTIMUS_SOCKET_PASSWORD` env, else the DPAPI-protected file under
/// `%LOCALAPPDATA%\optimus\`. Rebuilt per verify (cheap — no I/O until it actually reads).
fn password_store() -> PasswordStore {
    PasswordStore::new(
        Box::new(Win32SecretProtector),
        Box::new(|k| std::env::var(k).ok()),
        Box::new(|| std::env::var("LOCALAPPDATA").unwrap_or_default()),
        Box::new(|p| std::path::Path::new(p).exists()),
        Box::new(|p| std::fs::read(p)),
    )
}

/// Start the named-pipe control server on the resolved pipe name + control mode (env-overridable,
/// same scheme external agents connect to). Commands land on `host` (the real model plane). Kept
/// in Tauri managed state so it lives for the app and stops cleanly on exit.
fn start_pipe_server(host: DomainHost) -> PipeServer {
    let resolved = naming::resolve_from_environment(|k| std::env::var(k).ok());
    let pipe_name = if resolved.trim().is_empty() {
        naming::build_pipe_name("stable", None)
    } else {
        resolved
    };
    let mode = access::parse_mode(std::env::var(access::CONTROL_MODE_ENV).ok().as_deref());
    PipeServer::start(
        PipeServerConfig {
            pipe_name,
            control_mode: mode,
            max_clients: 16,
        },
        build_dispatch(mode, host),
    )
}

/// The router dispatch closure the pipe server hands each request. Shared across client threads
/// (Send + Sync): it captures a Copy `AuthState` and a `DomainHost` handle, building a fresh
/// [`PipeEffects`] per request that marshals every verb to the single domain thread.
fn build_dispatch(mode: access::SocketControlMode, host: DomainHost) -> DispatchFn {
    // Fail-closed: Password mode marks commands as needing auth. Full per-connection auth-state
    // threading is deferred (the shared closure holds no per-connection state), so Password mode
    // currently blocks commands rather than tracking a login — secure default.
    let auth = if access::requires_password_auth(mode) {
        AuthState {
            requires_authentication: true,
            is_authenticated: false,
        }
    } else {
        AuthState::UNPROTECTED
    };
    Arc::new(move |line| {
        let effects = PipeEffects { host: host.clone() };
        router::dispatch(line, &effects, auth)
    })
}

fn main() {
    tauri::Builder::default()
        // Pipe name `optimus-stable` collides if two instances run; second
        // launch focuses the first window instead (plan §2 item 2).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        // Start the CLI control pipe only in the instance that actually becomes the app —
        // `setup` does not run in a secondary instance the single-instance plugin turns away,
        // so we never briefly bind a second server on the same pipe name.
        .setup(|app| {
            // The RAM safe-zone governor (crown jewel): a static cap computed from this machine's
            // memory at startup — the SurfaceManager refuses spawns past it.
            let capacity = Arc::new(CapacityModel::new(Arc::new(Win32CapacityProvider), None));
            // The engine-backed surface factory: the single owner of live engines. Held in managed
            // state too, so the spawn command can stage each invoke's output channel before create.
            // ponytail: no hard per-process memory cap (0) — the job still KILL_ON_JOB_CLOSE-ties
            // each shell's lifetime; the count-based safe zone is the real guarantee. Wire a real
            // per-process ceiling (≈2× a calibrated budget) once calibration is trustworthy.
            let factory = Arc::new(EngineSurfaceFactory::new(0));

            // The domain thread owns the workspace + notification + surface model; the pipe server
            // and the frontend commands both drive it through this one handle.
            let host =
                DomainHost::spawn_with(factory.clone() as Arc<dyn SurfaceFactory>, Some(capacity));
            app.manage(start_pipe_server(host.clone()));
            app.manage(host);
            app.manage(factory);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            dev_spawn_shell,
            dev_send_text,
            dev_resize,
            spawn,
            split,
            new_tab,
            tree_view,
            capacity_state,
            sidebar_state,
            listen_notifications,
            new_workspace,
            close_workspace,
            select_workspace,
            close_surface,
            close_focused,
            focus_pane,
            move_focus,
            select_tab,
            select_next_tab,
            select_previous_tab,
            toggle_zoom,
            set_divider,
            equalize
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Optimus shell");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The app's own dispatch closure + effects must answer the wire protocol: a `capabilities`
    /// V2 frame round-trips to `ok:true` with the advertised `v1,v2`. Guards the main-side wiring
    /// (PipeEffects + build_dispatch) the same way the pipe transport test guards the server.
    #[test]
    fn app_dispatch_answers_capabilities() {
        let dispatch = build_dispatch(access::SocketControlMode::OptimusOnly, DomainHost::spawn());
        let response =
            dispatch(r#"{"id":"1","method":"system.capabilities"}"#).expect("a response");
        let root: Value = serde_json::from_str(&response).expect("valid json");
        assert_eq!(root["ok"], true, "server rejected capabilities: {response}");
        assert_eq!(root["result"]["capabilities"], "v1,v2");
    }

    /// End-to-end through the real wiring: a `notification.create_for_caller` frame reaches the
    /// domain thread, records a notification against the seeded surface, and a follow-up
    /// `notification.list` reads it back. Proves the pipe→router→PipeEffects→DomainHost→coordinator
    /// path actually mutates model state (not the old no-op).
    #[test]
    fn app_dispatch_records_and_lists_a_notification() {
        let dispatch = build_dispatch(access::SocketControlMode::OptimusOnly, DomainHost::spawn());

        let created = dispatch(
            r#"{"id":"1","method":"notification.create_for_caller","params":{"title":"build","body":"done"}}"#,
        )
        .expect("a response");
        assert_eq!(
            serde_json::from_str::<Value>(&created).unwrap()["ok"],
            true,
            "create rejected: {created}"
        );

        let listed = dispatch(r#"{"id":"2","method":"notification.list"}"#).expect("a response");
        let root: Value = serde_json::from_str(&listed).expect("valid json");
        let notifications = root["result"]["notifications"]
            .as_array()
            .expect("notifications array");
        assert_eq!(
            1,
            notifications.len(),
            "expected one recorded notification: {listed}"
        );
        assert_eq!(notifications[0]["title"], "build");
    }

    /// The `surface.send_key` encoder maps the keys agents actually send and drops the rest.
    #[test]
    fn encode_key_maps_the_minimal_vt_set() {
        assert_eq!(Some("\r".to_string()), encode_key(0x0D, 0)); // Enter
        assert_eq!(Some("\x1b[A".to_string()), encode_key(0x26, 0)); // Up arrow → CSI A
        assert_eq!(Some("\x03".to_string()), encode_key(0x43, 1)); // Ctrl-C → 0x03
        assert_eq!(Some("\x1b\x1b[D".to_string()), encode_key(0x25, 4)); // Alt-Left → meta ESC + CSI D
        assert_eq!(None, encode_key(0x70, 0)); // F1 — unmapped, dropped not mis-encoded
    }

    /// The factory refuses (no engine spawned) when the spawn command never staged a sink — the
    /// invariant that keeps a surface's PTY output wired to the right invoke's Channel.
    #[test]
    fn factory_refuses_to_create_without_a_staged_sink() {
        let factory = EngineSurfaceFactory::new(0);
        let err = match factory.create(SurfaceId(1), None, None) {
            Ok(_) => panic!("expected an error with no staged sink"),
            Err(e) => e,
        };
        assert!(err.contains("no staged output channel"), "{err}");
    }

    /// The real capacity provider reads this machine's RAM — proves the `GlobalMemoryStatusEx`
    /// wiring (feature + struct + call) is correct, so the safe-zone cap is computed from real
    /// numbers rather than a zeroed struct (which would floor the cap to 0 and refuse every spawn).
    #[test]
    fn win32_capacity_provider_reads_nonzero_physical_ram() {
        let provider = Win32CapacityProvider;
        assert!(
            provider.total_phys_bytes() > 0,
            "GlobalMemoryStatusEx returned 0 total physical RAM"
        );
    }
}
