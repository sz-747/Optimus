import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

const token = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

export class TerminalWorkspace {
  constructor(tauri) {
    this.tauri = tauri;
    this.entries = new Map();
  }

  async create() {
    const channel = new this.tauri.Channel();
    const entry = this.createEntry();
    channel.onmessage = (chunk) => entry.terminal.write(chunk);

    try {
      const session = await this.tauri.invoke("create_terminal", { onOutput: channel });
      entry.id = session.id;
      this.entries.set(session.id, entry);
      return session;
    } catch (error) {
      entry.terminal.dispose();
      throw error;
    }
  }

  createEntry() {
    const host = document.createElement("div");
    host.className = "xterm-host";
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim(),
      fontSize: 14,
      scrollback: 5000,
      theme: {
        background: token("--canvas"),
        foreground: token("--text"),
        cursor: token("--green"),
        selectionBackground: token("--selected"),
      },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host);
    const entry = { host, terminal, fit, id: null };
    terminal.onData((input) => {
      if (entry.id !== null) {
        this.tauri.invoke("send_terminal_input", { id: entry.id, input }).catch(() => {});
      }
    });
    return entry;
  }

  show(id, stage) {
    const entry = this.entries.get(id);
    if (!entry) return;
    for (const candidate of this.entries.values()) candidate.host.style.display = "none";
    entry.host.style.display = "block";
    stage.append(entry.host);
    requestAnimationFrame(() => {
      entry.fit.fit();
      entry.terminal.focus();
    });
  }

  async close(id) {
    const closed = await this.tauri.invoke("close_terminal", { id });
    if (!closed) throw new Error("terminal was already closed");
    const entry = this.entries.get(id);
    entry?.terminal.dispose();
    this.entries.delete(id);
  }
}
