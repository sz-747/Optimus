//! Named-pipe server: the app side of the CLI socket. Port of `app/Ipc/PipeServer.cs`.
//!
//! Raw `CreateNamedPipeW` with a `SECURITY_ATTRIBUTES` carrying an owner-SID **protected** DACL
//! (only the current user gets FullControl) — tokio's named-pipe API can't set that owner-SID ACL,
//! which is why this is raw Win32. Blocking I/O, one OS thread per connected client (fine at the
//! 16-client cap), newline-framed requests dispatched through an injected router closure. Every
//! mode except `AllowAll` enforces a peer-SID identity check before reading commands.
//!
//! The pipe name scheme is the external contract agents connect to — this server binds the exact
//! `\\.\pipe\optimus-{variant}` names produced by [`crate::ipc::naming`]; nothing here rewrites them.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::mem::size_of;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use crate::ipc::access::{requires_peer_sid_check, SocketControlMode};
use crate::ipc::peer;

/// Non-zero: a 0-byte pipe buffer makes every write block until the peer reads, which deadlocks
/// the request/response handshake (see the C# note at `PipeServer.cs:268`).
const PIPE_BUFFER_SIZE: u32 = 4096;

/// Router closure: newline-stripped request → optional response line. Shared across client
/// threads, so it is `Send + Sync` (the real one dispatches into the workspace host + router).
pub type DispatchFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct PipeServerConfig {
    /// Full Win32 pipe path, e.g. `\\.\pipe\optimus-stable` (from [`crate::ipc::naming`]).
    pub pipe_name: String,
    pub control_mode: SocketControlMode,
    pub max_clients: usize,
}

/// A `Send` wrapper to move a connected pipe handle onto its dedicated client thread. That
/// thread is the handle's sole user and closes it, so the one-way move is sound (mirrors the
/// engine's `OutputReader`).
struct SendHandle(HANDLE);
// SAFETY: exactly one thread ever touches the wrapped handle.
unsafe impl Send for SendHandle {}

/// Returns a concurrency slot to the accept loop when the client thread ends — on normal return
/// *or* on unwind. Without this, a panic in the dispatch closure (C4/C5) would skip the return and
/// permanently leak a slot; enough leaks would starve the server of its `max_clients` capacity.
struct SlotGuard(SyncSender<()>);
impl Drop for SlotGuard {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// A running pipe server. Dropping it stops the accept loop and joins the accept thread.
pub struct PipeServer {
    pipe_name: String,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl PipeServer {
    /// Start accepting connections on a background thread. Returns immediately.
    pub fn start(config: PipeServerConfig, dispatch: DispatchFn) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let max = config.max_clients.max(1);
        let name = config.pipe_name.clone();
        let name_wide = to_wide_null(&config.pipe_name);
        let mode = config.control_mode;
        let stop_worker = Arc::clone(&stop);

        let accept = std::thread::Builder::new()
            .name("optimus-pipe-accept".into())
            .spawn(move || accept_loop(name_wide, mode, max, dispatch, stop_worker))
            .expect("spawn pipe accept thread");

        Self {
            pipe_name: name,
            stop,
            accept: Some(accept),
        }
    }

    /// Signal the accept loop to stop and unblock a pending `ConnectNamedPipe` by connecting a
    /// throwaway client to the pipe (the standard blocking-pipe shutdown wake).
    pub fn stop(&self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        // ponytail: wakes the common case (idle server blocked on connect); a fully saturated
        // server instead drains as its in-flight clients finish and observe the stop flag.
        let _ = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.pipe_name);
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        self.stop();
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

fn accept_loop(
    name: Vec<u16>,
    mode: SocketControlMode,
    max: usize,
    dispatch: DispatchFn,
    stop: Arc<AtomicBool>,
) {
    let current_sid = peer::current_user_sid();
    let descriptor = build_security_descriptor(current_sid.as_deref());
    let attributes = descriptor.map(|psd| SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: psd.0,
        bInheritHandle: BOOL(0),
    });

    // Bounded-concurrency semaphore: `max` tokens; take one before creating an instance, the
    // client thread returns it when the connection ends.
    let (slot_tx, slot_rx) = sync_channel::<()>(max);
    for _ in 0..max {
        let _ = slot_tx.send(());
    }

