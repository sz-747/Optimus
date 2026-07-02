# Hackathon Mode — first mode, spec

> Modes retune the whole system for a context. This is the first one.
> Grounded in 4-angle web research (2026-07-02): judging criteria, workflow
> failures, AI-agents-at-hackathons, demo mechanics. Sources at bottom.
>
> Design stance (ponytail): a **mode is a named preset over knobs that already
> exist** — threshold values, policy-enable flags, and what the control plane
> surfaces — swapped atomically. It is NOT a plugin engine, not new subsystems.
> A few genuinely new bits (freeze phase, demo-path guard, readiness gate,
> context-health meter); everything else is a dial on M1/M2/M6/M7 turned to a
> different number.

---

## 1. What a hackathon demo actually needs (research → requirement)

Compressed from the 4 research angles. Each line is a requirement the mode must serve, with the evidence tier and where it maps.

**Speed / scope**
- Only ~25% of planned scope ships; winning projects cap at **~3 features done perfectly** over a broad platform where nothing works. [strong] → scope discipline, freeze
- AI coding does **not** remove the crunch — it moves it from generation to *verification/debugging* (METR: devs feel 24% faster, are 19% slower). [strong] → the bottleneck Optimus should attack is review/merge, not typing
- Deploy/CI/env validation gets skipped and left too late — **the single recurring killer** is "no time to set up deploy." Teams validate the concept before the environment; should be reversed. [strong] → early deploy-readiness nudge

**Parallel-agent reality (the money angle)**
- Real counts: **5-6 agents in parallel, one per domain** (Anthropic hackathon 2nd place); up to 23 in extreme runs. [strong] → this IS Optimus's load profile
- Same-file collisions are the #1 coordination failure: `globals.css`, ORM schema edited by 3 sprints at once; "two agents writing the same file produce a merge conflict neither knows about." [strong] → `contradicts` + `same_file` must be LOUD here
- "Which agent is stuck / finished / about to merge a conflict?" — the core visibility gap. [strong] → control plane front-and-center
- Diverging context: 4 sessions compact independently → 4 contradictory summaries. [medium] → memory/correlation is the antidote
- Spend: **$600-800 per overnight sprint**, ~$10.50/agent-hr; and Claude **silently degrades work 20-44% when low on tokens, no warning**. [strong/medium] → hard sprint budget + degradation warning
- Agents go off-rails near deadline: fake seed data, wrong branch, retry loops, one **deleted a prod DB in 9s** guessing a destructive call. Guardrail advice: **kill + reassign after 3 stuck iterations**, per-agent token budgets, **auto-pause at 85% capacity**. [strong] → leak-kill retune

**Demo mechanics / freeze**
- **Freeze the code in the last ~4h** (30min for a short event): no new features, no refactors, no "one more bug" — "the bug spawns three more." Shift to test + rehearse. [strong]
- Freeze phase has deliverables: **record demo video ×2, rehearse the happy path 5×, write a judge README, submit 30min early.** Submit-early is itself a health signal. [strong]
- Localhost + flaky API = a demo that fails on stage. **Hardcode the demo path, cache the LLM response, screenshots as fallback.** Judges penalize a demo that never reaches its punchline, not one that mocks a call. [strong]
- Keep **main branch demo-able at all times**; risky work on a feature branch. [strong/medium]
- ~90-second problem→working-demo, single wow moment, meet the stated requirements first. [strong] → out of scope for the tool, but the freeze checklist can nudge

---

## 2. The mode model (cheapest thing that works)

A mode is one struct, swapped atomically, read by the subsystems that already have the knobs:

```rust
struct Mode {
    name: &'static str,
    // M7 spend
    sprint_budget_usd: Option<f64>,     // hard ceiling for the whole session
    per_agent_budget_usd: Option<f64>,
    autopause_capacity_pct: u8,         // pause new spawns at this % of safe-zone
    // M6 leak-kill
    stuck_iterations_kill: Option<u32>, // kill+flag after N no-progress cycles
    leak_warn_factor: f32,              // ×baseline RSS
    leak_kill_factor: f32,
    // M1 policies: which correlations are HOT (surfaced loud) vs quiet vs off
    policy_weights: PolicyWeights,      // per-relation: Alarm | Badge | Off
    // M2 control plane
    surface: SurfaceProfile,            // what the live surface foregrounds
    // new: phase awareness
    phases: &'static [Phase],           // Build → Freeze → Demo, with T-triggers
}
```

