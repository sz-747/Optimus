//! Optimus shell library target. The domain core (P2 port of `core/` C#) lives here so
//! `cargo test -p optimus-shell` links unit tests without going through the Tauri binary.

pub mod domain;
pub mod ipc;
