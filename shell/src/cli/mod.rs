//! Rust port of the C# `optimus` CLI (`cli/`), P3 unit 7 of the Tauri migration. Pure argv →
//! wire-frame parsing (`parser`), agent-hook runtime + snippet generation (`hooks`), and the
//! hang-safe redirected-stdin reader (`stdin`). The binary that does the actual pipe/file I/O is a
//! thin layer over these (deferred to the CLI bin target).

pub mod hooks;
pub mod parser;
pub mod stdin;

#[cfg(test)]
mod roundtrip_tests;
