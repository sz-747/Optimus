# Tournament Scoreboard — Optimus web-overlay **command palette**

> Artifact: the command palette for the "Tauri-like" web-overlay navigation layer
> (Option 1 hybrid — a WebView2 surface over the native WinUI 3 shell). Hero surface
> of a larger set (switcher / settings / capacity dashboard follow the winning language).
> Date: 2026-06-17. Mockups: `01..06` (field), `07-merge.html` (shipped). View: `index.html`.

## Confirmed criteria (Step 0 echo-loop, plain-language wording)
1. **Find-fast** *(weighted heaviest)* — open, type a couple letters, the target is ranked at the top with a clear selection, Enter, under a second, zero mouse.
2. **Capacity always visible** — the N/max safe-zone count + escalation color (calm→amber→red) is shown inside the palette itself; slot-consuming actions are flagged.
3. **Looks like Optimus** — near-black value-step surfaces, no decorative blur, Cascadia Mono for counts/branch/PR, color carries state (teal = live), restrained/industrial.
4. **Keyboard-complete + you always know where you are** — fully keyboard-operable, visible key hints, explicit mode.
5. **Realistic to build + stays fast** — static HTML/CSS + light JS over the native bridge, motion within the 80/200/300ms budget, no GPU-hungry effects next to the wgpu engine.

## Diversity matrix (structure × posture — declared before writing)
| Variant | Structure | Posture / bet |
|---|---|---|
| V1 Spotlight | single centered input + ranked column | pure speed, minimal disclosure |
| V2 Two-pane | results left + live workspace detail right | decide with full context |
| V3 Launcher grid | filterable tile grid | spatial memory over scanning |
| V4 Capacity-first | safe-zone slot meter is the frame | differentiator leads everything |
| V5 Drill-down | category → filtered sublist + breadcrumb | mode always explicit, wizard posture |
| V6 Terminal-line | shell prompt + tmux status line | maximal Graphite cohesion, power-user |

## Results — Round 1 (Borda, descending; n=6, top=5…0)
| Variant | Borda | Kills | EngLead | FirstUse | Support | CompPM | A11y | Status |
|---|---|---|---|---|---|---|---|---|
| **V1 Spotlight** | **22** | 0 | #1 | #1 | #2 | #3 | #1 | **WINNER → merge base** |
| **V4 Capacity-first** | **20** | 0 | #3 | #3 | #1 | #1 | #2 | survivor (steal) |
| **V6 Terminal-line** | **15** | 0 | #2 | #2 | #4 | #2 | #5 | survivor (steal) |
| V2 Two-pane | 7 | 1 | #5 | #4 | #6 | #4 | #4 | DEAD (bottom-half + 2-judge violation) |
| V5 Drill-down | 6 | 3 | #6 | #6 | #3 | #6 | #3 | DEAD (3 kills + violation) |
| V3 Launcher grid | 5 | 1 | #4 | #5 | #5 | #5 | #6 | DEAD (3-judge violation + bottom-half) |

## Judge verdicts — Round 1
- **Pragmatic eng lead** — top: V1, *"one ranked column, top row pre-selected, zero layout reflow on keystroke — cheapest build that nails find-fast (C1)."* Kill: V5, *"the level/Esc state machine hides the fast path behind a category hop."* Violations: V2 (C1,C2), V3 (C1), V5 (C1).
- **Impatient first-time user** — top: V1, *"one focused input that already says 'Jump to a workspace', pre-rendered ranked column, single teal selection bar."* Kill: V5, *"makes me pick a category and cross a mode boundary before I can type."* Violations: V5 (C1).
- **Support lead** — top: V4, *"the dashed-teal 'next slot' indicator + per-row +1/−1 tags + footer legend."* Kill: V2, *"a '⇥ actions' hint wired to nothing, and switch-only with zero slot-consuming actions flagged."* Violations: V2 (C2,C4), V3 (C2).
- **Competitor PM** — top: V4, *"making the RAM safe-zone slot meter THE frame is the one thing my Spotlight/Raycast/Warp palette cannot copy."* Kill: V5, *"inserts a category drill-down before the first keystroke can rank a workspace."* Violations: none.
- **Accessibility design critic** — top: V1, *"selection carried by BOTH a #2D2D2D surface lift and the teal left bar — unmistakable without relying on color."* Kill: V3, *"selected tile changes only background (no teal bar), and a 10px muted branch line is the only disambiguator."* Violations: V3 (C1,C4), V6 (C3).

