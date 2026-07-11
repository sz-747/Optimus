# Moat Features — Technical Implementation Plan

> Step-by-step plan for the differentiators: **correlation + provenance memory (R2)**,
> **control plane with replay (R6)**, **isolation slots + leak-kill (R1 reframed)**, and
> **spend control (R4)** — the things competitors don't have or paywall at 13× (mem0's
> $249 graph tier is this feature, hosted).
> Companion to `docs/design/tauri-migration-plan.md` (the base this builds on),
> `docs/technical-requirements.md` (the *what*), `docs/product-requirements.md` (R1–R6),
> `docs/pricing-strategy.md` (free/paid gates). Written 2026-07-02.

---

## 0. Sequencing vs the Tauri migration

This plan starts **after migration P2** (domain logic in Rust) and runs alongside
migration P4–P6. The memory/correlation core (phases M0–M1 below) is a pure Rust crate
with no UI dependency — it can be built and tested in a worktree *during* the migration
without touching migration files. Control-plane UI (M2+) needs the Tauri frontend base.

**Dependency spine:** record store → capture → rule edges → live control plane →
JIT injection → replay → proof instrumentation. Isolation slots and spend control hang
off the existing capacity governor + job objects, independent of the memory spine —
they can run in parallel.

Guiding constraints (locked earlier, do not drift):

- **Rule-based edges, not embeddings, not LLM-on-write extraction.** This is the answer
  to the mem0/Zep 14–77×-cost / 31–33%-less-accurate benchmark. No model call on the
  write path, ever. Embedding similarity only if rules demonstrably miss (TRD ponytail note).
- **Edges are the product.** Native memory stores facts; we store the *connections*.
- **Control plane is a read model** over the same record+edge store — no second data source.
- **Free/paid line:** basic live status free; provenance depth + replay + correlation +
  shared/team memory paid (pricing doc pillar 3).

---

## 1. Phase map

| Phase | What | Sellable output | Gate |
|---|---|---|---|
| M0 | Record store + capture pipeline | (foundation) | records land from a real agent session |
| M1 | Correlation engine (rule edges) | "why" trail exists | policy unit tests green; edges explainable |
| M2 | Control plane: live status + agent tree + activity stream | **Free-tier hero surface** | "which agent is doing what" answerable at a glance |
| M3 | Retrieval + bounded JIT injection | **Pro: stop re-explaining** | injected context ≤ cap; with/without demo runs |
| M4 | Replay | **Pro hero: "see & explain what every agent did"** | reconstruct yesterday's 5-agent run |
| M5 | Proof instrumentation + in-product wins | marketing numbers | 4 proxy events flowing; first tokens-to-answer chart |
| M6 | Isolation slots + leak-kill | **Pro meter: safe parallelism** | leak drill: runaway agent killed, box alive |
| M7 | Spend control: estimates + caps | **Pro: budget caps** | pre-run estimate within honest error band; cap halts agent |

---

## 2. Phase M0 — Record store + capture pipeline

The store first; nothing correlates until records exist.

