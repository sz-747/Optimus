//! Optimus terminal engine.
//!
//! Post-Tauri-migration shape (migration plan D1′/D2): a plain Rust library linked
//! straight into the Tauri shell — no FFI, no GPU renderer. The engine owns the ConPTY
//! and its reader thread and hands raw VT bytes to the caller (the shell forwards them
//! to xterm.js over a `tauri::ipc::Channel`); xterm.js owns parsing, grid, scrollback,
//! selection, and input encoding. The only VT the engine still understands is OSC 99
//! (Kitty notifications), sniffed Rust-side because agents emit it from arbitrary child
//! processes and the backend is authoritative for notifications.
#![warn(clippy::unwrap_used)]

pub mod engine;
pub mod pty;
pub mod vt;

pub use engine::{Engine, EngineError, EngineEvent, EngineOptions};
