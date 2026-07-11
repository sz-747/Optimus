#![cfg(windows)]

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use optimus_engine::{Engine, EngineOptions};
use optimus_shell::child_environment::for_surface;
use optimus_shell::cli::hooks;
use optimus_shell::host::Domain;
use optimus_shell::ipc::access::SocketControlMode;
use optimus_shell::ipc::pipe_server::{DispatchFn, PipeServer, PipeServerConfig};
use optimus_shell::ipc::wire::{parse_v2, serialize_ok};
use serde_json::{json, Value};

fn test_pipe() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    format!(r"\\.\pipe\optimus-pane-hook-{}-{nonce}", std::process::id())
}

fn test_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!("optimus-pane-hook-{}-{nonce}", std::process::id()))
}

fn recording_server(pipe_name: String, request_tx: SyncSender<(String, Value)>) -> PipeServer {
    let dispatch: DispatchFn = Arc::new(move |line| {
        let request = parse_v2(line).expect("hook should send a V2 request");
        request_tx
            .send((request.method, request.params))
            .expect("record hook request");
        Some(serialize_ok(&request.id, json!({"recorded": true})))
    });

    PipeServer::start(
        PipeServerConfig {
            pipe_name,
            control_mode: SocketControlMode::AllowAll,
            max_clients: 4,
        },
        dispatch,
    )
}

#[test]
fn migrated_pane_hook_round_trips_with_its_surface_identity() {
    let surface = Domain::new()
        .selected_focused_surface()
        .expect("seeded pane surface");
    let pipe_name = test_pipe();
    let (request_tx, request_rx) = mpsc::sync_channel(1);
    let _server = recording_server(pipe_name.clone(), request_tx);

    let directory = test_directory();
    fs::create_dir_all(&directory).expect("create hook test directory");
    fs::copy(env!("CARGO_BIN_EXE_optimus"), directory.join("optimus.exe"))
        .expect("stage the real Optimus CLI on child PATH");

    let hook_path = directory.join("optimus-claude-hook.cmd");
    fs::write(
        &hook_path,
        hooks::snippet(hooks::find("claude").expect("claude hook"), "cmd"),
    )
    .expect("write generated Claude hook");

    fs::write(
        directory.join("payload.json"),
        r#"{"message":"pane hook done"}"#,
    )
    .expect("write hook payload");

    let runner_path = directory.join("run-hook.cmd");
    fs::write(
        &runner_path,
        "@call \"optimus-claude-hook.cmd\" Stop < \"payload.json\"\r\n",
    )
    .expect("write hook runner");

    let mut environment = for_surface(surface, &pipe_name);
    let inherited_path = std::env::var("PATH").expect("test runner PATH");
    environment.push((
        "PATH".to_string(),
        format!("{};{inherited_path}", directory.display()),
    ));

    let mut engine = Engine::new(EngineOptions::default(), Box::new(|_| {}), Box::new(|_| {}))
        .expect("create pane engine");
    engine
        .spawn_shell_with_environment(
            "cmd.exe /d /c run-hook.cmd",
            directory.to_str(),
            &environment,
        )
        .expect("spawn migrated pane hook");

    let (method, params) = request_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("hook request should reach the named pipe");
    assert_eq!(method, "notification.create_for_caller");
    assert_eq!(params["preferred_surface_id"], surface.to_string());
    assert_eq!(params["body"], "pane hook done");

    drop(engine);
    let _ = fs::remove_dir_all(directory);
}
