# Pricing Strategy — Optimus

> Companion to `STRATEGY.md`. Full tier detail + rationale from the 2026-07-01
> competitor-pricing research (`agent-tool-pricing-research` workflow, 4 buckets +
> synthesis). STRATEGY.md carries the one-paragraph stance; this is the working detail.

## Value metric

**Concurrent isolated runtime slots** — a hardware-derived capacity unit (RAM + ports +
secrets + disk per agent, with runaway-leak kill), *not* per-seat, *not* per-token. The
capacity indicator already core to the product **is** the billing meter, so the user always
sees exactly what they pay for. It's the one unit competitors can't tell a story around,
because it's the safe-zone made billable: *"produce like you own 5 Mac Studios."* Memory
correlation rides on top as the recurring reason-to-subscribe, not as its own meter (meter
the write, give the read away — Zep pattern).

> **Repositioned 2026-07-01** after demand validation: "RAM cap" → "isolated runtime slots
> + leak-kill" (the loudest crashes are unbounded per-process leaks a static cap can't stop;
> the market's real pain is runtime isolation, not raw RAM). Memory paywall narrowed to the
> defensible **provenance ("why") + shared multi-agent** slice — single-session recall is
> commoditized and shipped natively by model vendors. P1 (built-in skills) cut: no demand.

## Tiers

| Tier | Price | Behind the paywall | Rationale |
|------|-------|--------------------|-----------|
| **Free** (land-grab) | $0 | Full orchestration forever (spawn parallel agents in worktrees, diff→PR→merge→archive); BYO-agent (your Claude Code/Codex/Cursor sub); **low slot cap (2–3)**; basic single-session recall; zero-setup built-in skills; read-only spend/capacity dashboard. | Never charge for the multiplexer — the whole local camp is $0, incl. same-stack free competitor TUICommander. Free must be genuinely good to win the land grab. The low cap is a deliberate concurrency cliff (how CI vendors convert on queue pain). |
| **Pro** | **$20/mo** | Isolated runtime slots unlocked to the machine's true safe max (RAM+ports+secrets+disk) + runaway-leak kill; **HERO: cross-session memory correlation (provenance + shared)**; pre-run cost estimates + per-agent budget caps; solo merge-conflict assist. | $20 is the locked category anchor (Cursor/Windsurf/Warp/Factory/Devin). The two things no competitor charges for — safe-parallelism unlock + provenance/shared memory — live here. Memory is **not** usage-metered; it compounds → retention flywheel. |
| **Team** | **$40/seat/mo** | Everything in Pro + shared org memory + org capacity policy + **merge-conflict governance across many agents on a moving main** + SSO / audit / ZDR / pooled budgets. Seats grant pooled capacity. | The market monetizes team *governance* separately from capability; enterprise WTP is proven (Factory $1.5B, Devin/Nubank). This is the real revenue engine. |
| **Enterprise** | Custom | On-prem / RBAC / dedicated policy. | Where Factory & Devin actually extract dollars. |

Power-user ceiling reference: category tops out ~$200/mo (Cursor Ultra, Windsurf Max, Warp Max) — leave room for a high-capacity Pro tier if solo power users demand more slots.

## Pillar → willingness-to-pay

| Pillar | Role | Note |
|--------|------|------|
| 3 — Visibility + provenance memory (control plane) | **paywall (hero)** | Re-aimed 2026-07-01: the loudest demand is *visibility* ("which agent did what, with replay"), not "stop re-explaining." Basic live agent status = free; gate provenance depth + **replay** + correlation. Native memory *recalls but doesn't correlate* (flat, taxes window >200 lines); mem0/Zep memory layers carry a 14–77× cost / lower-accuracy anti-signal — so frame ours as *execution-state provenance* (rule-derived edges), not LLM-on-write extraction. Store distilled connections: cheaper on context. Don't usage-bill it. |
| 4 — Isolated runtime slots + leak-kill | **paywall (meter)** | Reframed from "RAM cap" — the loudest crashes are unbounded leaks a static cap can't stop; the felt pain is runtime isolation (ports/secrets/DB/disk). Still a concurrency metric (CI caps, Devin session caps); the governor makes it credible (safety, not greed). |
| 5 — Merge-conflict reduction | **paywall** | Folded under Safe parallel execution. Team headline. Raises the share of parallel-agent spend that converts to merged work. Solo scope in Pro, org governance in Team. |
| 1 — Built-in agent skills | **cut** | No demand evidence (2026-07-01 validation). Removed pending signal. |
| 2 — Spend/token-cost control | **table-stakes** | Basic visibility is free (every peer has it). The transparency *upgrade* — pre-run estimates + caps — moves to Pro. |

## Market patterns (why this shape)

- **Orchestration has commoditized to $0.** Conductor (free, YC land-grab), Vibe Kanban (OSS), TUICommander (free, same Tauri/Rust stack). Any paywall on the multiplexer gets undercut instantly.
- **Hosted-orchestration-as-paid-service is dying** — Terragon shut Feb 2026; Bloop/Vibe Kanban hosting wound down. Local-first is the survivable posture.
- **Hybrid two-part tariff won:** low subscription floor + metered/capacity ceiling. Pure per-seat is fading (IDC: 70% of vendors off it by 2028). Capacity, not features, is the upsell axis.
- **Metric legibility is where players bleed:** Cursor's "silent 20x hike" backlash (opaque token billing); Windsurf retreated to visible daily quotas. Predictable, visible capacity wins — which the safe-zone meter delivers by design.

## How the value is monetized today

Nobody sells "cognitive-load reduction" as a line item. The value gets monetized three
non-overlapping ways — and the tools closest to our loudest demand (visibility) don't
monetize it at all.

