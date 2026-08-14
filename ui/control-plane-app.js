import {
  AlertTriangle,
  Bot,
  ChevronDown,
  ChevronRight,
  CircleStop,
  Command,
  FolderGit2,
  Menu,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Radio,
  Search,
  Settings2,
  X,
  createIcons,
} from "lucide";
import { createControlPlaneTransport } from "./control-plane-transport.js";
import { createControlPlaneFixture } from "./control-plane-fixture.js";

document.title = "Optimus Control Plane";
document.body.className = "cp-mode";
document.body.innerHTML = `
  <main class="cp-shell" data-testid="control-plane">
    <header class="cp-topbar">
      <div class="cp-brand"><strong>Optimus</strong><span>Control Plane</span></div>
      <div class="cp-scope" data-testid="active-scope">All workspaces</div>
      <div class="cp-top-actions">
        <button class="cp-icon-button cp-tree-toggle" type="button" aria-label="Open agents" title="Agents"><i data-lucide="menu"></i></button>
        <button class="cp-connection" type="button" data-testid="connection-state" data-state="stale" aria-label="Connection settings">
          <span class="cp-connection-dot"></span><span class="cp-connection-label">Connecting</span>
        </button>
        <button class="cp-icon-button cp-inspector-toggle" type="button" aria-label="Toggle inspector" title="Inspector"><i data-lucide="panel-right-open"></i></button>
        <button class="cp-icon-button cp-palette-toggle" type="button" aria-label="Open command palette" title="Commands"><i data-lucide="command"></i></button>
        <button class="cp-command primary cp-new-run" type="button"><i data-lucide="plus"></i><span>New run</span></button>
      </div>
    </header>
    <nav class="cp-mobile-tabs" aria-label="Control plane views">
      <button class="cp-mobile-tab" type="button" data-view="agents">Agents</button>
      <button class="cp-mobile-tab active" type="button" data-view="activity">Activity</button>
    </nav>
    <section class="cp-main">
      <aside class="cp-panel cp-tree-panel" aria-label="Agent tree">
        <div class="cp-panel-head"><span class="cp-panel-title">Agents</span><button class="cp-icon-button cp-new-run" type="button" aria-label="New run" title="New run"><i data-lucide="plus"></i></button></div>
        <div class="cp-tree" role="tree" data-testid="workspace-tree"></div>
        <div class="cp-tree-capacity" data-testid="capacity-meter" data-level="calm"></div>
      </aside>
      <section class="cp-panel cp-feed-panel mobile-active" aria-label="Activity">
        <div class="cp-summary" data-testid="status-summary"></div>
        <div class="cp-toolbar">
          <label class="cp-search"><i data-lucide="search"></i><input data-testid="activity-search" type="search" placeholder="Filter activity" aria-label="Filter activity" /></label>
          <div class="cp-segments" role="tablist" aria-label="Activity kind"></div>
        </div>
        <div class="cp-feed-wrap">
          <button class="cp-new-activity" data-testid="live-follow" type="button"></button>
          <div class="cp-activity" role="feed" data-testid="activity-stream" tabindex="0"></div>
        </div>
      </section>
      <aside class="cp-panel cp-inspector" data-testid="record-inspector" aria-label="Inspector">
        <div class="cp-panel-head"><span class="cp-panel-title">Inspector</span><button class="cp-icon-button cp-inspector-close" type="button" aria-label="Close inspector" title="Close"><i data-lucide="x"></i></button></div>
        <div class="cp-inspector-body"></div>
      </aside>
    </section>
    <footer class="cp-statusbar">
      <div class="cp-statusbar-left"><span data-testid="transport-mode">LOCAL</span><span class="cp-status-message">Waiting for snapshot</span></div>
      <div class="cp-statusbar-right"><span class="cp-revision">REV 0</span><span class="cp-safe-zone">SAFE ZONE</span></div>
    </footer>
    <button class="cp-drawer-backdrop" type="button" aria-label="Close panels"></button>
  </main>
  <dialog class="cp-dialog cp-run-dialog" aria-labelledby="cp-run-title">
    <div class="cp-dialog-head"><h2 id="cp-run-title">Start agent run</h2><button class="cp-icon-button cp-dialog-close" type="button" aria-label="Close" title="Close"><i data-lucide="x"></i></button></div>
    <form class="cp-form" method="dialog">
      <div class="cp-field full cp-local-only"><label for="cp-repo">Repository path</label><input id="cp-repo" name="repoRoot" value="C:\\dev\\Optimus" /></div>
      <div class="cp-field cp-remote-only"><label for="cp-workspace">Workspace</label><select id="cp-workspace" name="workspaceId"></select></div>
      <div class="cp-field cp-remote-only"><label for="cp-profile">Model profile</label><select id="cp-profile" name="profileId"></select></div>
      <div class="cp-field"><label for="cp-session">Run name</label><input id="cp-session" name="sessionName" required value="Product dashboard" /></div>
      <div class="cp-field"><label for="cp-agent">Agent name</label><input id="cp-agent" name="agentName" required value="Web surface" /></div>
      <div class="cp-field full"><label for="cp-task">Task</label><textarea id="cp-task" name="task" required>Build and verify the Optimus control-plane web surface.</textarea></div>
      <div class="cp-field cp-local-only"><label for="cp-command">Agent command</label><input id="cp-command" name="command" value="codex" /></div>
      <div class="cp-field cp-local-only"><label for="cp-branch">Branch</label><input id="cp-branch" name="branch" value="feat/control-plane-web" /></div>
      <div class="cp-field"><label for="cp-kind">Agent type</label><select id="cp-kind" name="kind"><option value="writer">Writer</option><option value="recon">Read-only recon</option></select></div>
      <div class="cp-field"><label for="cp-scope">File scope</label><input id="cp-scope" name="fileScope" value="ui" /></div>
      <div class="cp-field full cp-remote-only cp-parallel-field">
        <div class="cp-parallel-head"><span>Additional agents</span><button class="cp-icon-button cp-add-agent" type="button" aria-label="Add parallel agent" title="Add agent"><i data-lucide="plus"></i></button></div>
        <div class="cp-parallel-agents"></div>
      </div>
      <div class="cp-dialog-actions"><button class="cp-text-button cp-dialog-cancel" type="button">Cancel</button><button class="cp-command primary" type="submit"><i data-lucide="radio"></i>Start</button></div>
    </form>
  </dialog>
  <dialog class="cp-dialog cp-auth-dialog" aria-labelledby="cp-auth-title">
    <div class="cp-dialog-head"><h2 id="cp-auth-title">Dashboard connection</h2><button class="cp-icon-button cp-auth-close" type="button" aria-label="Close" title="Close"><i data-lucide="x"></i></button></div>
    <form class="cp-form" method="dialog">
      <input name="username" type="text" value="optimus" autocomplete="username" hidden />
      <div class="cp-field full"><label for="cp-token">Access token</label><input id="cp-token" name="token" type="password" autocomplete="current-password" /></div>
      <div class="cp-dialog-actions"><button class="cp-text-button cp-auth-cancel" type="button">Cancel</button><button class="cp-command primary" type="submit"><i data-lucide="settings-2"></i>Connect</button></div>
    </form>
  </dialog>
  <div class="cp-palette" role="dialog" aria-modal="true" aria-label="Command palette">
    <div class="cp-palette-box"><input type="search" placeholder="Type a command" aria-label="Command search" /><div class="cp-palette-list"></div><div class="cp-palette-capacity"></div></div>
  </div>
`;

