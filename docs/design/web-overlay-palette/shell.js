/* Shared scaffold for the command-palette tournament.
   - One mock dataset so every variant shows identical content.
   - Injects the dimmed Optimus shell backdrop + scrim.
   - Small helpers (capacity color, fuzzy filter) so variants only differ in structure. */

window.OPTIMUS = (function () {
  const capacity = { live: 4, max: 8, warnAt: 6, capAt: 8 };

  // Live terminal counts sum to capacity.live (4). Dehydrated workspaces show 0 live.
  const workspaces = [
    { id: 0, name: "tokyo",     branch: "feat/safe-zone",        pr: "#11", prState: "open",   live: 2, ram: "1.2 GB", title: "claude · ce-work", hue: "--id-1" },
    { id: 3, name: "warsaw-v2", branch: "fix/res-u4-d3d12",      pr: "#10", prState: "merged", live: 1, ram: "0.4 GB", title: "cargo test",        hue: "--id-3" },
    { id: 4, name: "kyoto",     branch: "feat/p6-u1-renderer",   pr: "",    prState: "",       live: 1, ram: "0.9 GB", title: "vim engine/src",    hue: "--id-4" },
    { id: 2, name: "lagos",     branch: "chore/tokens",          pr: "#8",  prState: "merged", live: 0, ram: "—",      title: "dehydrated",        hue: "--id-2" },
    { id: 0, name: "oslo",      branch: "main",                  pr: "",    prState: "dirty",  live: 0, ram: "—",      title: "dehydrated",        hue: "--id-0" },
  ];

  // Command-mode actions. `cost` flags ones that consume a safe-zone slot.
  const commands = [
    { label: "New workspace",         hint: "Ctrl+Shift+N", cost: true,  group: "Create" },
    { label: "New web pane",          hint: "Ctrl+Shift+G", cost: true,  group: "Create" },
    { label: "Split right",           hint: "Ctrl+\\",      cost: true,  group: "Create" },
    { label: "Split down",            hint: "Ctrl+Shift+\\",cost: true,  group: "Create" },
    { label: "Dehydrate coldest",     hint: "Ctrl+Shift+D", cost: false, group: "Capacity", frees: true },
    { label: "Open capacity dashboard", hint: "Ctrl+K C",   cost: false, group: "Capacity" },
    { label: "Close pane",            hint: "Ctrl+W",       cost: false, group: "Pane" },
    { label: "Settings",              hint: "Ctrl+,",       cost: false, group: "App" },
  ];

  function capColor(live, c = capacity) {
    if (live >= c.capAt)  return getCss("--capacity-cap");
    if (live >= c.warnAt) return getCss("--capacity-warn");
    return getCss("--capacity-calm");
  }
  function getCss(v) { return getComputedStyle(document.documentElement).getPropertyValue(v).trim(); }

  // Subsequence ("fuzzy") match, case-insensitive. Returns score (lower = better) or -1.
  function fuzzy(query, text) {
    if (!query) return 0;
    query = query.toLowerCase(); text = text.toLowerCase();
    let qi = 0, score = 0, last = -1;
    for (let ti = 0; ti < text.length && qi < query.length; ti++) {
      if (text[ti] === query[qi]) { score += (last >= 0 ? ti - last : 0); last = ti; qi++; }
    }
    return qi === query.length ? score - (text.startsWith(query) ? 100 : 0) : -1;
  }

  function injectBackdrop() {
    const bd = document.createElement("div");
    bd.id = "backdrop";
    const pct = capacity.live / capacity.max;
    bd.innerHTML = `
      <aside class="bd-sidebar">
        <div class="bd-side-head">Workspaces · 1 repo</div>
        ${workspaces.map(w => `
          <div class="bd-row" style="border-left-color:${w.live ? `var(${w.hue})` : "transparent"}">
            <div class="t">${w.name}</div>
            <div class="m">${w.branch}${w.pr ? " · " + w.pr : ""} · ${w.live}/${capacity.max}</div>
          </div>`).join("")}
        <div class="bd-cap">
          <div class="lbl">${capacity.live} / ${capacity.max} terminals</div>
          <div class="track"><div class="fill" style="width:${pct*100}%;background:${capColor(capacity.live)}"></div></div>
        </div>
      </aside>
      <main class="bd-main">tokyo $ claude --resume<br>· running ce-work on BUILD-TRACKER<br>· p6 U4 webview2 pane · PR #11<br>&nbsp;</main>`;
    document.body.appendChild(bd);
    const scrim = document.createElement("div"); scrim.id = "scrim";
    document.body.appendChild(scrim);
  }

  return { capacity, workspaces, commands, capColor, getCss, fuzzy, injectBackdrop };
})();

document.addEventListener("DOMContentLoaded", () => window.OPTIMUS.injectBackdrop());