| Who | What they sell | Model | Is the cog-load value the meter? |
|-----|----------------|-------|----------------------------------|
| **Memory infra** (mem0, Zep, Letta) | Memory-as-a-service to *devs building agents* | Usage-metered freemium — mem0: free 10K mem/1K retrievals → $19/mo (50K) → **$249/mo Pro**; Zep Flex usage-based (<$200/mo early) | **Yes, directly** — and *correlation* (mem0 graph memory) is the top paywall at $249 (13× the $19 tier). Meters storage + retrieval calls. |
| **Coding assistants** (Cursor, Copilot) | A general IDE/agent seat | Flat seat + capacity ceiling — Cursor $0/$20/$60/$200, $40 Business; Copilot $10–39. Memory **bundled in all tiers** | **No** — memory is a *retention feature* inside the seat; **capacity** (usage pools) is the upsell axis. |
| **Local orchestrators / control planes** (Superset, Sculptor, Conductor) | Visibility + parallel-agent orchestration — *the loud demand* | **Free / OSS land-grab** — Superset Apache-2.0, free solo, $15–20/seat teams, no model markup; Sculptor free beta ($232M raised, no model yet); Conductor free ($22M Series A) | **No** — visibility value is **given away**; revenue deferred to team seats. No proven standalone paid model; some shut down (Bloop, Vibe Kanban). |

**mem0's $249 is a fence, not a cost.** The engine is open-source (Apache-2.0, public arXiv
paper); $249 buys hosting convenience + a wall around graph memory (the correlation feature).
It prices *running a service*, not *building the thing* — a cost structure a local, in-process,
rule-based layer does not carry.

**Three hard facts for our pricing:**

1. **The loud value (visibility) has no proven price.** Everyone closest to it gives it away,
   VC-funded, to grab land. Charging for *basic* visibility (R6) is greenfield and risky — free
   tier must include it.
2. **The one place correlation is directly paid is usage-metered** (mem0 $249), sold to *builders
   wiring their own agents*, not end-devs. We chose flat-for-retention over metering — forgoing the
   single proven direct-memory revenue mechanism in exchange for stickiness. **Resolved 2026-07-02:
   flat, locked (see below) — metering local memory fights zero-COGS reality, the privacy boundary,
   and the compounding moat.**
3. **Durable money is where all three converge: team seats + capacity ceiling.** $20 solo anchor →
   $40 team seat → ~$200 power ceiling. Cognitive-load is the *retention hook that justifies the
   seat*, never a standalone SKU.

**Implication (already reflected in Tiers above):** give visibility away (land-grab), charge for
correlation depth + replay + shared/team memory at Pro/Team, let capacity be the legible meter.

**Resolved 2026-07-02 (Fable) — flat memory, metered slots. Locked, not a live fork.** Never meter
local memory. Metering is only justified when the price fences a cost *the seller* bears; mem0 meters
because every item costs it hosting + embeddings + LLM-extraction on write. Optimus has **zero** of
that — deterministic Rust edges, no write-time inference, stored in the user's own SQLite on the user's
own disk. Metering it = rent on a resource the user already owns; prosumer devs see through it. It also
**contradicts the privacy boundary**: "your memory never leaves your machine" and "we bill by how much
memory you have" can't both hold — the second implies phone-home counting of the data we swore not to
touch. And it **suppresses the moat**: correlation memory compounds (graph value grows superlinearly
with density = the switching cost); a taxi-meter on accumulation makes users prune to stay under tiers,
so the graph stays shallow and churn stays cheap. Flat = memory accumulates by default → irreplaceable
graph in 18 months (Lightroom-catalog / Obsidian-vault dynamics). Slots are the honest meter: they map
to the value moment (more parallel work) and a real felt constraint (the safe-zone number already on
screen); memory is the *multiplier* that makes each slot worth more — free memory is the demand
generator for the slot meter, not a giveaway. Gating advanced memory *features* (graph queries,
cross-workspace linking) behind flat Pro stays fine — the ban is on metering *volume*, not on memory
selling Pro.

**Guiding principle (quotable):** *Optimus never meters data at rest on the user's machine; metering
applies only to resources Optimus itself provisions.* This makes the local-memory promise permanent
**and** leaves the door open for the one future where metering is legit: **hosted Team memory sync**
(shared correlation graph across 3–15 devs, where Optimus does host/replicate/serve → real per-seat /
per-GB COGS → meter *that*, or fold into the Team seat price). No metering of local memory now.

## Top risks

1. **Paywall proximity to the commoditized orchestration line.** If Pro reads as "pay to run parallel agents," TUICommander (free, same stack) undercuts instantly. Mitigation: orchestration unambiguously free forever; paywall is visibly only memory + capacity-unlock + governance.
2. **Capacity-cap resentment** ("rent-seeking on my own hardware"). Capping slots the machine could physically run feels like greed-gating (the Cursor-backlash emotion). Mitigation: never cap below what's demonstrably safe; frame the unlock as "use your full hardware safely"; lean the narrative on memory so capacity isn't the sole thing sold.
3. **Novel value metric / hard-to-convey ROI.** "Correlations surfaced across sessions" is abstract vs. the market's legible "cost per successful task," and correlation value compounds slowly → low perceived value at signup. Mitigation: keep the *billing* meter legible (visible slots) while memory is the reason-to-stay; show correlation wins concretely in-product early.
4. **Thin monetization surface.** Local-first + BYO means no token markup to fall back on. Mitigation: make Team/Enterprise governance the revenue engine; position Optimus as the multiplier on subs users already pay for.

_Sources: Conductor, Devin, Factory.ai, Cursor, Vibe Kanban, TUICommander, Terragon, Warp, Windsurf, Zed, Copilot, Anthropic/OpenAI plans, Replit, Jules, mem0, Letta, Zep — pricing pages + coverage, July 2026._
