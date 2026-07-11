//! Rust port of the C# `optimus` CLI (`cli/`), P3 unit 7 of the Tauri migration. Pure argv →
//! wire-frame parsing (`parser`), agent-hook runtime + snippet generation (`hooks`), and the
//! hang-safe redirected-stdin reader (`stdin`). `client` and `runtime` provide the testable I/O
//! boundary used by the bundled `optimus` binary.

pub mod client;
pub mod hooks;
pub mod parser;
pub mod runtime;
pub mod stdin;

#[cfg(test)]
mod roundtrip_tests;