1. **Storage: SQLite** (`rusqlite`, bundled). One file:
   `%LOCALAPPDATA%\optimus\memory.db`.
   `// ponytail: SQLite, not a graph DB — edges are rows with two FKs; a graph DB is
   justified only if multi-hop queries measurably outgrow recursive CTEs.`

   ```sql
   CREATE TABLE record (
     id INTEGER PRIMARY KEY,      -- monotonic; the TRUE order (ts can tie/skew)
     fact TEXT NOT NULL,          -- what changed / what is true  (scrubbed, ≤4 KB)
     why TEXT,                    -- rationale/decision (provenance — the paid slice; scrubbed, ≤4 KB)
     agent_id TEXT NOT NULL,      -- session that produced it (set by orchestrator, not the writer)
     kind TEXT NOT NULL,          -- decision | change | status | spawn | error
     trust TEXT NOT NULL,         -- 'trusted' (orchestrator/capture) | 'untrusted' (agent self-report)
     file_key TEXT,               -- repo-relative path, NO branch (stable across rename/merge)
     branch TEXT,                 -- branch recorded separately, never part of the correlation key
     workspace_id INTEGER NOT NULL,
     reverses INTEGER REFERENCES record(id),  -- agent-declared explicit reversal target (nullable)
     ts INTEGER NOT NULL          -- unix ms (display + windows only; NOT the tiebreak)
   );
   CREATE TABLE edge (
     from_id INTEGER NOT NULL REFERENCES record(id),
     to_id INTEGER NOT NULL REFERENCES record(id),
     relation TEXT NOT NULL,      -- same_file | depends_on | supersedes | contradicts |
                                  -- decision_reversed | spawned
     rule TEXT NOT NULL,          -- which policy produced it (explainability = pitch)
     PRIMARY KEY (from_id, to_id, relation)
   );
   CREATE TABLE symbol (          -- indexed extraction; kills depends_on full-table scan
     record_id INTEGER NOT NULL REFERENCES record(id),
     sym TEXT NOT NULL,           -- symbol/path this record defines or references
     PRIMARY KEY (sym, record_id)
   );
   CREATE INDEX record_ts ON record(ts);
   CREATE INDEX record_filekey ON record(file_key) WHERE file_key IS NOT NULL;  -- partial: NULL never indexed
   CREATE INDEX record_agent ON record(agent_id);
   CREATE INDEX record_ws ON record(workspace_id);
   ```

2. **Write path — two APIs, one trust boundary** (battle-test RC1/RC4; see Appendix A):
   - `MemoryStore::append_trusted(rec)` — orchestrator-only (spawn events, git capture,
     status tee). Sets `agent_id`, `kind=spawn`, `trust='trusted'`. Not reachable from the
     agent-writable verb.
   - `MemoryStore::append_agent(surface_id, {fact, why?, kind∈{decision,change,status,error},
     code_ref?, reverses?})` — the `memory.record` verb. **Cannot** set `agent_id` (derived
     from the authenticated `surface_id`), **cannot** set `kind=spawn`, **cannot** set
     `trust`. Always lands `trust='untrusted'`. Rate-limited per surface (token bucket,
     default 20/s → excess coalesces to one record + a dropped-count).
   - **Record insert holds the lock; correlation does NOT.** `append_*` writes the row +
     symbol rows under the mutex and returns; a single background worker drains a queue and
     runs the policies (M1). Edges appear a beat later — fine for control plane + injection.
     `// ponytail: async correlation worker; the write lock guards only the row insert, so a
     slow rule can never head-of-line-block the agent fleet.`

