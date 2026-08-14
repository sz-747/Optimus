import assert from "node:assert/strict";
import test from "node:test";
import { MemoryRelayStore } from "../memory-store.js";
import { parseSnapshotEnvelope } from "../contracts.js";
import { safeEnvelope } from "./fixture.js";

test("snapshot revisions are monotonic, stale writes fail, and retries are idempotent", async () => {
  let now = 1_000;
  const store = new MemoryRelayStore({ now: () => now });
  assert.deepEqual(await store.publishSnapshot("primary", parseSnapshotEnvelope(safeEnvelope(7))), { revision: 1, duplicate: false });
  assert.deepEqual(await store.publishSnapshot("primary", parseSnapshotEnvelope(safeEnvelope(7))), { revision: 1, duplicate: true });
  await assert.rejects(store.publishSnapshot("primary", parseSnapshotEnvelope(safeEnvelope(6))), /stale/);
  now += 1;
  assert.deepEqual(await store.publishSnapshot("primary", parseSnapshotEnvelope(safeEnvelope(8))), { revision: 2, duplicate: false });
  assert.equal((await store.snapshot("primary")).revision, 2);
  const restarted = safeEnvelope(1);
  restarted.instanceId = "22222222-2222-4222-8222-222222222222";
  restarted.capturedAt = safeEnvelope(8).capturedAt + 100;
  restarted.snapshot.generatedAt = restarted.capturedAt;
  await assert.rejects(store.publishSnapshot("primary", parseSnapshotEnvelope(restarted)), (error) => error.code === "instance_conflict");
  now += 30_001;
  assert.deepEqual(await store.publishSnapshot("primary", parseSnapshotEnvelope(restarted)), { revision: 3, duplicate: false });
  const delayed = safeEnvelope(9);
  delayed.capturedAt = restarted.capturedAt - 1;
  delayed.snapshot.generatedAt = delayed.capturedAt;
  await assert.rejects(store.publishSnapshot("primary", parseSnapshotEnvelope(delayed)), (error) => error.code === "instance_conflict");
});

test("commands are idempotent, leased once, reclaimed, and scrubbed after acknowledgement", async () => {
  let now = 1_000;
  let sequence = 0;
  const uuid = () => `${String(++sequence).padStart(8, "0")}-0000-4000-8000-000000000000`;
  const store = new MemoryRelayStore({ now: () => now, uuid });
  const input = { type: "start_run", payload: { workspaceId: "workspace-primary", profileId: "codex-subscription", sessionName: "Run", agentName: "Agent", task: "Build", kind: "writer" } };
  const first = await store.enqueue("primary", input, "11111111-1111-4111-8111-111111111111");
  const duplicate = await store.enqueue("primary", input, "11111111-1111-4111-8111-111111111111");
  assert.equal(duplicate.commandId, first.commandId);
  const lease = (await store.lease("primary", "22222222-2222-4222-8222-222222222222"))[0];
  assert.equal((await store.lease("primary", "33333333-3333-4333-8333-333333333333")).length, 0);
  now += 31_000;
  const reclaimed = (await store.lease("primary", "33333333-3333-4333-8333-333333333333"))[0];
  assert.equal(reclaimed.commandId, lease.commandId);
  await assert.rejects(store.acknowledge("primary", "22222222-2222-4222-8222-222222222222", { commandId: lease.commandId, leaseId: lease.leaseId, status: "succeeded", errorCode: null }), /lease/);
  await store.acknowledge("primary", "33333333-3333-4333-8333-333333333333", { commandId: reclaimed.commandId, leaseId: reclaimed.leaseId, status: "succeeded", errorCode: null });
  assert.equal((await store.command("primary", first.commandId)).status, "succeeded");
});

test("running memory commands remain acknowledgeable beyond the queue deadline", async () => {
  let now = 1_000;
  let sequence = 100;
  const uuid = () => `${String(++sequence).padStart(8, "0")}-0000-4000-8000-000000000000`;
  const store = new MemoryRelayStore({ now: () => now, uuid });
  const input = { type: "start_run", payload: { workspaceId: "workspace-primary", profileId: "codex-subscription", sessionName: "Run", agentName: "Agent", task: "Build", kind: "writer" } };
  const command = await store.enqueue("primary", input, "11111111-1111-4111-8111-111111111111");
  const lease = (await store.lease("primary", "22222222-2222-4222-8222-222222222222"))[0];
  await store.acknowledge("primary", "22222222-2222-4222-8222-222222222222", { commandId: command.commandId, leaseId: lease.leaseId, status: "running", errorCode: null });
  now += 120_001;
  assert.equal((await store.command("primary", command.commandId)).status, "running");
  await store.acknowledge("primary", "22222222-2222-4222-8222-222222222222", { commandId: command.commandId, leaseId: lease.leaseId, status: "succeeded", errorCode: null });
  assert.equal((await store.command("primary", command.commandId)).status, "succeeded");
});
