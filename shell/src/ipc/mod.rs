//! Rust port of the C# IPC layer (`core/Ipc/` + `app/Ipc/`), P3 of the Tauri migration.
//! Newline-framed JSON over a Windows named pipe: wire envelopes, method constants,
//! socket naming/discovery/access policy, the DPAPI password store, and the command router.

pub mod access;
pub mod dpapi;
pub mod naming;
pub mod password;
pub mod peer;
pub mod pipe_server;
pub mod router;
pub mod wire;
