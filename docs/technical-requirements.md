# Optimus — Technical Requirements

> The technical *how*. Product intent lives in `docs/product-requirements.md`; pivot
> scope (Tauri/Rust backend, render integration, KEEP/DROP/REBUILD) in
> `docs/design/tauri-switch-scope.md` — not re-duplicated here. Last updated 2026-07-02.

## Architecture summary

Tauri desktop app, Windows-only. Single **Rust backend, in-process** (no C-ABI FFI, no
.NET): engine (ConPTY/VT/OSC-99), capacity governor, split-tree, workspace manager,
notifications, IPC, named-pipe server, job objects, memory. Web frontend (HTML/CSS/JS)
over `tokens.css`. See `tauri-switch-scope.md` for the full port map and the airspace
resolution still open.

## Lowering cognitive load (technical)

The hero requirement (R2). The goal is a memory layer that *correlates* rather than just
recalls, is *shared* across parallel agents, and stays *cheap on context* as it grows —
the three things native flat-file memory does not do (see
`docs/research/cognitive-load-landscape.md`).

### Data model

A memory **record**, not a note:

```
Record {
  id
  fact          // what changed / what is true
  why           // rationale / decision behind it  (provenance — the un-commoditized slice)
  agent_id      // which agent session produced it
  code_ref      // file path + commit/branch it attaches to
  timestamp
}
```

Records are connected by typed **correlation edges**:

```
Edge { from_id, to_id, relation }
relation ∈ { same_file, depends_on, supersedes, contradicts, decision_reversed }
```

The edges are the product. Native memory stores records (files); Optimus stores the
*connections between them* so the human — and the agent — don't re-derive them each time.

### Correlation engine

Edges are produced by **policies (rules), not embeddings, first.**
`// ponytail: rule-based correlation first; add embedding similarity only if rules
demonstrably miss connections users want.` Rules are explainable (we can show *why* two
facts are linked — matters for a provenance-led pitch), cheap (no vector DB, no GPU), and
deterministic (testable). **These edges are *execution-state provenance derived by rules* —
not LLM-on-write fact extraction** (the pattern a public benchmark punished at 14–77× cost
and 31–33% lower accuracy than passing full history). That distinction is our answer to the
memory-layer cost/accuracy objection; do not drift into per-write LLM extraction. Example
policies:

- Two records touching the same `code_ref` → `same_file` edge.
- A record whose `fact` overwrites an earlier record's `fact` on the same `code_ref` →
  `supersedes` / `decision_reversed` edge (this is the "why did we change our mind" trail).
- A record referencing a symbol defined in another record's `code_ref` → `depends_on`.

### Retrieval / injection (the context-cost win)

At agent task start, select correlation edges relevant to the current `code_ref`/task and
inject them under a **bounded token budget** (hard cap); everything else stays retrievable
**just-in-time** on demand. This is the opposite of native auto-memory, which reloads a
flat index every session and grows unbounded (files >200 lines degrade adherence). Storing
*distilled edges* instead of raw transcript keeps injected context roughly flat as the
project's history grows.

Requirement: injected memory context per task MUST stay under a configurable cap
(default target: well below the native flat-index cost, measured — see instrumentation).

### Shared cross-agent store

One backend memory store (Rust, in-process). All agent sessions read and **write-through**:
a record produced by agent A is visible to agent B on its next retrieval. This is the
mechanism that kills re-explaining — the dev states a decision once, every agent inherits it.

Concurrency: single-writer or per-`code_ref` lock on the store.
`// ponytail: global write lock first; shard by code_ref only if write contention shows up.`

### Control plane (visibility surface)

The hero surface (R6), and the loudest demand in the space. It is a *read model* over the
same record+edge store — no new data source:

- **Agent tree** — reconstructed from spawn/`depends_on` edges: who spawned whom, which agent
  owns which `code_ref`/worktree.
