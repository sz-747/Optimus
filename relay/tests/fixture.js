export function safeEnvelope(sourceRevision = 7) {
  return {
    schemaVersion: 1,
    instanceId: "11111111-1111-4111-8111-111111111111",
    sourceRevision,
    capturedAt: 1_750_000_000_000 + sourceRevision,
    snapshot: {
      generatedAt: 1_750_000_000_000 + sourceRevision,
      workspaces: [{ id: "workspace-primary", sessionCount: 1, activeAgents: 1 }],
      sessions: [{ id: "session-1", label: "Remote run", workspaceId: "workspace-primary", state: "active", createdAt: 1_750_000_000_000 }],
      agents: [{ id: "agent-1", label: "Remote runner", sessionId: "session-1", workspaceId: "workspace-primary", parentId: null, kind: "writer", state: "running", startedAt: 1_750_000_000_000, elapsedMs: 10_000, lastActivityAt: 1_750_000_005_000, contradictionCount: 1 }],
      activity: [{ id: 1, workspaceId: "workspace-primary", agentId: "agent-1", kind: "status", timestamp: 1_750_000_005_000 }],
      capacity: { used: 1, reserved: 0, max: 8, level: "calm", fraction: 0.125, atCap: false },
      totals: { running: 1, stalled: 0, waiting: 0, done: 0, failed: 0 },
      catalog: {
        workspaces: [{ id: "workspace-primary", label: "Primary workspace" }],
        profiles: [{ id: "codex-subscription", label: "Codex subscription" }],
      },
    },
  };
}
