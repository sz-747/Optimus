//! Rust port of the C# core domain (`core/`), unit by unit in dependency order (plan §4).
//! Each module mirrors one C# area; its C# tests are translated into `#[cfg(test)]` blocks.

pub mod capacity;
pub mod ids;
pub mod layout;
pub mod notifications;
pub mod projections;
pub mod split_tree;
pub mod surface_manager;
pub mod workspace;