- **Per-agent live status** — running/stalled/done, current task, token + cost counters, from
  the engine's session state.
- **Activity stream** — records as they land, tagged by `agent_id` and `code_ref`.
- **Replay** — reconstruct a multi-agent run by ordering records/edges on `timestamp`; answers
  "what did each agent do, why, and in what order" after the fact.

`// ponytail: replay is a timestamp-ordered scan of the record log, not an event-sourcing
framework — add snapshots only if a run's log gets too large to scan interactively.`
Basic live status is free; provenance depth + replay are the paid tier (see PRD R6).

### Instrumentation (so we can prove it)

The layer MUST emit events that feed the four proof proxies (per PRD / research doc) —
proving cognitive-load reduction requires measurable proxies, not the abstraction:

- `tokens_to_answer` — context tokens consumed to reach a result, tagged with/without the
  correlation layer (the spine number; enables the with-vs-without demo).
- `re_explain` — increment whenever the dev manually re-feeds context an existing record
  already holds.
- `correlation_surfaced` / `correlation_accepted` — edges shown vs. edges the user acts on.
- `head_survey` — one-tap perceptual response ("saved you holding this in your head?").

These are product-analytics events, not user-facing.

### Verification

One runnable check on the non-trivial logic (the correlation policies): a unit test that
feeds a small fixture of records and asserts the engine emits the expected edges (e.g. two
same-`code_ref` records → `same_file`; an overwrite → `supersedes`). No framework beyond the
Rust test harness.

## Data storage & privacy boundary

**Engine: SQLite (`rusqlite`), embedded in-process — not Postgres.** Optimus is a local-first,
single-machine desktop app; each user runs their own copy with their own DB file under
`%LOCALAPPDATA%`. One file, zero-config, ships inside the binary — no server to run, no external
dependency, public-domain license. Postgres' multi-user-over-network model buys nothing here and
adds an ops burden. WAL mode covers the concurrency needed (many readers + the single global
write lock already planned). A server-side store only enters later, as a *sync layer alongside*
SQLite, if Team-shared memory or cross-user analytics ship — it never replaces the local file.

**Stored** (SQLite): records + edges; session state (mode, phase, freeze checklist, override
flags); worktree/spawn rows (`base_commit`, file scope, cost class, spawn tree); instrumentation
events. **Not stored**: mode presets (compiled consts), thresholds/budgets (config), replay (a
query, not a store), live status (ephemeral engine state). **Never stored**: code diff bodies —
only `code_ref` (path + commit/branch) + git metadata; secrets scrubbed at the write boundary.

### Privacy boundary — two buckets

Local-first storage is a first-class selling point (your memory never leaves your machine — vs
GitHub-hosted Copilot memory, hosted mem0/Letta; the wedge for confidentiality-bound teams that
*can't* send code provenance to a vendor cloud). To keep the promise honest, data splits into two
buckets with different rules:

**Bucket 1 — memory content. Never leaves the machine. Absolute.**
Records, edges, the `why`/provenance, `code_ref`s, git metadata. The developer has no access,
there is no toggle, and no mode or tier changes this. This is the promise the local-first pitch
rests on — enforced by never opening an outbound socket for content, not by a setting.

**Bucket 2 — product telemetry. Anonymized aggregate counts only, never content.**
The four proof proxies (`tokens_to_answer`, `re_explain`, `correlation_surfaced`/`_accepted`,
`head_survey`) as *numbers* — never the text of a record, never a `code_ref`, never a diff. Ships
to improve the product. **Toggle defaults ON, opt-out** (industry norm; the user unchecks to
withhold). The product is fully functional with telemetry off — nothing gates on it.

**Proof-to-user is local.** The with-vs-without-correlation demo and the head-survey number are
shown to the user from their own local DB — no exfiltration needed. Only coarse aggregate metrics
ever leave, and only under Bucket 2's opt-out toggle.

```
// ponytail: one boolean (telemetry_enabled, default true) gates Bucket 2. Bucket 1 has no
// flag — it is enforced by there being no outbound path for content at all.
```

**Trade-off, recorded:** default-on telemetry matches how most coding tools ship, but it *does*
soften the privacy wedge for confidentiality-sensitive / enterprise buyers — the exact segment
the local-first story wins hardest. Leave room to default Bucket 2 **off** for a Team/enterprise
tier (or honor an org policy), without ever touching Bucket 1.

## Contrast with the field

Optimus is **not** competing against flat Markdown files — that would be a strawman. Most
tools' "memory" *is* flat files (Cursor Rules, Windsurf Rules, Codex `AGENTS.md`, Cline Memory
Bank, Aider `CONVENTIONS.md`, Continue Rules — and Cursor *killed* its dynamic Memories in 2.1,
retreating to static Rules). But two real competitors are past that bar, and the honest
comparison must be against them:

