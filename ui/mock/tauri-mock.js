// Browser mock of `window.__TAURI__.core` (invoke + Channel) so the chrome renders and reacts in a
// plain browser — no Tauri, no Rust, no PTY. Lets the UI be iterated on with real vision.
//
// It is a genuine stateful fake: a JS split-tree with the same even-split geometry the backend's
// compute_leaf_rects / collect_dividers produce, so split/close/focus/zoom/divider all reshape the
// layout for real. Terminals get local echo (dev_send_text bounces back) so panes are typeable.
// Only PTY output and cross-process integration are absent — everything visual is live.
//
// Loaded ONLY when window.__TAURI__ is missing (see the guard in index.html); the real app never
// runs this.

import { createControlPlaneFixture } from "../control-plane-fixture.js";

// ---- Channel ------------------------------------------------------------------------------------

class Channel {
  constructor() {
    this.onmessage = null;
  }
}
const push = (ch, payload) => ch && ch.onmessage && ch.onmessage(payload);
const bytes = (str) => Array.from(new TextEncoder().encode(str));

const BANNER = (sid) =>
  `\x1b[38;5;75mOptimus\x1b[0m mock terminal — surface S${sid}\r\n` +
  `\x1b[38;5;245mno PTY behind this pane; type to see local echo\x1b[0m\r\n` +
  `\x1b[38;5;114mPS\x1b[0m C:\\dev\\Optimus> `;

// ---- Tree state ---------------------------------------------------------------------------------

let root = null; // leaf | branch | null
let focusedPane = 0;
let zoomedPane = null;
let version = 0;
let nextSurface = 1;
let nextPane = 1;
let nextBranch = 1;
const surfaceChannels = new Map(); // surfaceId -> its onOutput Channel (for local echo)

const leaf = (surface) => ({ kind: "leaf", paneId: nextPane++, tabs: [surface], selected: surface });

function leaves(node, out = []) {
  if (!node) return out;
  if (node.kind === "leaf") out.push(node);
  else {
    leaves(node.first, out);
    leaves(node.second, out);
  }
  return out;
}
const findLeaf = (id) => leaves(root).find((l) => l.paneId === id) || null;
const focusedLeaf = () => findLeaf(focusedPane) || leaves(root)[0] || null;

// Replace the leaf `paneId` with `node` anywhere in the tree.
function replaceLeaf(paneId, node) {
  if (root && root.kind === "leaf" && root.paneId === paneId) {
    root = node;
    return;
  }
  const walk = (b) => {
    for (const side of ["first", "second"]) {
      const child = b[side];
      if (child.kind === "leaf" && child.paneId === paneId) {
        b[side] = node;
        return true;
      }
      if (child.kind === "branch" && walk(child)) return true;
    }
    return false;
  };
  if (root && root.kind === "branch") walk(root);
}

// Remove the leaf `paneId`; its sibling takes the whole region. Returns true if the tree emptied.
function removeLeaf(paneId) {
  if (root && root.kind === "leaf" && root.paneId === paneId) {
    root = null;
    return true;
  }
  const walk = (parent) => {
    for (const side of ["first", "second"]) {
      const child = parent[side];
      if (child.kind === "leaf" && child.paneId === paneId) {
        const sibling = parent[side === "first" ? "second" : "first"];
        // Collapse the branch into its sibling by copying the sibling's fields over the branch.
        Object.keys(parent).forEach((k) => delete parent[k]);
        Object.assign(parent, sibling);
        return true;
      }
      if (child.kind === "branch" && walk(child)) return true;
    }
    return false;
  };
  if (root && root.kind === "branch") {
    // Special-case: sibling collapse when the branch IS the root.
    if (root.first.kind === "leaf" && root.first.paneId === paneId) root = root.second;
    else if (root.second.kind === "leaf" && root.second.paneId === paneId) root = root.first;
    else walk(root);
  }
  return false;
}

// ---- Geometry (mirrors compute_leaf_rects / collect_dividers) -----------------------------------

function paneRects(node, rect, out = new Map()) {
  if (node.kind === "leaf") {
    out.set(node.paneId, rect);
    return out;
  }
  const [a, b] = splitRect(rect, node);
  paneRects(node.first, a, out);
  paneRects(node.second, b, out);
  return out;
}

function splitRect(rect, branch) {
  const f = Math.min(Math.max(branch.fraction, 0), 1);
  if (branch.orientation === "vertical") {
    const w1 = rect.width * f;
    return [
      { x: rect.x, y: rect.y, width: w1, height: rect.height },
      { x: rect.x + w1, y: rect.y, width: rect.width - w1, height: rect.height },
    ];
  }
  const h1 = rect.height * f;
  return [
    { x: rect.x, y: rect.y, width: rect.width, height: h1 },
    { x: rect.x, y: rect.y + h1, width: rect.width, height: rect.height - h1 },
  ];
}

