//! Spike 2 (plan §7.2): ConPTY round-trip.
//!
//! Spawns a real shell in a pseudoconsole, writes a command, reads VT output on a
//! dedicated thread to EOF, and exercises `ResizePseudoConsole` + clean shutdown.

use optimus_engine::pty::ConPty;
use std::thread;
use std::time::Duration;

#[test]
fn conpty_roundtrip_cmd_echo() {
    let pty = ConPty::spawn("cmd.exe", None, 80, 25).expect("spawn conpty");

    // Drain the pseudoconsole output to EOF on a dedicated thread. Start it immediately so
    // we capture the boot banner and every rendered frame conhost emits.
    let mut reader = pty.output_reader();
    let reader_handle = thread::spawn(move || {
        let mut out: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        let mut reads = 0u32;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    eprintln!(
                        "[reader] EOF after {reads} reads, {} bytes total",
                        out.len()
                    );
                    break; // EOF: pseudoconsole closed / shell exited
                }
                Ok(n) => {
                    reads += 1;
                    out.extend_from_slice(&buf[..n]);
                }
                Err(e) => {
                    eprintln!("[reader] ReadFile error after {reads} reads: {e:?}");
                    break;
                }
            }
        }
        out
    });

    // Let the shell finish booting (banner + prompt) before driving it.
    thread::sleep(Duration::from_millis(500));
    assert!(
        pty.child_alive(),
        "shell should still be running after boot"
    );

    // Drive the shell: type a command that prints a unique marker.
    pty.write(b"echo OPTIMUS_CONPTY_OK\r\n")
        .expect("write echo command");
    // Give conhost time to process input and render the marker into the VT stream.
    thread::sleep(Duration::from_millis(500));

    // Exercise a resize mid-session (must not error or wedge the stream).
    pty.resize(100, 30)
        .expect("ResizePseudoConsole must succeed cleanly");
    thread::sleep(Duration::from_millis(300));

    // Close the pseudoconsole: this terminates the shell and closes conhost's output-pipe
    // write end, which makes the reader's blocked ReadFile return EOF. MUST happen before
    // the join — joining first deadlocks (the failure mode this spike guards against).
    pty.shutdown();

    // Reader returns once it drains to EOF, proving clean shutdown. Join BEFORE dropping the
    // PTY (the reader borrows the output handle that Drop closes).
    let out = reader_handle.join().expect("reader thread joined");
    drop(pty);

    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains("OPTIMUS_CONPTY_OK"),
        "expected marker in pseudoconsole output; got {} bytes:\n{text}",
        out.len()
    );
}
