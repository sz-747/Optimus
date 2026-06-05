---
date: 2026-06-06
topic: discoverable-pane-controls
---

# Discoverable pane controls

## Summary

Add always-visible icon buttons to each pane's tab strip — split-right,
split-down, and a zoom toggle — beside the existing new-tab `+`, each firing the
same action its keyboard chord already triggers and surfacing that chord on
hover. This is a discoverability layer over the Phase-2 split model: it makes the
signature tiling actions visible (and teaches their shortcuts) without changing
any terminal, controller, or engine behavior.

---

## Problem Frame

The Phase-2 workspace exposes splitting, zoom, and equalize only through
`Ctrl+Shift+_` chords with zero on-screen trace. New-tab and close-tab already
have buttons (`+` / `✕` in the strip), and focus is mouse-reachable via
click-to-focus — but **split, the headline tiling feature, is invisible**. A
user who hasn't read the docs has no way to learn it exists, let alone recall the
chord.

The friction we are targeting is **discovery, not recall**: the operations are
undiscoverable, so the product's defining capability goes unused. The fix is the
smallest visible affordance that advertises the feature where the eye already
goes — the tab strip — while staying within a terminal user's tolerance for
chrome.

---

## Key Decisions

- **Persistent icons, not a menu or palette.** A right-click menu, command
  palette, or shortcut overlay all gate the headline action behind a gesture the
  user must first know to make. Discovery requires splitting to be *impossible to
  miss*, so the affordance is always visible in the strip.

- **Tooltips carry the chord.** Each button's tooltip names the action and its
  keyboard shortcut (e.g. "Split right · Ctrl+Shift+D"). This is the mechanism
  that converts a pure-discovery affordance into one that *graduates* users to
  the keyboard over time — the real payoff, at near-zero cost.

- **Equalize is excluded from the strip because it is global.** The tab strip is
  per-pane, which fits split and zoom (they act on one pane). Equalize resets
  *every* divider in the workspace, so a per-pane button for it is a conceptual
  mismatch; keyboard-only (`Ctrl+Shift+0`) is clearer than a misplaced control.

- **View-only addition.** The buttons reuse the existing controller operations
  and the existing strip's visual style. The split-tree model, shortcut table,
  heal/focus rules, and engine lifecycle are untouched; the only model read is
  the controller's zoom state, used to render the zoom toggle's active
  appearance.

---

## Requirements

### Affordances

- R1. Each pane's tab strip shows persistent, always-visible icon buttons for
  split-right, split-down, and zoom, positioned beside the existing new-tab `+`.
- R2. Each button invokes the same action as its keyboard shortcut: split-right =
  side-by-side split (KTD7 `Orientation.Vertical`), split-down = stacked split
  (`Orientation.Horizontal`), zoom = toggle zoom. No new model behavior is
  introduced — the buttons are alternative entry points to existing operations.
- R3. Hovering a button shows a tooltip naming the action and its current
  keyboard chord (e.g. "Split right (Ctrl+Shift+D)"), sourced from the same
  shortcut definition the accelerators use so the two never drift apart.
- R6. The new buttons match the visual language of the existing `+` / `✕` strip
  controls (flat style, shared sizing and brushes) so the strip reads as one
  coherent control set rather than bolted-on chrome.

### Behavior

- R4. A button acts on the pane whose strip contains it. Clicking it focuses that
  pane first, then performs the action — consistent with the existing
  click-to-focus behavior — so clicking split on a pane that was not focused
  splits *that* pane.
- R5. The zoom button reflects state: it shows an active/toggled appearance while
  its pane is zoomed and returns to the default appearance when un-zoomed;
  clicking it toggles between the two.

---

## Acceptance Examples

- AE1. Parity with the keyboard.
  - **Covers R2.** Given a focused pane, When the user clicks its split-right
    button, Then the result is identical to pressing `Ctrl+Shift+D` in that pane:
    a side-by-side pair with a new shell surface in the new pane.

- AE2. Buttons act on their own pane.
  - **Covers R4.** Given panes A (focused) and B side-by-side, When the user
    clicks split-down on B's strip, Then B becomes focused and B splits into a
    stacked pair, and A is left untouched.

- AE3. Zoom toggles and reflects state.
  - **Covers R5.** Given a pane that is not zoomed, When the user clicks its zoom
    button, Then that pane fills the workspace and the button shows its active
    state; When the user clicks it again, Then the previous layout and divider
    positions are restored and the button returns to its default state.

---

## Scope Boundaries

### Deferred for later

- A visible affordance for **equalize** and an explicit **close-pane** button
  (close-pane stays implicit: closing a pane's last tab heals the tree).
- Alternative discovery surfaces considered and set aside this round: right-click
  context menu, command palette, first-run coachmark, and a shortcut cheat-sheet
  / help overlay.
- An overflow (`⋯`) menu in the strip for less-common actions.
- Drag-to-split and other drag affordances (already a Phase-2.5 deferral
  alongside tab/pane drag-and-drop reordering).

### Outside this phase's identity

- No changes to the keyboard chords themselves or to the split-tree model
  semantics — this is purely an additional, visible entry point.
- No engine / FFI / Rust work; this is a C#-only view-layer addition.

---

## Sources / Research

- `app/Splits/PaneTabStrip.cs` — the existing per-pane strip: `+` / `✕` chips,
  shared brushes, and the `MakeFlatButton` helper. The new icons live here and
  should reuse its style; it already raises `NewTabRequested` / `TabClosed` /
  `TabSelected` and is the natural place for new split/zoom events.
- `app/Splits/PaneView.cs` — wires strip events to the `SplitTreeController`
  (`SelectTab` / `CloseTab` / `NewTab`); the new split/zoom events route through
  here, and this is where per-pane focus-then-act (R4) is enforced.
- `core/Splits/Shortcuts.cs` — `ShortcutMap.Apply` maps SplitRight→Vertical,
  SplitDown→Horizontal, and ToggleZoom; `ShortcutMap.Defaults` is the
  authoritative chord source for the R3 tooltips (read it rather than hardcoding
  the chord text).
- `core/Splits/SplitTreeController.cs` — `Split(PaneId, Orientation)`,
  `ToggleZoom`, and the zoom state the toggle's active appearance (R5) reads.
- `app/Splits/WorkspaceView.cs` — focus-follows-derived-focus on snapshot change;
  relevant to confirming a button click lands focus on the acting pane (R4).