const icons = { AlertTriangle, Bot, ChevronDown, ChevronRight, CircleStop, Command, FolderGit2, Menu, PanelRightClose, PanelRightOpen, Plus, Radio, Search, Settings2, X };
const q = (selector, root = document) => root.querySelector(selector);
const qa = (selector, root = document) => [...root.querySelectorAll(selector)];
const params = new URLSearchParams(location.search);
const demoMode = import.meta.env.DEV && params.get("demo") === "1";
const state = {
  snapshot: emptySnapshot(),
  connection: demoMode ? "connected" : "stale",
  workspaceId: params.get("workspace") || "all",
  agentId: params.get("agent") || "all",
  kind: params.get("kind") || "all",
  search: params.get("q") || "",
  selectedRecordId: null,
  selectedAgentId: params.get("agent") || null,
  collapsed: new Set(),
  following: true,
  pendingActivity: null,
  newCount: 0,
  mobileView: "activity",
  inspectorOpen: false,
  commandMessage: null,
  commandMessageUntil: 0,
};

let refreshTimer = null;
let commandMessageTimer = null;
let connectionGeneration = 0;
let snapshotRequestSequence = 0;
let transport;
const onConnection = (connection) => {
  connectionGeneration += 1;
  state.connection = connection;
  renderConnection();
};
const onRevision = (revision) => {
  if (revision <= state.snapshot.revision) return;
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(refreshSnapshot, 120);
};
const onCommand = (command) => {
  const run = command.type === "start_run";
  const message = {
    queued: run ? "Run queued" : "Stop queued",
    leased: run ? "Run dispatching" : "Stop dispatching",
    running: run ? "Run dispatching" : "Stop dispatching",
    succeeded: run ? "Run started" : "Agent stopped",
    failed: `${run ? "Run" : "Stop"} failed: ${command.errorCode || "unknown error"}`,
    rejected: `${run ? "Run" : "Stop"} rejected: ${command.errorCode || "rejected"}`,
    expired: `${run ? "Run" : "Stop"} expired before desktop dispatch`,
  }[command.status];
  if (message) {
    state.commandMessage = message;
    state.commandMessageUntil = Date.now() + (terminalCommand(command.status) ? 8_000 : 60_000);
    clearTimeout(commandMessageTimer);
    commandMessageTimer = setTimeout(() => {
      state.commandMessage = null;
      state.commandMessageUntil = 0;
      renderStatus();
    }, state.commandMessageUntil - Date.now());
    renderStatus();
  }
  if (command.status === "succeeded") {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(refreshSnapshot, 120);
  }
};
transport = demoMode
  ? demoTransport(onConnection, onRevision)
  : createControlPlaneTransport({ onConnection, onRevision, onCommand });
