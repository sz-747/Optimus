//! Optimus shell library target. The domain core (P2 port of `core/` C#) lives here so
//! `cargo test -p optimus-shell` links unit tests without going through the Tauri binary.

pub mod child_environment;
pub mod cli;
pub mod control_plane;
pub mod domain;
pub mod host;
pub mod ipc;
pub mod orchestration;
pub mod relay;
pub mod view;
