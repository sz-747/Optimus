// P4.1: a real xterm.js terminal backed by the engine surface. Replaces the P1 <pre> stub.
// Backend Channel -> term.write; term.onData -> dev_send_text; fit -> dev_resize.
// Split view, keyed pane reuse, and the pane-visibility manager layer on in later P4 units.
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

const { invoke, Channel } = window.__TAURI__.core;

// Theme from tokens.css (DESIGN.md graphite). Kept minimal — the full ANSI palette lands with
// the design pass; these are the surfaces/text/cursor the thesis chrome actually specifies.
const css = getComputedStyle(document.documentElement);
const token = (name) => css.getPropertyValue(name).trim();

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
term.open(document.getElementById("app"));

// WebGL renderer for perf; falls back to the DOM renderer if the context is unavailable or
// lost (plan C8 — no wgpu in-process; renderer crashes are Chromium's problem, not ours).
try {
  const webgl = new WebglAddon();
  webgl.onContextLoss(() => webgl.dispose()); // drop back to DOM renderer on GPU context loss
  term.loadAddon(webgl);
} catch (e) {
  console.warn("WebGL renderer unavailable, using DOM renderer:", e);
}

fit.fit();
term.focus();

let surfaceId = null;

// Push the current grid size to the backer, but only once a surface exists. cols/rows come
// from the fit addon's measurement of the actual viewport.
function pushResize() {
  if (surfaceId === null) return;
  invoke("dev_resize", { id: surfaceId, cols: term.cols, rows: term.rows }).catch((e) =>
    console.error("resize failed:", e),
  );
}

new ResizeObserver(() => {
  fit.fit();
  pushResize();
}).observe(document.getElementById("app"));

async function boot() {
  const onOutput = new Channel();
  onOutput.onmessage = (bytes) => term.write(new Uint8Array(bytes)); // xterm decodes UTF-8 itself

  const onEvent = new Channel();
  onEvent.onmessage = (msg) => console.log("engine event:", msg);

  surfaceId = await invoke("dev_spawn_shell", { onOutput, onEvent });
  pushResize(); // size the child PTY to the fitted grid now that it exists

  term.onData((data) => {
    invoke("dev_send_text", { id: surfaceId, text: data }).catch((e) =>
      console.error("send failed:", e),
    );
  });
}

boot().catch((e) => {
  term.write(`\r\n\x1b[31mboot failed: ${e}\x1b[0m\r\n`);
  console.error(e);
});