document.body.dataset.transport = transport.mode;
configureRunForm();

function emptySnapshot() {
  return {
    revision: 0,
    generatedAt: Date.now(),
    workspaces: [],
    sessions: [],
    agents: [],
    activity: [],
    contradictions: [],
    capacity: { used: 0, reserved: 0, max: 0, level: "calm", fraction: 0, atCap: false },
    totals: { running: 0, stalled: 0, waiting: 0, done: 0, failed: 0 },
    catalog: { workspaces: [], profiles: [] },
  };
}

function configureRunForm() {
  const remote = transport.mode === "web";
  qa(".cp-local-only input, .cp-local-only select, .cp-local-only textarea").forEach((control) => { control.required = !remote; });
  qa(".cp-remote-only input, .cp-remote-only select, .cp-remote-only textarea").forEach((control) => { control.required = remote; });
}

let parallelAgentSequence = 1;

function addParallelAgent() {
  parallelAgentSequence += 1;
  const group = node("div", "cp-parallel-agent");
  group.setAttribute("role", "group");
  group.setAttribute("aria-label", `Agent ${parallelAgentSequence}`);
  const name = document.createElement("input");
  name.required = true;
  name.value = `Agent ${parallelAgentSequence}`;
  name.setAttribute("aria-label", `Agent ${parallelAgentSequence} name`);
  name.dataset.field = "agentName";
  const kind = document.createElement("select");
  kind.setAttribute("aria-label", `Agent ${parallelAgentSequence} type`);
  kind.dataset.field = "kind";
  kind.append(new Option("Writer", "writer"), new Option("Read-only recon", "recon"));
  const remove = node("button", "cp-icon-button cp-remove-agent");
  remove.type = "button";
  remove.setAttribute("aria-label", `Remove agent ${parallelAgentSequence}`);
  remove.title = "Remove agent";
  remove.append(node("i", ""));
  remove.firstChild.dataset.lucide = "x";
  remove.addEventListener("click", () => group.remove());
  const task = document.createElement("textarea");
  task.required = true;
  task.setAttribute("aria-label", `Agent ${parallelAgentSequence} task`);
  task.dataset.field = "task";
  const scope = document.createElement("input");
  scope.required = true;
  scope.value = `agent-${parallelAgentSequence}`;
  scope.setAttribute("aria-label", `Agent ${parallelAgentSequence} file scope`);
  scope.dataset.field = "fileScope";
  group.append(name, kind, remove, task, scope);
  q(".cp-parallel-agents").append(group);
  refreshIcons();
}

function formAgentSpecifications(form, data) {
  const splitScopes = (value) => String(value || "").split(",").map((entry) => entry.trim()).filter(Boolean);
  const agents = [{
    agentName: String(data.get("agentName") || "").trim(),
    task: String(data.get("task") || "").trim(),
    kind: String(data.get("kind") || "writer"),
    fileScope: splitScopes(data.get("fileScope")),
  }];
  qa(".cp-parallel-agent", form).forEach((group) => {
    agents.push({
      agentName: q("[data-field='agentName']", group).value.trim(),
      task: q("[data-field='task']", group).value.trim(),
      kind: q("[data-field='kind']", group).value,
      fileScope: splitScopes(q("[data-field='fileScope']", group).value),
    });
  });
  return agents;
}

function renderRunCatalog() {
  if (transport.mode !== "web") return;
  const catalog = state.snapshot.catalog || { workspaces: [], profiles: [] };
  replaceOptions(q("#cp-workspace"), catalog.workspaces, "No registered workspaces");
  replaceOptions(q("#cp-profile"), catalog.profiles, "No registered profiles");
}

function replaceOptions(select, entries, emptyLabel) {
  const selected = select.value;
  select.replaceChildren();
  if (!entries.length) {
    const option = node("option", "", emptyLabel);
    option.value = "";
    select.append(option);
    return;
  }
  for (const entry of entries) {
    const option = node("option", "", entry.label);
    option.value = entry.id;
    select.append(option);
  }
  if (entries.some((entry) => entry.id === selected)) select.value = selected;
}

function demoTransport(onConnectionChange, revisionListener) {
  let snapshot = createControlPlaneFixture();
  return {
    mode: "demo",
    async snapshot() { onConnectionChange("connected"); return structuredClone(snapshot); },
    async subscribe() {},
    async startRun(payload) {
      const id = `agent-${snapshot.agents.length + 1}`;
      snapshot.agents.push({ ...snapshot.agents[0], id, parentId: "agent-1", name: payload.agentName, task: payload.task, status: "Starting", branch: payload.branch, state: "running", contradictionCount: 0 });
      snapshot.revision += 1;
      revisionListener(snapshot.revision);
      return { id };
    },
    async stopAgent(agentId) { snapshot.agents.find((agent) => agent.id === agentId).state = "done"; snapshot.revision += 1; revisionListener(snapshot.revision); },
    authenticated() { return true; },
    async authenticate() {},
  };
}

