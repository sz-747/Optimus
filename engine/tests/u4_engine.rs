//! Engine-core integration tests (migration plan P1 exit gate): drive a real shell through
//! the slimmed stack — `Engine` → ConPTY → PTY reader thread → `on_bytes` hook — and assert
//! the shell's output reaches the caller, then tear down cleanly.
//!
//! Characterization port of the pre-migration U4 tests: same assertions, but the oracle is
//! the raw byte stream (what xterm.js will consume) instead of a wezterm-term grid.

use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use optimus_engine::{Engine, EngineEvent, EngineOptions};

type SharedBytes = Arc<Mutex<Vec<u8>>>;
type SharedEvents = Arc<Mutex<Vec<EngineEvent>>>;

/// Build an engine whose hooks accumulate output bytes + events into shared buffers.
fn engine_with_capture() -> (Engine, SharedBytes, SharedEvents) {
    let bytes: SharedBytes = Arc::new(Mutex::new(Vec::new()));
    let events: SharedEvents = Arc::new(Mutex::new(Vec::new()));
    let b = Arc::clone(&bytes);
    let e = Arc::clone(&events);
    let engine = Engine::new(
        EngineOptions::default(),
        Box::new(move |chunk| b.lock().expect("bytes lock").extend_from_slice(chunk)),
        Box::new(move |ev| e.lock().expect("events lock").push(ev)),
    )
    .expect("create engine");
    (engine, bytes, events)
}

