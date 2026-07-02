//! Port of cli/StdinReader.cs (res U2) — read redirected stdin without hanging when the handle is
//! open but silent. A blocking read-to-end deadlocks the CLI whenever a parent redirects stdin but
//! never writes or closes it. Hook payloads are small one-shot JSON blobs, so: no first byte within
//! `first_byte_timeout` means "no stdin" (`None`); once data starts, read until EOF, until the
//! stream goes quiet for `QUIET_WINDOW`, or until `drain_timeout` — whichever comes first.

use std::io::Read;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Default wait for the first byte before treating stdin as absent.
pub const DEFAULT_FIRST_BYTE_TIMEOUT: Duration = Duration::from_millis(500);
/// Default hard cap on draining after the first byte arrives.
pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// Inactivity window that ends the drain early — PowerShell 5.1 pushes a lone BOM then goes silent,
/// which would otherwise make every hang-case invocation pay the full drain timeout.
pub const QUIET_WINDOW: Duration = Duration::from_millis(150);

#[derive(Default)]
struct Shared {
    buf: Vec<u8>,
    got_first: bool,
    finished: bool,
}

/// Read what's available on `reader`, bounded by the two timeouts. `None` = no first byte arrived.
///
/// The reader is moved into a background thread that may outlive this call (a blocked read can
/// complete much later); the thread is deliberately not joined. For a process that exits right
/// after, a leaked parked thread is the cheaper trade — the OS reaps it. (Mirrors the C# note.)
pub fn read_available<R: Read + Send + 'static>(
    reader: R,
    first_byte_timeout: Duration,
    drain_timeout: Duration,
) -> Option<String> {
    let state = Arc::new((Mutex::new(Shared::default()), Condvar::new()));

    let worker = state.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break, // EOF or a broken/closed pipe: report what was read.
                Ok(n) => {
                    let (lock, cvar) = &*worker;
                    let mut inner = lock.lock().unwrap();
                    inner.buf.extend_from_slice(&chunk[..n]);
                    inner.got_first = true;
                    cvar.notify_all();
                }
            }
        }
        let (lock, cvar) = &*worker;
        lock.lock().unwrap().finished = true;
        cvar.notify_all();
    });

    let (lock, cvar) = &*state;
    let mut inner = lock.lock().unwrap();

    // Wait for the first byte (or an immediate EOF), then decide whether stdin is present at all.
    let (guard, timed_out) = cvar
        .wait_timeout_while(inner, first_byte_timeout, |s| !s.got_first && !s.finished)
        .unwrap();
    inner = guard;
    if timed_out.timed_out() && !inner.got_first && !inner.finished {
        return None;
    }

    // Drain until EOF, a quiet window, or the hard cap.
    let drain_start = Instant::now();
    while !inner.finished && drain_start.elapsed() < drain_timeout {
        let len_before = inner.buf.len();
        let (guard, _) = cvar.wait_timeout_while(inner, QUIET_WINDOW, |s| !s.finished).unwrap();
        inner = guard;
        if inner.finished || inner.buf.len() == len_before {
            break;
        }
    }

    // Strip a leading BOM so JSON payloads from BOM-emitting writers still parse.
    Some(String::from_utf8_lossy(&inner.buf).trim_start_matches('\u{feff}').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SHORT: Duration = Duration::from_millis(150);
    const LONG: Duration = Duration::from_secs(5);

    /// Serves an optional initial payload, then blocks forever — an open, silent pipe. The parked
    /// read leaks; the test process reaps it on exit (mirrors the C# BlockingReader).
    struct BlockingReader {
        pending: Vec<u8>,
    }

    impl BlockingReader {
        fn new(initial: &str) -> Self {
            Self { pending: initial.as_bytes().to_vec() }
        }
        fn silent() -> Self {
            Self { pending: Vec::new() }
        }
    }

    impl Read for BlockingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.pending.is_empty() {
                let n = buf.len().min(self.pending.len());
                buf[..n].copy_from_slice(&self.pending[..n]);
                self.pending.drain(..n);
                return Ok(n);
            }
            loop {
                std::thread::park();
            }
        }
    }

    #[test]
    fn returns_full_payload_when_stdin_closes_normally() {
        let result = read_available(Cursor::new(br#"{"message":"done"}"#.to_vec()), LONG, LONG);
        assert_eq!(result.as_deref(), Some(r#"{"message":"done"}"#));
    }

    #[test]
    fn returns_empty_string_immediately_for_empty_closed_stdin() {
        let start = Instant::now();
        let result = read_available(Cursor::new(Vec::new()), LONG, LONG);
        assert_eq!(result.as_deref(), Some(""));
        assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
    }

    #[test]
    fn returns_none_when_stdin_stays_open_and_silent() {
        assert_eq!(read_available(BlockingReader::silent(), SHORT, SHORT), None);
    }

    #[test]
    fn returns_partial_payload_when_writer_never_closes_after_writing() {
        let result = read_available(BlockingReader::new(r#"{"message":"hi"}"#), LONG, SHORT);
        assert_eq!(result.as_deref(), Some(r#"{"message":"hi"}"#));
    }

    #[test]
    fn quiet_window_ends_drain_early_instead_of_waiting_full_drain_timeout() {
        let start = Instant::now();
        let result = read_available(BlockingReader::new("\u{feff}"), LONG, Duration::from_secs(10));
        assert_eq!(result.as_deref(), Some(""));
        assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
    }

    #[test]
    fn strips_leading_bom_from_payload() {
        let result = read_available(Cursor::new("\u{feff}{\"message\":\"done\"}".as_bytes().to_vec()), LONG, LONG);
        assert_eq!(result.as_deref(), Some(r#"{"message":"done"}"#));
    }
}