function node(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}

function icon(name) {
  const element = document.createElement("i");
  element.dataset.lucide = name;
  return element;
}

function refreshIcons() {
  createIcons({ icons });
}

async function refreshSnapshot() {
  const connectionAtStart = connectionGeneration;
  const requestSequence = ++snapshotRequestSequence;
  try {
    const next = await transport.snapshot();
    if (Number(next.revision) < Number(state.snapshot.revision)) return;
    if (
      transport.mode === "web"
      && requestSequence === snapshotRequestSequence
      && connectionGeneration === connectionAtStart
    ) {
      onConnection(next.desktop?.online === false ? "offline" : "connected");
    }
    const feed = q(".cp-activity");
    const oldIds = new Set(state.snapshot.activity.map((item) => item.id));
    const added = next.activity.filter((item) => !oldIds.has(item.id)).length;
    state.snapshot = next;
    renderRunCatalog();
    if (!state.following && added) {
      state.pendingActivity = next.activity;
      state.newCount += added;
      renderAll(false);
      renderLiveFollow();
    } else {
      state.pendingActivity = null;
      state.newCount = 0;
      renderAll(true);
      if (state.following) feed.scrollTop = 0;
    }
  } catch (error) {
    if (error.status !== 401) console.warn("control-plane snapshot failed:", error.message || error);
    if (requestSequence === snapshotRequestSequence && connectionGeneration === connectionAtStart) {
      onConnection("offline");
      q(".cp-status-message").textContent = error.message || String(error);
    }
    if (transport.mode === "web" && !transport.authenticated() && !q(".cp-auth-dialog").open) q(".cp-auth-dialog").showModal();
  }
}

function renderAll(renderFeed = true) {
  renderConnection();
  renderScope();
  renderTree();
  renderCapacity();
  renderSummary();
  renderSegments();
  if (renderFeed) renderActivity();
  renderInspector();
  renderStatus();
  renderResponsive();
  refreshIcons();
}

function renderConnection() {
  const button = q(".cp-connection");
  button.dataset.state = state.connection;
  q(".cp-connection-label", button).textContent = label(state.connection);
  const unauthenticated = transport?.mode === "web" && !transport.authenticated();
  qa(".cp-new-run").forEach((control) => { control.disabled = unauthenticated; });
}

function renderScope() {
  const workspace = state.snapshot.workspaces.find((item) => item.id === state.workspaceId);
  const agent = state.snapshot.agents.find((item) => item.id === state.agentId);
  q("[data-testid='active-scope']").textContent = agent?.branch || workspace?.name || "All workspaces";
  syncUrl();
}

function renderTree() {
  const tree = q(".cp-tree");
  tree.replaceChildren();
  if (!state.snapshot.workspaces.length) {
    tree.append(node("div", "cp-empty", "No active workspaces"));
    return;
  }
  for (const [index, workspace] of state.snapshot.workspaces.entries()) {
    const section = node("section", `cp-workspace identity-${index % 8}`);
    section.dataset.workspaceId = workspace.id;
    const head = node("div", `cp-workspace-head${state.workspaceId === workspace.id ? " selected" : ""}`);
    const disclosure = node("button", "cp-workspace-disclosure");
    disclosure.type = "button";
    disclosure.setAttribute("aria-label", `${state.collapsed.has(workspace.id) ? "Expand" : "Collapse"} ${workspace.name}`);
    disclosure.setAttribute("aria-expanded", String(!state.collapsed.has(workspace.id)));
    disclosure.append(icon(state.collapsed.has(workspace.id) ? "chevron-right" : "chevron-down"));
    disclosure.addEventListener("click", () => toggleWorkspace(workspace.id));
    const selection = node("button", "cp-workspace-select");
    selection.type = "button";
    selection.setAttribute("role", "treeitem");
    selection.append(node("span", "cp-workspace-name", workspace.name));
    selection.append(node("span", "cp-workspace-count", `${workspace.activeAgents}/${agentsForWorkspace(workspace.id).length}`));
    selection.addEventListener("click", () => selectWorkspace(workspace.id));
    head.append(disclosure, selection);
    section.append(head);
    if (!state.collapsed.has(workspace.id)) {
      const list = node("div", "cp-agent-list");
      const agents = agentsForWorkspace(workspace.id);
      for (const agent of agents) list.append(agentTreeRow(agent, depth(agent, agents)));
      section.append(list);
    }
    tree.append(section);
  }
}

function agentTreeRow(agent, treeDepth) {
  const row = node("button", `cp-tree-row depth-${treeDepth}${state.agentId === agent.id ? " selected" : ""}`);
  row.type = "button";
  row.setAttribute("role", "treeitem");
  row.dataset.agentId = agent.id;
  row.dataset.state = agent.state;
  row.title = `${agent.branch}\n${agent.task}`;
  row.append(node("span", "cp-state-dot"));
  const copy = node("span", "cp-agent-copy");
  const primary = node("span", "cp-agent-primary");
  primary.append(node("span", "cp-agent-name", agent.name));
  if (agent.contradictionCount) {
    const conflict = node("span", "cp-conflict-count", String(agent.contradictionCount));
    conflict.append(icon("alert-triangle"));
    primary.append(conflict);
  }
  copy.append(primary, node("span", "cp-agent-branch", agent.branch), node("span", "cp-agent-status", agent.status));
  row.append(copy, node("span", "cp-agent-time", formatDuration(agent.elapsedMs)));
  row.addEventListener("click", () => selectAgent(agent.id));
  return row;
}