- **GitHub Copilot Agentic Memory** (GA preview Jan 2026) — the strongest native store:
  structured records (`subject`/`fact`/`citations`/`reason`), cross-agent within a repo,
  re-verified against live code, self-superseding.
- **Dedicated memory layers** — mem0 (vector + KV + a now-ranking-only entity graph), Letta
  (vector archival + shared memory blocks).

| Axis | Flat-file tools | Copilot Agentic Memory | mem0 / Letta | **Optimus** |
|---|---|---|---|---|
| Storage | Markdown files | Hosted structured records | Vector DB (+ thin graph, mem0) | Local records + typed edges (SQLite) |
| Captures | Facts / rules | Facts + code citations | Facts; mem0 entity triples | Facts + **why** (provenance) |
| Typed relationships | None | Citations + simple self-supersede | mem0 thin triples (ranking-only in OSS); Letta none | **6 typed edges** (`same_file`, `contradicts`, `supersedes`, `depends_on`, `decision_reversed`, `spawned`) |
| **Where edges come from** | Human-authored | Extracted from runs vs code (LLM) | LLM extraction from conversation/NL | **Execution state** — spawn tree, worktree diffs, same-file collisions, contradictions across *live parallel agents* (data none of the others can see) |
| Cross-agent sharing | Per-repo files (static) | Cross-agent within a repo (hosted) | Identity-scoped / shared blocks | Shared write-through store, local |
| Determinism / explainability | n/a | Hosted black box | LLM extraction (nondeterministic) | **Deterministic rules** — shows *why* two facts link |
| Write cost | None | Hosted (opaque) | **LLM-on-write tax** (public benchmark: 14–77× cost, 31–33% lower accuracy vs full history) | None — rules, no per-write LLM |
| Hosting / durability | Local, manual, drifts | GitHub-hosted, **28-day auto-expiry** | Hosted or self-host infra (vector DB / Neo4j) | Local-first, no hosting, no expiry |
| Retrieval / context cost | Reload index each session; >200 lines degrade | Hosted retrieval | Vector similarity | Bounded JIT injection of distilled edges, budget-capped |

**The moat, stated honestly:** cross-agent sharing and structured memory are *not* unique —
Copilot, mem0, and Letta all have them. What no one else has is the specific stack Optimus is
built on: **edges derived from live multi-agent execution state** (not conversation text),
**deterministic and explainable** (no LLM-on-write cost/accuracy tax), **local with no expiry**.
Optimus does not win a generic "better memory" contest; it wins the one it is actually built
for — real-time correlation across many parallel coding agents on one machine.

---

## Implementation inventory (post-migration)

Everything below lands **after** the Tauri migration (`docs/design/tauri-migration-plan.md`
phases P0–P7). Detailed per-phase design lives in `docs/design/moat-features-plan.md`
(M0–M7); this is the categorized *what-gets-built* index, not a re-spec. Each item names
its owning phase.

