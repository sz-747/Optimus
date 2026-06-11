---
title: RAM Safe-Zone Capacity Governor — MVP
status: active
date: 2026-06-10
sequence: 001
kind: feat
plan_file: docs/plans/2026-06-10-001-feat-ram-safe-zone-mvp-plan.md
source_ideation: docs/ideation/2026-06-10-ram-safe-zone-ideation.md
---

# RAM Safe-Zone Capacity Governor — MVP

## Goal

Make Optimus deliver on its stated differentiator: at startup, measure available
system RAM, compute the maximum number of terminals the machine can host without
exhausting memory, lock that as a hard "safe-zone" cap, and surface it as an
always-visible chrome indicator. Spawning parallel agents must never crash the
machine by over-allocating.

The MVP is the spine: empirical per-terminal cost calibration (#2 from ideation),
a reserve-then-commit gate plus per-terminal Job Object backstop at the single
spawn choke point (#3), the DESIGN.md capacity indicator (#7), and a thin
re-measure loop that tightens the cap under pressure (slice of #4).

## Scope Boundaries

In scope (MVP spine):
- Calibrate real per-terminal memory cost from the ConPTY child process; seed
  conservative on first launch, firm up after first N spawns.
- Soft admission gate at `SurfaceManager.CreateSurface()` — reserve-then-commit
  against a measured safe-zone ceiling.
- Hard OS backstop — per-terminal `JobObject` with `JOB_OBJECT_LIMIT_PROCESS_MEMORY`
  and `KILL_ON_JOB_CLOSE`. Win32 fails allocations inside the job rather than
  terminating the tree (confirmed in research).
- Always-visible chrome indicator in the sidebar near the New-Workspace button,
  with calm → `git-dirty` amber → `pr-closed` red escalation per DESIGN.md.
- 1 Hz `GlobalMemoryStatusEx` re-measure + `CreateMemoryResourceNotification`
  subscription. Cap **tightens new spawns only** under pressure; live terminals
  are never reaped.

Out of scope (deferred to follow-up plans, tracked in the ideation doc):
- Full process-tree metering (#1) — MVP backstops the ConPTY child tree via
  Job Object, but does not aggregate descendant private bytes into a single
  honest workspace number. Indicator is engine-anchored, not tree-anchored.
- Engine dehydration of idle surfaces (#5) — contradicts the deliberate
  "never tear down" posture; revisit only if calibration shows the seed is too
  generous on commodity laptops.
- QoS tiers / per-workspace quotas (#6) — single global safe-zone for v1.
- Tray / system-wide capacity report — chrome-only for v1.

## Architecture Sketch

Two-tier governor; the tiers compose, not duplicate.

**Tier 1 — soft admission (Core, math-only, fully testable).**
`CapacityModel` lives in `core/Capacity/`. It owns:
- `SafeZoneBytes` — `min(AvailPhys − Reserve, CommitLimit − CommitTotal − Reserve)`,
  re-evaluated on a 1 Hz tick and on low-memory signal.
- `PerTerminalBudgetBytes` — seeded (e.g. 200 MB); recalibrated from a moving
  average of measured `PrivateUsage` across live ConPTY children after the
  first N spawns settle.
- `MaxTerminals` — `floor(SafeZoneBytes / PerTerminalBudgetBytes)`, monotonically
  non-increasing within a session under pressure (we tighten, never silently
  relax — relax requires a deliberate "headroom recovered" event).
- `Reserve(SurfaceId)` / `Commit(SurfaceId)` / `Release(SurfaceId)` — the
  two-phase ticket the spawn gate uses to avoid TOCTOU across parallel spawns.

`ICapacityProvider` interface in Core; production binding is `Win32CapacityProvider`
in `app/Interop/`, test binding is a deterministic fake.

**Tier 2 — hard enforcement (app, OS-level).**
`TerminalJobObject` (app/Interop/) wraps a `SafeHandle` around a per-terminal
Job Object configured with `JOB_OBJECT_LIMIT_PROCESS_MEMORY = 2 × budget`,
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = true`. The ConPTY child PID gets
`AssignProcessToJobObject`'d immediately after spawn. A runaway inside the
terminal gets `VirtualAlloc` failures; the rest of the tree is untouched.
Optional: IOCP subscription for `JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT` to
surface a "Terminal hit memory ceiling" badge in chrome (stretch in U4).

**Indicator (chrome).**
`CapacityIndicatorView` in `app/Sidebar/` binds to `CapacityModel` via a small
view-model. Always visible above the New-Workspace button. Calm at < 75% of
cap, `git-dirty` (#D9A04E) at ≥ 75%, `pr-closed` (#D96A6A) at cap; button
disabled at cap with a one-line "Safe-zone full — close a workspace to spawn
more" hint.

## Implementation Units

### U1 — Win32 interop surface

**Goal:** Hand-written P/Invokes for the memory APIs the governor needs.
Sized small so it lands first and unblocks every later unit.

**Files (Create):**
- `app/Interop/MemoryNativeMethods.cs` — `[LibraryImport]` stubs for
  `GlobalMemoryStatusEx`, `GetPerformanceInfo`, `GetProcessMemoryInfo`,
  `OpenProcess` (PROCESS_QUERY_LIMITED_INFORMATION), `CreateMemoryResourceNotification`,
  `QueryMemoryResourceNotification`.
- `app/Interop/JobObjectNativeMethods.cs` — `CreateJobObject`,
  `AssignProcessToJobObject`, `SetInformationJobObject`, `CloseHandle`; structs
  `JOBOBJECT_BASIC_LIMIT_INFORMATION`, `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`,
  `IO_COUNTERS`; const flags.
- `app/Interop/SafeJobHandle.cs` — `SafeHandle` wrapping the job handle; finalizer
  closes via `CloseHandle`. Required to avoid GC race kills under
  `KILL_ON_JOB_CLOSE`.

**Patterns to follow:**
- Existing FFI conventions in `app/Interop/EngineHandle.cs` (SafeHandle wrap +
  `[LibraryImport]` source-generated marshalling). Do not mix `[DllImport]` in.
- `MEMORYSTATUSEX.dwLength` must be set with `Marshal.SizeOf<MEMORYSTATUSEX>()`
  before the call — research flagged this as the #1 P/Invoke failure mode.

**Verification:** Project compiles. A throwaway smoke test (or scratch
`Program.Main`) calls `GlobalMemoryStatusEx` once and prints `ullAvailPhys` —
non-zero confirms the marshalling is right. Smoke output not committed.

**Execution note:** Pragmatic — interop layer is plumbing, not behavior.

---

### U2 — CapacityModel + ICapacityProvider in Core

**Goal:** The math, state machine, and reservation ledger. Pure Core, no Win32.

**Files (Create):**
- `core/Capacity/ICapacityProvider.cs` — `AvailablePhysBytes`,
  `CommitHeadroomBytes`, `OnLowMemorySignal` event, `MeasureProcessPrivateBytes(int pid)`.
- `core/Capacity/CapacityModel.cs` — state (`SafeZoneBytes`,
  `PerTerminalBudgetBytes`, `MaxTerminals`, in-flight reservations dict);
  methods `TryReserve(SurfaceId) → ReservationToken?`, `Commit(token, pid)`,
  `Release(SurfaceId)`, `RecordMeasurement(SurfaceId, bytes)`,
  `OnTick(ICapacityProvider)`.
- `core/Capacity/CapacityState.cs` — readonly snapshot record:
  `(used, reserved, max, level: Calm|Warn|Cap)`. Indicator binds to this.

**Patterns to follow:**
- Idempotent state-machine style of `core/Splits/SurfaceManager.cs`.
- Inject provider; never reach for Win32 from Core.

**Calibration policy:**
- Seed `PerTerminalBudgetBytes = 200 MB` on first launch (conservative).
- After each terminal has been alive ≥ 30 s, record its `PrivateUsage`.
- Once ≥ 3 samples accumulate, set budget to `max(seed, P75(samples))`.
- Persist budget to a small `capacity.json` next to existing app state so the
  next launch starts already-calibrated. Include a hardware fingerprint
  (`ullTotalPhys` rounded to GB) so a calibration from a different machine
  doesn't poison this one.

**Cap-under-pressure policy:**
- `MaxTerminals` monotonically non-increasing within a session.
- Recovery requires `LowMemorySignal` to clear **and** `AvailPhysBytes` to be
  ≥ 1.25 × `SafeZoneBytes` for two consecutive ticks before easing.
- Never reaps live terminals.

**Test scenarios (xUnit, `tests/Capacity/CapacityModelTests.cs`):**
- *Happy path:* seed budget, 8 GB safe zone, `MaxTerminals` reports floor(zone/budget).
- *Reserve-then-commit:* two parallel `TryReserve` at cap−1 — one succeeds, one
  fails (TOCTOU guard).
- *Release frees a slot.*
- *Calibration:* after 3 measurements at 300 MB, budget rises to 300 MB and
  `MaxTerminals` drops accordingly.
- *Low-memory tick:* shrinks `SafeZoneBytes` mid-session, `MaxTerminals` drops,
  reservations beyond the new cap fail (existing live terminals untouched).
- *Hardware fingerprint mismatch:* persisted calibration from a 32 GB machine
  is ignored when loading on a 16 GB machine.
- *Recovery hysteresis:* one tick of recovered memory does not raise the cap;
  two consecutive do.

**Verification:** `dotnet test tests/Optimus.Core.Tests.csproj` passes new
suite; existing 203 tests still green.

**Execution note:** **Test-first.** This is the load-bearing model; the test
suite is the spec.

---

### U3 — Win32CapacityProvider + 1 Hz tick + low-memory subscription

**Goal:** Wire Tier-1 math to the OS. Lives in the app layer.

**Files (Create):**
- `app/Capacity/Win32CapacityProvider.cs` — implements `ICapacityProvider` using
  U1 P/Invokes. Returns `min(ullAvailPhys, CommitLimit − CommitTotal)` as the
  headroom feed.
- `app/Capacity/CapacityTicker.cs` — `System.Threading.Timer` at 1 Hz calling
  `CapacityModel.OnTick`. `ThreadPool.RegisterWaitForSingleObject` on the
  `CreateMemoryResourceNotification` handle for event-driven pressure.

**Files (Modify):**
- `app/Bootstrap/Services.cs` (or equivalent composition root) — register
  `CapacityModel` as singleton with `Win32CapacityProvider`; start ticker.

**Patterns to follow:**
- Background-timer pattern already used by IPC heartbeat (find via grep, mirror
  cancellation/dispose discipline).
- All UI marshalling stays in the indicator view (U6), not here.

**Test scenarios:** Adapter-level smoke test that calls each provider method
once and asserts non-zero / non-negative results (runs on the dev machine, gated
behind `[Fact(Skip = "win32-only")]` if CI lacks the API surface — but local
gates need it green).

**Verification:** App launches; logged debug line every second shows real
numbers; killing memory pressure with a synthetic allocator triggers the
low-memory callback within ~1 s.

**Execution note:** Pragmatic.

---

### U4 — TerminalJobObject + ConPTY child enrollment

**Goal:** Tier-2 hard enforcement. Each terminal gets its own Job Object before
the shell does any meaningful allocation.

**Files (Create):**
- `app/Interop/TerminalJobObject.cs` — `Create(long processMemoryLimit)` →
  configures `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` with
  `JOB_OBJECT_LIMIT_PROCESS_MEMORY` and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
  returns the `SafeJobHandle`. `Assign(int pid)` calls `AssignProcessToJobObject`.

**Files (Modify):**
- Engine FFI surface — extend `optimus_engine_create` (or add a sibling
  `optimus_engine_get_child_pid`) so C# can recover the ConPTY child PID after
  `Engine::create`. Rust side: `engine/src/pty/conpty.rs` already exposes
  `proc_info.dwProcessId` (research, line 50-ish region); plumb it through
  `engine.rs` and out via csbindgen-generated header.
- `app/Splits/TerminalPaneSurfaceFactory.cs` — after `EngineHandle.Create`,
  read the child PID, allocate a `TerminalJobObject`, `Assign(pid)`, and stash
  the job handle on the `TerminalPane` so dispose closes it (triggers
  `KILL_ON_JOB_CLOSE`).

**Patterns to follow:**
- Existing FFI plumbing in `app/Interop/EngineHandle.cs` and `NativeMethods.g.cs`.
- Resource ownership tied to `TerminalPane` lifetime so workspace teardown
  cleans up jobs automatically.

**Window between `CreateProcess` and `AssignProcessToJobObject`:** the child
may briefly run uncapped. Acceptable for MVP — ConPTY children spawn the user's
shell which won't allocate aggressively in the first few ms. Tracked as a
deferred hardening (use `CREATE_SUSPENDED` flag in ConPTY spawn + resume after
assign) but not blocking.

**Test scenarios:** Rust integration test under `engine/tests/` already spawns
a real ConPTY (`u4_engine.rs` pattern); add a C# integration test under
`tests/Capacity/` that spawns a real terminal via the factory, asserts the
job handle is non-null and the PID is enrolled (call `IsProcessInJob`).

**Verification:** `cargo test` (engine) + `dotnet test` (app) both pass. Manual
smoke: spawn a terminal, run a `python -c "x=' '*int(3e9)"` style runaway in it,
confirm only that terminal's process errors out and the rest of Optimus stays
up.

**Execution note:** Characterization-first — capture current spawn behavior
with a test, then add the enrollment step. Spawn path is load-bearing; do not
break Phase 2/3/4 shipped behavior.

---

### U5 — Wire the gate into SurfaceManager.CreateSurface

**Goal:** Single choke point upgrade. Reserve before factory call; commit on
success; release on failure or surface teardown.

**Files (Modify):**
- `core/Splits/SurfaceManager.cs` (lines 35-44 per research) — inject
  `CapacityModel`; before delegating to `ISurfaceFactory.Create`, call
  `TryReserve(id)`. If `null`, return a typed `CapacityRefused` result (new
  enum/result type) instead of throwing. On factory success, `Commit(token, pid)`;
  on factory exception, `Release(id)` and rethrow.
- `core/Splits/SurfaceManager` callers (sidebar host etc.) — handle
  `CapacityRefused` by surfacing a non-fatal toast / disabled-button state
  instead of crashing.

**Patterns to follow:**
- Existing `ISurfaceFactory` injection pattern in `SurfaceManagerTests`.
- Idempotency guarantees on `SurfaceId` already enforced — capacity ledger
  must respect the same idempotency (re-creating an existing surface does not
  consume a second slot).

**Test scenarios (extend `tests/Splits/SurfaceManagerTests.cs`):**
- *Refusal at cap:* fake provider reports `MaxTerminals=2`, create three —
  third returns `CapacityRefused`, no factory call observed for it.
- *Idempotent re-create does not double-reserve.*
- *Teardown releases:* `Dispose(SurfaceId)` frees the slot; next `Create`
  succeeds.
- *Factory throws → reservation released.*

**Verification:** `dotnet test`. The existing 203 tests stay green; new
capacity tests in both `tests/Capacity/` and `tests/Splits/` pass.

**Execution note:** **Test-first.** Spawn-path regression risk is high — write
the failing tests before changing `CreateSurface`.

---

### U6 — Chrome indicator + disabled new-workspace state

**Goal:** Glanceable capacity meter above the New-Workspace button; calm →
amber → red; button disabled at cap with a single-line hint.

**Files (Create):**
- `app/Sidebar/CapacityIndicatorView.cs` — small WinUI control: text label
  "X / Y terminals", thin bar, colored per `CapacityState.Level`.
- `app/Sidebar/CapacityIndicatorViewModel.cs` — subscribes to `CapacityModel`
  state changes; marshals to UI thread.

**Files (Modify):**
- `app/Sidebar/SidebarView.cs` (lines 66-78 — the New-Workspace button area)
  — insert the indicator above the button; bind the button's `IsEnabled` to
  `state.Level != Cap`; add a one-line hint text below the button when at cap.
- DESIGN.md token registry (if not yet centralized, see DESIGN.md lines 179-185
  flag) — at minimum, define `CapacityCalm`, `CapacityWarn` (= `git-dirty`
  `#D9A04E`), `CapacityCap` (= `pr-closed` `#D96A6A`) in whatever shared
  tokens file the chrome reads from. Do **not** write `Color.FromArgb` literals
  in the view per CLAUDE.md.

**Patterns to follow:**
- Existing sidebar row styling in `SidebarView.cs` (lines 25-36).
- Bind, don't poll — view-model fires `PropertyChanged` from `CapacityModel`'s
  state event.

**Test scenarios:** View-model unit tests in `tests/Capacity/` — given a
`CapacityState` snapshot, asserts level mapping (< 75% → Calm, ≥ 75% → Warn,
== cap → Cap) and that text label formats correctly. UI rendering itself is
verified by the existing screenshot smoke recipe (memory: `phase5-ship-state` —
`Start-Process app + Start-Job CLI + PrintWindow`).

**Verification:** `dotnet test` passes view-model tests; screenshot smoke shows
the indicator rendering with the right token at startup (calm) and after
spawning to within 1 of cap (amber).

**Execution note:** Pragmatic; visual change verified by screenshot.

---

### U7 — End-to-end smoke + docs

**Goal:** Prove the spine works on a live build and leave behind enough doc
that the next plan (#1 process-tree metering) knows what already exists.

**Files (Modify):**
- `README.md` (or wherever the differentiator is currently described) — replace
  any "just a terminal multiplexer" framing with "RAM-bounded safe-zone
  capacity guarantee" lead, per CLAUDE.md / memory entry.
- `DESIGN.md` — if the capacity-indicator spec is still aspirational, mark it
  shipped and note the actual token names used in U6.

**Files (Create):**
- `docs/runbooks/2026-06-10-ram-safe-zone-smoke.md` — short runbook: how to
  force a low-memory condition, how to verify the cap drops, how to verify
  the job-object backstop fires.

**Verification:** Run the screenshot smoke recipe end-to-end on the dev
machine. Confirm:
- Indicator visible on startup with a sane `MaxTerminals` for the machine
  (16 GB → ~70-ish at 200 MB seed).
- Spawning past the cap is gracefully refused (no crash, button disabled,
  hint visible).
- Synthetic runaway inside one terminal does not kill its siblings.

**Execution note:** Pragmatic; this is the ship-gate unit.

## Deferred to Implementation

- **Per-terminal budget seed value** (200 MB) is a planner guess. U2's
  calibration replaces it; if telemetry from the first few real spawns
  suggests the seed is way off (e.g., agents routinely spawn at 500+ MB on
  day one because of language servers), revisit during U7 smoke.
- **Persistence format for `capacity.json`** — JSON vs. existing config
  conventions. Match whatever the IPC/workspace state already uses; decide
  during U2.
- **IOCP wiring for `JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT`** — researched and
  documented; included in U4 only if implementation reveals the surface is
  trivial to add. Otherwise deferred to a follow-up "memory-pressure
  affordances" plan along with #1.
- **Recovery hysteresis constants** (1.25 × headroom, two ticks) — heuristic;
  refine if smoke reveals the cap is too sticky or too flappy.

## Dependencies

U1 → U2 (provider interface used in tests, implementation in U3).
U1 → U3 → U4 (Job Object work needs interop + provider running).
U2 → U5 (gate consumes CapacityModel).
U2 → U6 (indicator binds to CapacityModel state).
U4 + U5 + U6 → U7 (smoke needs all three).

Parallelizable: U2 and U3 can proceed in parallel once U1 lands. U4 and U5 can
proceed in parallel once U2 lands. U6 can proceed in parallel with U4/U5.

## Verification Gates

- `dotnet test tests/Optimus.Core.Tests.csproj` — was 203 (memory:
  `phase5-ship-state`); target ≥ 215 with new capacity suite.
- `cargo test` from `engine/` — was 17; target ≥ 18 if Rust-side PID exposure
  gets its own test.
- `dotnet build` of the app — must succeed; cargo build the engine `--lib`
  first so the DLL is fresh (memory: `phase3-ship-state`).
- Screenshot smoke per `phase5-ship-state` memory entry — indicator visible,
  graceful refusal at cap, runaway isolation confirmed.
