# Design System — Optimus ("Graphite")

> Single source of truth for Optimus's **app chrome** — the WinUI 3 shell: sidebar,
> tab strips, pane borders, badges, dividers. The wgpu terminal grid (the engine's
> glyph/color path) is **out of scope** here except where noted; it carries its own
> ANSI palette. Read this before any visual or UI change to the chrome. Do not
> deviate without explicit approval.

## Product Context
- **What this is:** A native Windows terminal multiplexer that **right-sizes terminal
  capacity to your machine's RAM.** At startup Optimus measures available system
  memory, computes the maximum number of terminals it can host without exhausting it,
  and locks that as a hard "safe zone" cap — so spawning parallel agents can never
  crash the machine by over-allocating. Capacity-aware multiplexing is the core
  differentiator; everything else (the sidebar, splits, notifications) serves it.
- **Differentiator (do not bury):** RAM-bounded safe-zone spawning. No other
  multiplexer measures the host and refuses to exceed it; this is *why Optimus
  exists*. Surface it in onboarding, empty states, and the capacity indicator — not
  as a buried setting.
- **Who it's for:** Developers running parallel coding-agent sessions who need to
  see, at a glance, what every workspace is doing — and who have crashed a box by
  spawning too many terminals at once.
- **Space/industry:** Developer tooling / terminal emulators (peers: Warp, WezTerm,
  Ghostty, Windows Terminal, Conductor).