function dividers(node, rect, out = []) {
  if (node.kind !== "branch") return out;
  out.push({ branch_id: node.branchId, orientation: node.orientation, fraction: node.fraction, rect });
  const [a, b] = splitRect(rect, node);
  dividers(node.first, a, out);
  dividers(node.second, b, out);
  return out;
}

function treeView() {
  const full = { x: 0, y: 0, width: 1, height: 1 };
  if (!root) return { panes: [], dividers: [], focused_pane: 0, zoomed_pane: null, version: ++version };
  const rects = paneRects(root, full);
  const panes = leaves(root).map((l) => ({
    pane_id: l.paneId,
    tabs: l.tabs.slice(),
    selected: l.selected,
    rect: rects.get(l.paneId),
    focused: l.paneId === focusedPane,
    zoomed: l.paneId === zoomedPane,
  }));
  return {
    panes,
    dividers: dividers(root, full),
    focused_pane: focusedPane,
    zoomed_pane: zoomedPane,
    version: ++version,
  };
}

// ---- Operations ---------------------------------------------------------------------------------

function ensureRoot() {
  if (!root) {
    root = leaf(nextSurface++);
    focusedPane = root.paneId;
  }
}

function spawnSurface(args) {
  const sid = focusedLeaf().selected;
  if (args && args.onOutput) {
    surfaceChannels.set(sid, args.onOutput);
    push(args.onOutput, bytes(BANNER(sid)));
  }
  return sid;
}

// right/left → vertical (side by side); down/up → horizontal (stacked). New pane is focused.
function split(direction, args) {
  ensureRoot();
  const target = focusedLeaf();
  const fresh = leaf(nextSurface++);
  const vertical = direction === "right" || direction === "left";
  const newFirst = direction === "left" || direction === "up";
  const branch = {
    kind: "branch",
    branchId: nextBranch++,
    orientation: vertical ? "vertical" : "horizontal",
    fraction: 0.5,
    first: newFirst ? fresh : target,
    second: newFirst ? target : fresh,
  };
  replaceLeaf(target.paneId, branch);
  focusedPane = fresh.paneId;
  const sid = fresh.selected;
  if (args && args.onOutput) {
    surfaceChannels.set(sid, args.onOutput);
    push(args.onOutput, bytes(BANNER(sid)));
  }
  toast("Pane split", direction, `surface S${sid} opened`);
  return sid;
}

function newTab(args) {
  ensureRoot();
  const l = focusedLeaf();
  const sid = nextSurface++;
  l.tabs.push(sid);
  l.selected = sid;
  if (args && args.onOutput) {
    surfaceChannels.set(sid, args.onOutput);
    push(args.onOutput, bytes(BANNER(sid)));
  }
  return sid;
}

function closeFocused() {
  const l = focusedLeaf();
  if (!l) return { closed: [], reseeded: false, tree: treeView() };
  if (l.tabs.length > 1) {
    const gone = l.selected;
    l.tabs = l.tabs.filter((t) => t !== gone);
    l.selected = l.tabs[l.tabs.length - 1];
    return { closed: [gone], reseeded: false, tree: treeView() };
  }
  const gone = l.selected;
  const emptied = removeLeaf(l.paneId);
  let reseeded = false;
  if (emptied || !root) {
    root = leaf(nextSurface++);
    focusedPane = root.paneId;
    reseeded = true;
  } else {
    focusedPane = leaves(root)[0].paneId;
  }
  return { closed: [gone], reseeded, tree: treeView() };
}

// ---- Sidebar + capacity + toasts ----------------------------------------------------------------

let sidebar = [
  { id: 1, title: "optimus", is_selected: true, git_branch: "feat/tauri-migration", git_dirty: true, pr_badge: "#12", pr_status: "open", cwd: null, status: "cargo test", progress: null, latest_text: null, unread_count: 3 },
  { id: 2, title: "web-app", is_selected: false, git_branch: "main", git_dirty: false, pr_badge: "#48", pr_status: "merged", cwd: null, status: "idle", progress: null, latest_text: null, unread_count: 0 },
  { id: 3, title: "infra", is_selected: false, git_branch: "fix/dns", git_dirty: false, pr_badge: null, pr_status: null, cwd: null, status: "deploying", progress: "3/5", latest_text: null, unread_count: 1 },
];
let nextWorkspace = 4;

