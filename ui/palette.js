// Command palette (Ctrl+Shift+P): a filtered overlay over the chrome's actions. Pure frontend —
// each command's `run` is the same handler the keyboard chords already call, so this adds discovery
// without a second command surface. Type to filter, ↑/↓ to move, Enter to run, Esc/click-out closes.
let overlay, input, listEl;
let commands = [];
let filtered = [];
let selected = 0;

export function initPalette(cmds) {
  commands = cmds;
  overlay = document.getElementById("palette");
  input = overlay.querySelector(".pal-input");
  listEl = overlay.querySelector(".pal-list");

  input.addEventListener("input", () => refresh(input.value));
  input.addEventListener("keydown", onInputKey);
  overlay.addEventListener("pointerdown", (e) => {
    if (e.target === overlay) close(); // click the backdrop, not the box
  });
}

export function isPaletteOpen() {
  return overlay.classList.contains("open");
}

export function openPalette() {
  overlay.classList.add("open");
  input.value = "";
  refresh("");
  input.focus();
}

function close() {
  overlay.classList.remove("open");
}

function refresh(query) {
  const q = query.trim().toLowerCase();
  filtered = q ? commands.filter((c) => c.label.toLowerCase().includes(q)) : commands.slice();
  selected = 0;
  paint();
}

function paint() {
  listEl.replaceChildren();
  filtered.forEach((cmd, i) => {
    const row = document.createElement("div");
    row.className = `pal-row${i === selected ? " active" : ""}`;
    const label = document.createElement("span");
    label.textContent = cmd.label;
    row.append(label);
    if (cmd.hint) {
      const hint = document.createElement("span");
      hint.className = "pal-hint";
      hint.textContent = cmd.hint;
      row.append(hint);
    }
    row.addEventListener("pointerdown", (e) => {
      e.preventDefault(); // keep focus off the row so the backdrop handler doesn't fight it
      pick(i);
    });
    listEl.append(row);
  });
}

function onInputKey(e) {
  switch (e.key) {
    case "Escape":
      e.preventDefault();
      close();
      break;
    case "ArrowDown":
      e.preventDefault();
      selected = Math.min(selected + 1, filtered.length - 1);
      paint();
      break;
    case "ArrowUp":
      e.preventDefault();
      selected = Math.max(selected - 1, 0);
      paint();
      break;
    case "Enter":
      e.preventDefault();
      pick(selected);
      break;
    default:
      e.stopPropagation(); // typing stays in the palette, not the terminal underneath
  }
}

function pick(i) {
  const cmd = filtered[i];
  close();
  cmd?.run();
}
