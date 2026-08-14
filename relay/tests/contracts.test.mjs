import assert from "node:assert/strict";
import test from "node:test";
import { parseCommandInput, parseSnapshotEnvelope } from "../contracts.js";
import { createSafeSnapshotEnvelope } from "../sanitize.js";
import { safeEnvelope } from "./fixture.js";

test("safe snapshot contract rejects raw local fields", () => {
  const envelope = safeEnvelope();
  envelope.snapshot.workspaces[0].repoRoot = "C:\\private\\repo";
  assert.throws(() => parseSnapshotEnvelope(envelope), /repoRoot is not allowed/);
});

test("remote commands accept only registered typed intents", () => {
  const catalog = safeEnvelope().snapshot.catalog;
  const command = parseCommandInput({ type: "start_run", payload: { workspaceId: "workspace-primary", profileId: "codex-subscription", sessionName: "Verification", agentName: "Runner", task: "Build the app", kind: "writer" } }, catalog);
  assert.equal(command.payload.profileId, "codex-subscription");
  assert.equal(command.payload.agents[0].agentName, "Runner");
  assert.throws(() => parseCommandInput({ type: "start_run", payload: { ...command.payload, repoRoot: "C:\\private", command: "powershell" } }, catalog), /not allowed/);
  assert.throws(() => parseCommandInput({ type: "start_run", payload: { ...command.payload, profileId: "arbitrary-shell" } }, catalog), /not registered/);
});

test("parallel commands require disjoint repository-relative file scopes", () => {
  const catalog = safeEnvelope().snapshot.catalog;
  const payload = {
    workspaceId: "workspace-primary",
    profileId: "codex-subscription",
    sessionName: "Parallel verification",
    agents: [
      { agentName: "Shell", task: "Build the shell", kind: "writer", fileScope: ["shell/src"] },
      { agentName: "Web", task: "Build the web", kind: "writer", fileScope: ["ui"] },
    ],
  };
  const command = parseCommandInput({ type: "start_run", payload }, catalog);
  assert.equal(command.payload.agents.length, 2);
  assert.throws(
    () => parseCommandInput({ type: "start_run", payload: { ...payload, agents: payload.agents.map((agent) => ({ ...agent, fileScope: ["ui"] })) } }, catalog),
    (error) => error.code === "scope_overlap",
  );
  assert.throws(
    () => parseCommandInput({ type: "start_run", payload: { ...payload, agents: [{ ...payload.agents[0], fileScope: ["../private"] }, payload.agents[1]] } }, catalog),
    /repository-relative/,
  );
  for (const [left, right] of [["ui", "ui/src"], ["UI", "ui"], ["./ui", "ui"], ["ui/./src", "ui/src"], ["ui//src", "ui/src"], ["ui/*", "docs"]]) {
    assert.throws(
      () => parseCommandInput({ type: "start_run", payload: { ...payload, agents: [{ ...payload.agents[0], fileScope: [left] }, { ...payload.agents[1], fileScope: [right] }] } }, catalog),
      (error) => error.code === "scope_overlap",
    );
  }
  assert.throws(
    () => parseCommandInput({ type: "start_run", payload: { ...payload, agents: [{ ...payload.agents[0], fileScope: ["./"] }, payload.agents[1]] } }, catalog),
    /must select/,
  );
  for (const alias of ["ui.", "ui /src"]) {
    assert.throws(
      () => parseCommandInput({ type: "start_run", payload: { ...payload, agents: [{ ...payload.agents[0], fileScope: [alias] }, payload.agents[1]] } }, catalog),
      /repository-relative/,
    );
  }
});

test("desktop sanitizer cannot serialize local facts, paths, tasks, branches, or rules", () => {
  const sentinel = "SENTINEL-PRIVATE-CONTENT";
  const local = {
    revision: 12,
    generatedAt: 10_000,
    workspaces: [{ id: "ws-1", name: sentinel, repoRoot: `C:\\${sentinel}`, sessionCount: 1, activeAgents: 1 }],
    sessions: [{ id: "session-1", workspaceId: "ws-1", name: sentinel, baseCommit: sentinel, state: "active", createdAt: 1 }],
    agents: [{ id: "agent-1", sessionId: "session-1", workspaceId: "ws-1", parentId: null, name: sentinel, task: sentinel, status: sentinel, branch: sentinel, worktree: sentinel, baseCommit: sentinel, kind: "writer", state: "running", startedAt: 1, elapsedMs: 2, lastActivityAt: 3, contradictionCount: 1 }],
    activity: [{ id: 1, workspaceId: "ws-1", agentId: "agent-1", kind: "status", fact: sentinel, fileKey: sentinel, branch: sentinel, trust: "trusted", timestamp: 3, hasProvenance: true }],
    contradictions: [{ fileKey: sentinel, rule: sentinel }],
    capacity: { used: 1, reserved: 0, max: 8, level: "calm", fraction: 0.125, atCap: false },
    totals: { running: 1, stalled: 0, waiting: 0, done: 0, failed: 0 },
  };
  const safe = createSafeSnapshotEnvelope(local, { instanceId: "11111111-1111-4111-8111-111111111111", capturedAt: 11_000, workspaces: [{ localId: "ws-1", id: "workspace-primary", label: "Primary workspace" }], profiles: [{ id: "codex-subscription", label: "Codex subscription" }] });
  assert.doesNotMatch(JSON.stringify(safe), new RegExp(sentinel));
  assert.doesNotThrow(() => parseSnapshotEnvelope(safe));
});
