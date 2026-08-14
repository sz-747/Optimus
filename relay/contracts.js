const ID_PATTERN = /^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,63}$/;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f-]{27,35}$/i;
const AGENT_STATES = new Set(["planned", "ready", "running", "stopping", "done", "failed", "interrupted", "stalled", "waiting"]);
const SESSION_STATES = new Set(["active", "completed", "failed"]);
const AGENT_KINDS = new Set(["writer", "recon"]);
const ACTIVITY_KINDS = new Set(["decision", "change", "status", "error", "spawn"]);
const ACK_STATES = new Set(["running", "succeeded", "failed", "rejected"]);

export class ContractError extends Error {
  constructor(message, code = "invalid_request", status = 400) {
    super(message);
    this.name = "ContractError";
    this.code = code;
    this.status = status;
  }
}

export function parseSnapshotEnvelope(input) {
  const envelope = object(input, "body");
  exactKeys(envelope, ["schemaVersion", "instanceId", "sourceRevision", "capturedAt", "snapshot"], "body");
  if (envelope.schemaVersion !== 1) fail("body.schemaVersion must be 1");
  const instanceId = text(envelope.instanceId, "body.instanceId", 36, UUID_PATTERN);
  const sourceRevision = integer(envelope.sourceRevision, "body.sourceRevision", 0);
  const capturedAt = integer(envelope.capturedAt, "body.capturedAt", 0);
  const snapshot = parseSafeSnapshot(envelope.snapshot);
  return { schemaVersion: 1, instanceId, sourceRevision, capturedAt, snapshot };
}

export function parseCommandInput(input, catalog) {
  const root = object(input, "body");
  exactKeys(root, ["type", "payload"], "body");
  const type = oneOf(root.type, "body.type", new Set(["start_run", "stop_agent"]));
  const payload = object(root.payload, "body.payload");

  if (type === "stop_agent") {
    exactKeys(payload, ["agentId"], "body.payload");
    return { type, payload: { agentId: id(payload.agentId, "body.payload.agentId") } };
  }

  const parallel = "agents" in payload;
  exactKeys(
    payload,
    parallel
      ? ["workspaceId", "profileId", "sessionName", "agents"]
      : ["workspaceId", "profileId", "sessionName", "agentName", "task", "kind"],
    "body.payload",
  );
  const workspaceId = id(payload.workspaceId, "body.payload.workspaceId");
  const profileId = id(payload.profileId, "body.payload.profileId");
  if (!catalog?.workspaces?.some((entry) => entry.id === workspaceId)) {
    throw new ContractError("workspace is not registered", "unknown_workspace", 422);
  }
  if (!catalog?.profiles?.some((entry) => entry.id === profileId)) {
    throw new ContractError("provider profile is not registered", "unknown_profile", 422);
  }
  const agents = parallel
    ? list(payload.agents, "body.payload.agents", 8, parseStartAgent)
    : [parseStartAgent({ agentName: payload.agentName, task: payload.task, kind: payload.kind, fileScope: [] }, "body.payload")];
  if (agents.length === 0) fail("body.payload.agents requires at least one agent");
  if (agents.length > 1) {
    const owners = [];
    for (const [index, agent] of agents.entries()) {
      if (agent.fileScope.length === 0) fail(`body.payload.agents[${index}].fileScope is required for parallel runs`);
      for (const scope of agent.fileScope) {
        if (owners.some((owner) => owner.index !== index && scopesOverlap(owner.scope, scope))) {
          throw new ContractError(`file scope ${scope} overlaps another agent`, "scope_overlap", 422);
        }
        owners.push({ index, scope });
      }
    }
  }
  return {
    type,
    payload: {
      workspaceId,
      profileId,
      sessionName: text(payload.sessionName, "body.payload.sessionName", 80),
      agents,
    },
  };
}

function parseStartAgent(value, path) {
  const item = object(value, path);
  exactKeys(item, ["agentName", "task", "kind", "fileScope"], path);
  return {
    agentName: text(item.agentName, `${path}.agentName`, 80),
    task: text(item.task, `${path}.task`, 4_000),
    kind: oneOf(item.kind, `${path}.kind`, AGENT_KINDS),
    fileScope: list(item.fileScope, `${path}.fileScope`, 32, (scope, scopePath) => safeScope(scope, scopePath)),
  };
}

function safeScope(value, path) {
  const raw = text(value, path, 160).replaceAll("\\", "/");
  if (
    /^(?:[a-z]:|\/)/i.test(raw)
    || raw.split("/").some((part) => part === ".." || (part !== "." && /[. ]$/.test(part)))
    || /[\r\n\0]/.test(raw)
  ) {
    fail(`${path} must be a repository-relative pattern`);
  }
  const scope = raw.split("/").filter((part) => part && part !== ".").join("/");
  if (!scope) fail(`${path} must select a repository-relative path`);
  return scope;
}

