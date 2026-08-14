const generatedAt = 1_783_748_900_000;

export function createControlPlaneFixture() {
  return {
    revision: 17,
    generatedAt,
    workspaces: [
      { id: "ws-1", name: "optimus", repoRoot: "C:\\dev\\Optimus", sessionCount: 1, activeAgents: 4 },
      { id: "ws-2", name: "client-portal", repoRoot: "C:\\dev\\client-portal", sessionCount: 1, activeAgents: 1 },
    ],
    sessions: [
      { id: "session-1", workspaceId: "ws-1", name: "Control plane foundation", baseCommit: "c1fb05fc0ab8f3d982c57eef481c4c6fc0c74462", state: "active", createdAt: generatedAt - 1_842_000 },
      { id: "session-2", workspaceId: "ws-2", name: "Checkout reliability", baseCommit: "8ca7e54681e04e402df921b469a09fd27d65aeb1", state: "active", createdAt: generatedAt - 780_000 },
    ],
    agents: [
      agent("agent-1", "session-1", "ws-1", null, "Coordinator", "Integrate the control-plane spine", "running", "feat/control-plane-foundation", 1_842_000, 0, "Reviewing correlation output"),
      agent("agent-2", "session-1", "ws-1", "agent-1", "Memory store", "Build the durable provenance store", "done", "feat/m0-memory-store", 1_524_000, 0, "Tests passed"),
      agent("agent-3", "session-1", "ws-1", "agent-1", "Web surface", "Build the product dashboard", "running", "feat/m2-web-surface", 604_000, 1, "Implementing the activity stream"),
      agent("agent-4", "session-1", "ws-1", "agent-1", "Git lifecycle", "Verify pinned-base worktrees", "stalled", "feat/worktree-lifecycle", 1_102_000, 1, "Waiting on a process"),
      agent("agent-5", "session-1", "ws-1", "agent-1", "IPC capture", "Attribute hooks to the caller", "waiting", "feat/ipc-capture", 0, 0, "Queued"),
      agent("agent-6", "session-2", "ws-2", null, "Checkout audit", "Trace the checkout regression", "running", "fix/checkout-retry", 476_000, 0, "Running integration tests"),
    ],
    activity: [
      activity(42, "ws-1", "agent-3", "change", "Added responsive control-plane layout", "ui/control-plane.css", generatedAt - 8_000, true),
      activity(41, "ws-1", "agent-1", "status", "Reviewing correlation output", null, generatedAt - 21_000, false),
      activity(40, "ws-1", "agent-4", "change", "Pinned the second writer to the shared base", "shell/src/orchestration/git.rs", generatedAt - 74_000, true),
      activity(39, "ws-1", "agent-3", "change", "Changed the control-plane projection", "shell/src/control_plane/read_model.rs", generatedAt - 96_000, true),
      activity(38, "ws-1", "agent-4", "change", "Also changed the control-plane projection", "shell/src/control_plane/read_model.rs", generatedAt - 118_000, true),
      activity(37, "ws-1", "agent-2", "decision", "Use SQLite rows for typed provenance edges", "shell/src/control_plane/memory.rs", generatedAt - 184_000, true),
      activity(36, "ws-1", "agent-2", "status", "Tests passed", null, generatedAt - 201_000, false),
      activity(35, "ws-2", "agent-6", "error", "Retry fixture returned an unexpected 409", "tests/checkout/retry.spec.ts", generatedAt - 240_000, true),
      activity(34, "ws-1", "agent-1", "spawn", "Spawned agent-5 from the pinned base", null, generatedAt - 306_000, true),
    ],
    contradictions: [
      { id: "edge-39-38-contradicts", workspaceId: "ws-1", leftAgentId: "agent-3", rightAgentId: "agent-4", fileKey: "shell/src/control_plane/read_model.rs", leftRecordId: 39, rightRecordId: 38, rule: "contradicts:v1:trusted-window-200" },
    ],
    capacity: { used: 5, reserved: 0, max: 8, level: "warn", fraction: 0.625, atCap: false },
    totals: { running: 3, stalled: 1, waiting: 1, done: 1, failed: 0 },
  };
}

function agent(id, sessionId, workspaceId, parentId, name, task, state, branch, elapsedMs, contradictionCount, status) {
  return {
    id,
    sessionId,
    workspaceId,
    parentId,
    name,
    task,
    kind: "writer",
    state,
    status,
    branch,
    worktree: `C:\\dev\\.worktrees\\optimus\\${sessionId}\\${id}`,
    baseCommit: sessionId === "session-1" ? "c1fb05fc0ab8" : "8ca7e54681e0",
    baseDrift: false,
    startedAt: elapsedMs ? generatedAt - elapsedMs : null,
    elapsedMs,
    lastActivityAt: state === "stalled" ? generatedAt - 360_000 : generatedAt - 21_000,
    contradictionCount,
  };
}

function activity(id, workspaceId, agentId, kind, fact, fileKey, timestamp, hasProvenance) {
  return { id, workspaceId, agentId, kind, fact, fileKey, branch: `feat/${agentId}`, trust: "trusted", timestamp, hasProvenance };
}
