// One xterm.js terminal per surface. Created detached, then registered under the surface id the
// spawn/split/new-tab command returns. The element is keyed and reused across relayouts (moved,
// never recreated) so a split never drops scrollback — the crash/UX-critical rule (plan §6 item 2).
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

const { invoke, Channel } = window.__TAURI__.core;

const css = getComputedStyle(document.documentElement);
const token = (name) => css.getPropertyValue(name).trim();

// Build a terminal + its per-surface output/event channels. `entry.surfaceId` is filled in once the
// backend hands back the id; `onData` reads it lazily so keystrokes route to the right surface.
export function createTerminal() {
  const el = document.createElement("div");
  el.className = "pane";

  const term = new Terminal({
    fontFamily: token("--mono") || "Consolas, monospace",
    fontSize: 13,
    scrollback: 10000, // today's engine default (plan §6 item 1)
    cursorBlink: true,
    theme: {
      background: token("--surface-0"),
      foreground: token("--text-primary"),
      cursor: token("--attention"),
      selectionBackground: token("--surface-selected"),
    },
  });

  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(el);

  // WebGL is NOT attached here — the pane-visibility manager (attachWebgl/detachWebgl, driven by
  // render()) attaches it only to on-screen panes so we stay under WebView2's ~16 live-context cap.
  const entry = { el, term, fit, webgl: null, surfaceId: null };

  const onOutput = new Channel();
  onOutput.onmessage = (bytes) => term.write(new Uint8Array(bytes)); // xterm decodes UTF-8 itself
  const onEvent = new Channel();
  onEvent.onmessage = (msg) => console.log("engine event:", msg);
  entry.channels = { onOutput, onEvent };

  term.onData((data) => {
    if (entry.surfaceId === null) return;
    invoke("dev_send_text", { id: entry.surfaceId, text: data }).catch((e) =>
      console.error("send failed:", e),
    );
  });

  return entry;
}

// Size the child PTY to the pane's fitted grid.
export function pushResize(entry) {
  if (entry.surfaceId === null) return;
  invoke("dev_resize", {
    id: entry.surfaceId,
    cols: entry.term.cols,
    rows: entry.term.rows,
  }).catch((e) => console.error("resize failed:", e));
}

// Pane-visibility manager (plan §6 item 1): a WebGL renderer is a governed scarce resource —
// WebView2 hard-caps live contexts at ~16 and silently evicts the oldest, blanking panes. So the
// renderer follows the pane on/off screen; the xterm buffer (scrollback) is independent of the
// renderer, so detach/reattach never loses history.

// Attach the WebGL renderer to an on-screen pane (idempotent). Falls back to the DOM renderer if
// the context can't be created or is later lost.
export function attachWebgl(entry) {
  if (entry.webgl) return;
  try {
    const addon = new WebglAddon();
    addon.onContextLoss(() => {
      addon.dispose();
      entry.webgl = null;
    });
    entry.term.loadAddon(addon);
    entry.webgl = addon;
  } catch (e) {
    console.warn("WebGL attach failed, using DOM renderer:", e);
  }
}

// Release an off-screen pane's WebGL context back to the budget; xterm reverts to the DOM renderer
// and keeps its buffer. Idempotent.
export function detachWebgl(entry) {
  if (!entry.webgl) return;
  entry.webgl.dispose();
  entry.webgl = null;
}

export function disposeTerminal(entry) {
  entry.term.dispose(); // disposes loaded addons too
  entry.el.remove();
}