    while !stop.load(Ordering::SeqCst) {
        if slot_rx.recv().is_err() {
            break;
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let Some(handle) = create_instance(&name, attributes.as_ref(), max as u32) else {
            // Instance creation failed hard; stop rather than spin.
            break;
        };

        let connected = wait_for_connection(handle);
        if stop.load(Ordering::SeqCst) {
            let _ = unsafe { CloseHandle(handle) };
            break;
        }
        if !connected {
            let _ = unsafe { CloseHandle(handle) };
            let _ = slot_tx.send(());
            continue;
        }

        if requires_peer_sid_check(mode) && !authorized(handle, current_sid.as_deref()) {
            write_line_and_close(handle, "ERROR: access denied");
            let _ = slot_tx.send(());
            continue;
        }

        let client_dispatch = Arc::clone(&dispatch);
        let slot_return = slot_tx.clone();
        let connection = SendHandle(handle);
        let spawned = std::thread::Builder::new()
            .name("optimus-pipe-client".into())
            .spawn(move || {
                // Bind the whole wrapper so the closure captures `SendHandle` (Send), not the
                // disjoint `HANDLE` field (which is Copy and !Send).
                let connection = connection;
                // Reclaim the slot on drop — survives a panic inside `handle_client`.
                let _slot = SlotGuard(slot_return);
                handle_client(connection.0, &client_dispatch);
            });
        if spawned.is_err() {
            // Couldn't spawn a handler; close the connection and reclaim the slot.
            let _ = unsafe { CloseHandle(handle) };
            let _ = slot_tx.send(());
        }
    }

    if let Some(psd) = descriptor {
        // SAFETY: the descriptor came from ConvertStringSecurityDescriptor…W (LocalAlloc).
        let _ = unsafe { LocalFree(Some(HLOCAL(psd.0))) };
    }
}

/// Build a SECURITY_DESCRIPTOR granting only `sid` FullControl over a protected (no-inherit)
/// DACL, owned by `sid` — the SDDL equivalent of the C# `PipeSecurity` (owner + protected +
/// FullControl-current-user). `None` when the SID is unknown → server falls back to a null
/// SA (the pipe's default DACL).
fn build_security_descriptor(sid: Option<&str>) -> Option<PSECURITY_DESCRIPTOR> {
    let sid = sid?;
    let sddl = format!("O:{sid}D:P(A;;FA;;;{sid})");
    let sddl_wide = to_wide_null(&sddl);
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl_wide` is a valid null-terminated wide string; `descriptor` is a valid out-param.
    // `1` is SDDL_REVISION_1.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl_wide.as_ptr()),
            1,
            &mut descriptor,
            None,
        )
        .ok()?;
    }
    Some(descriptor)
}

fn create_instance(
    name: &[u16],
    attributes: Option<&SECURITY_ATTRIBUTES>,
    max: u32,
) -> Option<HANDLE> {
    // SAFETY: `name` is null-terminated; `attributes`, when present, outlives the call.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            max,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            attributes.map(|a| a as *const SECURITY_ATTRIBUTES),
        )
    };
    if handle.is_invalid() {
        None
    } else {
        Some(handle)
    }
}

fn wait_for_connection(handle: HANDLE) -> bool {
    // SAFETY: `handle` is a valid server-instance handle; blocking connect (no overlapped).
    match unsafe { ConnectNamedPipe(handle, None) } {
        Ok(()) => true,
        // A client that connected between CreateNamedPipe and ConnectNamedPipe surfaces as
        // ERROR_PIPE_CONNECTED — that is success, not failure.
        Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
    }
}

fn authorized(handle: HANDLE, current_sid: Option<&str>) -> bool {
    // Fail closed: unknown peer or unknown self → deny (matches the app's clientSidValidator,
    // which compares the resolved peer SID to the current user's).
    match (peer::resolve_client_sid(handle), current_sid) {
        (Some(peer), Some(mine)) => peer == mine,
        _ => false,
    }
}

fn handle_client(handle: HANDLE, dispatch: &DispatchFn) {
    // Wrap the raw handle so std's buffered line I/O drives the byte pipe; the `File` owns the
    // handle and closes it on drop (do NOT also CloseHandle it).
    // SAFETY: `handle` is a live, connected pipe handle transferred to this thread exclusively.
    let file = unsafe { File::from_raw_handle(handle.0 as RawHandle) };
    let mut reader = BufReader::new(&file);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break, // EOF or broken pipe
            Ok(_) => {}
        }
        let request = line.trim_end_matches(['\r', '\n']);
        if request.is_empty() {
            continue;
        }
        match dispatch(request) {
            Some(response) => {
                let mut out = response;
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                if (&file).write_all(out.as_bytes()).is_err() {
                    break;
                }
            }
            // None = events.stream handoff; the streaming infra lands with the frontend event bus.
            None => break,
        }
    }
}

fn write_line_and_close(handle: HANDLE, message: &str) {
    // SAFETY: `handle` is a live connected handle; the `File` closes it on drop.
    let file = unsafe { File::from_raw_handle(handle.0 as RawHandle) };
    let _ = (&file).write_all(format!("{message}\n").as_bytes());
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Full transport round-trip on a real pipe (headless — no GUI): a client connects, sends a
    /// framed line, and gets the dispatch closure's response back. Exercises accept → connect →
    /// read-line → dispatch → write. AllowAll avoids the peer-SID path (can't fake a foreign SID
    /// in-process); same-user auth is covered by the naming/access unit tests.
    #[test]
    fn round_trips_a_line_through_the_dispatch_closure() {
        let name = format!(r"\\.\pipe\optimus-test-{}", std::process::id());
        let dispatch: DispatchFn = Arc::new(|req: &str| Some(format!("echo:{req}")));
        let server = PipeServer::start(
            PipeServerConfig {
                pipe_name: name.clone(),
                control_mode: SocketControlMode::AllowAll,
                max_clients: 4,
            },
            dispatch,
        );

        // The accept thread creates the first instance asynchronously; retry the connect briefly.
        let mut client = None;
        for _ in 0..50 {
            match OpenOptions::new().read(true).write(true).open(&name) {
                Ok(f) => {
                    client = Some(f);
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        let mut client = client.expect("connect to pipe server");

        client.write_all(b"hello\n").expect("write request");
        client.flush().ok();

        let mut reader = BufReader::new(&mut client);
        let mut response = String::new();
        reader.read_line(&mut response).expect("read response");
        assert_eq!(response, "echo:hello\n");

        // Explicitly drop the client (server-side read hits EOF), then the server.
        drop(reader);
        drop(client);
        drop(server);
    }

    /// Connect to a just-started server, retrying while the accept thread creates the first
    /// instance (or, after a client closes, the next one).
    fn connect(name: &str) -> File {
        for _ in 0..50 {
            if let Ok(f) = OpenOptions::new().read(true).write(true).open(name) {
                return f;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("could not connect to pipe server");
    }

    /// A dispatch panic must not leak the connection's concurrency slot (C5/C4). With
    /// `max_clients = 1`, a leaked slot would wedge the accept loop forever — the second client
    /// could never be served. The `SlotGuard` returns the slot during unwind, so it is.
    #[test]
    fn a_panicking_dispatch_releases_the_slot() {
        let name = format!(r"\\.\pipe\optimus-test-panic-{}", std::process::id());
        let dispatch: DispatchFn = Arc::new(|req: &str| {
            assert_ne!(req, "boom", "dispatch panics on this request");
            Some(format!("echo:{req}"))
        });
        let server = PipeServer::start(
            PipeServerConfig {
                pipe_name: name.clone(),
                control_mode: SocketControlMode::AllowAll,
                max_clients: 1,
            },
            dispatch,
        );

        // First client trips the panic in its server-side thread, then disconnects.
        let mut c1 = connect(&name);
        c1.write_all(b"boom\n").expect("write to first client");
        c1.flush().ok();
        drop(c1);

        // Second client is served only if the slot came back.
        let mut c2 = connect(&name);
        c2.write_all(b"hello\n").expect("write to second client");
        c2.flush().ok();
        let mut reader = BufReader::new(&mut c2);
        let mut response = String::new();
        reader
            .read_line(&mut response)
            .expect("second client served");
        assert_eq!(response, "echo:hello\n");

        drop(reader);
        drop(c2);
        drop(server);
    }
}