**A. Memory + correlation core** (M0–M1)
- Record store: SQLite (rusqlite, in-process), `Record` + `Edge` tables per schema above. [M0]
- Edge policies: the 6 rule-based relations. Deterministic, no embeddings, no LLM-on-write. [M1]
- Write boundary: secret-scrub before persist; git capture stores metadata, not diff body. [M0]
- Concurrency: single global write lock. `// ponytail: shard by code_ref only if contention shows.` [M0]
- Verification: fixture-in / expected-edges-out unit test on the policies. [M1]

**B. Control plane** (M2)
- Read model over record+edge store — no new data source.
- Agent tree (from `spawned`/`depends_on`), per-agent live status, activity stream, replay (ts-ordered scan).
- `SurfaceProfile` knob (what the live surface foregrounds) — read by the modes layer.

**C. Isolation + capacity governor** (M6 + governor)
- Capacity governor: startup RAM measure → safe-zone cap. **Hard, mode-invariant.** Port test-first.
- **WebGL renderer contexts = a governed capacity resource.** Chromium/WebView2 hard-caps live WebGL contexts at ~16/page (silent eviction beyond). The governor budgets renderer contexts alongside RAM/slots; a frontend **pane-visibility manager** attaches the xterm.js WebGL addon only to on-screen panes (≤ ~12), off-screen panes detach and sit as paused headless buffers (bytes accumulate in the backend Surface, replay on reveal). Invisible plumbing reacting to existing layout/viewport events — not the control plane (R6). Falls straight out of the differentiator: rendering ability is one more scarce thing the capacity model already governs. Impl in migration-plan Phase 4 item 1; soak-gated (WebView2).
- Isolation slots: one worktree/job-object per writer agent.
- Coordinated fan-out: `base_commit` pinned per writer-fan-out + base-drift flag + file-ownership partition (see the dedicated section below).
- Leak-kill: `leak_warn_factor` / `leak_kill_factor` (job-object kill), optional `stuck_iterations_kill`.
- Auto-pause new spawns at `autopause_capacity_pct` of safe-zone.

**D. Spend control** (M7)
- Live cost/token counters per agent (from engine session state).
- **Cost class per agent** (`recon` read-only vs `writer`): spend attributed and budgeted by class. Cost ≈ model tier × loop length — a swarm of read-only recon agents is near-free; writers are ~all the burn. Budget guard + burn banner rank by class so the builders, not the reader count, read as the cost. `// ponytail: two classes, not a full cost model; add tiers only if needed.`
- Optional hard `sprint_budget_usd`, `per_agent_budget_usd` (the per-agent cap applies to `writer`s; recon share a loose pool).
- Degradation low-water warning (agent tokens below threshold → warn; silent-degrade defense).

**E. Modes system** (new thin layer, builds on A–D)
- `Mode` struct = preset over the M1/M2/M6/M7 knobs; atomic swap; `current_mode` in session record.
- Preset rows: `MAIN`, `HACKATHON` (below). `// ponytail: adding a mode = adding a row.`
- Phase timeline (hackathon only): wall-clock T-minus state machine + spawn-gate check.
- Demo-path guard: `owns_the_demo` flag on a workspace row + scope filter on `contradicts` loudness.
- Freeze checklist: 6 booleans on the session record, rendered as a panel.
- Context-health meter ("dumb-zone proximity"): per-agent context-fill vs degradation-threshold readout + compact/clear nudge. Both modes (advisory in Main; red blocks in Hackathon).
- Readiness gate: pre-flight prompt-quality grade on long-horizon spawns (Hackathon); 3-bucket reject + `user_forced_readiness_override` flag. Advisory in Main.

**F. Agent interface + trust boundary** (folds into IPC port, migration P2)
- Named-pipe server (raw `CreateNamedPipeW`, owner-SID ACL); agents post `memory.record` JSON.
- Agent free-text is untrusted: fenced non-instruction render, trusted-tier gate before any inject.
- Workspace-scoping on search / clear / replay.

