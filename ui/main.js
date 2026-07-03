// P4.2: the split-tree chrome. Renders the backend TreeView as absolutely-positioned, keyed panes;
// drives structural ops (spawn/split/close/focus/zoom/tab/divider) through the Tauri command surface
// and re-renders from each call's returned TreeView. Terminals are keyed by surface id and moved on
// relayout, never recreated, so a split preserves scrollback + selection.
import { createTerminal, pushResize, disposeTerminal, attachWebgl, detachWebgl } from "./terminal.js";
import { initCapacity, refreshCapacity } from "./capacity.js";
import { initSidebar, renderSidebar } from "./sidebar.js";
import { initPalette, openPalette, isPaletteOpen } from "./palette.js";
import { initToasts, notify } from "./toasts.js";

const { invoke } = window.__TAURI__.core;
const app = document.getElementById("app");

// WebView2 hard-caps live WebGL contexts at ~16; stay comfortably under (plan §6 item 1). Visible
// panes claim a renderer up to this budget; the rest (and all background tabs) use the DOM renderer.
const MAX_RENDERERS = 12;

// surfaceId -> terminal entry (from terminal.js). The single source of live terminals — spans ALL
// workspaces (a terminal persists when you switch away, so switching back keeps its scrollback).
const terms = new Map();
// Divider handle elements, rebuilt each render (cheap DOM, no terminal state).
let dividerEls = [];

// ---- Rendering ---------------------------------------------------------------------------------

function placeEl(el, rect) {
  el.style.left = `${rect.x * 100}%`;
  el.style.top = `${rect.y * 100}%`;
  el.style.width = `${rect.width * 100}%`;
  el.style.height = `${rect.height * 100}%`;
}

// Render a TreeView: position each pane's selected terminal, hide off-screen tabs, dispose closed
// surfaces, and lay out draggable dividers. When a pane is zoomed it fills the viewport alone.
function render(view) {
  const zoomed = view.zoomed_pane;
  const panes = zoomed !== null ? view.panes.filter((p) => p.pane_id === zoomed) : view.panes;
  const full = { x: 0, y: 0, width: 1, height: 1 };

  const visible = new Set();
  let renderers = 0; // WebGL contexts claimed this pass, capped at MAX_RENDERERS
  for (const pane of panes) {
    const entry = terms.get(pane.selected);
    if (!entry) continue; // terminal not spawned yet (transient during a split round-trip)
    visible.add(pane.selected);
    if (!entry.el.isConnected) app.append(entry.el);
    entry.el.style.display = "";
    placeEl(entry.el, zoomed !== null ? full : pane.rect);
    entry.el.classList.toggle("focused", pane.focused);
    // On-screen panes claim a WebGL renderer up to the budget; the overflow uses DOM.
    if (renderers < MAX_RENDERERS) {
      attachWebgl(entry);
      renderers++;
    } else {
      detachWebgl(entry);
    }
    entry.fit.fit();
    pushResize(entry);
  }

  // Hide every terminal not shown this pass — background tabs AND other workspaces' terminals —
  // and release their renderer back to the budget. Disposal is NOT done here: a surface missing
  // from this workspace's tree may still be live in another workspace. Closes dispose explicitly.
  for (const [sid, entry] of terms) {
    if (!visible.has(sid)) {
      entry.el.style.display = "none";
      detachWebgl(entry);
    }
  }

  renderDividers(view, zoomed);

  const focusedPane = view.panes.find((p) => p.pane_id === view.focused_pane);
  if (focusedPane) terms.get(focusedPane.selected)?.term.focus();
}

// Dispose the terminals for surfaces the backend just closed (gone from the returned tree and not
// resurfaced elsewhere). `closed` is the surface ids the close op reported freeing.
function reap(closed) {
  for (const sid of closed) {
    const entry = terms.get(sid);
    if (entry) {
      disposeTerminal(entry);
      terms.delete(sid);
    }
  }
}