`policy_weights` reuses the M1 relations verbatim (`same_file`, `contradicts`,
`supersedes`, `depends_on`, `decision_reversed`, `spawned`) — the mode only
changes each relation's *loudness*, never its logic. No new correlation code.

```
// ponytail: modes are a preset TABLE, not an engine. Adding a mode = adding a
// row. If we ever need per-user overrides, add a merge over the base row then —
// not before.
```

---

## 3. Hackathon Mode — knob by knob

| Subsystem | Default | Hackathon | Why (research) |
|---|---|---|---|
| **`contradicts` (M1)** | Badge | **Alarm** — blocking chip on both agents + notification | same-file collision is the #1 parallel-agent failure; a bad merge to main pre-demo = death |
| **`same_file` (M1)** | Badge | **Alarm when ≥3 live agents share a `file_key`** | `globals.css`/schema hairball; H5 already flags "hackathon monolith" load |
| **`supersedes` / `decision_reversed`** | Badge | **Quiet** (Badge, no notify) | long-horizon cross-session edges matter less in a 24h sprint; keep signal-to-noise high |
| **`depends_on`** | Badge | Badge | unchanged |
| **Control plane surface (M2)** | tree + status | **foreground "stuck / done / about-to-conflict" + demo-path branch** | "which agent is stuck/finished/about-to-conflict" is the named visibility gap |
| **Spend (M7)** | live visibility | **hard `sprint_budget_usd` + `per_agent_budget_usd` + degradation warning** when an agent's remaining tokens cross the low-water mark | $600-800/sprint; silent 20-44% quality drop on low tokens |
| **Auto-pause (M6)** | off | **pause new spawns at 85% of safe-zone** | research guardrail; protects the machine mid-sprint |
| **Leak-kill (M6)** | 1.5× warn / 2× kill | **1.4× warn / 1.75× kill + `stuck_iterations_kill = 3`** (kill + flag `kind=error`, auto-offer reassign) | per-slot headroom scarcer at 5–23 agents (warn ~7% earlier); one leaker at 2× can starve a near-full sprint into autopause; "kill + reassign after 3 stuck iterations" for off-rails agents |
| **Capacity governor** | safe-zone cap | **unchanged — cap stays hard** | crashing the machine mid-sprint is the single worst outcome; speed never overrides the safe-zone (this is *why Optimus exists*) |

**The one thing that does NOT relax:** the RAM safe-zone cap. Hackathon mode
pushes toward *more* parallel throughput within the cap and auto-pauses before
it — it never lifts it. A crash at hour 20 loses the whole sprint.

---

## 4. The genuinely new bits

### 4a. Phase awareness — Build → Freeze → Demo

The mode carries a phase timeline keyed off a user-set deadline. Phases flip
thresholds again (a mode-within-the-mode), cheaply:

- **Build** (start → T-minus-freeze): everything above.
- **Freeze** (T-minus-4h, or user-triggered): **block new-feature spawns** — the control plane refuses to launch a fresh agent unless tagged `fix`/`rehearse`. Correlation goes fully quiet except `contradicts` (still Alarm). Surfaces the **freeze checklist** (below). Research: last hours are subtractive triage, not building.
- **Demo** (T-minus-30m): read-only posture. Surface the demo-path branch health + backup-ready checklist. Nudge **submit-early**.

Freeze is a hard, visible state change, not a suggestion — matches the "enforce
an artificial freeze deadline" finding. User can override per-spawn, but it's
opt-in friction, which is the point.

**Freeze checklist** (surfaced, checked off — not automated, just tracked):
demo video recorded ×2 · happy path rehearsed · judge README written · demo
path hardcoded / LLM response cached · main branch green · submitted 30min out.

```
// ponytail: checklist is 6 booleans in the session record, rendered as a
// panel. Not a workflow engine. Automating the video/deploy is v2 if asked.
```

### 4b. Demo-path guard

User marks one worktree/branch as **owns-the-demo**. Any `contradicts` or risky
write touching that branch's `file_key`s escalates to Alarm regardless of phase.
This is just `contradicts` with a scope filter + a max-loudness override — no
new correlation logic. Serves "keep main demo-able."

```
// ponytail: reuses the contradicts edge + file_key scope. A flag on the
// workspace row, not a new subsystem.
```

### 4c. Context-health meter — "how close to the dumb zone"

A per-agent readout of how full the context window is relative to the point
where model quality starts to fall off (the **dumb zone** — long-context
degradation, plus the low-token silent-degrade effect the research flagged at
20–44%). Derived from the token counters M2 already collects — no new data.

