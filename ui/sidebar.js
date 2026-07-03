// The workspace sidebar (DESIGN.md thesis chrome): one row per workspace with an identity stripe,
// git/PR/status projection, and an unread badge. Paints the backend `sidebar_state` projection —
// row click selects, the header "+" creates, the hover "×" closes. All copy uses design tokens.
const nav = document.getElementById("sidebar");

let list; // the row container, created in initSidebar
let handlers = { onSelect() {}, onNew() {}, onClose() {} };

export function initSidebar(h) {
  handlers = h;

  const head = document.createElement("div");
  head.className = "sb-head";
  const title = document.createElement("span");
  title.className = "sb-heading";
  title.textContent = "Workspaces";
  const add = document.createElement("button");
  add.className = "sb-new";
  add.textContent = "+";
  add.title = "New workspace";
  add.addEventListener("click", () => handlers.onNew());
  head.append(title, add);

  list = document.createElement("div");
  list.className = "sb-list";
  nav.append(head, list);
}

export function renderSidebar(rows) {
  list.replaceChildren();
  for (const r of rows) {
    const row = document.createElement("div");
    row.className = `sb-row${r.is_selected ? " selected" : ""}`;
    row.style.setProperty("--id", `var(--id-${((r.id % 5) + 5) % 5})`); // identity stripe, 5-cycle
    row.addEventListener("click", () => handlers.onSelect(r.id));

    const main = document.createElement("div");
    main.className = "sb-main";
    const name = document.createElement("div");
    name.className = "sb-name";
    name.textContent = r.title;
    main.append(name);

    const meta = document.createElement("div");
    meta.className = "sb-meta";
    if (r.git_branch) {
      const g = document.createElement("span");
      g.className = `sb-git${r.git_dirty ? " dirty" : ""}`;
      g.textContent = r.git_branch;
      meta.append(g);
    }
    if (r.pr_badge) {
      const p = document.createElement("span");
      p.className = `sb-pr ${(r.pr_status || "open").toLowerCase()}`;
      p.textContent = r.pr_badge;
      meta.append(p);
    }
    const statusText = r.progress || r.status;
    if (statusText) {
      const s = document.createElement("span");
      s.className = "sb-status";
      s.textContent = statusText;
      meta.append(s);
    }
    if (meta.childElementCount) main.append(meta);
    row.append(main);

    if (r.unread_count > 0) {
      const u = document.createElement("span");
      u.className = "sb-unread";
      u.textContent = r.unread_count;
      row.append(u);
    }

    const close = document.createElement("button");
    close.className = "sb-close";
    close.textContent = "×";
    close.title = "Close workspace";
    close.addEventListener("click", (e) => {
      e.stopPropagation(); // don't also select the row we're closing
      handlers.onClose(r.id);
    });
    row.append(close);

    list.append(row);
  }
}
