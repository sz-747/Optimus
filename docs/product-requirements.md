# Optimus — Product Requirements

> Source of truth for *what* Optimus is and the requirements it must meet.
> Strategy rationale lives in `STRATEGY.md`; pricing in `docs/pricing-strategy.md`;
> the pivot's technical scope in `docs/design/tauri-switch-scope.md`; the technical
> *how* in `docs/technical-requirements.md`. Last updated 2026-07-02.

## Overview

Optimus is a Windows-native desktop app (Tauri + Rust backend) for running many
parallel coding agents on one machine without crashing the box or losing track of
what each agent did. It gives away the orchestration and charges for the memory that
compounds and the safe parallelism the machine can unlock.

## Personas

- **Primary — solo / prosumer dev** running an agent fleet on their own Windows box.
  Hiring Optimus to run many agents at once without over-spawning the machine or holding
  every agent's context in their head.
- **Secondary — small startup team (3–15 devs)** coordinating agents on a shared, moving
  codebase. Revenue engine (governance, shared memory, org capacity policy).

## Differentiators

1. **Safe-zone capacity guarantee → isolated runtime slots.** Optimus measures the machine
   and caps concurrent agents at what it can *safely* host — reframed from a raw RAM cap to
   per-agent runtime isolation slots (RAM + ports + secrets + disk) with runaway-leak kill.
   Crash-proof multiplexing is the always-visible, non-arbitrary billing meter.

2. **Cognitive-load reduction via visibility + provenance** — a control plane that shows and
   explains what every agent did, powered by correlation memory *(see below — the hero differentiator)*.

3. **Windows-native first.** The flagship orchestrators (Conductor, Codex app) are Mac-first;
   Windows fleet-runners are underserved.

4. **Orchestration is free, BYO-agent.** Never paywall the multiplexer or resell tokens —
   the whole local camp is $0; the free tier must be genuinely good.

### Cognitive load (hero differentiator)

**Problem.** When a dev runs several agents, the bottleneck is no longer writing code — it's
the human holding every agent's context, decisions, and history in their head, and
re-explaining it every session. That mental tracking burden caps usable parallelism.

**Headline: see and explain what every agent did.** The loudest, most-upvoted demand in the
space (2026-07-01 social scan) is *visibility* — "which agent is doing what right now?", agent-tree
dashboards, per-subagent identity, historical replay — not "stop re-explaining" (that pain is real
but ranks 4th on loudness). So the hero surface is a **control plane** (see R6), and the memory
layer's first job is to power it: *who touched what, why, and in what order, with replay.*

**What we do that native memory does not.** Native memory (Anthropic's memory tool, verified
2026-07-01) is a flat Markdown store-and-reload: it *recalls* facts but does not *correlate*
them — the model re-derives every connection in-context, and the flat index taxes the window
(files >200 lines degrade adherence). Optimus adds a layer on top:

- **Correlation + provenance (the visibility engine).** Stores not just *what* changed but *why*
  (decision + rationale + code-state), and links facts across sessions with typed connections —
  the same data that drives the control plane's "explain what every agent did."
- **Shared cross-agent memory (secondary, not the banner).** One store all parallel agents read
  and write, so agents stop duplicating and the dev stops re-holding context. Deliberately *not*
  led with — the memory-persistence category is crowded and carries a live cost/accuracy objection
  (mem0/Zep benchmarked 14–77× cost and 31–33% *less* accurate than passing full history).
- **Distilled connections, not raw notes.** Stores the connections, not the transcript — cheaper
  on context as it grows. Crucially this is *execution-state* provenance (rule-derived edges),
  **not** the LLM-on-write fact-extraction pattern that lost the mem0/Zep benchmark; that's how
  we sidestep the objection.

**Bridge to capacity.** Cognitive load and the safe-zone are one story, not two: *run more agents
than you can hold in your head — safely.* The isolation-slot cap is the hardware answer to the
attention bottleneck the visibility layer addresses in software.

**Honest scope.** Zep already does temporal-graph correlation — but as a raw API memory layer
a dev must wire into their own agent. Optimus's wedge is correlation **inside the orchestrator,
across parallel agents, tied to live work, zero wiring**, surfaced as a control plane — not a
library. We do not claim to have invented correlation.

**How we prove it** (nobody proves "cognitive load" directly — we measure legible proxies;
see `docs/research/cognitive-load-landscape.md`):

- **Tokens-to-answer** — context needed to reach the same result, with vs. without the
  correlation layer (spine metric; demoable).
- **Re-explain count** — times the dev re-feeds context to an agent; must drop.
- **Correlations acted-on** — surfaced connections the user accepts/uses.
- **One-tap "saved my head" survey** — the perceptual half (DevEx-style).

Technical mechanism in `docs/technical-requirements.md` → "Lowering cognitive load."

## Core requirements

- **R1 — Capacity governor.** Measure available memory + runtime, compute a safe concurrent-slot
  cap, enforce it, and kill runaway/leaking sessions. Always-visible capacity indicator.
- **R2 — Correlation memory.** Provenance-tagged, cross-agent, correlation-linked memory store
  with bounded just-in-time injection (R2 is the hero; instrumented per the proof stack above).
- **R3 — Orchestration.** Spawn parallel agents in isolated worktrees; diff → PR → merge →
  archive. Free forever, BYO-agent.
- **R4 — Spend & cost control.** Live spend/capacity visibility (free); pre-run estimates +
  per-agent budget caps (paid).
- **R5 — Merge assist.** Reduce merge-conflict pain from parallel agents against a moving `main`.
- **R6 — Control plane (visibility surface).** Live per-agent status, token/cost, agent tree
  (who spawned whom), activity stream, and historical **replay** of a multi-agent run — in one
  UI, fed by the R2 provenance store (no new data source). This is the loudest demand in the
  space and the hero surface; basic live status is free, provenance depth + replay are paid.
  Two surfaced demands are *claims over mechanisms Optimus already has*, not new build — surface
  them here, don't rebuild:
  - **Fire-and-forget hand-off + notify-on-done.** The notifications subsystem already exists;
    frame it as "hand off, walk away, get pinged when done" — the antidote to babysitting. A
    "done" hook on agent completion routes to an OS toast that focuses the surface.
  - **Branch/worktree ownership at a glance.** Isolation slots + the workspace manager already
    track which agent owns which branch/worktree; the control plane surfaces it as a named
    coordination view (kills "which terminal owns which branch" + tab sprawl).

## Out of scope

Per `STRATEGY.md` → "Not working on": GPU renderer as a selling point, token resale, Mac-first /
cross-platform, paywalling the multiplexer, built-in zero-setup skills (no demand evidence),
centering a diff-review surface (revisit if memory traction is soft).

## Success metrics

See `STRATEGY.md` → "Key metrics." The hero-specific proof stack is the four cognitive-load
proxies above.
