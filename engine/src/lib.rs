//! cmux for Windows — terminal engine.
//!
//! This crate is built as a `cdylib` (`cmux_engine.dll`) consumed by the C# WinUI 3
//! app over a C ABI (see plan §6), and as an `rlib` so the in-tree spikes/tests can
//! link the engine directly.
//!
//! Phase 1 build order (plan §8): the modules below are filled in incrementally.
//! `pty` (ConPTY) lands first as spike 2 / unit U5.

pub mod pty;

// Filled in by later Phase-1 units:
// pub mod engine;   // U4 — Engine: owns vt + pty + renderer + channels
// pub mod vt;       // U4 — wezterm-term wiring + AlertHandler
// pub mod render;   // U6 — wgpu device/surface/present + grid + text
// pub mod ffi;      // U3 — the #[no_mangle] extern "C" ABI surface