/// Wait up to `timeout` for the accumulated output to contain `needle`.
fn wait_for_output(bytes: &SharedBytes, needle: &str, timeout: Duration) -> String {
    let start = Instant::now();
    loop {
        let text = String::from_utf8_lossy(&bytes.lock().expect("bytes lock")).into_owned();
        if text.contains(needle) || start.elapsed() > timeout {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn shell_output_reaches_the_bytes_hook() {
    let (mut engine, bytes, _events) = engine_with_capture();

    // `cmd.exe` is always present and deterministic; it echoes the marker then exits.
    engine
        .spawn_shell("cmd.exe /c echo optimus_marker_42", None)
        .expect("spawn shell");

    let out = wait_for_output(&bytes, "optimus_marker_42", Duration::from_secs(15));
    assert!(
        out.contains("optimus_marker_42"),
        "echoed marker never appeared in the byte stream; output was:\n{out}"
    );

    // Engine drop runs ordered teardown (ClosePseudoConsole → join reader → drop PTY).
    drop(engine);
}

#[test]
fn spawned_shell_receives_environment_overrides() {
    let (mut engine, bytes, _events) = engine_with_capture();
    let parent_value = std::env::var_os("OPTIMUS_ENGINE_ENV_TEST");
    let inherited_system_root = std::env::var("SystemRoot").expect("SystemRoot is inherited");
    let environment = vec![(
        "OPTIMUS_ENGINE_ENV_TEST".to_string(),
        "surface_marker_91".to_string(),
    )];

    engine
        .spawn_shell_with_environment(
            "cmd.exe /d /c echo %OPTIMUS_ENGINE_ENV_TEST% %SystemRoot% inherited_complete_91",
            None,
            &environment,
        )
        .expect("spawn shell with environment override");

    let out = wait_for_output(&bytes, "inherited_complete_91", Duration::from_secs(15));
    assert!(
        out.contains("surface_marker_91"),
        "child did not receive the environment override; output was:\n{out}"
    );
    assert!(
        out.contains(&inherited_system_root),
        "child lost an inherited environment variable; output was:\n{out}"
    );
    assert_eq!(
        std::env::var_os("OPTIMUS_ENGINE_ENV_TEST"),
        parent_value,
        "child environment overrides must not mutate the host process"
    );
}

#[test]
fn parallel_shells_keep_environment_overrides_isolated() {
    fn spawn_child(marker: &str, barrier: Arc<Barrier>) -> std::thread::JoinHandle<String> {
        let marker = marker.to_string();
        std::thread::spawn(move || {
            let (mut engine, bytes, _events) = engine_with_capture();
            let environment = vec![("OPTIMUS_PARALLEL_SURFACE_TEST".to_string(), marker.clone())];
            barrier.wait();
            engine
                .spawn_shell_with_environment(
                    "cmd.exe /d /c echo %OPTIMUS_PARALLEL_SURFACE_TEST%",
                    None,
                    &environment,
                )
                .expect("spawn parallel shell");
            let output = wait_for_output(&bytes, &marker, Duration::from_secs(15));
            drop(engine);
            output
        })
    }

    let barrier = Arc::new(Barrier::new(3));
    let first = spawn_child("surface_S31", Arc::clone(&barrier));
    let second = spawn_child("surface_S32", Arc::clone(&barrier));
    barrier.wait();

    let first_output = first.join().expect("first pane thread");
    let second_output = second.join().expect("second pane thread");
    assert!(first_output.contains("surface_S31"), "{first_output}");
    assert!(!first_output.contains("surface_S32"), "{first_output}");
    assert!(second_output.contains("surface_S32"), "{second_output}");
    assert!(!second_output.contains("surface_S31"), "{second_output}");
}

#[test]
fn child_exit_is_reported() {
    let (mut engine, _bytes, events) = engine_with_capture();
    engine
        .spawn_shell("cmd.exe /c exit 7", None)
        .expect("spawn shell");

    let start = Instant::now();
    loop {
        let evs = events.lock().expect("events lock").clone();
        if let Some(EngineEvent::ChildExit { code }) = evs
            .iter()
            .find(|e| matches!(e, EngineEvent::ChildExit { .. }))
        {
            assert_eq!(*code, 7, "child exit code must surface");
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "ChildExit never arrived; events: {evs:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn osc99_notification_surfaces_as_toast_event() {
    let (mut engine, _bytes, events) = engine_with_capture();

    // Emit a raw OSC-99 sequence from inside the shell. `echo` adds a trailing newline
    // after the BEL terminator, which the sniffer ignores. 0x1b can't be typed via cmd
    // echo directly — use PowerShell's `$([char]27)`.
    engine
        .spawn_shell(
            "powershell.exe -NoProfile -Command \"Write-Host \\\"$([char]27)]99;;ping_toast$([char]7)\\\"\"",
            None,
        )
        .expect("spawn shell");

    let start = Instant::now();
    loop {
        let evs = events.lock().expect("events lock").clone();
        if evs
            .iter()
            .any(|e| matches!(e, EngineEvent::Toast { title, .. } if title == "ping_toast"))
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "OSC-99 toast never arrived; events: {evs:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn spawned_shell_reports_nonzero_child_pid() {
    let (mut engine, _bytes, _events) = engine_with_capture();

    // Before any spawn the engine must report "no child" as 0 (callers skip Job Object
    // enrollment on 0).
    assert_eq!(engine.child_pid(), 0, "child_pid must be 0 before spawn");

    // Long-lived child so the PID is observably a live process right after spawn.
    engine
        .spawn_shell("cmd.exe /c ping -n 30 127.0.0.1 > NUL", None)
        .expect("spawn shell");

    // spawn_shell is synchronous (worker publishes the PID before replying), so the PID
    // must be valid immediately — no polling.
    let pid = engine.child_pid();
    assert_ne!(
        pid, 0,
        "spawned engine must report a non-zero ConPTY child PID"
    );

    drop(engine);
}

#[test]
fn child_process_handle_is_published_after_spawn() {
    let (mut engine, _bytes, _events) = engine_with_capture();

    // Before any spawn, the handle contract mirrors the PID contract: 0 = unavailable.
    assert_eq!(
        engine.child_process_handle(),
        0,
        "handle must be 0 before spawn"
    );

    // Long-lived child so the handle is observably a live process right after spawn.
    engine
        .spawn_shell("cmd.exe /c ping -n 30 127.0.0.1 > NUL", None)
        .expect("spawn shell");

    // spawn_shell is synchronous, so a duplicated, caller-owned handle must be available
    // immediately. Each call duplicates afresh, so two calls both succeed and yield
    // distinct live handles. (The duplicates leak in this test; the test process exits
    // right after, which is fine.)
    let first = engine.child_process_handle();
    let second = engine.child_process_handle();
    assert_ne!(
        first, 0,
        "spawned engine must return a duplicated child process handle"
    );
    assert_ne!(second, 0, "every call must yield a fresh duplicate");
    assert_ne!(
        first, second,
        "simultaneously live duplicates must be distinct handles"
    );

    // Engine drop runs teardown, which clears the published PID/handle and closes the
    // engine-owned duplicate before the ConPty (and its original handle) is dropped.
    drop(engine);
}

#[test]
fn resize_before_spawn_is_honored() {
    let (mut engine, bytes, _events) = engine_with_capture();
    // Resize before any shell exists must not error and should set the grid used at spawn.
    engine.resize(100, 30).expect("resize before spawn");

    engine
        .spawn_shell("cmd.exe /c echo sized", None)
        .expect("spawn");
    let out = wait_for_output(&bytes, "sized", Duration::from_secs(15));
    assert!(out.contains("sized"), "output was:\n{out}");
}

#[test]
fn send_text_reaches_the_shell() {
    let (mut engine, bytes, _events) = engine_with_capture();
    // Interactive cmd session: type an echo command through send_text.
    engine.spawn_shell("cmd.exe", None).expect("spawn shell");
    engine.send_text("echo typed_marker_7\r");

    let out = wait_for_output(&bytes, "typed_marker_7", Duration::from_secs(15));
    assert!(out.contains("typed_marker_7"), "output was:\n{out}");
    drop(engine);
}
