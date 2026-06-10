---
date: 2026-06-10
topic: ram-safe-zone
focus: RAM-bounded safe-zone terminal-capacity feature (core differentiator, unimplemented)
mode: repo-grounded
---

# Ideation: RAM Safe-Zone Capacity

Optimus's reason to exist is the RAM-bounded "safe zone": measure available system
memory, compute the max terminals the machine can host without OOM, and lock that as a
hard cap so spawning parallel agents can never crash the box. The feature is currently
**unimplemented** — pure product intent. This ideation maps the strongest directions
for building it.

## Grounding Context (Codebase)

- **One full engine per terminal**, held for the terminal's whole lifetime: own ConPTY +
  wgpu DX12 device + swapchain + render thread + PTY reader thread. ~15–25 MB RAM **plus
  distinct GPU/swapchain memory** each. Background shells are deliberately never torn down.
- **Single spawn choke point:** `core/Splits/SurfaceManager.cs` `CreateSurface()`;
  workspaces created in `core/Sidebar/WorkspaceManager.cs` `NewWorkspace()`.
- **Startup hook candidate:** `app/App.xaml.cs` `OnLaunched` (before any engine creation).
- **Known unmeasured risk:** GPU memory pressure with many collapsed swapchains
  (master-plan risk #9). A surface pool was explicitly *not* built ("switch to the pool
  only if GPU-memory/panel pressure is measured").
- **Scrollback** default 10,000 lines, configurable — the main per-terminal RAM variable.
- **No existing RAM/capacity/limit code** anywhere (only a wgpu 2048 texture-limit guard).
- **DESIGN.md already specs** a capacity indicator: calm → amber (`git-dirty`) near limit
  → red (`pr-closed`) at cap, where "New workspace" is disabled with a plain-language reason.
- **Target user:** developer running many parallel coding-agent sessions who has crashed a
  box by spawning too many terminals. Each agent may spawn heavy child processes (compilers,
  language servers, node, docker) that dwarf the terminal engine's own footprint.

### External / prior art
- Win32: `GlobalMemoryStatusEx` (`ullAvailPhys`), `GetPerformanceInfo` (`CommitLimit`),
  `GetProcessMemoryInfo` (`PrivateUsage` for calibration), Job Objects
  (`JOBOBJECT_EXTENDED_LIMIT_INFORMATION.JobMemoryLimit`, `KILL_ON_JOB_CLOSE`),
  `CreateMemoryResourceNotification` (live pressure).
- Mental model: budget against **available physical**, enforce against **commit**.
  Kubernetes requests-vs-limits; cgroup v2 `memory.high`/`memory.max`. Reserve 15–20% OS
  headroom before dividing by per-terminal cost.

## Topic Axes
- measurement (how RAM/capacity is measured)
- enforcement (how the cap is enforced at spawn)
- UX/indicator (how capacity is surfaced)
- per-terminal cost control (scrollback, GPU, calibration)
- lifecycle/recovery (what happens at/near the cap, reclaiming)

## Ranked Ideas

### 1. Budget the process tree, not the engine
**Description:** Meter each terminal's full descendant process tree (walk child PIDs of the
PTY, sum working sets), not just the engine. The cap becomes "stop agents from drowning the
machine," the actual failure mode. Reserve a tunable headroom band for child-process load.
**Axis:** measurement
**Basis:** `direct:` — the engine is ~20 MB, but each terminal runs a coding agent that
spawns `cargo build`, LSPs, node, docker — dwarfing the engine by ~100×.
**Rationale:** Capping on the 20 MB engine while ignoring the 4 GB build it launches is a
guarantee that feels safe while the box still OOMs — a placebo vs a real safe zone.
**Downsides:** PID-tree walking on Windows is fiddly (Toolhelp snapshots / job-object
accounting); child cost is spiky and hard to predict before spawn.
**Confidence:** 85% · **Complexity:** High · **Status:** Unexplored

### 2. Self-calibrating per-terminal cost
**Description:** Don't hardcode cost. Spawn the first N engines, measure real committed-RAM +
VRAM delta on *this* hardware, derive the cap empirically. Cache per-engine cost keyed by a
hardware fingerprint so later launches lock the cap in <1 ms while recalibrating lazily.
**Axis:** per-terminal cost control
**Basis:** `reasoned:` + `external:` (`GetProcessMemoryInfo`/`PROCESS_MEMORY_COUNTERS_EX.PrivateUsage`)
— the "15–25 MB" figure varies wildly by GPU, driver, and scrollback; a constant is wrong everywhere.
**Rationale:** A guessed constant makes the cap too conservative (wastes the box) or too loose
(crashes it) — neither earns trust in the headline guarantee.
**Downsides:** Trades startup determinism (fixed number shown immediately) for accuracy (number
firms up after a few spawns).
**Confidence:** 88% · **Complexity:** Medium · **Status:** Unexplored

### 3. OS-enforced hard cap via Job Object + reserve-then-commit
**Description:** Put every engine subprocess in one Job Object with a `JobMemoryLimit` so the
OS enforces the ceiling rather than trusting our arithmetic, and route `CreateSurface()`
through a reserve-then-commit gate so two concurrent spawns can't both pass the check and
jointly blow the cap (TOCTOU race the parallel-agent workflow makes likely).
**Axis:** enforcement
**Basis:** `external:` + `direct:` — Win32 Job Objects (`JobMemoryLimit`, `KILL_ON_JOB_CLOSE`);
single choke point at `SurfaceManager.CreateSurface()`.
**Rationale:** Math-only enforcement lags reality; the OS failing the allocation is the backstop
that makes "can never crash the machine" literally true. One choke point = contained change.
**Downsides:** Job-object kill semantics are blunt (could kill a child mid-write);
reserve-then-commit adds latency and state to the spawn path.
**Confidence:** 82% · **Complexity:** Medium · **Status:** Unexplored