function scopesOverlap(left, right) {
  const a = left.toLowerCase();
  const b = right.toLowerCase();
  if (/[?*\[\]{}!]/.test(a) || /[?*\[\]{}!]/.test(b)) return true;
  return a === "." || b === "." || a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`);
}

export function parseDesktopAck(input) {
  const root = object(input, "body");
  exactKeys(root, ["commandId", "leaseId", "status", "errorCode"], "body");
  return {
    commandId: text(root.commandId, "body.commandId", 36, UUID_PATTERN),
    leaseId: text(root.leaseId, "body.leaseId", 36, UUID_PATTERN),
    status: oneOf(root.status, "body.status", ACK_STATES),
    errorCode: root.errorCode == null ? null : text(root.errorCode, "body.errorCode", 64, ID_PATTERN),
  };
}

export function parseDeviceId(value) {
  return id(value, "device id");
}

export function parseConsumerId(value) {
  return text(value, "consumer", 36, UUID_PATTERN);
}

export function normalizeRemoteSnapshot(envelope, relayRevision, publishedAt) {
  const snapshot = envelope.snapshot;
  const workspaceLabels = new Map(snapshot.catalog.workspaces.map((entry) => [entry.id, entry.label]));
  return {
    revision: relayRevision,
    sourceRevision: envelope.sourceRevision,
    generatedAt: snapshot.generatedAt,
    publishedAt,
    workspaces: snapshot.workspaces.map((entry, index) => ({
      ...entry,
      name: workspaceLabels.get(entry.id) || `Workspace ${index + 1}`,
      repoRoot: null,
    })),
    sessions: snapshot.sessions.map((entry, index) => ({
      ...entry,
      name: entry.label || `Run ${index + 1}`,
      baseCommit: "",
    })),
    agents: snapshot.agents.map((entry, index) => ({
      ...entry,
      name: entry.label || `Agent ${index + 1}`,
      task: "Private instruction",
      status: stateLabel(entry.state),
      branch: "",
      worktree: null,
      baseCommit: "",
      baseDrift: false,
    })),
    activity: snapshot.activity.map((entry) => ({
      ...entry,
      fact: activityLabel(entry.kind),
      fileKey: null,
      branch: null,
      trust: "remote",
      hasProvenance: false,
    })),
    contradictions: [],
    capacity: snapshot.capacity,
    totals: snapshot.totals,
    catalog: snapshot.catalog,
  };
}

function parseSafeSnapshot(input) {
  const root = object(input, "body.snapshot");
  exactKeys(root, ["generatedAt", "workspaces", "sessions", "agents", "activity", "capacity", "totals", "catalog"], "body.snapshot");
  return {
    generatedAt: integer(root.generatedAt, "body.snapshot.generatedAt", 0),
    workspaces: list(root.workspaces, "body.snapshot.workspaces", 100, parseWorkspace),
    sessions: list(root.sessions, "body.snapshot.sessions", 500, parseSession),
    agents: list(root.agents, "body.snapshot.agents", 1_000, parseAgent),
    activity: list(root.activity, "body.snapshot.activity", 250, parseActivity),
    capacity: parseCapacity(root.capacity),
    totals: parseTotals(root.totals),
    catalog: parseCatalog(root.catalog),
  };
}

function parseWorkspace(value, path) {
  const item = object(value, path);
  exactKeys(item, ["id", "sessionCount", "activeAgents"], path);
  return { id: id(item.id, `${path}.id`), sessionCount: integer(item.sessionCount, `${path}.sessionCount`, 0), activeAgents: integer(item.activeAgents, `${path}.activeAgents`, 0) };
}

function parseSession(value, path) {
  const item = object(value, path);
  exactKeys(item, ["id", "label", "workspaceId", "state", "createdAt"], path);
  return { id: id(item.id, `${path}.id`), label: item.label == null ? null : text(item.label, `${path}.label`, 80), workspaceId: id(item.workspaceId, `${path}.workspaceId`), state: oneOf(item.state, `${path}.state`, SESSION_STATES), createdAt: integer(item.createdAt, `${path}.createdAt`, 0) };
}

function parseAgent(value, path) {
  const item = object(value, path);
  exactKeys(item, ["id", "label", "sessionId", "workspaceId", "parentId", "kind", "state", "startedAt", "elapsedMs", "lastActivityAt", "contradictionCount"], path);
  return {
    id: id(item.id, `${path}.id`),
    label: item.label == null ? null : text(item.label, `${path}.label`, 80),
    sessionId: id(item.sessionId, `${path}.sessionId`),
    workspaceId: id(item.workspaceId, `${path}.workspaceId`),
    parentId: item.parentId == null ? null : id(item.parentId, `${path}.parentId`),
    kind: oneOf(item.kind, `${path}.kind`, AGENT_KINDS),
    state: oneOf(item.state, `${path}.state`, AGENT_STATES),
    startedAt: optionalInteger(item.startedAt, `${path}.startedAt`),
    elapsedMs: integer(item.elapsedMs, `${path}.elapsedMs`, 0),
    lastActivityAt: optionalInteger(item.lastActivityAt, `${path}.lastActivityAt`),
    contradictionCount: integer(item.contradictionCount, `${path}.contradictionCount`, 0),
  };
}

function parseActivity(value, path) {
  const item = object(value, path);
  exactKeys(item, ["id", "workspaceId", "agentId", "kind", "timestamp"], path);
  return { id: integer(item.id, `${path}.id`, 0), workspaceId: id(item.workspaceId, `${path}.workspaceId`), agentId: id(item.agentId, `${path}.agentId`), kind: oneOf(item.kind, `${path}.kind`, ACTIVITY_KINDS), timestamp: integer(item.timestamp, `${path}.timestamp`, 0) };
}

function parseCapacity(value) {
  const item = object(value, "body.snapshot.capacity");
  exactKeys(item, ["used", "reserved", "max", "level", "fraction", "atCap"], "body.snapshot.capacity");
  return {
    used: integer(item.used, "body.snapshot.capacity.used", 0),
    reserved: integer(item.reserved, "body.snapshot.capacity.reserved", 0),
    max: integer(item.max, "body.snapshot.capacity.max", 0),
    level: oneOf(item.level, "body.snapshot.capacity.level", new Set(["calm", "warn", "cap"])),
    fraction: finiteNumber(item.fraction, "body.snapshot.capacity.fraction", 0, 1),
    atCap: boolean(item.atCap, "body.snapshot.capacity.atCap"),
  };
}

function parseTotals(value) {
  const item = object(value, "body.snapshot.totals");
  exactKeys(item, ["running", "stalled", "waiting", "done", "failed"], "body.snapshot.totals");
  return Object.fromEntries(["running", "stalled", "waiting", "done", "failed"].map((key) => [key, integer(item[key], `body.snapshot.totals.${key}`, 0)]));
}

function parseCatalog(value) {
  const item = object(value, "body.snapshot.catalog");
  exactKeys(item, ["workspaces", "profiles"], "body.snapshot.catalog");
  const entry = (candidate, path) => {
    const row = object(candidate, path);
    exactKeys(row, ["id", "label"], path);
    return { id: id(row.id, `${path}.id`), label: text(row.label, `${path}.label`, 80) };
  };
  return { workspaces: list(item.workspaces, "body.snapshot.catalog.workspaces", 100, entry), profiles: list(item.profiles, "body.snapshot.catalog.profiles", 20, entry) };
}

function exactKeys(value, expected, path) {
  const allowed = new Set(expected);
  for (const key of Object.keys(value)) if (!allowed.has(key)) fail(`${path}.${key} is not allowed`);
  for (const key of expected) if (!(key in value)) fail(`${path}.${key} is required`);
}

function list(value, path, maximum, parser) {
  if (!Array.isArray(value)) fail(`${path} must be an array`);
  if (value.length > maximum) fail(`${path} exceeds ${maximum} items`, "payload_too_large", 413);
  return value.map((entry, index) => parser(entry, `${path}[${index}]`));
}

function object(value, path) {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail(`${path} must be an object`);
  return value;
}

function text(value, path, maximum, pattern) {
  if (typeof value !== "string" || value.trim() === "") fail(`${path} must be a non-empty string`);
  if (value.length > maximum) fail(`${path} exceeds ${maximum} characters`, "payload_too_large", 413);
  if (pattern && !pattern.test(value)) fail(`${path} has an invalid format`);
  return value;
}

function id(value, path) {
  return text(value, path, 64, ID_PATTERN);
}

function integer(value, path, minimum) {
  if (!Number.isSafeInteger(value) || value < minimum) fail(`${path} must be an integer >= ${minimum}`);
  return value;
}

function optionalInteger(value, path) {
  return value == null ? null : integer(value, path, 0);
}

function finiteNumber(value, path, minimum, maximum) {
  if (!Number.isFinite(value) || value < minimum || value > maximum) fail(`${path} must be between ${minimum} and ${maximum}`);
  return value;
}

function boolean(value, path) {
  if (typeof value !== "boolean") fail(`${path} must be a boolean`);
  return value;
}

function oneOf(value, path, allowed) {
  if (typeof value !== "string" || !allowed.has(value)) fail(`${path} is not supported`);
  return value;
}

function fail(message, code = "invalid_request", status = 400) {
  throw new ContractError(message, code, status);
}

function stateLabel(state) {
  return ({ planned: "Planned", ready: "Ready", running: "Running", stopping: "Stopping", done: "Done", failed: "Failed", interrupted: "Stopped", stalled: "Stalled", waiting: "Waiting" })[state] || "Unknown";
}

function activityLabel(kind) {
  return ({ decision: "Decision recorded", change: "Change recorded", status: "Status updated", error: "Error reported", spawn: "Agent started" })[kind] || "Activity recorded";
}
