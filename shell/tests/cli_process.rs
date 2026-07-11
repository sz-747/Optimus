#![cfg(windows)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use optimus_shell::ipc::access::SocketControlMode;
use optimus_shell::ipc::naming;
use optimus_shell::ipc::pipe_server::{DispatchFn, PipeServer, PipeServerConfig};
use optimus_shell::ipc::wire::{parse_v2, serialize_ok};
use serde_json::json;

fn test_pipe(label: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    format!(
        r"\\.\pipe\optimus-cli-{label}-{}-{nonce}",
        std::process::id()
    )
}

fn server(pipe_name: String, requests: Arc<Mutex<Vec<(String, serde_json::Value)>>>) -> PipeServer {
    let dispatch: DispatchFn = Arc::new(move |line| {
        let request = parse_v2(line).expect("CLI should send a V2 request");
        requests
            .lock()
            .expect("request recorder")
            .push((request.method, request.params));
        Some(serialize_ok(
            &request.id,
            json!({"transport": "named-pipe"}),
        ))
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

fn cli_executable() -> PathBuf {
    std::env::var_os("OPTIMUS_CLI_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_optimus")))
}

#[test]
fn capabilities_round_trip_through_the_cli_process() {
    let pipe_name = test_pipe("capabilities");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let _server = server(pipe_name.clone(), Arc::clone(&requests));
    let bare_pipe_name = naming::to_local_name(&pipe_name);

    let output = Command::new(cli_executable())
        .args(["--socket", &bare_pipe_name, "capabilities"])
        .stdin(Stdio::null())
        .output()
        .expect("launch optimus CLI");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        r#"{"transport":"named-pipe"}"#
    );

    let requests = requests.lock().expect("request recorder");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "system.capabilities");
}

#[test]
fn accepted_connection_without_a_response_times_out() {
    let pipe_name = test_pipe("stalled");
    let dispatch: DispatchFn = Arc::new(|_| {
        std::thread::sleep(Duration::from_secs(30));
        None
    });
    let _server = PipeServer::start(
        PipeServerConfig {
            pipe_name: pipe_name.clone(),
            control_mode: SocketControlMode::AllowAll,
            max_clients: 2,
        },
        dispatch,
    );

    let started = Instant::now();
    let output = Command::new(cli_executable())
        .args(["--socket", &pipe_name, "capabilities"])
        .stdin(Stdio::null())
        .output()
        .expect("launch optimus CLI");

    assert!(!output.status.success());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "CLI exceeded its response deadline: {:?}",
        started.elapsed()
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("timed out"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn claude_stop_hook_reads_stdin_and_carries_surface_identity() {
    let pipe_name = test_pipe("hook");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let _server = server(pipe_name.clone(), Arc::clone(&requests));

    let mut child = Command::new(cli_executable())
        .args(["--socket", &pipe_name, "hooks", "claude", "stop"])
        .env("OPTIMUS_SURFACE_ID", "S7")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch optimus CLI");

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(br#"{"message":"Finished verifier work"}"#)
        .expect("write hook payload");

    let output = child.wait_with_output().expect("wait for optimus CLI");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = requests.lock().expect("request recorder");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "notification.create_for_caller");
    assert_eq!(requests[0].1["preferred_surface_id"], "S7");
    assert_eq!(requests[0].1["body"], "Finished verifier work");
}