## Merge — V7 (`07-merge.html`)
- **Base:** V1 Spotlight (Borda winner; strongest C1 + the selection state the a11y critic singled out).
- **Steal A — slim 8-slot meter + per-row `+1 / −1 slot` cost tags** *(from V4)* — cited by **Support lead** (*"per-row +1/−1 tags + footer legend"*) and **Competitor PM** (*"the one thing my palette cannot copy — the capacity model"*). Serves **C2**.
- **Steal B — persistent bottom status line (mode + `SAFE-ZONE` chip), Cascadia Mono** *(from V6)* — cited by **Eng lead** (*"persistent tmux status-line — mode + capacity never disappear, best Graphite cohesion"*); fixes the **A11y** critic's round-1 complaint that V1's capacity was *"an 11px pill."* Serves **C2 + C3**.
- **Fix carried in:** wired the `Enter` (commit) + `Escape` handlers — closing the field-wide *"⏎ hinted but no Enter handler"* gap the eng lead flagged across every raw variant.
- **Verification gate (P0):** re-judged V7 vs finalists {V1,V4,V6}, same panel, independent. **All 5 judges ranked V7 #1; merge Borda = 15/15 (swept) vs best raw V1 = 7 → SHIPPED MERGE.**

## Why it won
V7 keeps V1's load-bearing strength on the heaviest criterion — a single ranked column with a pre-selected top row and the dual-carrier selection state (surface lift **+** teal bar) that reads without relying on hue (**C1**, **C4**). Onto that it grafts the differentiator the two capacity-lens judges ranked V4 first for: a quiet 8-pip slot meter plus per-row `+1/−1 slot` tags, so the safe-zone cost is glanceable and the act of spending a slot is legible at the moment of commit (**C2**) — the one thing a generic Spotlight/Raycast/Warp palette structurally cannot copy. The borrowed status line keeps **mode + capacity** permanently on screen in Cascadia Mono, fixing V1's too-small capacity pill while staying inside the restrained near-black register (**C3**), unlike V6's solid-teal bar which strained it. It stays a static-render, light-JS overlay bridgeable via `WebMessageReceived` (**C5**).

## Residual work (judge-flagged regressions to resolve at implementation)
1. **Slot-cost tags only show in command mode (`>`).** In Switch mode the costly create-actions stay behind `>`, so the persistent meter mitigates but doesn't fully remove surprise-cap risk (Support lead, Competitor PM). *Fix:* surface a "near cap" affordance in Switch mode and/or fold create-actions inline as the count approaches the limit.
2. **`+1/−1` tag is 10px (`caption`)** — smallest type carrying decision-critical info (A11y; survives only because the literal text, not color, carries meaning). *Fix:* promote to `meta` (11px) or larger.
3. **Mock-fidelity gaps:** `commit()` overwrites the live result count toast without restoring it; the meter doesn't re-render on capacity change (Eng lead, Competitor PM). *Fix:* re-render the count on next input and repaint the meter on capacity events in the real build.

## Winning matrix cell (reusable knowledge)
**Single-column "Spotlight" structure + speed posture** won the heaviest criterion outright; the **capacity-meter / slot-cost** elements from the differentiator-forward cell are worth grafting onto *any* future overlay surface, but as a quiet strip — not as the frame (V4's full-size meter cost find-speed and lost on C1).