function renderCapacity() {
  const root = q("[data-testid='capacity-meter']");
  const cap = state.snapshot.capacity;
  root.dataset.level = cap.level;
  root.replaceChildren();
  const line = node("div", "cp-capacity-line");
  const pips = node("div", "cp-pips");
  const maxPips = 8;
  const filled = cap.max > 0 ? Math.ceil(((cap.used + cap.reserved) / cap.max) * maxPips) : 0;
  for (let index = 0; index < maxPips; index += 1) pips.append(node("span", `cp-pip${index < filled ? " on" : ""}`));
  const count = node("div", "cp-capacity-label");
  count.append(document.createTextNode(`${cap.used + cap.reserved} / ${cap.max} `), node("span", "", "slots"));
  line.append(count, pips);
  root.append(line, node("div", "cp-capacity-copy", cap.atCap ? "AT CAP" : "RAM-safe parallel capacity"));
}

function renderSummary() {
  const root = q(".cp-summary");
  root.replaceChildren();
  for (const kind of ["running", "stalled", "waiting", "done", "failed"]) {
    const cell = node("div", "cp-summary-cell");
    cell.dataset.kind = kind;
    cell.append(node("strong", "cp-summary-value", state.snapshot.totals[kind]), node("span", "cp-summary-label", label(kind)));
    root.append(cell);
  }
  const session = state.snapshot.sessions.find((item) => item.workspaceId === state.workspaceId) || state.snapshot.sessions[0];
  root.append(node("div", "cp-summary-note", session ? `BASE ${session.baseCommit.slice(0, 12)}  ${session.name}` : "No active run"));
}

const kinds = ["all", "decision", "change", "status", "error", "contradiction"];
function renderSegments() {
  const root = q(".cp-segments");
  root.replaceChildren();
  for (const kind of kinds) {
    const button = node("button", `cp-segment${state.kind === kind ? " active" : ""}`, label(kind));
    button.type = "button";
    button.dataset.kind = kind;
    button.setAttribute("role", "tab");
    button.setAttribute("aria-selected", String(state.kind === kind));
    button.addEventListener("click", () => { state.kind = kind; renderAll(); });
    root.append(button);
  }
}

function filteredActivity() {
  let activity = state.pendingActivity || state.snapshot.activity;
  if (state.workspaceId !== "all") activity = activity.filter((item) => item.workspaceId === state.workspaceId);
  if (state.agentId !== "all") activity = activity.filter((item) => item.agentId === state.agentId);
  if (state.kind === "contradiction") {
    const ids = new Set(state.snapshot.contradictions.flatMap((item) => [item.leftRecordId, item.rightRecordId]));
    activity = activity.filter((item) => ids.has(item.id));
  } else if (state.kind !== "all") activity = activity.filter((item) => item.kind === state.kind);
  const search = state.search.trim().toLowerCase();
  if (search) activity = activity.filter((item) => `${item.fact} ${item.fileKey || ""} ${item.agentId}`.toLowerCase().includes(search));
  return activity;
}

function renderActivity() {
  const root = q(".cp-activity");
  root.replaceChildren();
  const activity = filteredActivity();
  if (!activity.length) {
    root.append(node("div", "cp-empty", "No matching activity"));
  } else {
    for (const item of activity) {
      const row = node("article", `cp-activity-row${state.selectedRecordId === item.id ? " selected" : ""}`);
      row.tabIndex = 0;
      row.dataset.recordId = item.id;
      row.dataset.kind = item.kind;
      row.setAttribute("aria-posinset", String(activity.indexOf(item) + 1));
      row.setAttribute("aria-setsize", String(activity.length));
      row.append(node("span", "cp-kind", item.kind));
      const copy = node("span", "cp-activity-copy");
      copy.append(node("span", "cp-activity-fact", item.fact));
      const meta = node("span", "cp-activity-meta");
      meta.append(node("span", "", agentName(item.agentId)));
      if (item.fileKey) meta.append(node("span", "cp-activity-file", item.fileKey));
      copy.append(meta);
      row.append(copy, node("time", "cp-activity-time", relativeTime(item.timestamp)));
      row.addEventListener("click", () => selectRecord(item.id));
      row.addEventListener("keydown", (event) => {
        if (["Enter", " "].includes(event.key)) {
          event.preventDefault();
          selectRecord(item.id);
        }
      });
      root.append(row);
    }
  }
  renderLiveFollow();
}

function renderLiveFollow() {
  const banner = q(".cp-new-activity");
  banner.textContent = state.newCount ? `${state.newCount} new` : "Live";
  banner.classList.toggle("visible", state.newCount > 0);
}

