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

  // WebGL renderer, DOM fallback on context loss (plan C8). ponytail: attached to every pane for
  // now; the pane-visibility manager (next P4 unit) detaches it from off-screen panes to stay under
  // WebView2's ~16 live-context cap.
  let webgl = null;
  try {
    webgl = new WebglAddon();
    webgl.onContextLoss(() => webgl.dispose());
    term.loadAddon(webgl);
  } catch (e) {
    console.warn("WebGL renderer unavailable, using DOM renderer:", e);
  }

  const entry = { el, term, fit, webgl, surfaceId: null };

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

export function disposeTerminal(entry) {
  entry.term.dispose(); // disposes loaded addons too
  entry.el.remove();
}