**G. Instrumentation** (M5)
- The 4 proof proxies: `tokens_to_answer`, `re_explain`, `correlation_surfaced`/`_accepted`, `head_survey`.
- Product-analytics events, not user-facing.

**H. Paid-tier gating** (M3–M4)
- JIT edge injection under a 2k-token hard cap (Pro).
- Replay depth + provenance trail (Pro). Basic live status stays free.

---

## Operating modes — Main and Hackathon

Optimus ships **two modes at launch: Main (default) and Hackathon.** A mode is a named
preset over knobs that subsystems A–E already own, swapped atomically — not a plugin
engine, not new subsystems. Switching re-evaluates in-flight agents against the new
thresholds on their next tick. Narrative rationale + research sourcing live in
`docs/design/hackathon-mode.md`; the concrete knob values are here.

**Invariant across every mode:** the capacity-governor RAM safe-zone cap is measured at
startup, locked, and identical in all modes. No mode may raise, relax, or bypass it. Modes
tune how early Optimus warns/kills/shouts *inside* the safe zone — never the zone itself.

### Main mode (default)

Steady-state: sane defaults, low interrupt rate, no artificial budgets, no phase timeline.

| Subsystem | Knob | Main default |
|---|---|---|
| M1 | `same_file` | Badge |
| M1 | `contradicts` | Badge |
| M1 | `supersedes` | Badge |
| M1 | `depends_on` | Off (queryable in replay) |
| M1 | `decision_reversed` | Badge |
| M1 | `spawned` | Off (tree renders it) |
| M2 | `SurfaceProfile` | `tree-first` — agent tree + status foregrounded; cost per-agent, not bannered |
| M6 | `leak_warn_factor` | 1.5× RSS baseline |
| M6 | `leak_kill_factor` | 2.0× RSS baseline (job-object kill) |
| M6 | `stuck_iterations_kill` | Off (stalled = status badge, human decides) |
| M6 | `autopause_capacity_pct` | 90% of safe-zone |
| M7 | `sprint_budget_usd` | Off |
| M7 | `per_agent_budget_usd` | Off |
| M7 | degradation low-water | On at 10% tokens remaining |
| Capacity | safe-zone cap | Locked at startup |
| Phases | timeline | None |

### Hackathon mode

Posture for a time-boxed sprint (default assumed **12h overnight**; `// ponytail:` sprint
length is a mode parameter, not auto-detected). Tightens toward: don't collide, don't
overspend, don't let an agent run off-rails, land a demo.

| Subsystem | Knob | Hackathon value | Why (research) |
|---|---|---|---|
| M1 | `same_file` | **Alarm** (blocking chip on every agent touching the file + notify) | Same-file collisions (globals.css / ORM schema edited by 3 agents) = #1 parallel-agent coordination failure |
| M1 | `contradicts` | **Alarm** | AI moves crunch to verification/merge (METR: feel 24% faster, are 19% slower); a merge-time contradiction is the expensive kind |
| M1 | `supersedes` | Badge | Long-horizon lineage; matters post-sprint, not in 12h |
| M1 | `depends_on` | **Badge** (up from Off) | Merge order is the bottleneck; make the dependency chain visible without interrupting |
| M1 | `decision_reversed` | Badge | Review-queue item, not stop-the-world mid-sprint |
| M1 | `spawned` | Off | Unchanged |
| M2 | `SurfaceProfile` | **`burn-and-collision`**: banner = live burn $/hr vs budget remaining + capacity gauge; agent tree ranked by cost-burn desc; `same_file`/`contradicts` alarm chips pinned; stalled agents promoted w/ no-progress count | Only ~25% of scope ships; surface must make spend, collisions, stuck agents impossible to miss |
| M6 | `leak_warn_factor` | **1.4×** | 5–6 agents standard (up to 23 extreme); per-slot headroom scarcer, warn ~7% earlier |
| M6 | `leak_kill_factor` | **1.75×** (down from 2.0×) | One leaker at 2× can push a near-full sprint into autopause and starve spawns; kill earlier |
| M6 | `stuck_iterations_kill` | **3** → kill + flag for reassignment | Folklore: "kill + reassign after 3 stuck iterations"; retry loops are a documented off-rails mode |
| M6 | `autopause_capacity_pct` | **85%** of safe-zone | Folklore: "auto-pause at 85%"; 15% band for existing agents' growth before the hard cap |
| M7 | `sprint_budget_usd` | **$750** hard ceiling | Observed $600–800/sprint; 6 agents × $10.50/agent-hr × 12h = $756; $750 = top-of-band hard stop |
| M7 | `per_agent_budget_usd` | **$125** → pause + triage (**writers only**; recon share a loose pool) | $750 / 6 agents ≈ one agent's full 12h sprint-share at $10.50/hr; read-only recon barely burns, so the cap guards builders |
| M7 | degradation low-water | **20%** tokens remaining (up from 10%) | Claude silently degrades 20–44% when low on tokens; warn *before* the degrade zone, not inside it |
| Capacity | safe-zone cap | **Locked — identical to Main** | Over-allocating RAM to "go faster" crashes the machine mid-demo; the one thing a sprint may never buy back |
| Phases | timeline | **Build → Freeze → Demo** (below) | Last ~4h = code freeze is the documented winning pattern |

