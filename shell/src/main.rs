#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{Manager, State, WindowEvent};

const MAX_LOCAL_SESSIONS: usize = 12;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

#[derive(Clone, Serialize)]
struct TerminalSessionView {
    id: u64,
    title: String,
}

struct TerminalSession {
    child: Child,
    input: ChildStdin,
}

#[derive(Default)]
struct TerminalRegistry {
    next_id: AtomicU64,
    sessions: Mutex<HashMap<u64, TerminalSession>>,
}

impl TerminalRegistry {
    fn list(&self) -> Vec<TerminalSessionView> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut views = sessions
            .keys()
            .map(|id| TerminalSessionView {
                id: *id,
                title: format!("Terminal {id}"),
            })
            .collect::<Vec<_>>();
        views.sort_by_key(|view| view.id);
        views
    }

    fn create(&self, output: Channel<String>) -> Result<TerminalSessionView, String> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if sessions.len() >= MAX_LOCAL_SESSIONS {
            return Err(format!(
                "The local interface limit is {MAX_LOCAL_SESSIONS} terminals. The safe-capacity governor is not yet ported."
            ));
        }

        let mut command = Command::new("cmd.exe");
        command
            .arg("/Q")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);

        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start cmd.exe: {error}"))?;
        let input = child
            .stdin
            .take()
            .ok_or("cmd.exe did not expose standard input")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("cmd.exe did not expose standard output")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("cmd.exe did not expose standard error")?;

        forward_output(stdout, output.clone());
        forward_output(stderr, output);

        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let view = TerminalSessionView {
            id,
            title: format!("Terminal {id}"),
        };
        sessions.insert(view.id, TerminalSession { child, input });
        Ok(view)
    }

    fn write(&self, id: u64, input: String) -> Result<(), String> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let session = sessions
            .get_mut(&id)
            .ok_or_else(|| format!("terminal {id} no longer exists"))?;
        session
            .input
            .write_all(input.as_bytes())
            .and_then(|_| session.input.flush())
            .map_err(|error| format!("could not write to terminal {id}: {error}"))
    }

    fn close(&self, id: u64) -> bool {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(mut session) = sessions.remove(&id) else {
            return false;
        };
        let _ = session.input.flush();
        let _ = session.child.kill();
        let _ = session.child.wait();
        true
    }

    fn close_all(&self) {
        let ids = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for id in ids {
            self.close(id);
        }
    }
}

fn forward_output(mut reader: impl Read + Send + 'static, output: Channel<String>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let _ = output.send(String::from_utf8_lossy(&buffer[..read]).into_owned());
                }
            }
        }
    });
}

#[tauri::command]
fn terminal_sessions(registry: State<'_, TerminalRegistry>) -> Vec<TerminalSessionView> {
    registry.list()
}

#[tauri::command]
fn create_terminal(
    registry: State<'_, TerminalRegistry>,
    on_output: Channel<String>,
) -> Result<TerminalSessionView, String> {
    registry.create(on_output)
}

#[tauri::command]
fn send_terminal_input(
    registry: State<'_, TerminalRegistry>,
    id: u64,
    input: String,
) -> Result<(), String> {
    registry.write(id, input)
}

#[tauri::command]
fn close_terminal(registry: State<'_, TerminalRegistry>, id: u64) -> bool {
    registry.close(id)
}

fn main() {
    tauri::Builder::default()
        .manage(TerminalRegistry::default())
        .setup(|app| {
            let user_data = app.path().app_local_data_dir()?.join("webview2");
            fs::create_dir_all(&user_data)?;
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", user_data);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            terminal_sessions,
            create_terminal,
            send_terminal_input,
            close_terminal
        ])
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::CloseRequested { .. }) {
                window.state::<TerminalRegistry>().close_all();
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run Optimus shell");
}