### 4. Live safe-zone that breathes with memory pressure
**Description:** A cap locked at `OnLaunched` is fiction once the user opens Chrome or starts a
Docker build. Re-derive the effective cap continuously from current available RAM and subscribe
to the low-memory notification; tighten the *new-spawn* ceiling under external pressure — but
never reap live terminals (data loss), only block new ones.
**Axis:** measurement
**Basis:** `external:` — `GlobalMemoryStatusEx` (`ullAvailPhys`) + `CreateMemoryResourceNotification`;
Linux PSI / cgroup `memory.high` mental model.
**Rationale:** The pain isn't only "too many terminals" — it's the box dying from total system
load. A cap that ignores non-Optimus memory gives false safety exactly when the machine is full.
**Downsides:** A floating cap is harder to explain than a fixed number; flapping (amber↔calm)
needs hysteresis.
**Confidence:** 80% · **Complexity:** Medium · **Status:** Unexplored

### 5. Dehydrate idle engines to reclaim capacity
**Description:** After a terminal sits idle and unfocused past a threshold, tear down the
expensive half (wgpu device + swapchain + render thread — the GPU pressure) while keeping the
ConPTY alive streaming into a ring buffer. Rehydrate a fresh engine on refocus and replay. The
cap counts only *live* engines, so a 4 GB laptop runs 2 live + 40 dehydrated instead of a wall at 2.
**Axis:** lifecycle/recovery
**Basis:** `direct:` (inversion) — grounding: engines are "held for the terminal's whole lifetime;
background shells never torn down." Inverting that deliberate decision is the highest-leverage move.
**Rationale:** Background shells holding full engines are pure dead weight; reclaiming the
GPU/render half multiplies effective capacity without killing the agent — a hard wall becomes an
LRU working set.
**Downsides:** Contradicts a deliberate architecture decision; rehydration must be fast and
seamless or it feels broken; PTY continuity during teardown is delicate.
**Confidence:** 72% · **Complexity:** High · **Status:** Unexplored

### 6. Weighted slots / QoS tiers for admission and eviction
**Description:** Stop counting terminals as equal units. Tag each: Guaranteed (reserves full
cost, never reclaimed), Burstable (reserves a floor), BestEffort (scratch shells reserve nothing,
first to be dehydrated). The cap is computed against summed weights; eviction order for idea #5
falls out of the tier.
**Axis:** enforcement
**Basis:** `external:` — Kubernetes QoS classes (Guaranteed / Burstable / BestEffort) + requests-vs-limits.
**Rationale:** A flat "N max" wastes capacity — idle scratch shells shouldn't block the critical
agent you're watching. Tiering protects what matters, sheds what doesn't, and gives dehydration a
principled victim-selection policy.
**Downsides:** Asks the user (or a heuristic) to classify terminals; another concept in already-dense chrome.
**Confidence:** 70% · **Complexity:** Medium · **Status:** Unexplored

### 7. Capacity indicator with forecast and graceful refusal
**Description:** Drive the indicator not just from current count but from a short-horizon forecast
("at this spawn rate you'll hit the cap in ~90 s"), show a per-spawn preflight when entering amber
("this terminal ≈ 1.2 GB; 2 fit after"), and at the cap offer graceful refusal *with a way forward*
(close one, or dehydrate the coldest) instead of a dead greyed button.
**Axis:** UX/indicator
**Basis:** `direct:` — DESIGN.md already mandates the calm→amber→red indicator and a disabled
"New workspace" with a plain-language reason; this defines its behavior.
**Rationale:** Today's pain is *silent* degradation; making cost legible at the decision point
converts a limit into a feeling of control — and turns the differentiator into something the user
sees working, not a buried setting.
**Downsides:** Forecasting from spawn rate is noisy; over-prompting at amber could annoy; depends
on #2/#4 for real numbers.
**Confidence:** 83% · **Complexity:** Low–Medium · **Status:** Unexplored

## Suggested build shape
- **MVP spine:** #2 → #3 → #7 (calibrate cost → OS-enforced cap → visible meter).
- **Honesty layer:** #1 and #4 (the real risk is child processes + external load, measured live).
- **Ambitious layer:** #5 and #6 (turn the hard wall into an elastic working set; #5 reopens the
  deliberate "never tear down engines" decision).

## Rejection Summary

| # | Idea | Reason Rejected |
|---|------|-----------------|
| 1 | Soft-cap with consented override | Folded into #6/#7 — a policy knob on enforcement, not standalone |
| 2 | Queue/waitlist scheduler at cap | Brainstorm variant; depends on #5 dehydration existing first |
| 3 | Capacity ledger as platform API | The substrate implied by #1/#2/#4, not a user-facing improvement to rank |
| 4 | Per-agent RAM attribution | Strong follow-on once the ledger exists; derivative of #1 |
| 5 | Crash-safe session journaling | Valuable but a separate feature — scope overrun beyond capacity |
| 6 | Cross-machine capacity overflow | Too expensive / far-future relative to an unbuilt MVP |
| 7 | Cgroup / WSL / job-object-limit awareness | Important edge case, but a refinement of #1/#4 measurement |
| 8 | Elastic scrollback spill-to-disk | Smaller lever; folded into #5's cost reclamation |
| 9 | Brownout (throttle fps/scrollback near cap) | Overlaps #5 and riskier (visible degradation) — keep #5's cleaner dehydration |
