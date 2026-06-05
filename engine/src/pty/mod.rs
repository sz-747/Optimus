//! Pseudo-terminal backend.
//!
//! Phase 1 ships the Windows ConPTY implementation (`conpty`). The engine owns the
//! ConPTY (plan §2.1): it creates the pseudoconsole, spawns the shell, runs the
//! reader loop, and feeds bytes to the VT parser. C# only forwards input + resize.

pub mod conpty;

pub use conpty::ConPty;