`// ponytail:` per-agent budget is flat $125 for all agents, not role-weighted. Weighted
budgets are a post-hackathon refinement; flat is enforceable today with the existing M7 knob.

### Phase timeline (Hackathon only)

Sub-presets *within* Hackathon, triggered by wall-clock T-minus against an operator-entered
deadline. `// ponytail:` triggers are wall-clock only — no auto-detection of "how done are
we." Operator may advance a phase early, never rewind past Freeze.

| Phase | Trigger | What changes |
|---|---|---|
| **Build** | activation → T-4h | Full hackathon knob table. New-feature spawns allowed. |
| **Freeze** | **T-4h** | New spawns **blocked unless tagged `fix`/`rehearse`** (M6 spawn-gate dial). `SurfaceProfile` swaps to `freeze-checklist` (6 items below replace the activity stream). Unspent budget above $100 re-fenced as reserve, not permission. |
| **Demo** | **T-45min** | Read-only: all spawns blocked (incl. `fix`), running agents drain/pause, M1 alarms mute to Badge (no blocking chip mid-demo), M2 locks to replay + happy-path. T-45 (not T-30) so Demo posture begins *before* the 30-min submit buffer. |

**Freeze checklist** (6 booleans, rendered in M2 at T-4h — tracked, not automated):
1. Demo video recorded **twice** · 2. Happy path rehearsed live end-to-end · 3. Judge README
written (what/why-it-wins, not build steps) · 4. Demo path hardcoded (seed data, fixed ports,
skip auth) · 5. Submitted **30min early** · 6. `main` demo-able — every post-T-4h merge leaves it runnable.

Main mode has **no** phase timeline: no clock, no freeze gate, no read-only posture.

### Readiness gate + context-health meter (Hackathon)

**Context-health meter ("dumb-zone proximity").** Per-agent readout of context-window fill
vs the point where quality degrades (long-context falloff + the 20–44% low-token silent-degrade
effect). Derived from M2 token counters + a per-model degradation threshold (`// ponytail:`
empirically calibrated constant — leave the knob). Amber → compact-or-clear nudge; red →
blocking nudge in Hackathon. Present in both modes; advisory-only in Main.