- **Project type:** Native desktop app — WinUI 3 (C#) chrome over a Rust/wgpu engine.

## Thesis
Most terminals minimize chrome because the grid is the product and chrome is
overhead. **Optimus is the inverse: the sidebar is mission control.** Chrome legibility
*is* the feature. So we invest in dense, scannable, semantically-colored chrome —
and keep the terminal grid itself pristine. Color and motion are never decorative;
they always carry state.

**The chrome must make the safe zone visible.** Optimus's reason to exist is the
RAM-bounded capacity cap (see Product Context). The design therefore owes a
first-class **capacity indicator** — current terminals vs. the computed safe-zone
max — that is always glanceable, not a buried number. As the count approaches the
cap the indicator escalates through the semantic palette (calm → `git-dirty` amber
near the limit → `pr-closed` red at the cap, where "New workspace" is disabled with
a plain-language reason). Capacity is the one number the user must never have to
hunt for.

> **Shipped (2026-06-10, plan U6).** `CapacityIndicatorView` sits in the sidebar
> directly above the "+ New workspace" button: a Cascadia Mono `meta` label
> ("X / Y terminals") over a thin fill bar (`hairline` track). Escalation maps to
> tokens in `app/Design/Tokens.cs`: `CapacityCalm` (= `text-muted` `#8A8A8A`,
> < 75%), `CapacityWarn` (= `git-dirty` `#D9A04E`, ≥ 75%), `CapacityCap`
> (= `pr-closed` `#D96A6A`, at cap — button disabled with hint "Safe-zone full —
> close a workspace to spawn more"). When the governor is unavailable the
> indicator shows "— / — terminals" in calm grey.

## Aesthetic Direction
- **Direction:** Industrial-utilitarian, near-black.
- **Decoration level:** Minimal — flat surfaces, 1px hairline dividers, elevation by
  value-step only. **No acrylic/blur** (deliberate departure from the Windows
  Terminal Fluent default): it costs GPU next to the wgpu surface, lowers text
  contrast, and fights the mission-control thesis.
- **Mood:** Quiet, dense, precise. The chrome recedes to near-black so a small set
  of desaturated semantic colors does all the signalling.
- **Reference research:** Warp (one accent + one consistent surface treatment),
  WezTerm (project/git-aware chrome tinting → our per-workspace identity accent),
  Ghostty (restraint), Windows Terminal (what we reject: acrylic).

## Typography
- **UI / titles:** Segoe UI Variable — native Windows, no webfont. Correct default.
- **Metadata (branch, PR#, status, progress, counts):** Cascadia Mono — ships with
  Windows, tabular figures. Aligns `#42`, `3/5`, branch names in the sidebar row.
- **Chrome affordance icons:** Segoe MDL2 Assets (`Tokens.IconFont`) — the Windows
  system icon font, for monochrome glyph buttons in the chrome (e.g. the tab-strip
  "web" globe, p6 U4). Use it over an emoji glyph so affordance icons render as crisp,
  on-theme monochrome marks that inherit `Foreground` (no fixed-color emoji). Ships
  with Windows 10+; no webfont. Decorative split/zoom glyphs that already render as
  plain Unicode (◨ ⬓ ⤢) stay as-is — `IconFont` is for the PUA icon glyphs.
- **Terminal grid:** owned by the engine (monospace); not specified here.
- **Scale (named; these are the sizes already in use):**
  | Token | px | Role |
  |-------|----|------|
  | `caption` | 10 | badge text, smallest meta |
  | `meta` | 11 | row metadata (branch · PR · status) |
  | `body` | 12 | tab chip labels |
  | `title` | 13 | sidebar row title (workspace name) |

## Color

All chrome color is a token. No raw `Color.FromArgb` literals in view code — see
the centralization plan in the Decisions Log.

### Surfaces (elevation ramp — value-step only, no blur)
| Token | Hex | Use |
|-------|-----|-----|
| `surface-0` | `#0C0C0C` | pane content background (deepest) |
| `surface-1` | `#121212` | sidebar panel |
| `surface-2` | `#161616` | tab strip |
| `surface-selected` | `#2D2D2D` | selected row / active tab chip |
| `hairline` | `#2B2B2B` | dividers, split-tree gutters |

### Text
| Token | Hex | Use |
|-------|-----|-----|
| `text-primary` | `#E6E6E6` | titles, active labels |
| `text-muted` | `#8A8A8A` | metadata, inactive labels |
| `text-on-accent` | `#FFFFFF` | text/glyphs on a colored badge |

### Semantic (state — keep; maps to dev conventions)
| Token | Hex | Meaning |
|-------|-----|---------|
| `git-branch` | `#7FB36E` | branch name (clean) |
| `git-dirty` | `#D9A04E` | dirty working tree marker |
| `pr-open` | `#4D9CF0` | PR open |
| `pr-merged` | `#A87FE0` | PR merged |
| `pr-closed` | `#D96A6A` | PR closed |

### Attention (unified — RISK #2)
One hue means "live / needs your eye," and "unread" gets its own dedicated color so
no signal is ambiguous. **Resolves the pre-existing overload** where `#4D9CF0` meant
unread *and* flash *and* PR-open simultaneously, and where focus (teal) and flash
(blue) were two different "attention" colors.
| Token | Hex | Use | Was |
|-------|-----|-----|-----|
| `attention` | `#2DD4BF` | focused-pane border **and** notification flash pulse | flash was `#4D9CF0` |
| `unread` | `#D86FB0` | unread notification dot / sidebar badge | was `#4D9CF0` (collided with `pr-open`) |

After this change, `#4D9CF0` means **only** `pr-open`. A teal cue always means
"active/live." A magenta dot always means "unread."

### Workspace identity accent (RISK #1 — WezTerm-inspired)
Each workspace gets a **stable** identity hue (hash workspace-id → index into the
curated set below), applied to its focused-pane border *tint* and a 2px spine on its
sidebar row. Makes N concurrent workspaces distinguishable at a glance. Tones are
kept desaturated so they never compete with the semantic signals above.

> **Identity = stable name + stable hue, one shared seed (Conductor-validated).**
> Conductor names each workspace after a city (`tokyo`, `warsaw-v2`) and treats the
> git *branch* as the primary label. Optimus today derives the row title from the
> *focused surface*, which shifts as you switch panes — volatile identity. Fix: a
> workspace carries a **stable secondary name** derived from the *same* workspace-id
> seed as its identity hue, so name and color move together ("the teal `tokyo`
> workspace"). Two redundant glance channels for the price of one seed. Row label
> hierarchy: branch (primary) → stable name + identity hue (secondary) → live title.
| # | Hex | # | Hex |
|---|-----|---|-----|
| 0 | `#5BA3C4` | 4 | `#B07FD9` |
| 1 | `#2DD4BF` | 5 | `#D97FA8` |
| 2 | `#6FC28C` | 6 | `#7F92D9` |
| 3 | `#C49A5B` | 7 | `#9FC25B` |

> Interaction with `attention`: a focused pane shows its **workspace identity** hue
> as a steady border; the **notification flash** is the shared `attention` teal pulse
> layered briefly over it. Identity = "which workspace," attention = "look here now."

### Dark mode
The app is dark-only by design (terminal context). No light theme. If one is ever
added, redesign surfaces from scratch and drop semantic-color saturation ~15%.

## Spacing
- **Base unit:** 4px. **Density:** compact (matches existing row metrics).
- **Scale:** `2xs`(2) `xs`(4) `sm`(8) `md`(12) `lg`(16) `xl`(24)

## Layout
- **Approach:** grid-disciplined.
- **Sidebar:** fixed-width rows, consistent gutters, predictable metadata columns
  (title line; then branch · PR · status·progress; then latest-notification line).
- **Anticipate repo-grouping (Conductor-validated).** Conductor shipped a flat list,
  then added "Group workspaces by repo" once users ran many workspaces across repos.
  Optimus's list is flat today; design the row so a **repo header row** (repo name,
  hairline-separated, collapsible) can sit above its workspace rows *without*
  reworking the row itself. Keep workspace rows indented one `md` (12px) under their
  repo header so the hierarchy is unambiguous when grouping turns on. Single-repo
  case: render no header (flat), so nothing is lost before grouping is needed.
- **Border radius:** minimal — `sm`(2px) on badges/chips only; everything else square.
  Industrial, not bubbly.

## Motion
- **Approach:** minimal-functional.
- **Focus:** instant. Focus is a *state*, not an event — no animation on the focused
  border appearing.
- **Notification flash:** the one expressive motion. A single ~200ms `attention`-teal
  pulse on the target pane border, ease-out, no loop.
- **Easing:** enter `ease-out` · exit `ease-in`. **Duration:** micro 80ms · short
  200ms (the flash) · medium 300ms. Nothing longer in chrome.

## Decisions Log
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-06-10 | Initial system "Graphite" created (chrome scope) | `/design-consultation`. Codifies the existing ad-hoc palette (scattered across `SidebarView`, `PaneTabStrip`, `PaneView`, `SplitTreeView`) into named tokens; grounded in Warp/WezTerm/Ghostty/Windows Terminal research. |
| 2026-06-10 | RISK #1: per-workspace identity accent | A multiplexer's job is glanceability across many workspaces (WezTerm git-tinting pattern). Fixed teal focus border didn't distinguish workspaces. |
| 2026-06-10 | RISK #2: unify attention, free the blue | `#4D9CF0` was overloaded (unread + flash + pr-open) and "attention" was split across teal focus / blue flash. Each color now means exactly one thing. |
| 2026-06-10 | Reject acrylic/blur | GPU cost next to the wgpu surface, lower text contrast, contradicts the chrome-is-mission-control thesis. Departs from Windows Terminal's Fluent default on purpose. |
| 2026-06-10 | Keep Segoe UI Variable for UI; Cascadia Mono for metadata | Native, no webfonts; mono gives tabular alignment for branch/PR/status in dense rows. |
| 2026-06-10 | Identity = stable name + stable hue (shared seed) | Conductor research: it uses city names + branch-primary labels; Optimus's focused-surface-derived title is volatile. Pairing a stable name with the RISK #1 hue gives two redundant glance channels off one seed. |
| 2026-06-10 | Sidebar row anticipates repo-grouping | Conductor shipped flat, then added group-by-repo at scale (0.35). Designing the row to accept a collapsible repo header now avoids a retrofit later. |
| 2026-06-10 | Capacity indicator shipped; `Tokens.cs` seeded | RAM safe-zone plan U6/U7. Indicator escalation reuses semantic hues via `CapacityCalm`/`CapacityWarn`/`CapacityCap` aliases in `app/Design/Tokens.cs` rather than inventing new colors; `Tokens.cs` becomes the (partial) central token registry. |
| 2026-06-12 | Older views migrated to `Tokens.cs`; RISK #2 applied; guard test added | res U3. `SidebarView`/`PaneTabStrip`/`PaneView`/`SplitTreeView` now consume named tokens (`Surface*`, `Text*`, `Git*`, `Pr*`, `Attention`, `Unread`, plus the existing `Font*` sizes). Pane flash → `Attention` teal; unread dot/badge → dedicated `Unread` magenta; `PrOpen` blue stops doubling as either. `tests/Design/TokensGuardTests.cs` fails the build on regression. |
| 2026-06-13 | Add `Tokens.IconFont` (Segoe MDL2 Assets) for the web-pane affordance | p6 U4. The tab-strip globe button (and its runtime-missing fallback panel) needed a browser glyph. Chose the Windows system icon font over an emoji so the icon stays monochrome and inherits `Foreground` like the other chrome buttons. New token keeps it out of view code as a raw family string. The inline "runtime required" panel reuses existing `Surface*`/`Text*`/`Font*` tokens — no new colors. |

## Implementation note (applied)
`app/Design/Tokens.cs` is the central token registry — the one place in the app
assembly where raw hex values may live. It was seeded by the capacity indicator
(plan U6) and extended in **res U3 (2026-06-12)** to cover the older views
(`SidebarView`, `PaneTabStrip`, `PaneView`, `SplitTreeView`). The token set now
carries: surfaces (`Surface0`, `Surface1`, `Surface2`, `SurfaceSelected`,
`Hairline`, `Transparent`); text (`TextPrimary`, `TextMuted`, `TextOnAccent`);
semantic (`GitBranch`, `GitDirty`, `PrOpen`, `PrMerged`, `PrClosed`); attention
(`Attention`, `Unread`); capacity aliases (`CapacityCalm`, `CapacityWarn`,
`CapacityCap`); the `Mono` (Cascadia Mono) family; and the
`caption`/`meta`/`body`/`title` font sizes.

**RISK #2 applied:** the pane flash now uses `Attention` (teal) and unread
badges/dots use the dedicated `Unread` (magenta), so `pr-open` blue no longer
doubles as "attention" or "unread". **RISK #1** (per-workspace identity hue
derivation, sidebar identity spine) remains a separately tracked follow-up.

A guard test (`tests/Design/TokensGuardTests.cs`) scans `app/**/*.cs` on every
test run and fails the build if any chrome view re-introduces raw
`Color.FromArgb` or inline numeric `FontSize` literals; `Tokens.cs` itself is
the only whitelisted file. New chrome code must consume `Tokens` and never
introduce fresh literals.