- Green / amber / red band against a per-model degradation threshold.
- Amber → a **compact-or-clear nudge** notification; red → strong nudge, and in
  Hackathon a **blocking chip** (this context is too degraded to trust).
- Two inputs: window fill %, and remaining-token low-water (reuses the M7
  degradation-warning threshold from §3).

Present in both modes (advisory in Main); in Hackathon red is blocking and feeds
the readiness gate below.

```
// ponytail: derived readout over counters we already have + one per-model
// threshold constant. Calibrate the threshold empirically per model — leave the
// knob, don't hardcode a guess.
```

### 4d. Readiness gate — pre-flight input-quality check before a long-horizon spawn

Before Hackathon commits an agent to a **long-horizon task** (the 12h-sprint
kind), it grades the prompt + handoff context and **refuses to start if the
input is too weak to justify the burn**. One grading pass on the *spawn path* —
not the memory write path, so it does **not** violate no-LLM-on-write; the graded
text is the user's own trusted input, so no injection surface.

Rubric returns pass/fail + three buckets, in the user's terms:
- **Missing** — required context absent (no acceptance criteria, no target
  files, no definition of done).
- **Vague** — underspecified instructions the model will interpret inconsistently.
- **Over-free** — latitude the agent will fill by **guessing / hallucinating**.

Fail → reject the spawn, surface the three buckets, ask the user to tighten.
Auto-fail if the handoff context is already in the dumb zone (4c red = garbage-in).

**Override toggle:** a confident user can force-start past a rejection. Overriding
stamps the spawn `user_forced_readiness_override = true` in the session record —
so if the 12h run comes out bad, accountability is explicit and it is not on
Optimus.

```
// ponytail: one grading call per long-horizon spawn, not per event. Rubric is a
// prompt returning 3 string lists + a bool. Main mode: advisory (warn, never block).
```

---

## 5. What Hackathon mode deliberately does NOT do

- **No deploy automation.** Research says deploy-left-late is the killer, but Optimus is a local orchestrator, not a CD pipeline. It *nudges* deploy-readiness early in the checklist; it does not run the deploy. Building a deploy engine is scope creep away from the moat.
- **No demo-video recording / screen capture.** Nudged, not built. v2 if users ask.
- **No relaxing of correctness in the correlation engine.** Speed comes from *loudness/threshold* changes, never from making edges dumber or skipping the secret-scrub / trust boundary. The Appendix-A hardening holds in every mode.
- **No lifting the safe-zone cap.** Ever. (§3.)
- **No new agent-side surface.** Agents still post the same `memory.record` verb; the mode is entirely orchestrator-side config.

---

## 6. Build cost

Small, because it's a preset over shipped subsystems:
1. The `Mode` struct + a `HACKATHON` const row + atomic swap + "current mode" in the session record.
2. `policy_weights` read in M2's badge renderer (Alarm vs Badge vs Off branch).
3. Phase timer + freeze spawn-gate (one check in the spawn path).
4. Demo-path flag on the workspace row + scope filter on `contradicts` loudness.
5. Freeze checklist panel (6 booleans).
6. Spend/leak knob values wired to M7/M6 (the mechanisms already exist).
7. Context-health meter: a derived readout over the token counters M2 already has + one per-model degradation threshold + a nudge notification.
8. Readiness gate: one grading call on the long-horizon spawn path + a 3-bucket rubric + the `user_forced_readiness_override` flag on the session record.

Depends on: M1, M2, M6, M7 landed. Modes are a thin layer on top — build after
the moat core, not before. No migration dependency beyond what those phases need.

---

## 7. Sources

Judging criteria: MLH judging plan; JetBrains "How to win a hackathon — notes from the judging table" (2026); HackerEarth 10 tips; Devpost judging tips.
Workflow failures: "The Hackathon We Didn't Finish"; "How We Lost the Hackathon" (Around25); METR via "Vibe Coding Reality Check"; Railway deploy-issues thread.
AI agents: Anthropic "Built with Opus 4.7" winners; "I Ran 23 AI Agents Overnight" (dev.to); Cursor parallel-agents guide; repomirror YC field report; "Agent-of-Agents Problem"; Tom's Hardware "deleted DB in 9s."
Demo mechanics: HackerEarth freeze-phase checklist; tathagata.dev hackathon strategies; Devpost video best-practices; Hackaday "Don't Tempt the Demo Gods."