**Readiness gate.** Before a **long-horizon spawn**, Hackathon grades the prompt + handoff
context and rejects weak input rather than burning 12h on it. One grading pass on the spawn
path — *not* the memory write path (so no no-LLM-on-write conflict), and the graded text is the
user's own trusted input (so no injection surface). Returns pass/fail + three buckets:
**missing** (required context absent), **vague** (underspecified), **over-free** (latitude the
agent will hallucinate to fill). Fail → reject + surface buckets. Auto-fail if the handoff
context is already in the dumb zone (meter red). **Override toggle:** force-start stamps
`user_forced_readiness_override = true` on the session record — bad output is then the user's
call, not Optimus's. Main mode: advisory (warn, never block).

### Delta summary

| Knob | Main | Hackathon |
|---|---|---|
| M1 `same_file` | Badge | **Alarm** |
| M1 `contradicts` | Badge | **Alarm** |
| M1 `supersedes` | Badge | Badge |
| M1 `depends_on` | Off | **Badge** |
| M1 `decision_reversed` | Badge | Badge |
| M1 `spawned` | Off | Off |
| M2 `SurfaceProfile` | `tree-first` | **`burn-and-collision`** → `freeze-checklist` at T-4h |
| M6 `leak_warn_factor` | 1.5× | **1.4×** |
| M6 `leak_kill_factor` | 2.0× | **1.75×** |
| M6 `stuck_iterations_kill` | Off | **3 → kill + flag** |
| M6 `autopause_capacity_pct` | 90% | **85%** |
| M7 `sprint_budget_usd` | Off | **$750 hard** |
| M7 cost-class attribution | On (spend shown by recon/writer) | On |
| M7 `per_agent_budget_usd` | Off | **$125 → pause + triage (writers only)** |
| M7 degradation low-water | 10% remaining | **20% remaining** |
| Phase timeline | None | **Build → Freeze (T-4h) → Demo (T-45min)** |
| Context-health meter | Advisory readout | **Blocking nudge at red + feeds readiness gate** |
| Readiness gate (long-horizon spawns) | Advisory warn | **Hard reject unless `user_forced` override** |
| Capacity safe-zone cap | Locked at startup | **Identical — never changes** |

---

## Coordinated fan-out (both modes)

The answer to the "reduce merge-conflict pain against a moving main" pillar. Applies in
**both** modes; Hackathon only turns the collision alarms louder (M1 `same_file`/`contradicts`
→ Alarm). Optimus already owns every primitive this needs — spawn tree, worktree isolation,
the M1 edges — so this is orchestration policy over existing parts, not a new subsystem.

**The invariant is a shared base commit, not simultaneity.** "Deploy all writers at once" is a
proxy; what actually prevents rebase hell is pinning every writer in a fan-out to the *same*
`main` HEAD, so the base never moves under them mid-sprint. A writer spawned an hour later,
pinned to the same base, gets the same benefit.

Three parts:

1. **Base pin.** On the first writer spawn of a fan-out, record `base_commit` = current `main`
   HEAD; every sibling writer inherits it (a field on the spawn/worktree record). The control
   plane flags any writer whose base drifts from its siblings (**base-drift warning**). *Reuses
   the spawn tree + one commit field.*
2. **File-ownership partition.** Each writer declares/receives a non-overlapping file scope.
   `same_file` / `contradicts` fires the moment two *live* writers' scopes overlap — a shared
   base makes a collision static and knowable, it does not remove it; the partition does.
   *Reuses the M1 edges + `file_key` scope.*
3. **Single coordinated integration.** All writers merge back against the pinned base in one
   pass at sprint end (or via a designated integrator); conflicts surface via `contradicts`.
   **No mid-flight rebase onto a moving `main`.**

**Dependency caveat — parallelize independent work only.** A shared base hides sibling work
until integration, so if writer B needs writer A's output, base-pin fan-out is *wrong* for that
pair — sequence them instead. The `depends_on` edge is exactly the signal: independent domains
fan out off the shared base; dependency chains queue.

```
// ponytail: base_commit is one column on the worktree row + a drift check in the control
// plane's read model. Partition + conflict surfacing are the M1 edges we already build.
// Integrator agent is optional v2 — a manual coordinated merge covers v1.
```
