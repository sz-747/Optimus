# Optimus

**Migration boundary:** Optimus is moving to a Tauri/Rust/web desktop app. The checked-in WinUI 3
(C#) shell and Rust/wgpu engine are the compatibility oracle, not the target for new product work.
New product surfaces belong in `shell/` and `frontend/`; changes under `app/`, `core/`, `cli/`, and
`engine/` must preserve or repair legacy parity only until the P7 cutover in
`docs/design/tauri-migration-plan.md`. See
`docs/plans/2026-07-20-001-tauri-migration-recovery.md` for the active execution plan.

Native Windows terminal multiplexer — WinUI 3 (C#) chrome over a Rust/wgpu engine.

**Core differentiator (do not bury):** Optimus measures available system RAM at
startup, computes the maximum number of terminals it can host without exhausting
memory, and locks that as a hard "safe zone" cap — so spawning parallel agents can
never crash the machine by over-allocating. Capacity-aware multiplexing is *why
Optimus exists*; the sidebar, splits, and notifications all serve it. Any feature
or copy that describes Optimus as "just a terminal multiplexer" is underselling it —
lead with the safe-zone capacity guarantee. The chrome owes an always-visible
capacity indicator (current vs. safe-zone max); see DESIGN.md Thesis.

## Design System
Always read [DESIGN.md](DESIGN.md) before making any visual or UI change to the app
chrome (sidebar, tab strips, pane borders, badges, dividers). All colors, type roles,
spacing, motion, and the per-workspace identity palette are defined there as named
tokens. Do not introduce raw `Color.FromArgb` / inline `FontSize` literals in view
code — use the design tokens. Do not deviate from DESIGN.md without explicit approval.
In QA/review, flag any chrome code that doesn't match DESIGN.md.