function renderInspector() {
  const root = q(".cp-inspector-body");
  root.replaceChildren();
  const record = state.snapshot.activity.find((item) => item.id === state.selectedRecordId);
  const agent = state.snapshot.agents.find((item) => item.id === state.selectedAgentId);
  if (record) renderRecordInspector(root, record);
  else if (agent) renderAgentInspector(root, agent);
  else root.append(node("div", "cp-inspector-empty", "Select an agent or activity item"));
  q(".cp-inspector").classList.toggle("open", state.inspectorOpen);
  q(".cp-main").classList.toggle("inspector-open", state.inspectorOpen);
}

function renderRecordInspector(root, record) {
  const fact = node("section", "cp-detail-section");
  fact.append(node("div", "cp-detail-label", record.kind), node("div", "cp-detail-value", record.fact));
  root.append(fact);
  const detail = node("section", "cp-detail-section");
  const grid = node("dl", "cp-detail-grid");
  addDetail(grid, "Agent", agentName(record.agentId));
  addDetail(grid, "Branch", record.branch || "-");
  addDetail(grid, "File", record.fileKey || "-");
  addDetail(grid, "Recorded", formatTimestamp(record.timestamp));
  addDetail(grid, "Trust", record.trust);
  addDetail(grid, "Provenance", record.hasProvenance ? "available" : "none");
  detail.append(grid);
  root.append(detail);
  const contradiction = state.snapshot.contradictions.find((edge) => edge.leftRecordId === record.id || edge.rightRecordId === record.id);
  if (contradiction) root.append(contradictionNode(contradiction));
}

function renderAgentInspector(root, agent) {
  const heading = node("section", "cp-detail-section");
  heading.append(node("div", "cp-detail-label", agent.state), node("div", "cp-detail-value", agent.task));
  root.append(heading);
  const detail = node("section", "cp-detail-section");
  const grid = node("dl", "cp-detail-grid");
  addDetail(grid, "Agent", agent.name);
  addDetail(grid, "Branch", agent.branch);
  addDetail(grid, "Worktree", agent.worktree || "-");
  addDetail(grid, "Base", agent.baseCommit);
  addDetail(grid, "Elapsed", formatDuration(agent.elapsedMs));
  addDetail(grid, "Activity", agent.lastActivityAt ? relativeTime(agent.lastActivityAt) : "none");
  detail.append(grid);
  root.append(detail);
  for (const conflict of state.snapshot.contradictions.filter((item) => item.leftAgentId === agent.id || item.rightAgentId === agent.id)) root.append(contradictionNode(conflict));
  if (["running", "stalled"].includes(agent.state)) {
    const actions = node("div", "cp-inspector-actions");
    const stop = node("button", "cp-text-button danger", "Stop agent");
    stop.type = "button";
    stop.prepend(icon("circle-stop"));
    stop.addEventListener("click", () => stopAgent(agent.id));
    actions.append(stop);
    root.append(actions);
  }
}

function contradictionNode(conflict) {
  const section = node("section", "cp-detail-section");
  const warning = node("button", "cp-warning");
  warning.type = "button";
  warning.dataset.edgeId = conflict.id;
  warning.dataset.relation = "contradicts";
  warning.append(icon("alert-triangle"));
  const copy = node("span");
  copy.append(node("strong", "", "Same-file collision"), node("div", "cp-detail-mono", conflict.fileKey), node("div", "cp-detail-label", conflict.rule));
  warning.append(copy);
  warning.addEventListener("click", () => {
    state.workspaceId = conflict.workspaceId;
    state.agentId = "all";
    state.kind = "contradiction";
    state.search = conflict.fileKey;
    q("[data-testid='activity-search']").value = state.search;
    state.selectedRecordId = conflict.leftRecordId;
    renderAll();
  });
  section.append(warning);
  return section;
}

function addDetail(grid, term, value) {
  grid.append(node("dt", "", term), node("dd", "", value));
}

function renderStatus() {
  q("[data-testid='transport-mode']").textContent = transport.mode.toUpperCase();
  const commandMessage = state.commandMessageUntil > Date.now() ? state.commandMessage : null;
  q(".cp-status-message").textContent = commandMessage || (state.connection === "connected" ? `${state.snapshot.agents.length} agents / ${state.snapshot.workspaces.length} workspaces` : label(state.connection));
  q(".cp-revision").textContent = `REV ${state.snapshot.revision}`;
}

function terminalCommand(status) {
  return ["succeeded", "failed", "rejected", "expired"].includes(status);
}

function renderResponsive() {
  qa(".cp-mobile-tab").forEach((tab) => tab.classList.toggle("active", tab.dataset.view === state.mobileView));
  q(".cp-tree-panel").classList.toggle("mobile-active", state.mobileView === "agents");
  q(".cp-feed-panel").classList.toggle("mobile-active", state.mobileView === "activity");
}

function selectWorkspace(workspaceId) {
  state.workspaceId = state.workspaceId === workspaceId ? "all" : workspaceId;
  state.agentId = "all";
  state.selectedAgentId = null;
  state.selectedRecordId = null;
  closeDrawers();
  renderAll();
}

