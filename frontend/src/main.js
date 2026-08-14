import "./style.css";
import { Channel, invoke } from "@tauri-apps/api/core";
import { addSession, createSessionState, removeSession, selectSession } from "./session-state.js";
import { TerminalWorkspace } from "./terminal-workspace.js";

const app = document.querySelector("#app");
const workspace = new TerminalWorkspace({ Channel, invoke });
let state = createSessionState();
let notice = "No terminals running";

function render() {
  const selected = state.sessions.find((session) => session.id === state.selectedId);
  app.innerHTML = `
    <header class="titlebar">
      <span class="brand-mark">◇</span><strong>Optimus</strong>
      <span class="product-label">Local terminal workspace</span>
      <span class="drag-region"></span>
      <button class="new-terminal" data-action="new">＋ New terminal <kbd>Ctrl Shift N</kbd></button>
    </header>
    <section class="workspace-shell">
      <aside class="sidebar">
        <div class="sidebar-heading"><span>TERMINALS</span><strong>${state.sessions.length}</strong></div>
        <p class="sidebar-copy">Each terminal is a local command-shell process owned by Optimus.</p>
        <div class="session-list">
          ${state.sessions.length === 0
            ? '<p class="empty-list">No open terminals</p>'
            : state.sessions.map((session) => `
              <button class="session-row ${session.id === state.selectedId ? "selected" : ""}" data-session="${session.id}">
                <span class="session-dot"></span><span>${session.title}</span>
                <small>${session.id === state.selectedId ? "Active" : ""}</small>
              </button>`).join("")}
        </div>
        <div class="sidebar-footer"><span>PROCESS-BACKED</span><small>Safe-capacity governor pending migration</small></div>
      </aside>
      <main class="terminal-area">
        ${selected
          ? `<div class="terminal-toolbar"><div><strong>${selected.title}</strong><span>cmd.exe</span></div><button class="close-terminal" data-action="close">Close terminal</button></div><div id="terminal-stage" class="terminal-stage"></div>`
          : `<section class="empty-state"><span class="empty-mark">◇</span><h1>Start with a clean workspace</h1><p>Open a terminal when you are ready. Nothing is running yet.</p><button class="primary-action" data-action="new">Open first terminal</button></section>`}
        <p class="notice" aria-live="polite">${notice}</p>
      </main>
    </section>`;

  for (const button of app.querySelectorAll("[data-session]")) {
    button.addEventListener("click", () => {
      state = selectSession(state, Number(button.dataset.session));
      notice = `Selected Terminal ${state.selectedId}`;
      render();
    });
  }
  for (const button of app.querySelectorAll('[data-action="new"]')) button.addEventListener("click", createTerminal);
  app.querySelector('[data-action="close"]')?.addEventListener("click", closeSelectedTerminal);

  if (selected) workspace.show(selected.id, app.querySelector("#terminal-stage"));
}

async function createTerminal() {
  notice = "Starting terminal…";
  render();
  try {
    const session = await workspace.create();
    state = addSession(state, session);
    notice = `${session.title} started`;
  } catch (error) {
    notice = `Could not start a terminal: ${error}`;
  }
  render();
}

async function closeSelectedTerminal() {
  if (state.selectedId === null) return;
  const id = state.selectedId;
  try {
    await workspace.close(id);
    state = removeSession(state, id);
    notice = `Terminal ${id} closed`;
  } catch (error) {
    notice = `Could not close Terminal ${id}: ${error}`;
  }
  render();
}

document.addEventListener("keydown", (event) => {
  if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === "n") {
    event.preventDefault();
    createTerminal();
  }
});

render();
