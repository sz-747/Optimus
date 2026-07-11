//! One-shot named-pipe transport for the bundled `optimus` CLI.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::ipc::naming;

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const RETRY_INTERVAL: Duration = Duration::from_millis(10);

pub fn can_connect(pipe_path: &str) -> bool {
    connect(pipe_path, PROBE_TIMEOUT).is_ok()
}

pub fn send(
    pipe_path: &str,
    frames: &[String],
    connect_timeout: Duration,
) -> io::Result<Vec<Option<String>>> {
    send_with_timeouts(pipe_path, frames, connect_timeout, DEFAULT_RESPONSE_TIMEOUT)
}

pub fn send_with_timeouts(
    pipe_path: &str,
    frames: &[String],
    connect_timeout: Duration,
    response_timeout: Duration,
) -> io::Result<Vec<Option<String>>> {
    let mut pipe = connect(pipe_path, connect_timeout)?;
    let mut responses = Vec::with_capacity(frames.len());

    for frame in frames {
        pipe.write_all(frame.as_bytes())?;
        pipe.write_all(b"\n")?;
        pipe.flush()?;

        let response = read_response(pipe.try_clone()?, response_timeout)?;
        let closed = response.is_none();
        responses.push(response);
        if closed {
            responses.resize(frames.len(), None);
            break;
        }
    }

    Ok(responses)
}

fn connect(pipe_path: &str, timeout: Duration) -> io::Result<File> {
    let pipe_path = local_pipe_path(pipe_path);
    let started = Instant::now();
    loop {
        match OpenOptions::new().read(true).write(true).open(&pipe_path) {
            Ok(file) => return Ok(file),
            Err(error) if started.elapsed() >= timeout => return Err(error),
            Err(_) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                thread::sleep(RETRY_INTERVAL.min(remaining));
            }
        }
    }
}

fn local_pipe_path(pipe_path: &str) -> String {
    format!(r"\\.\pipe\{}", naming::to_local_name(pipe_path))
}

fn read_response(pipe: File, timeout: Duration) -> io::Result<Option<String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("optimus-cli-response".into())
        .spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut response = String::new();
            let result = match reader.read_line(&mut response) {
                Ok(0) => Ok(None),
                Ok(_) => Ok(Some(response.trim_end_matches(['\r', '\n']).to_string())),
                Err(error) => Err(error),
            };
            let _ = sender.send(result);
        })?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for pipe response",
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "pipe response reader stopped",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_bare_and_full_local_pipe_names() {
        assert_eq!(
            local_pipe_path("optimus-stable"),
            r"\\.\pipe\optimus-stable"
        );
        assert_eq!(
            local_pipe_path(r"\\.\pipe\optimus-stable"),
            r"\\.\pipe\optimus-stable"
        );
    }

    #[test]
    fn early_eof_fills_remaining_responses_without_a_second_write() {
        let name = format!(r"\\.\pipe\optimus-cli-eof-{}", std::process::id());
        let server = crate::ipc::pipe_server::PipeServer::start(
            crate::ipc::pipe_server::PipeServerConfig {
                pipe_name: name.clone(),
                control_mode: crate::ipc::access::SocketControlMode::AllowAll,
                max_clients: 1,
            },
            std::sync::Arc::new(|_| None),
        );

        let responses = send_with_timeouts(
            &name,
            &[String::from("one"), String::from("two")],
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("EOF is a protocol result, not a transport error");
        assert_eq!(responses, vec![None, None]);
        drop(server);
    }
}