3. **Capture sources** (each = one small adapter; all go through `append_trusted` except
   `memory.record`):
   - **CLI hook events** (claude/codex/gemini/cursor/copilot × session lifecycle). The
     `memory.record` verb → `append_agent` (untrusted tier). `agent_id` is **never** taken
     from the payload — it is resolved from the pipe-authenticated `OPTIMUS_SURFACE_ID`.
   - **Orchestration events:** spawn → `append_trusted` `kind=spawn` + `spawned` edge
     parent→child. The agent tree's ground truth — the one tier the control plane trusts.
   - **Git state per worktree:** on agent stop, capture **metadata only** — touched
     `file_key`s, lines±, symbols changed — **never the diff body** (RC2: the diff body is
     the #1 secret-exfiltration vector, and it bloats rows). Skip files matching `.gitignore`
     and secret-name patterns (`.env*`, `*.pem`, `id_*`). `append_trusted` `kind=change`.
   - **Status stream:** `set-status` verbs tee into `kind=status` records (trusted).
   - **Secret scrub at the boundary (all writes):** before any row is stored, redact
     high-entropy tokens + known key shapes (`AKIA…`, `ghp_…`, `sk-…`, `xox…`,
     `-----BEGIN`, JWT triple, `KEY=value` env lines) → `[REDACTED:kind]`.
     `// ponytail: entropy + prefix regex scrub, not a full DLP engine; err toward redacting.`

4. **Retention:** append-only; no deletes in v1 except `memory.clear` (user-invoked,
   whole-workspace). Size guard: warn at 500 MB.

Gate: run a real Claude Code session inside a pane → records visible via a debug
`memory.dump` verb, with correct agent_id/code_ref/ts.

## 3. Phase M1 — Correlation engine (the moat core)

Runs synchronously on `append` (rules are cheap — index lookups, no model, no I/O
beyond SQLite). Each rule: `fn(new: &Record, store: &Store) -> Vec<Edge>`.

**v1 policy set** (hardened after the Appendix-A battle-test — every matcher is now a
total, deterministic, index-driven function with a bounded candidate set):

| Rule | Emits | Hardened logic |
|---|---|---|
| `same_file` | new ↔ the **K most-recent** prior records with the same `file_key` (default K=20) | partial index on `file_key`; **NULL file_key never correlates** (kills the null-explosion); K-cap turns a hot monolith file from O(k²) into O(k·K) |
| `supersedes` | new → prior, same `file_key` + same `kind`, same `subject_key` | `subject_key` = **NFC-normalized, case-folded, whitespace-collapsed, 200-char-capped** first line; tiebreak + direction by **record `id`** (monotonic), never `ts` (ts can tie/skew) |
| `decision_reversed` | a `supersedes` where reversal is **explicitly declared** (`reverses` field set by the agent) **or** the pair matches a **closed antonym vocabulary** (adopt/drop, use/remove, enable/disable, add/delete) as whole tokens | no free-text "stance" NLP — the demo edge is precise or it isn't emitted (a mis-fired hero edge is worse than none) |
| `depends_on` | new → prior via the **`symbol` table join** (new references a `sym` another record defines) | **index join, not substring scan** — O(matches), never O(n²); if symbol extraction is unavailable for a record, the rule simply emits nothing |
| `contradicts` | new ↔ concurrent record: **different `agent_id`**, same `file_key`, both `kind=change`, `id` within a bounded recent window (not an unbounded ts scan) | the cross-agent collision detector; window is a fixed row count, so cost is bounded under a spawn storm |
| `spawned` | `append_trusted` only (M0) — never inferred, never agent-settable | ground truth the control plane trusts |

Trust & scope rules baked into every policy:

- **Untrusted records do not auto-inject and do not raise trusted-tree edges.** An
  `append_agent` record is visible + attributed in the control plane, but `spawned` and the
  cross-agent `contradicts` badge only trust orchestrator-written records — so a hostile or
  careless agent can pollute *its own* lane, never forge the tree or bury a peer's decision.
- **All rules are workspace-scoped**: candidates are filtered by `workspace_id` — no
  cross-workspace edges, no cross-project leakage.

Design rules:

- Every edge stores `rule` — the UI always answers "*why* are these linked": explainability
  is the pitch vs. embedding black boxes.
- Rules are **total, deterministic, adversary-aware, unit-tested**: fixture records in →
  exact edges out, *including* the adversarial fixtures from Appendix A (homoglyph subject,
  null file_key flood, hot-file, negation fact, forged kind). An underspecified matcher is
  an attacker-specified matcher — so each matcher has a written spec + a test vector.
- **Perf budget:** row insert **< 1 ms under the lock** at 100k records; correlation worker
  throughput ≥ sustained write rate; **no rule performs a full-table or substring scan** —
  enforced by review + a bench that fails on a query plan without an index.
- No retro-correlation pass in v1 (rules see records written after the rule ships).
  `// ponytail: backfill command only when a shipped rule proves worth re-running on history.`

Gate: `cargo test` policy suite green **including the Appendix-A adversarial fixtures**;
seeded 3-agent fixture produces the expected same_file/supersedes/contradicts graph; bench
proves insert <1 ms and every rule index-driven; a null-file_key flood and a 10k-record
hot-file produce bounded edge counts.

## 4. Phase M2 — Control plane: live surface (FREE tier)

The loudest market demand ("which agent is doing what right now") — free, because every
competitor gives basic visibility away; this is the land-grab surface that sells the paid
depth behind it.

1. **Agent tree view:** built from `spawned` edges + live session state (SurfaceManager
   already knows running/dead per SurfaceId). Node = agent: name, workspace, worktree
   branch, state (running/stalled/done), spawn parent.
2. **Per-agent live status:** current `set-status` value, elapsed time, last-activity
   ts. *Stalled* = no record and no PTY output for N minutes (N configurable, default 5) —
   directly answers the "babysitting" pain.
3. **Activity stream:** records as they land, newest-first, tagged agent_id + code_ref,
   filterable by workspace/agent. It's a SQL query with a LIMIT — no new infra.
4. **Contradiction badges:** `contradicts` edges surface as a warning chip on both
   agents in the tree ("agent 3 and agent 5 both touched capacity.rs") — first visible
   correlation payoff, free tier, deliberately: it demos the paid layer.
5. Surface: new sidebar panel + full-screen view in the Tauri frontend; DESIGN.md tokens;
   capacity indicator stays adjacent (bridge story: "run more agents than you can hold
   in your head — safely").

Token/cost counters per agent live here too but land in M7 (they need the spend plumbing).

Gate: 5 parallel agents running → tree shows correct parentage, states flip
running→done live, a forced same-file collision shows the contradiction badge.

## 5. Phase M3 — Retrieval + bounded JIT injection (PRO)

The context-cost win over native flat-index memory.

1. **Selection:** at agent task start (session-start hook), query: records linked by ≤2
   edge hops to the task's `file_key`s + the workspace's most-recent `decision` records,
   **filtered to the caller's `workspace_id`**. Rank: edge distance, then record `id`
   (recency). Take top-K until token budget.
2. **Budget:** hard cap, configurable, default 2,000 tokens (~4 chars/token estimate;
   `// ponytail: chars/4 token estimate — real tokenizer only if the cap proves
   inaccurate enough to matter`). Injected block is distilled: `fact — why (agent, when)`
   lines, not transcripts. **Cap is a MUST (TRD): injected memory stays flat as history
   grows — the anti-flat-index demo.**
3. **Injection mechanism + prompt-injection defense (battle-test RC1 — critical):** the
   memory layer moves text written by one agent into another agent's context, so record
   text is **untrusted data, never instructions.** Two hard rules:
   - **Only `trust='trusted'` records auto-inject** (orchestrator/capture-derived —
     spawn tree, git metadata, our own status). Agent free-text (`append_agent`,
     untrusted) is **never** auto-injected; it is retrievable only via an explicit
     `memory.search` the receiving agent chooses to run.
   - Injected content is rendered by **us** into a fenced, labelled, non-instruction block
     from structured fields (`fact`, `why`, `file_key`, `ts`, `rule`) — not the raw string
     spliced into the prompt. Control/instruction tokens stripped; a static preamble marks
     it reference-only. `// ponytail: fence + structured render + trusted-tier gate; a
     model-side "is this an injection" classifier only if the fence proves insufficient.`
4. **`memory.search` verb:** **workspace-scoped by default** (cross-workspace requires an
   explicit, logged flag; a Team-tier permission later). Agents query just-in-time — the
   pattern native memory got right — but never reach another project's records or secrets.
5. **Shared cross-agent by construction:** one store, all agents in a workspace read it —
   agent B's session-start sees agent A's *trusted* decisions with zero extra machinery.
   The re-explain killer is this phase; it needs no separate "sharing" feature.
6. **Paywall seam:** free tier injects nothing (memory off) or last-session-only; Pro gets
   full correlation-ranked injection + `memory.search`. Gate lives at the selection query.

Gate: two-session demo — session 1 states a decision, session 2 (different agent) receives
it under cap; `memory.search` returns ranked hits scoped to the workspace; injected size
flat after 10× record growth; **an untrusted record containing "ignore prior instructions…"
does NOT appear in another agent's auto-injected context** (RC1 regression test).

## 6. Phase M4 — Replay (PRO hero)

"See and explain what every agent did, in order, after the fact."

1. **Mechanism:** timestamp-ordered scan of records (+ edges overlaid), filtered by
   workspace + time range. `// ponytail: replay is an ORDER BY ts scan, not
   event-sourcing — snapshots only if a run's log is too big to scan interactively.`
2. **UI:** timeline view — lanes per agent, records as dots, edges as connecting arcs
   (supersedes/contradicts colored per DESIGN.md status hues), scrubber. Click a dot →
   fact + why + code_ref + rule-explained links.
3. **The demo moment** (build the UI around it): scrub to a `decision_reversed` edge and
   read *why the plan changed* — nobody else can show this; it's the provenance pitch in
   one screen.
4. Export: `memory.replay --json` verb for the CLI — **workspace-scoped, secret-scrubbed
   at source** (records were scrubbed at capture, so export can't leak keys the store never
   holds; RC2). Agents/scripts consume replays; cheap since it's the same query.
5. **Destructive verbs scoped:** `memory.clear` only clears the **caller's own
   `workspace_id`** — no agent can wipe another workspace's provenance (RC6).

Gate: replay a real prior 5-agent run end-to-end; the reversed-decision click-through
works; loads interactively at 10k records.

## 7. Phase M5 — Proof instrumentation (the "prove it" layer)

Product-analytics events, local-first (a JSONL sidecar next to memory.db; no telemetry
upload without opt-in):

- `tokens_to_answer` — injected-context size + session outcome, tagged with/without
  correlation layer (the A/B lever: free tier IS the without-arm baseline).
- `re_explain` — session-start injection offered a fact the user then re-typed anyway
  (detected: user prompt contains near-duplicate of an injected fact) → increment.
  Crude match is fine v1.
- `correlation_surfaced` / `correlation_accepted` — badge/injection shown vs. clicked
  /acted-on.
- `head_survey` — one-tap toast after a heavy multi-agent session: "did Optimus save
  you holding this in your head?" (DevEx perceptual half). Throttled: max 1/week.

Plus one **in-product wins panel**: "this week: N decisions recalled, M collisions
caught, ~X tokens saved" — the retention flywheel made visible (pricing risk #3
mitigation: correlation value compounds slowly, so *show* it compounding).

Gate: all four event types flowing from a real session; wins panel renders non-zero
numbers after a week of dogfooding.

## 8. Phase M6 — Isolation slots + leak-kill (PRO meter)

Extends the capacity governor (crown jewel, ported in migration P2) from RAM-cap to
per-agent runtime isolation. Independent of the memory spine.

1. **Slot = existing reserve/commit ledger entry, enriched:** RAM budget (exists) +
   port range + worktree disk root + secrets scope. Slot record lives with the
   SurfaceManager entry.
2. **Leak-kill (the reframed pain — unbounded leaks, not static caps):**
   - Job object already enforces hard `JOB_OBJECT_LIMIT_PROCESS_MEMORY` at 2× budget
     (exists — this is the backstop).
   - Add trajectory watch: governor's 1 Hz tick already calls
     `MeasureProcessPrivateBytes(pid)`; keep a short window per agent; sustained growth
     crossing budget → warn chip in control plane at 1.5×, kill + toast + record
     (`kind=error`, so it appears in replay) at 2×. Kill = job-object terminate (exists).
   - `// ponytail: linear-growth heuristic over a 60s window; smarter leak detection
     only if false-kill reports appear.`
3. **Port allocation:** per-slot port range lease (e.g. 100 ports/slot from a
   configurable base); exposed to the agent via env (`OPTIMUS_PORT_BASE/COUNT`) injected
   at spawn beside `OPTIMUS_SURFACE_ID`. Collision pain killed by convention, not by a
   firewall. Disk: the worktree root is already per-agent (orchestration invariant);
   surface it in the slot UI.
4. **Secrets scope (v1 = minimal):** per-slot env allowlist — agent processes get only
   the env vars the slot grants. `// ponytail: env filtering only; vault/broker
   integration is a Team-tier feature, later.`
5. **Paywall seam:** free = 2–3 slots (the concurrency cliff); Pro = machine's safe max.
   The gate already exists in `TryReserve` — it takes a tier-supplied max.

Gate: leak drill — agent with a deliberate allocator loop gets warned at 1.5×, killed
at 2×, box stays responsive, kill visible in control plane + replay; two agents get
non-overlapping port leases.

## 9. Phase M7 — Spend control (PRO upgrade of a free baseline)

1. **Free: live spend visibility.** Per-agent token/cost counters in the control plane.
   Source: CLI hooks (claude/codex expose usage in stop events) → `kind=status` cost
   records; sum per agent/workspace/day.
2. **Pro: pre-run estimates.** Estimate = history: median cost of similar past runs
   (same agent kind + workspace), shown at spawn time with an honest band ("similar runs:
   $0.80–$2.40"). `// ponytail: percentile lookup over own history, not a cost model.`
3. **Pro: budget caps.** Per-agent and per-workspace daily cap; on breach → pause agent
   (stop feeding input + status chip), user chooses kill/continue. Enforcement at the
   orchestrator, best-effort (BYO-agent means we see costs at hook granularity, not
   per-request — say so in the UI; never fake precision).

Gate: spawn with estimate shown; breach a $1 test cap mid-run → agent paused, toast,
control-plane chip; daily rollup matches hook-reported usage.

---

## 10. Risk register

| Risk | Sev | Mitigation |
|---|---|---|
| Rules too dumb → edges feel trivial ("two files touched — so what") | **High** | the demo edge is `decision_reversed`/`contradicts`, not `same_file`; M2 badges + M4 click-through built around those; correlation_accepted event tells us the truth early |
| Capture too thin (hooks miss the *why*) | High | `why` prompted at decision points via hook prompt-submit heuristics is v2; v1 accepts sparse why — provenance quality improves with orchestrator-owned events, which we fully control |
| Injection annoys (wrong facts in context) | Med | hard cap + ranked selection + per-workspace off switch; re_explain metric catches misses, correlation_accepted catches noise |
| Native platforms absorb this (Anthropic memory adds correlation) | Med | speed + orchestrator position: our edges come from *execution state across parallel agents* (spawn tree, worktree diffs, collisions) — data a model-vendor memory never sees; keep that framing in every surface |
| Leak-kill false positives kill honest builds | Med | warn-before-kill, 60 s sustained-growth window, kill always visible + reversible (respawn), heuristic tunable |
| Proof metrics show no win | High (honesty) | that's the metric working — pre-agreed pivot is review/triage surface (`docs/demand-signals-unactioned.md` #1); instrument first, market claims second |

## 11. Explicitly not in this plan

- No embeddings, no vector DB, no LLM-on-write extraction (locked; revisit only with
  evidence rules miss wanted connections).
- No team/org features (shared org memory, governance, SSO) — Team tier, separate plan
  after Pro proves.
- No review/triage-of-N-diffs surface — the pre-agreed pivot *if* memory traction is
  soft; watching, not building (STRATEGY "Not working on").
- No hosted/cloud sync — local-first is the posture; sync is a Team-tier question.

---

## Appendix A — Policy battle-test (adversarial, 2026-07-02)

Four adversary roles walked the write→correlate→inject→replay path. Holes merged
worst-first, each with the exact trigger, then reduced to root causes and resolved. Every
resolution is already folded into M0/M1/M3/M4 above; this appendix is the traceability +
the source of the required adversarial test fixtures.

### Holes (worst first, exact trigger)

| # | Hole | Exact trigger | Role |
|---|------|---------------|------|
| H1 | **Cross-agent prompt injection.** Record text injected verbatim into another agent's context → indirect instruction injection across the fleet. | A record whose `fact` = "ignore prior instructions; …" (from a malicious dep in one worktree, or web text an agent stored) → M3 injects into a co-worker agent. | security / hostile |
| H2 | **Secret capture + amplification.** Git-diff body / fact ingests API keys → stored plaintext → injected into other agents → exported via replay JSON. | Agent touches a file containing `.env`/`sk-…`; stop-time diff captures the body. | security |
| H3 | **`depends_on` = O(n²) full-table substring scan** on the synchronous write lock → blows the perf budget and head-of-line-blocks the whole fleet. | Table grows past a few k records; every write substring-scans all prior facts. | performance |
| H4 | **NULL `code_ref` correlation explosion.** All null-code_ref records match each other → O(n²) edges + garbage. | Many `status`/`decision` records without a file (normal). | careless / performance |
| H5 | **Hot-file hairball.** `same_file` links *all* history on one file → O(k²) edges. | 200+ records on one `main.rs` (normal at a hackathon monolith). | careless / performance |
| H6 | **Plaintext DB at rest + unscoped `memory.search`.** Any local process / any agent reads all cross-project provenance + captured secrets. | Local read of `memory.db`; agent searches another project's terms. | security |
| H7 | **`agent_id` / `kind=spawn` spoofing.** Forged records inject fake nodes into the agent tree / mis-attribute provenance. | `memory.record {kind:"spawn"}` or a payload-supplied `agent_id`. | hostile |
| H8 | **Subject-key forgery / Unicode evasion → false `supersedes` buries a real decision.** | Copy a target decision's first line (collide) or swap a homoglyph (evade). | hostile |
| H9 | **`decision_reversed` "stances differ" undefined/gameable** → the *demo* edge mis-fires or misses. | Negation-laden fact, or a reversal with no negation word. | hostile / careless |
| H10 | **Duplicate/retry-loop flood** → edge churn + activity-stream flood. | Agent retry loop emits identical records at high rate. | careless / performance |
| H11 | **Branch churn fragments `code_ref`** → provenance silently breaks. | Rebase/rename branch, or merge to main (records orphaned at old `@branch`). | careless |
| H12 | **Identical-`ts` nondeterminism** in `supersedes` "newest wins" → flaky edges/tests. | Two records in the same millisecond. | performance / careless |
| H13 | **Oversized `fact`/`why`** → row bloat, injection truncation mid-word. | Paste a full stack trace as `fact`. | careless |
| H14 | **`memory.clear` cross-workspace** wipes another workspace's provenance. | Call `memory.clear` with a foreign `workspace_id`. | hostile |

### Root causes (first principles)

- **RC1 — Data provenance ≠ data trust.** We record who-said-what, then *consume* it as if
  who-said-what were authoritative and safe. Everything flowing OUT (inject/search/export)
  inherits attacker-controlled content. → H1, H7, part of H2.
- **RC2 — A memory that stores everything stores what must never be remembered.** No
  sensitivity classification at capture. → H2, part of H6.
- **RC3 — Correlation keyed on a mutable, low-cardinality, sometimes-null string** (`code_ref`
  as both identity and join key) yields too many edges (null/hot-file) or too few (branch
  churn). → H4, H5, H11.
- **RC4 — A hot-path op with no upper bound on work has no upper bound on latency.** Rules ran
  unbounded, synchronously, under one lock. → H3, H4/H5 (perf), fleet stall.
- **RC5 — An underspecified matcher is an attacker-specified matcher.** "first line", "stances
  differ", "containment" defined in prose, not as total deterministic functions. → H8, H9, H12.
- **RC6 — Shared-by-default without scope is leak-by-default.** No workspace scoping / authz on
  cross-agent reads and destructive verbs. → H6 (search), H14, part of H7.
- **RC7 — No input bounds at the write boundary** (size, rate, cardinality). → H10, H13.

### Resolutions (deployed above)

- **RC1 →** trust tiers (`trusted` orchestrator/capture vs `untrusted` agent free-text); only
  trusted auto-injects; injection renders **structured fields inside a fenced non-instruction
  block**, never raw text; `kind=spawn` + `agent_id` only settable by the orchestrator API.
- **RC2 →** secret scrub at the write boundary; git capture stores **metadata, not diff body**;
  skip `.gitignore`/secret-name files.
- **RC3 →** split `file_key` (branch-free, stable) from `branch`; **NULL file_key never
  correlates** (partial index); **K-most-recent cap** on `same_file`.
- **RC4 →** record insert holds the lock, **correlation runs on an async worker**; every rule
  index-driven + candidate-bounded; bench fails on any scan without an index.
- **RC5 →** `subject_key` = NFC + case-fold + collapse + 200-char cap; tiebreak by monotonic
  `id` not `ts`; `depends_on` via the indexed `symbol` table (no substring scan);
  `decision_reversed` = explicit `reverses` field or closed antonym vocab (no stance NLP).
- **RC6 →** all rules + `memory.search` + `memory.clear` **workspace-scoped**; cross-workspace
  is explicit + logged.
- **RC7 →** `fact`/`why` capped at 4 KB; per-surface write **rate limit** (token bucket, coalesce
  floods); deterministic `id`-based ordering.

Each hole becomes a **failing-then-passing test fixture** in the M1 policy suite (the M1 gate
now requires them). Deferred to v2 (logged, not silently dropped): DB encryption at rest (H6
residual — v1 relies on OS user-account isolation + the secret scrub); a model-side injection
classifier (H1 residual — v1 relies on the trusted-tier gate + fenced render).
