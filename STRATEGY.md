---
name: Optimus
last_updated: 2026-07-01
---

# Optimus Strategy

## Target problem

Developers now run several coding agents at once, but one machine and one human brain can't keep up: you crash the box by over-spawning, you drown trying to track what each agent did and why, and parallel branches collide against a `main` that keeps moving. The bottleneck has moved from *"can the AI write the code?"* to *"can one person on one machine safely supervise a fleet of agents?"*

## Our approach

Remove the human and the hardware as the two bottlenecks. The guiding bet is **cognitive-load reduction through visibility + provenance**: *see and explain what every agent did* — which agent touched what, why, and in what order, with replay — powered by a correlation memory layer that stores the connections, not raw notes (native memory recalls but does not correlate; verified 2026-07-01). Shared cross-agent memory is the secondary win — agents stop duplicating and the dev stops re-holding context — but *not* the banner: the memory-persistence category is crowded and carries a live cost/accuracy objection (mem0/Zep benchmarked 14–77× cost, less accurate) that our rule-based provenance edges sidestep. This bridges to the hardware story — **run more agents than you can hold in your head, safely** — via per-agent **runtime isolation slots** (RAM, ports, secrets, disk) + leak-kill. Monetization: **give away the orchestration (commoditized to $0), charge for the provenance/correlation depth and the safe parallelism** — never resell tokens, never paywall the multiplexer.

## Who it's for

**Primary:** Solo / prosumer developers running agent fleets on their own Windows machine. They're hiring Optimus to run many agents at once without crashing the box or losing track of what each one is doing — and to produce like a small team without renting a data center.

**Secondary:** Small startup teams (3–15 devs) coordinating agents on a shared, moving codebase. They are the revenue engine (governance, shared memory, org capacity policy), but solo-dev needs win ties on product decisions.

## Key metrics

- **Concurrent agents actually run** — per active user per week; the throughput that's really used, not just spawned. (Product analytics.)
- **Acted-on cross-session correlations** — count of memory-surfaced connections the user accepts/uses; proves the hero value and drives retention. (In-product events.)
- **Clean parallel-merge rate** — % of parallel-agent merges landed without manual conflict resolution; can regress if orchestration degrades. (Git/merge telemetry.)
- **Free→Pro conversion at the concurrency cliff** — share of capped free users who upgrade when they hit the slot limit. (Billing + analytics.)
- **Week-4 retention** of activated users — lagging health of the whole thesis. (Analytics.)

## Tracks

### Agent visibility & provenance memory

The control plane — live per-agent status, cost, agent tree (who spawned whom), activity stream, and historical **replay** — powered by a correlation/provenance memory layer that captures the *why* behind each change and links facts across sessions. Shared cross-agent so agents don't duplicate and the dev doesn't re-hold context.

_Why it serves the approach:_ It *is* the guiding bet and the premium paywall, aimed at the loudest, most-upvoted demand ("which agent did what, with replay") rather than the quieter "stop re-explaining." Native memory (Anthropic's memory tool, verified 2026-07-01) is flat Markdown store-and-reload — it *recalls* but does not *correlate*, and its index taxes the window (>200 lines degrades adherence). Our layer stores distilled provenance edges instead of raw notes — cheaper on context, and *execution-state* provenance, not the LLM-on-write fact extraction the mem0/Zep benchmark punished. That's the one memory slice the vendors haven't shipped.

### Safe parallel execution

Per-agent runtime isolation slots (RAM, ports, secrets, disk) with a capacity governor that detects and kills runaway/leaking sessions, plus merge-conflict reduction at the integration end.

_Why it serves the approach:_ Removes the runtime bottleneck the evidence actually names (isolation and leaks, not a static RAM cap), doubles as the visible non-arbitrary billing meter, and lets many agents run and land without stepping on each other.

### Spend & cost control

Live spend/capacity visibility (free) plus pre-run cost estimates and per-agent budget caps (paid).

_Why it serves the approach:_ Turns the token-bill anxiety competitors leave open into a legible upgrade, without building a compute-resale margin the market punishes.

## Not working on

- A GPU terminal renderer as a selling point — it was inherited from the cmux port; terminals render as web under Tauri. See `docs/design/tauri-switch-scope.md`.
- Reselling model/token compute — bring-your-own-agent; the market punishes markup.
- Mac-first or cross-platform — Windows-native first.
- Charging for the orchestration/multiplexer itself — commoditized to $0 (Conductor, TUICommander).
- Built-in zero-setup agent skills as a pillar — no demand evidence found (2026-07-01 validation); cut pending signal.
- Centering a diff-review/triage surface — the strongest *unmet* demand in the space, but out of scope while the spine stays memory-led; first thing to revisit if memory traction is soft.

## Marketing

**One-liner:** Run an AI dev team on one machine — Optimus shows you what every agent did and why, with replay, so you stop babysitting, and safely runs more agents than you can hold in your head.

**Key message:** See and explain what every agent did — that kills the babysitting; isolated runtime slots let you run more agents than you can track, safely. Orchestration is free forever; you pay for the provenance/correlation depth and the safe parallelism your machine can unlock.
