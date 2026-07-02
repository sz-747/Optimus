// No console window in release — hard requirement (plan §2, GUI launch).
// Debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

use optimus_engine::{Engine, EngineEvent, EngineOptions};
use tauri::ipc::Channel;
use tauri::Manager;

use optimus_shell::domain::ids::SurfaceId;
use optimus_shell::domain::notifications::{NotificationId, TerminalNotification};
use optimus_shell::ipc::access::{self, AuthState};
use optimus_shell::ipc::dpapi::Win32SecretProtector;
use optimus_shell::ipc::naming;
use optimus_shell::ipc::password::PasswordStore;
use optimus_shell::ipc::pipe_server::{DispatchFn, PipeServer, PipeServerConfig};
use optimus_shell::ipc::router::{self, SocketEffects};

/// Live terminal surfaces. P1 dev plumbing — the real SurfaceManager (reserve/commit,
/// capacity gate) arrives with the P2 domain port and replaces this flat list.
#[derive(Default)]
struct Surfaces(Mutex<Vec<Engine>>);

/// P1 exit-gate command: spawn a shell and stream its raw VT bytes to the frontend over a
/// Channel (backpressured, per-surface — plan §3 item 4; never `AppHandle::emit` for PTY
/// output). Returns the index the caller uses for input/resize.
#[tauri::command]
fn dev_spawn_shell(
    state: tauri::State<'_, Surfaces>,
    on_output: Channel<Vec<u8>>,
    on_event: Channel<String>,
) -> Result<usize, String> {
    let events = on_event.clone();
    let mut engine = Engine::new(
        EngineOptions::default(),
        Box::new(move |bytes| {
            let _ = on_output.send(bytes.to_vec());
        }),
        Box::new(move |ev| {
            let msg = match ev {
                EngineEvent::Toast { title, body } => format!("toast: {title} — {body}"),
                EngineEvent::ChildExit { code } => format!("child exit: {code}"),
            };
            let _ = events.send(msg);
        }),
    )
    .map_err(|e| e.to_string())?;
    engine.spawn_shell("", None).map_err(|e| e.to_string())?;

    let mut surfaces = state.0.lock().map_err(|_| "surfaces poisoned")?;
    surfaces.push(engine);
    Ok(surfaces.len() - 1)
}

#[tauri::command]
fn dev_send_text(state: tauri::State<'_, Surfaces>, id: usize, text: String) -> Result<(), String> {
    let mut surfaces = state.0.lock().map_err(|_| "surfaces poisoned")?;
    let engine = surfaces.get_mut(id).ok_or("no such surface")?;
    engine.send_text(&text);
    Ok(())
}

#[tauri::command]
fn dev_resize(
    state: tauri::State<'_, Surfaces>,
    id: usize,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let mut surfaces = state.0.lock().map_err(|_| "surfaces poisoned")?;
    let engine = surfaces.get_mut(id).ok_or("no such surface")?;
    engine.resize(cols, rows).map_err(|e| e.to_string())
}

/// App-side implementation of the socket command surface. Auth is live (DPAPI/env password
/// store); the domain-affecting verbs are no-ops until the workspace host is instantiated.
struct PipeEffects;

impl SocketEffects for PipeEffects {
    fn capabilities(&self) -> String {
        "v1,v2".to_string()
    }

    fn authenticate(&self, credential: &str) -> bool {
        password_store().verify(credential)
    }

    // ponytail: the transport, owner-SID ACL, peer-auth, and capabilities/authenticate handshake
    // are live now; these domain verbs bind to the workspace host when it lands (P4/P5). Until
    // then they no-op rather than touch state that does not exist yet.
    fn focus_surface(&self, _surface: SurfaceId) {}
    fn send_text(&self, _surface: SurfaceId, _text: &str) {}
    fn send_key(&self, _surface: SurfaceId, _virtual_key: u32, _modifiers: u32) {}
    fn create_notification_for_target(&self, _w: &str, _s: SurfaceId, _t: &str, _sub: &str, _b: &str) {}
    fn create_notification_for_caller(&self, _p: Option<&str>, _t: &str, _sub: &str, _b: &str) {}
    fn notification_list(&self) -> Vec<TerminalNotification> {
        Vec::new()
    }
    fn notification_dismiss(&self, _id: NotificationId) {}
    fn notification_dismiss_for_surface(&self, _surface: SurfaceId) {}
    fn notification_dismiss_all_read(&self) {}
    fn notification_clear(&self) {}
    fn notification_mark_read(&self, _id: NotificationId) {}
    fn notification_mark_read_for_surface(&self, _surface: SurfaceId) {}
    fn notification_mark_all_read(&self) {}
    fn notification_open(&self, _id: NotificationId) {}
    fn jump_to_unread(&self) -> bool {
        false
    }
    fn set_status(&self, _status: &str) {}
    fn set_progress(&self, _progress: &str) {}
    fn log_line(&self, _line: &str) {}
    fn sidebar_state(&self, _payload: &str) {}
    fn report_git_branch(&self, _surface: SurfaceId, _branch: &str, _is_dirty: bool) {}
    fn report_pr(&self, _s: SurfaceId, _n: &str, _l: &str, _st: &str, _br: Option<&str>, _stale: bool) {}
    fn report_pwd(&self, _surface: SurfaceId, _path: &str) {}
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
/// same scheme external agents connect to). Kept in Tauri managed state so it lives for the app
/// and stops cleanly on exit.
fn start_pipe_server() -> PipeServer {
    let resolved = naming::resolve_from_environment(|k| std::env::var(k).ok());
    let pipe_name = if resolved.trim().is_empty() {
        naming::build_pipe_name("stable", None)
    } else {
        resolved
    };
    let mode = access::parse_mode(std::env::var(access::CONTROL_MODE_ENV).ok().as_deref());
    PipeServer::start(
        PipeServerConfig { pipe_name, control_mode: mode, max_clients: 16 },
        build_dispatch(mode),
    )
}

/// The router dispatch closure the pipe server hands each request. Shared across client threads
/// (Send + Sync): it captures only a Copy `AuthState` and builds a stateless [`PipeEffects`].
fn build_dispatch(mode: access::SocketControlMode) -> DispatchFn {
    // Fail-closed: Password mode marks commands as needing auth. Full per-connection auth-state
    // threading is deferred (the shared closure holds no per-connection state), so Password mode
    // currently blocks commands rather than tracking a login — secure default.
    let auth = if access::requires_password_auth(mode) {
        AuthState { requires_authentication: true, is_authenticated: false }
    } else {
        AuthState::UNPROTECTED
    };
    Arc::new(move |line| router::dispatch(line, &PipeEffects, auth))
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
        .manage(Surfaces::default())
        // Start the CLI control pipe only in the instance that actually becomes the app —
        // `setup` does not run in a secondary instance the single-instance plugin turns away,
        // so we never briefly bind a second server on the same pipe name.
        .setup(|app| {
            app.manage(start_pipe_server());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            dev_spawn_shell,
            dev_send_text,
            dev_resize
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
        let dispatch = build_dispatch(access::SocketControlMode::OptimusOnly);
        let response = dispatch(r#"{"id":"1","method":"system.capabilities"}"#).expect("a response");
        let root: Value = serde_json::from_str(&response).expect("valid json");
        assert_eq!(root["ok"], true, "server rejected capabilities: {response}");
        assert_eq!(root["result"]["capabilities"], "v1,v2");
    }
}
