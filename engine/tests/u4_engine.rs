//! U4 engine-core integration test (plan §8 U4): drive a real shell through the full stack
//! — `Engine` → ConPTY → PTY reader thread → `wezterm-term` `advance_bytes` → grid — and
//! assert the shell's output lands in the terminal screen, then tear down cleanly.
//!
//! This exercises everything U4 adds without the GPU: thread plumbing, the command channel,
//! VT parsing, and ordered shutdown (Engine drop closes the pseudoconsole, joins the reader).

use std::time::{Duration, Instant};

use optimus_engine::engine::Engine;
use optimus_engine::ffi::events::EngineOptions;

/// Wait up to `timeout` for the visible grid to contain `needle`.
fn wait_for_screen(engine: &Engine, needle: &str, timeout: Duration) -> String {
    let start = Instant::now();
    loop {
        let screen = engine.screen_text();
        if screen.contains(needle) {
            return screen;
        }
        if start.elapsed() > timeout {
            return screen;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn shell_output_reaches_the_grid() {
    let mut engine = Engine::new(EngineOptions::default());

    // `cmd.exe` is always present and deterministic; it echoes the marker then exits.
    // (The pseudoconsole stays open after the child exits, so the grid retains the output.)
    engine
        .spawn_shell("cmd.exe /c echo optimus_marker_42", None)
        .expect("spawn shell");

    let screen = wait_for_screen(&engine, "optimus_marker_42", Duration::from_secs(15));
    assert!(
        screen.contains("optimus_marker_42"),
        "echoed marker never appeared in the grid; screen was:\n{screen}"
    );

    // Engine drop runs ordered teardown (ClosePseudoConsole → join reader → drop terminal).
    drop(engine);
}

#[test]
fn spawned_shell_reports_nonzero_child_pid() {
    let mut engine = Engine::new(EngineOptions::default());

    // Before any spawn the engine must report "no child" as 0 (the FFI contract for
    // `optimus_engine_child_pid` — the host skips Job Object enrollment on 0).
    assert_eq!(engine.child_pid(), 0, "child_pid must be 0 before spawn");

    // Long-lived child so the PID is observably a live process right after spawn.
    engine
        .spawn_shell("cmd.exe /c ping -n 30 127.0.0.1 > NUL", None)
        .expect("spawn shell");

    // spawn_shell is synchronous (render thread publishes the PID before replying),
    // so the PID must be valid immediately — no polling.
    let pid = engine.child_pid();
    assert_ne!(pid, 0, "spawned engine must report a non-zero ConPTY child PID");

    drop(engine);
}

#[test]
fn resize_before_spawn_is_honored() {
    let mut engine = Engine::new(EngineOptions::default());
    // Resize without a panel/surface must not error (renderer absent) and should set the grid.
    engine
        .resize(100, 30, 0, 0, 1.0)
        .expect("resize without surface");

    engine.spawn_shell("cmd.exe /c echo sized", None).expect("spawn");
    let screen = wait_for_screen(&engine, "sized", Duration::from_secs(15));
    assert!(screen.contains("sized"), "screen was:\n{screen}");
}