function capacityView() {
  const used = leaves(root).length;
  const max = 8;
  const fraction = Math.min(used / max, 1);
  const level = fraction >= 1 ? "cap" : fraction >= 0.75 ? "warn" : "calm";
  return {
    used,
    reserved: 0,
    max,
    level,
    fraction,
    label: `${used} / ${max} terminals`,
    at_cap: level === "cap",
    hint: level === "cap" ? "At the RAM safe-zone cap — close a pane to open another." : null,
  };
}

let toastChannel = null;
function toast(title, subtitle, body) {
  push(toastChannel, [{ title, subtitle: subtitle || "", body: body || "", flash: false }]);
}

// ---- Control-plane state ------------------------------------------------------------------------

let controlSnapshot = createControlPlaneFixture();
let controlChannel = null;
let nextControlSession = 3;
let nextControlAgent = 7;

function emitControlUpdate() {
  controlSnapshot.revision += 1;
  controlSnapshot.generatedAt += 1_000;
  push(controlChannel, { revision: controlSnapshot.revision });
}

const controlMock = {
  snapshot: () => structuredClone(controlSnapshot),
  updateAgent(id, patch) {
    Object.assign(controlSnapshot.agents.find((agent) => agent.id === id), patch);
    recalculateControlTotals();
    emitControlUpdate();
  },
  appendActivity(item) {
    const id = Math.max(0, ...controlSnapshot.activity.map((entry) => entry.id)) + 1;
    controlSnapshot.activity.unshift({
      id,
      workspaceId: "ws-1",
      agentId: "agent-1",
      kind: "status",
      fact: "Live update",
      fileKey: null,
      branch: "feat/control-plane-foundation",
      trust: "trusted",
      timestamp: controlSnapshot.generatedAt + 1_000,
      hasProvenance: false,
      ...item,
    });
    emitControlUpdate();
    return id;
  },
  emitRevision: emitControlUpdate,
};

function recalculateControlTotals() {
  controlSnapshot.totals = {
    running: controlSnapshot.agents.filter((agent) => agent.state === "running").length,
    stalled: controlSnapshot.agents.filter((agent) => agent.state === "stalled").length,
    waiting: controlSnapshot.agents.filter((agent) => agent.state === "waiting").length,
    done: controlSnapshot.agents.filter((agent) => agent.state === "done").length,
    failed: controlSnapshot.agents.filter((agent) => ["failed", "interrupted"].includes(agent.state)).length,
  };
}

// ---- invoke -------------------------------------------------------------------------------------