function selectAgent(agentId) {
  const agent = state.snapshot.agents.find((item) => item.id === agentId);
  state.agentId = state.agentId === agentId ? "all" : agentId;
  state.workspaceId = agent?.workspaceId || state.workspaceId;
  state.selectedAgentId = agentId;
  state.selectedRecordId = null;
  state.inspectorOpen = true;
  closeTreeDrawer();
  renderAll();
}

function selectRecord(recordId) {
  state.selectedRecordId = recordId;
  state.selectedAgentId = null;
  state.inspectorOpen = true;
  renderAll(false);
}

function toggleWorkspace(workspaceId) {
  if (state.collapsed.has(workspaceId)) state.collapsed.delete(workspaceId);
  else state.collapsed.add(workspaceId);
  renderTree();
  refreshIcons();
}

function agentsForWorkspace(workspaceId) {
  return state.snapshot.agents.filter((agent) => agent.workspaceId === workspaceId);
}

function depth(agent, agents) {
  let value = 0;
  let current = agent;
  const seen = new Set();
  while (current.parentId && !seen.has(current.id)) {
    seen.add(current.id);
    current = agents.find((candidate) => candidate.id === current.parentId);
    if (!current) break;
    value += 1;
  }
  return Math.min(value, 5);
}

function agentName(agentId) {
  return state.snapshot.agents.find((agent) => agent.id === agentId)?.name || agentId;
}

async function startRun(form) {
  const data = new FormData(form);
  const value = (name) => String(data.get(name) || "").trim();
  const payload = {
    sessionName: value("sessionName"),
    agentName: value("agentName"),
    task: value("task"),
    kind: data.get("kind"),
  };
  if (transport.mode === "web") {
    payload.workspaceId = value("workspaceId");
    payload.profileId = value("profileId");
    payload.agents = formAgentSpecifications(form, data);
  } else {
    payload.repoRoot = value("repoRoot");
    payload.command = value("command");
    payload.branch = value("branch");
    payload.fileScope = value("fileScope").split(",").map((entry) => entry.trim()).filter(Boolean);
  }
  const submit = q("button[type='submit']", form);
  submit.disabled = true;
  try {
    await transport.startRun(payload);
    q(".cp-run-dialog").close();
    q(".cp-status-message").textContent = transport.mode === "web" ? "Run queued" : "Agent started";
    if (transport.mode !== "web") await refreshSnapshot();
  } catch (error) {
    q(".cp-status-message").textContent = error.message || String(error);
  } finally {
    submit.disabled = false;
  }
}

async function connectDashboard(form) {
  const submit = q("button[type='submit']", form);
  const token = new FormData(form).get("token");
  submit.disabled = true;
  try {
    await transport.authenticate(String(token || ""));
    q("#cp-token").value = "";
    q(".cp-auth-dialog").close();
    await refreshSnapshot();
    await transport.subscribe();
  } catch (error) {
    q(".cp-status-message").textContent = error.message || String(error);
  } finally {
    submit.disabled = false;
  }
}

async function stopAgent(agentId) {
  try {
    await transport.stopAgent(agentId);
    q(".cp-status-message").textContent = transport.mode === "web" ? "Stop queued" : "Agent stopped";
    if (transport.mode !== "web") await refreshSnapshot();
  } catch (error) {
    q(".cp-status-message").textContent = error.message || String(error);
  }
}

function syncUrl() {
  const url = new URL(location.href);
  for (const [key, value] of [["workspace", state.workspaceId], ["agent", state.agentId], ["kind", state.kind], ["q", state.search]]) {
    if (value && value !== "all") url.searchParams.set(key, value);
    else url.searchParams.delete(key);
  }
  history.replaceState(null, "", url);
}

function openTreeDrawer() {
  q(".cp-tree-panel").classList.add("open");
  q(".cp-drawer-backdrop").classList.add("visible");
}
function closeTreeDrawer() { q(".cp-tree-panel").classList.remove("open"); q(".cp-drawer-backdrop").classList.remove("visible"); }
function closeDrawers() { closeTreeDrawer(); state.inspectorOpen = false; q(".cp-inspector").classList.remove("open"); }

function openPalette() {
  const palette = q(".cp-palette");
  palette.classList.add("open");
  const input = q("input", palette);
  input.value = "";
  renderPalette("");
  input.focus();
}

function openRunDialog() {
  renderRunCatalog();
  q(".cp-run-dialog").showModal();
}

const paletteCommands = [
  { label: "Start agent run", hint: "+1 slot", run: openRunDialog },
  { label: "Show all activity", hint: "A", run: () => { state.kind = "all"; state.agentId = "all"; renderAll(); } },
  { label: "Show contradictions", hint: "C", run: () => { state.kind = "contradiction"; renderAll(); } },
  { label: "Focus activity search", hint: "/", run: () => q("[data-testid='activity-search']").focus() },
  { label: "Open connection settings", hint: "", run: () => q(".cp-auth-dialog").showModal() },
];

