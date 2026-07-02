# Demand Signals Surfaced in Research — Not Yet Actioned

> Scratch doc. Pulled from cognitive-load social scan + demand-validation workflow.
> Loudest first. Feature ideas = TBD (you fill in). Source: docs/research/cognitive-load-landscape.md.

---

## 1. Review / triage of N parallel diffs  ← LOUDEST UNMET

- **The pain:** reviewing many agents' diffs caps usable parallelism at 2–4 agents. The human review bottleneck, not the AI.
- **Their words:** "per-agent diffs for review," review/eval burden.
- **Status:** no pillar touches it. Named as pre-agreed first pivot if memory traction soft.
- **Currently:** parked, invisible in docs.
- **Optimus feature:** **PARK (declared fallback pivot).** Reviewing N diffs is the human bottleneck; a diff-triage surface is the pre-agreed pivot *if* memory traction goes soft. STRATEGY says don't center diff-review yet — memory hero ships first. Promote only on soft Pro conversion.

## 2. Goal-drift — agents wandering off intent

- **The pain:** agents drift off the original goal mid-run.
- **Their words:** "agents drifting off the goal."
- **Status:** no requirement. Provenance-adjacent (could detect contradicts / off-task edges).
- **Currently:** unowned.
- **Optimus feature:** **PARK (rides on correlation edges later).** Detecting "off original goal" falls out of the memory layer for free once `contradicts`/off-task edges exist — no new subsystem. Reframe after the memory core lands; don't build standalone now.

## 3. Fire-and-forget async hand-off + notify-on-done

- **The pain:** want to hand off a task, walk away, get pinged when done — instead of babysitting.
- **Their words:** "fire-and-forget hand-off."
- **Status:** notifications subsystem EXISTS but not framed as this demand. Plumbing, not positioned.
- **Currently:** cheap reframe available.
- **Optimus feature:** ✅ **LOCKED — claim now (free reframe).** Notifications subsystem already exists; this is positioning + one "done" hook, not new plumbing. Frame the existing notify path as fire-and-forget hand-off in copy + control plane.

## 4. "Multi-clauding" burnout — emotional driver

- **The pain:** exhaustion of juggling many agents at once.
- **Their words:** "multi-clauding," "babysitting" (dominant metaphor).
- **Status:** no direct product line. Emotional version of visibility + fire-and-forget.
- **Currently:** marketing voice ("stop babysitting"), not a feature.
- **Optimus feature:** **NO BUILD — marketing voice.** The emotional layer over #3/#6 ("stop babysitting your agents"). Lives in copy, not code. Zero build.

## 5. Structured workflow vs raw grid

- **The pain:** users want opinionated structure, not another tmux grid.
- **Their words:** "the biggest bottleneck isn't the AI — it's the human," "power but no structure" (re tmux).
- **Status:** partly served by control plane; not stated as a design principle.
- **Currently:** positioning gap — Optimus should read as *structured control*, not *more terminals*.
- **Optimus feature:** **NO BUILD — design principle.** "Structured control, not more terminals" governs how existing surfaces (control plane, workspace manager) read. Bake into framing + copy; no separate feature.

## 6. Coordination / "which terminal owns which branch" + tab sprawl

- **The pain:** losing track of which terminal owns which branch; tab sprawl.
- **Their words:** "which agent is doing what," "juggling," coordination/multiplexing burden.
- **Status:** workspace manager + isolation slots technically cover it, but never surfaced as a demand hook.
- **Currently:** buried as plumbing; it's a named pain you already solve.
- **Optimus feature:** ✅ **LOCKED — claim now (free reframe).** Workspace manager + isolation slots already solve this; it's buried. Surface as a named demand hook (which agent owns which branch/worktree) in the control plane — labeling work, not building work.

---

## Triage buckets (my read)

- **Decide now (load-bearing):** #1 review/triage — loudest unmet + named fallback pivot. Promote or consciously park.
- **Cheap reframes (already have mechanism):** #3, #5, #6 — claim the demand, don't rebuild.
- **Park w/ note:** #2 goal-drift (revisit later), #4 burnout (marketing voice).
