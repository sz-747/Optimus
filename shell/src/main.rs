// No console window in release — hard requirement (plan §2, GUI launch).
// Debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use optimus_engine::{Engine, EngineEvent, EngineOptions};
use tauri::ipc::Channel;
use tauri::Manager;

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
fn dev_resize(state: tauri::State<'_, Surfaces>, id: usize, cols: u16, rows: u16) -> Result<(), String> {
    let mut surfaces = state.0.lock().map_err(|_| "surfaces poisoned")?;
    let engine = surfaces.get_mut(id).ok_or("no such surface")?;
    engine.resize(cols, rows).map_err(|e| e.to_string())
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
        .invoke_handler(tauri::generate_handler![
            dev_spawn_shell,
            dev_send_text,
            dev_resize
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Optimus shell");
}
