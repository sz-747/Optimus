# Optimus

**Optimus is the terminal multiplexer that cannot crash your machine.** At startup
it measures available system RAM, computes the maximum number of terminals it can
host without exhausting memory, and locks that as a hard **safe-zone cap** — so
spawning a fleet of parallel coding agents can never over-allocate the box. Every
other terminal lets you spawn until the machine dies; Optimus refuses, gracefully,
one terminal before that happens.

The safe zone is live, not a one-shot guess:

- **Always-visible capacity indicator** — the sidebar shows `X / Y terminals` with
  a fill bar at all times (calm grey → amber at ≥ 75% → red at the cap, where
  "New workspace" is disabled with a plain-language reason). Capacity is the one
  number you never have to hunt for.
- **Live governor** — a 1 Hz ticker re-reads available physical RAM and commit
  headroom and tightens the cap under pressure (it never silently relaxes;
  recovery requires sustained headroom). Per-terminal cost is calibrated from
  real terminals and persisted (`%LOCALAPPDATA%\optimus\capacity.json`), so the
  cap gets more accurate every session.
- **Job-object backstop** — each terminal's shell is enrolled in a Windows Job
  Object capped at 2× the per-terminal budget with kill-on-close. A runaway
  process inside one terminal fails *alone*; its siblings and the app stay up.

Everything else — workspaces, splits, the notification sidebar — exists to serve
capacity-aware multiplexing of parallel agent sessions.

## What it is

A native Windows terminal multiplexer: a WinUI 3 (C#) chrome over a Rust/wgpu
terminal engine, plus an `optimus` CLI that talks to the app over a named pipe
(notifications, sending text/keys to surfaces, git/PR status reporting, agent
hooks).

## Layout

| Path | What |
|------|------|
| `app/` | WinUI 3 shell — sidebar, splits, capacity indicator, governor wiring |
| `core/` | Pure C# domain — capacity model, split tree, notifications, IPC framing |
| `engine/` | Rust/wgpu terminal engine (ConPTY, grid, rendering), built as a DLL |
| `cli/` | `optimus` CLI (named-pipe client) |
| `tests/` | C# test suite (`Optimus.Core.Tests`) |
| `DESIGN.md` | "Graphite" design system — read before any chrome change |
| `docs/runbooks/` | Operational runbooks, incl. the RAM safe-zone smoke test |

## Build & test

```powershell
# Engine first so the app picks up a fresh DLL
cd engine; cargo build --lib; cargo test
# App + tests
dotnet build app\Optimus.App.csproj
dotnet test tests\Optimus.Core.Tests.csproj
```
