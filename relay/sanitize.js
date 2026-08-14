export function createSafeSnapshotEnvelope(localSnapshot, options) {
  const workspaceMap = new Map(options.workspaces.map((entry) => [entry.localId, entry]));
  const allowedWorkspace = (id) => workspaceMap.has(id);
  const workspaceId = (id) => workspaceMap.get(id)?.id;
  const allowedSessions = new Set(localSnapshot.sessions.filter((entry) => allowedWorkspace(entry.workspaceId)).map((entry) => entry.id));
  const allowedAgents = new Set(localSnapshot.agents.filter((entry) => allowedWorkspace(entry.workspaceId) && allowedSessions.has(entry.sessionId)).map((entry) => entry.id));
  return {
    schemaVersion: 1,
    instanceId: options.instanceId,
    sourceRevision: localSnapshot.revision,
    capturedAt: options.capturedAt,
    snapshot: {
      generatedAt: localSnapshot.generatedAt,
      workspaces: localSnapshot.workspaces.filter((entry) => allowedWorkspace(entry.id)).map((entry) => ({
        id: workspaceId(entry.id),
        sessionCount: entry.sessionCount,
        activeAgents: entry.activeAgents,
      })),
      sessions: localSnapshot.sessions.filter((entry) => allowedSessions.has(entry.id)).map((entry) => ({
        id: entry.id,
        label: null,
        workspaceId: workspaceId(entry.workspaceId),
        state: entry.state,
        createdAt: entry.createdAt,
      })),
      agents: localSnapshot.agents.filter((entry) => allowedAgents.has(entry.id)).map((entry) => ({
        id: entry.id,
        label: null,
        sessionId: entry.sessionId,
        workspaceId: workspaceId(entry.workspaceId),
        parentId: allowedAgents.has(entry.parentId) ? entry.parentId : null,
        kind: entry.kind,
        state: entry.state,
        startedAt: entry.startedAt,
        elapsedMs: entry.elapsedMs,
        lastActivityAt: entry.lastActivityAt,
        contradictionCount: entry.contradictionCount,
      })),
      activity: localSnapshot.activity.filter((entry) => allowedWorkspace(entry.workspaceId) && allowedAgents.has(entry.agentId)).slice(0, 250).map((entry) => ({
        id: entry.id,
        workspaceId: workspaceId(entry.workspaceId),
        agentId: entry.agentId,
        kind: entry.kind,
        timestamp: entry.timestamp,
      })),
      capacity: {
        used: localSnapshot.capacity.used,
        reserved: localSnapshot.capacity.reserved,
        max: localSnapshot.capacity.max,
        level: localSnapshot.capacity.level,
        fraction: localSnapshot.capacity.fraction,
        atCap: localSnapshot.capacity.atCap,
      },
      totals: remoteTotals(localSnapshot.agents.filter((entry) => allowedAgents.has(entry.id))),
      catalog: {
        workspaces: options.workspaces.map(({ id, label }) => ({ id, label })),
        profiles: options.profiles.map(({ id, label }) => ({ id, label })),
      },
    },
  };
}

function remoteTotals(agents) {
  const count = (...states) => agents.filter((agent) => states.includes(agent.state)).length;
  return {
    running: count("running"),
    stalled: count("stalled"),
    waiting: count("waiting"),
    done: count("done"),
    failed: count("failed", "interrupted"),
  };
}