function renderDividers(view, zoomed) {
  for (const el of dividerEls) el.remove();
  dividerEls = [];
  if (zoomed !== null) return; // a zoomed pane hides its splitters

  for (const d of view.dividers) {
    const el = document.createElement("div");
    el.className = `divider ${d.orientation}`;
    if (d.orientation === "vertical") {
      const x = d.rect.x + d.rect.width * d.fraction;
      el.style.left = `${x * 100}%`;
      el.style.top = `${d.rect.y * 100}%`;
      el.style.height = `${d.rect.height * 100}%`;
    } else {
      const y = d.rect.y + d.rect.height * d.fraction;
      el.style.top = `${y * 100}%`;
      el.style.left = `${d.rect.x * 100}%`;
      el.style.width = `${d.rect.width * 100}%`;
    }
    attachDividerDrag(el, d);
    app.append(el);
    dividerEls.push(el);
  }
}

// ---- Divider drag ------------------------------------------------------------------------------

let dragBusy = false; // one in-flight set_divider at a time; skip intermediate moves to stay smooth

function attachDividerDrag(el, divider) {
  el.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    el.setPointerCapture(e.pointerId);

    const onMove = (ev) => {
      if (dragBusy) return;
      const bounds = app.getBoundingClientRect();
      const nx = (ev.clientX - bounds.left) / bounds.width;
      const ny = (ev.clientY - bounds.top) / bounds.height;
      const fraction =
        divider.orientation === "vertical"
          ? (nx - divider.rect.x) / divider.rect.width
          : (ny - divider.rect.y) / divider.rect.height;
      dragBusy = true;
      invoke("set_divider", { branch: divider.branch_id, fraction })
        .then(render)
        .catch((err) => console.error("set_divider failed:", err))
        .finally(() => {
          dragBusy = false;
        });
    };
    const onUp = (ev) => {
      el.releasePointerCapture(ev.pointerId);
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", onUp);
    };
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", onUp);
  });
}

// ---- Structural commands (each spawns/mutates, then re-renders from the returned TreeView) ------

async function spawnInto(command, args = {}) {
  const entry = createTerminal();
  try {
    const { surface_id, tree } = await invoke(command, {
      ...args,
      onOutput: entry.channels.onOutput,
      onEvent: entry.channels.onEvent,
    });
    entry.surfaceId = surface_id;
    terms.set(surface_id, entry);
    render(tree);
    refreshCapacity(); // a spawn consumed a safe-zone slot
  } catch (e) {
    disposeTerminal(entry); // refused at cap (or failed) — drop the orphan terminal
    console.error(`${command} failed:`, e);
    // A cap refusal is silent otherwise (the split rolls back, nothing appears) — the tiny AT-CAP
    // chip is easy to miss. Tell the user why. ponytail: one toast per refusal; hammering the key
    // stacks a few 5s cards, which is fine — they stop once they see it.
    const cap = await refreshCapacity(); // renders the now-full meter and hands back the view
    if (cap?.at_cap) notify("Safe-zone cap reached", cap.hint || cap.label);
  }
}

const doSplit = (direction) => spawnInto("split", { direction });
const doNewTab = () => spawnInto("new_tab");
const refreshFrom = (command, args) => invoke(command, args).then(render).catch(console.error);
const refreshSidebar = () => invoke("sidebar_state").then(renderSidebar).catch(console.error);
async function newWorkspace() {
  await spawnInto("new_workspace"); // seeds + backs a pane in the new workspace, then renders it
  refreshSidebar();
}
// Close frees a slot — re-render, dispose exactly the terminals the backend freed, refresh meters.
// If closing emptied the tree, the backend re-seeded a fresh pane (never contentless) — back it.
const doClose = () =>
  invoke("close_focused")
    .then(({ closed, reseeded, tree }) => {
      render(tree);
      reap(closed);
      refreshCapacity();
      if (reseeded) spawnInto("spawn");
    })
    .catch(console.error);