async function invoke(cmd, args = {}) {
  switch (cmd) {
    case "spawn":
      ensureRoot();
      return { surface_id: spawnSurface(args), tree: treeView() };
    case "split":
      return { surface_id: split(args.direction, args), tree: treeView() };
    case "new_tab":
      return { surface_id: newTab(args), tree: treeView() };
    case "new_workspace": {
      const id = nextWorkspace++;
      sidebar.forEach((r) => (r.is_selected = false));
      sidebar.push({ id, title: `workspace-${id}`, is_selected: true, git_branch: "main", git_dirty: false, pr_badge: null, pr_status: null, cwd: null, status: "new", progress: null, latest_text: null, unread_count: 0 });
      root = leaf(nextSurface++); // fresh tree for the new workspace
      focusedPane = root.paneId;
      zoomedPane = null;
      return { surface_id: spawnSurface(args), tree: treeView() };
    }
    case "tree_view":
      return treeView();
    case "capacity_state":
      return capacityView();
    case "sidebar_state":
      return sidebar.map((r) => ({ ...r }));
    case "listen_notifications":
      toastChannel = args.onToast;
      setTimeout(() => toast("Agent finished", "optimus", "cargo test — 291 passed"), 1200);
      return;
    case "control_plane_snapshot":
      return structuredClone(controlSnapshot);
    case "listen_control_plane":
      controlChannel = args.onUpdate;
      push(controlChannel, { revision: controlSnapshot.revision });
      return controlSnapshot.revision;
    case "control_session_start": {
      const id = nextControlSession++;
      controlSnapshot.sessions.push({
        id: `session-${id}`,
        workspaceId: "ws-1",
        name: args.name,
        baseCommit: "c1fb05fc0ab8f3d982c57eef481c4c6fc0c74462",
        state: "active",
        createdAt: controlSnapshot.generatedAt,
      });
      emitControlUpdate();
      return { id };
    }
    case "control_agent_prepare": {
      const numericId = nextControlAgent++;
      const id = `agent-${numericId}`;
      controlSnapshot.agents.push({
        id,
        sessionId: `session-${args.sessionId}`,
        workspaceId: "ws-1",
        parentId: args.parentId ? `agent-${args.parentId}` : null,
        name: args.name,
        task: args.task,
        kind: args.kind,
        state: "waiting",
        status: "Preparing worktree",
        branch: args.branch,
        worktree: `C:\\dev\\Optimus\\.worktrees\\optimus\\session-${args.sessionId}\\${id}`,
        baseCommit: "c1fb05fc0ab8",
        baseDrift: false,
        startedAt: null,
        elapsedMs: 0,
        lastActivityAt: null,
        contradictionCount: 0,
      });
      recalculateControlTotals();
      emitControlUpdate();
      return { id: numericId };
    }
    case "control_agent_start": {
      const agent = controlSnapshot.agents.find((item) => item.id === `agent-${args.agentId}`);
      if (agent) {
        agent.state = "running";
        agent.status = "Agent started";
        agent.startedAt = controlSnapshot.generatedAt;
        agent.lastActivityAt = controlSnapshot.generatedAt;
      }
      recalculateControlTotals();
      emitControlUpdate();
      return { id: args.agentId };
    }
    case "control_agent_stop": {
      const agent = controlSnapshot.agents.find((item) => item.id === `agent-${args.agentId}`);
      if (agent) {
        agent.state = "done";
        agent.status = "Stopped";
      }
      recalculateControlTotals();
      emitControlUpdate();
      return { id: args.agentId };
    }
    case "select_workspace":
      sidebar.forEach((r) => (r.is_selected = r.id === args.id));
      return treeView();
    case "close_workspace": {
      const wasLast = sidebar.length === 1;
      sidebar = sidebar.filter((r) => r.id !== args.id);
      let reseeded = false;
      if (sidebar.length === 0) {
        sidebar.push({ id: nextWorkspace++, title: "workspace", is_selected: true, git_branch: "main", git_dirty: false, pr_badge: null, pr_status: null, cwd: null, status: null, progress: null, latest_text: null, unread_count: 0 });
        reseeded = true;
      } else if (!sidebar.some((r) => r.is_selected)) {
        sidebar[0].is_selected = true;
      }
      return { closed: [], reseeded: reseeded || wasLast, tree: treeView() };
    }
    case "close_focused":
      return closeFocused();
    case "focus_pane":
      focusedPane = args.pane;
      return treeView();
    case "move_focus": {
      const ls = leaves(root);
      const i = ls.findIndex((l) => l.paneId === focusedPane);
      const back = args.direction === "left" || args.direction === "up";
      focusedPane = ls[(i + (back ? ls.length - 1 : 1)) % ls.length].paneId;
      return treeView();
    }
    case "select_tab": {
      const l = focusedLeaf();
      if (l && l.tabs.includes(args.surface)) l.selected = args.surface;
      return treeView();
    }
    case "select_next_tab":
    case "select_previous_tab": {
      const l = focusedLeaf();
      if (l) {
        const i = l.tabs.indexOf(l.selected);
        const d = cmd === "select_next_tab" ? 1 : l.tabs.length - 1;
        l.selected = l.tabs[(i + d) % l.tabs.length];
      }
      return treeView();
    }
    case "toggle_zoom":
      zoomedPane = zoomedPane === focusedPane ? null : focusedPane;
      return treeView();
    case "set_divider": {
      const b = dividers(root, { x: 0, y: 0, width: 1, height: 1 }).find((d) => d.branch_id === args.branch);
      if (b) {
        const find = (n) => (n.kind === "branch" ? (n.branchId === args.branch ? n : find(n.first) || find(n.second)) : null);
        const node = find(root);
        if (node) node.fraction = Math.min(Math.max(args.fraction, 0.05), 0.95);
      }
      return treeView();
    }
    case "equalize": {
      const eq = (n) => {
        if (n.kind === "branch") {
          n.fraction = 0.5;
          eq(n.first);
          eq(n.second);
        }
      };
      if (root) eq(root);
      return treeView();
    }
    case "dev_send_text": {
      const ch = surfaceChannels.get(args.id);
      if (ch) push(ch, bytes(args.text === "\r" ? "\r\n\x1b[38;5;114mPS\x1b[0m C:\\dev\\Optimus> " : args.text)); // local echo
      return;
    }
    case "dev_resize":
      return;
    default:
      console.warn("[tauri-mock] unhandled command:", cmd, args);
      return;
  }
}

const core = { invoke, Channel };
// Browser: install the global the frontend reads. Node (self-check): skip the DOM, just export.
if (typeof window !== "undefined") {
  window.__TAURI__ = { core };
  window.__OPTIMUS_MOCK__ = controlMock;
  console.log("[tauri-mock] installed — chrome runs without Tauri");
}
export { controlMock, core, treeView };