function renderPalette(search) {
  const root = q(".cp-palette-list");
  root.replaceChildren();
  const commands = paletteCommands.filter((command) => command.label.toLowerCase().includes(search.toLowerCase()));
  commands.forEach((command, index) => {
    const row = node("div", `cp-palette-row${index === 0 ? " active" : ""}`);
    row.append(node("span", "", command.label), node("span", "cp-palette-hint", command.hint));
    row.addEventListener("click", () => commitPalette(command));
    root.append(row);
  });
  const cap = state.snapshot.capacity;
  q(".cp-palette-capacity").textContent = `${cap.used + cap.reserved} / ${cap.max} slots  SAFE ZONE`;
}

function commitPalette(command) {
  q(".cp-palette").classList.remove("open");
  command?.run();
}

function formatDuration(ms) {
  if (!ms) return "queued";
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}
function relativeTime(timestamp) {
  const seconds = Math.max(0, Math.floor((state.snapshot.generatedAt - timestamp) / 1_000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3_600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3_600)}h`;
}
function formatTimestamp(timestamp) { return new Date(timestamp).toLocaleString(); }
function label(value) { return String(value).replaceAll("_", " ").replace(/^./, (letter) => letter.toUpperCase()); }

q("[data-testid='activity-search']").value = state.search;
q("[data-testid='activity-search']").addEventListener("input", (event) => { state.search = event.target.value; renderActivity(); syncUrl(); });
q(".cp-activity").addEventListener("scroll", (event) => { state.following = event.currentTarget.scrollTop <= 12; });
q(".cp-new-activity").addEventListener("click", () => { state.pendingActivity = null; state.newCount = 0; state.following = true; renderActivity(); q(".cp-activity").scrollTop = 0; });
qa(".cp-new-run").forEach((button) => button.addEventListener("click", openRunDialog));
q(".cp-add-agent").addEventListener("click", addParallelAgent);
q(".cp-run-dialog form").addEventListener("submit", (event) => { event.preventDefault(); void startRun(event.currentTarget); });
q(".cp-dialog-close").addEventListener("click", () => q(".cp-run-dialog").close());
q(".cp-dialog-cancel").addEventListener("click", () => q(".cp-run-dialog").close());
q(".cp-connection").addEventListener("click", () => { q("#cp-token").value = ""; q(".cp-auth-dialog").showModal(); });
q(".cp-auth-dialog form").addEventListener("submit", (event) => { event.preventDefault(); void connectDashboard(event.currentTarget); });
q(".cp-auth-close").addEventListener("click", () => q(".cp-auth-dialog").close());
q(".cp-auth-cancel").addEventListener("click", () => q(".cp-auth-dialog").close());
q(".cp-tree-toggle").addEventListener("click", openTreeDrawer);
q(".cp-inspector-toggle").addEventListener("click", () => { state.inspectorOpen = !state.inspectorOpen; renderInspector(); });
q(".cp-inspector-close").addEventListener("click", () => { state.inspectorOpen = false; renderInspector(); });
q(".cp-drawer-backdrop").addEventListener("click", closeDrawers);
q(".cp-palette-toggle").addEventListener("click", openPalette);
q(".cp-palette input").addEventListener("input", (event) => renderPalette(event.target.value));
q(".cp-palette input").addEventListener("keydown", (event) => { if (event.key === "Enter") commitPalette(paletteCommands.find((command) => command.label.toLowerCase().includes(event.currentTarget.value.toLowerCase()))); });
qa(".cp-mobile-tab").forEach((tab) => tab.addEventListener("click", () => { state.mobileView = tab.dataset.view; renderResponsive(); }));

document.addEventListener("keydown", (event) => {
  if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === "p") { event.preventDefault(); openPalette(); return; }
  if (event.key === "/" && !["INPUT", "TEXTAREA"].includes(document.activeElement?.tagName)) { event.preventDefault(); q("[data-testid='activity-search']").focus(); return; }
  if (event.key === "Escape") { q(".cp-palette").classList.remove("open"); closeDrawers(); return; }
  if (!["j", "k", "Enter"].includes(event.key)) return;
  const activity = filteredActivity();
  if (!activity.length) return;
  const current = activity.findIndex((item) => item.id === state.selectedRecordId);
  if (event.key === "Enter" && current >= 0) { state.inspectorOpen = true; renderInspector(); return; }
  if (event.key === "j") state.selectedRecordId = activity[Math.min(current + 1, activity.length - 1)].id;
  if (event.key === "k") state.selectedRecordId = activity[Math.max(current - 1, 0)].id;
  if (["j", "k"].includes(event.key)) { event.preventDefault(); renderActivity(); q(`[data-record-id='${state.selectedRecordId}']`)?.focus(); }
});

async function initializeControlPlane() {
  renderAll();
  await refreshSnapshot();
  await transport.subscribe();
}

void initializeControlPlane().catch((error) => {
  console.error("control-plane initialization failed:", error);
  state.connection = "offline";
  q(".cp-status-message").textContent = error.message || String(error);
  renderConnection();
});

window.addEventListener("pagehide", () => transport.close?.());