// ---- Chrome shortcuts (mirror the ported ShortcutMap defaults; all Ctrl(+Shift)) ---------------
// Reserved chords act on the chrome; everything else falls through to the focused xterm untouched.

function onKeydown(e) {
  if (!e.ctrlKey || isPaletteOpen()) return; // while the palette owns focus, chords are its own
  const shift = e.shiftKey;
  const k = e.key;

  const action = (() => {
    if (k === "Tab") return shift ? () => refreshFrom("select_previous_tab") : () => refreshFrom("select_next_tab");
    if (!shift) return null;
    switch (k.toLowerCase()) {
      case "arrowleft": return () => refreshFrom("move_focus", { direction: "left" });
      case "arrowright": return () => refreshFrom("move_focus", { direction: "right" });
      case "arrowup": return () => refreshFrom("move_focus", { direction: "up" });
      case "arrowdown": return () => refreshFrom("move_focus", { direction: "down" });
      case "d": return () => doSplit("right");
      case "e": return () => doSplit("down");
      case "t": return doNewTab;
      case "w": return doClose;
      case "z": return () => refreshFrom("toggle_zoom");
      case "p": return openPalette;
      case ")":
      case "0": return () => refreshFrom("equalize"); // Ctrl+Shift+0 (shifted '0' is ')')
      default: return null;
    }
  })();

  if (action) {
    e.preventDefault();
    e.stopPropagation();
    action();
  }
}

// ---- Boot --------------------------------------------------------------------------------------

document.addEventListener("keydown", onKeydown, true); // capture: intercept before xterm sees it
window.addEventListener("resize", () => invoke("tree_view").then(render).catch(console.error));

initSidebar({
  onSelect: (id) =>
    invoke("select_workspace", { id })
      .then(render)
      .then(refreshSidebar)
      .catch(console.error),
  onNew: newWorkspace,
  onClose: (id) =>
    invoke("close_workspace", { id })
      .then(({ closed, reseeded, tree }) => {
        render(tree);
        reap(closed);
        refreshCapacity();
        refreshSidebar();
        if (reseeded) spawnInto("spawn"); // closed the last workspace → back its replacement
      })
      .catch(console.error),
});

// The palette lists the same actions the chords fire (single command surface, two entry points).
initPalette([
  { label: "Split Right", hint: "Ctrl+Shift+D", run: () => doSplit("right") },
  { label: "Split Down", hint: "Ctrl+Shift+E", run: () => doSplit("down") },
  { label: "Split Left", run: () => doSplit("left") },
  { label: "Split Up", run: () => doSplit("up") },
  { label: "New Tab", hint: "Ctrl+Shift+T", run: doNewTab },
  { label: "Close Pane", hint: "Ctrl+Shift+W", run: doClose },
  { label: "Toggle Zoom", hint: "Ctrl+Shift+Z", run: () => refreshFrom("toggle_zoom") },
  { label: "Equalize Panes", hint: "Ctrl+Shift+0", run: () => refreshFrom("equalize") },
  { label: "Next Tab", hint: "Ctrl+Tab", run: () => refreshFrom("select_next_tab") },
  { label: "Previous Tab", hint: "Ctrl+Shift+Tab", run: () => refreshFrom("select_previous_tab") },
  { label: "Focus Left", run: () => refreshFrom("move_focus", { direction: "left" }) },
  { label: "Focus Right", run: () => refreshFrom("move_focus", { direction: "right" }) },
  { label: "Focus Up", run: () => refreshFrom("move_focus", { direction: "up" }) },
  { label: "Focus Down", run: () => refreshFrom("move_focus", { direction: "down" }) },
  { label: "New Workspace", run: newWorkspace },
]);

initCapacity();
initToasts();
refreshSidebar();
spawnInto("spawn").catch((e) => {
  const banner = document.createElement("pre");
  banner.textContent = `boot failed: ${e}`;
  app.append(banner);
  console.error(e);
});
