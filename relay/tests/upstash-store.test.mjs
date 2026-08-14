import assert from "node:assert/strict";
import test from "node:test";

import { parseSnapshotEnvelope } from "../contracts.js";
import { UpstashRelayStore } from "../upstash-store.js";
import { safeEnvelope } from "./fixture.js";

class FakeRedis {
  constructor(now) {
    this.now = now;
    this.values = new Map();
    this.sorted = new Map();
  }

  async eval(script, keys, args) {
    if (script.includes("optimus:snapshot-cas:v1")) return this.#snapshotCas(keys, args);
    if (script.includes("optimus:enqueue:v1")) return this.#enqueue(keys, args);
    if (script.includes("optimus:acknowledge:v1")) return this.#acknowledge(keys, args);
    if (script.includes("optimus:expire-command:v1")) return this.#expireCommand(keys, args);
    if (script.includes("optimus:rate-limit:v1")) return this.#rateLimit(keys, args);
    throw new Error("unexpected script");
  }

  async get(key) {
    const row = this.values.get(key);
    if (!row) return null;
    if (row.expiresAt != null && row.expiresAt <= this.now()) {
      this.values.delete(key);
      return null;
    }
    return row.value;
  }

  async set(key, value, options = {}) {
    if (options.nx && await this.get(key) != null) return null;
    this.values.set(key, {
      value,
      expiresAt: options.ex == null ? null : this.now() + Number(options.ex) * 1_000,
    });
    return "OK";
  }

  async incr(key) {
    const next = Number(await this.get(key) || 0) + 1;
    await this.set(key, String(next));
    return next;
  }

  async expire(key, seconds) {
    const value = await this.get(key);
    if (value == null) return 0;
    this.values.set(key, { value, expiresAt: this.now() + Number(seconds) * 1_000 });
    return 1;
  }

  async del(...keys) {
    let removed = 0;
    for (const key of keys) removed += Number(this.values.delete(key));
    return removed;
  }

  async zadd(key, entry) {
    const rows = this.sorted.get(key) || new Map();
    rows.set(String(entry.member), Number(entry.score));
    this.sorted.set(key, rows);
    return 1;
  }

  async zrange(key, start, stop) {
    const rows = [...(this.sorted.get(key) || new Map())]
      .sort((left, right) => left[1] - right[1] || left[0].localeCompare(right[0]))
      .map(([member]) => member);
    return rows.slice(start, stop === -1 ? undefined : stop + 1);
  }

  async zrem(key, ...members) {
    const rows = this.sorted.get(key);
    if (!rows) return 0;
    let removed = 0;
    for (const member of members) removed += Number(rows.delete(String(member)));
    return removed;
  }

  multi() {
    const operations = [];
    const chain = {};
    for (const name of ["set", "del", "expire", "zadd", "zrem"]) {
      chain[name] = (...args) => {
        operations.push(() => this[name](...args));
        return chain;
      };
    }
    chain.exec = async () => {
      const results = [];
      for (const operation of operations) results.push(await operation());
      return results;
    };
    return chain;
  }

  async #snapshotCas(keys, args) {
    const current = decode(await this.get(keys[0]));
    const sourceRevision = Number(args[1]);
    const capturedAt = Number(args[2]);
    if (current) {
      if (current.instanceId === args[0]) {
        if (sourceRevision < current.sourceRevision) return [-1, current.revision];
        if (sourceRevision === current.sourceRevision) {
          await this.set(keys[3], args[3], { ex: Number(args[5]) });
          return [0, current.revision];
        }
      } else if (await this.get(keys[3])) {
        return [-2, current.revision];
      }
    }
    const revision = await this.incr(keys[1]);
    const snapshot = JSON.parse(args[4]);
    snapshot.revision = revision;
    snapshot.publishedAt = Number(args[3]);
    await this.set(keys[0], JSON.stringify({ instanceId: args[0], sourceRevision, capturedAt, revision }), { ex: Number(args[6]) });
    await this.set(keys[2], JSON.stringify(snapshot), { ex: Number(args[6]) });
    await this.set(keys[3], args[3], { ex: Number(args[5]) });
    return [1, revision];
  }

  async #enqueue(keys, args) {
    const existing = await this.get(keys[0]);
    if (existing) return [0, existing];
    const cutoff = Number(args[2]) - Number(args[7]);
    const rows = this.sorted.get(keys[3]) || new Map();
    for (const [member, score] of rows) if (score <= cutoff) rows.delete(member);
    if (rows.size >= Number(args[6])) return [-1, ""];
    await this.set(keys[1], args[3], { ex: Number(args[8]) });
    await this.set(keys[2], args[4], { ex: Number(args[5]) });
    await this.zadd(keys[3], { score: Number(args[2]), member: args[0] });
    await this.set(keys[0], args[0], { ex: Number(args[9]) });
    return [1, args[0]];
  }

  async #acknowledge(keys, args) {
    const command = decode(await this.get(keys[0]));
    if (!command || command.deviceId !== args[0]) return [-1, ""];
    const lease = decode(await this.get(keys[1]));
    if (!lease || lease.consumerId !== args[1] || lease.leaseId !== args[2]) return [-2, ""];
    command.status = args[3];
    command.errorCode = args[4] || null;
    command.updatedAt = Number(args[5]);
    if (["succeeded", "failed", "rejected", "expired"].includes(command.status)) {
      command.leaseId = null;
      command.leaseOwner = null;
      command.leaseUntil = null;
      await this.zrem(keys[2], command.commandId);
      await this.del(keys[3], keys[1]);
    } else {
      command.expiresAt = Number(args[5]) + Number(args[7]) * 1_000;
      command.leaseUntil = command.expiresAt;
      await this.set(keys[1], JSON.stringify(lease), { ex: Number(args[7]) });
    }
    const encoded = JSON.stringify(command);
    await this.set(keys[0], encoded, { ex: Number(args[6]) });
    return [1, encoded];
  }

  async #expireCommand(keys, args) {
    const command = decode(await this.get(keys[0]));
    if (!command) return [0, ""];
    if (["succeeded", "failed", "rejected", "expired"].includes(command.status) || command.expiresAt > Number(args[0])) {
      return [0, JSON.stringify(command)];
    }
    Object.assign(command, { status: "expired", errorCode: "command_expired", updatedAt: Number(args[0]), leaseId: null, leaseOwner: null, leaseUntil: null });
    await this.zrem(keys[1], command.commandId);
    await this.del(keys[2], keys[3]);
    const encoded = JSON.stringify(command);
    await this.set(keys[0], encoded, { ex: Number(args[1]) });
    return [1, encoded];
  }

  async #rateLimit(keys, args) {
    const count = await this.incr(keys[0]);
    if (count === 1) await this.expire(keys[0], Number(args[0]));
    return count;
  }
}

test("Upstash adapter atomically rejects stale snapshots and refreshes duplicate presence", async () => {
  let now = 1_000;
  const redis = new FakeRedis(() => now);
  const store = new UpstashRelayStore(redis, { now: () => now });
  assert.deepEqual(await store.publishSnapshot("primary", parseSnapshotEnvelope(safeEnvelope(7))), { revision: 1, duplicate: false });
  now = 5_000;
  assert.deepEqual(await store.publishSnapshot("primary", parseSnapshotEnvelope(safeEnvelope(7))), { revision: 1, duplicate: true });
  assert.deepEqual(await store.presence("primary"), { online: true, lastSeenAt: 5_000 });
  await assert.rejects(store.publishSnapshot("primary", parseSnapshotEnvelope(safeEnvelope(6))), (error) => error.code === "stale_revision");
  assert.equal((await store.snapshot("primary")).revision, 1);

  const restarted = safeEnvelope(1);
  restarted.instanceId = "22222222-2222-4222-8222-222222222222";
  restarted.capturedAt = 1_750_000_010_000;
  restarted.snapshot.generatedAt = restarted.capturedAt;
  await assert.rejects(store.publishSnapshot("primary", parseSnapshotEnvelope(restarted)), (error) => error.code === "instance_conflict");
  now = 35_001;
  assert.deepEqual(await store.publishSnapshot("primary", parseSnapshotEnvelope(restarted)), { revision: 2, duplicate: false });
  const delayedOldInstance = safeEnvelope(8);
  delayedOldInstance.capturedAt = restarted.capturedAt - 1;
  delayedOldInstance.snapshot.generatedAt = delayedOldInstance.capturedAt;
  await assert.rejects(store.publishSnapshot("primary", parseSnapshotEnvelope(delayedOldInstance)), (error) => error.code === "instance_conflict");

  now = 70_002;
  assert.deepEqual(await store.presence("primary"), { online: false, lastSeenAt: 35_001 });
});

test("Upstash adapter atomically enqueues idempotently and expires raw task payload after two minutes", async () => {
  let now = 1_000;
  let sequence = 0;
  const uuid = () => `${String(++sequence).padStart(8, "0")}-0000-4000-8000-000000000000`;
  const redis = new FakeRedis(() => now);
  const store = new UpstashRelayStore(redis, { now: () => now, uuid });
  const input = startInput("PRIVATE_TASK_SENTINEL");
  const idempotencyKey = "11111111-1111-4111-8111-111111111111";
  const first = await store.enqueue("primary", input, idempotencyKey);
  const duplicate = await store.enqueue("primary", input, idempotencyKey);
  assert.equal(duplicate.commandId, first.commandId);
  const commandRaw = await redis.get(`op:cp:v1:primary:command:${first.commandId}`);
  assert.ok(!commandRaw.includes("PRIVATE_TASK_SENTINEL"));
  assert.ok((await redis.get(`op:cp:v1:primary:payload:${first.commandId}`)).includes("PRIVATE_TASK_SENTINEL"));

  now += 120_001;
  assert.equal((await store.command("primary", first.commandId)).status, "expired");
  assert.equal(await redis.get(`op:cp:v1:primary:payload:${first.commandId}`), null);
  assert.deepEqual(await redis.zrange("op:cp:v1:primary:commands", 0, -1), []);
});

test("Upstash adapter removes more than fifty dead queue entries before leasing live work", async () => {
  let now = 10_000;
  let sequence = 100;
  const uuid = () => `${String(++sequence).padStart(8, "0")}-0000-4000-8000-000000000000`;
  const redis = new FakeRedis(() => now);
  for (let index = 0; index < 60; index += 1) {
    await redis.zadd("op:cp:v1:primary:commands", { score: now + index, member: `dead-${index}` });
  }
  const store = new UpstashRelayStore(redis, { now: () => now, uuid });
  const command = await store.enqueue("primary", startInput("Build it"), "11111111-1111-4111-8111-111111111111");
  const leased = await store.lease("primary", "22222222-2222-4222-8222-222222222222");
  assert.equal(leased.length, 1);
  assert.equal(leased[0].commandId, command.commandId);
  assert.equal(leased[0].payload.task, "Build it");
  assert.deepEqual(await redis.zrange("op:cp:v1:primary:commands", 0, -1), [command.commandId]);
});

test("Upstash adapter scrubs payload on terminal acknowledgement", async () => {
  let now = 1_000;
  let sequence = 200;
  const uuid = () => `${String(++sequence).padStart(8, "0")}-0000-4000-8000-000000000000`;
  const redis = new FakeRedis(() => now);
  const store = new UpstashRelayStore(redis, { now: () => now, uuid });
  const command = await store.enqueue("primary", startInput("Build it"), "11111111-1111-4111-8111-111111111111");
  const lease = (await store.lease("primary", "22222222-2222-4222-8222-222222222222"))[0];
  const result = await store.acknowledge("primary", "22222222-2222-4222-8222-222222222222", {
    commandId: command.commandId,
    leaseId: lease.leaseId,
    status: "succeeded",
    errorCode: null,
  });
  assert.equal(result.status, "succeeded");
  assert.equal(await redis.get(`op:cp:v1:primary:payload:${command.commandId}`), null);
  await assert.rejects(
    store.acknowledge("primary", "22222222-2222-4222-8222-222222222222", { commandId: command.commandId, leaseId: lease.leaseId, status: "running", errorCode: null }),
    (error) => error.code === "lease_mismatch",
  );
});

test("running acknowledgement extends the command and lease beyond the queue deadline", async () => {
  let now = 1_000;
  let sequence = 300;
  const uuid = () => `${String(++sequence).padStart(8, "0")}-0000-4000-8000-000000000000`;
  const redis = new FakeRedis(() => now);
  const store = new UpstashRelayStore(redis, { now: () => now, uuid });
  const command = await store.enqueue("primary", startInput("Build it"), "11111111-1111-4111-8111-111111111111");
  const lease = (await store.lease("primary", "22222222-2222-4222-8222-222222222222"))[0];
  const running = await store.acknowledge("primary", "22222222-2222-4222-8222-222222222222", {
    commandId: command.commandId,
    leaseId: lease.leaseId,
    status: "running",
    errorCode: null,
  });
  now += 120_001;
  assert.equal((await store.command("primary", command.commandId)).status, "running");
  assert.ok(running.expiresAt > now);
  const succeeded = await store.acknowledge("primary", "22222222-2222-4222-8222-222222222222", {
    commandId: command.commandId,
    leaseId: lease.leaseId,
    status: "succeeded",
    errorCode: null,
  });
  assert.equal(succeeded.status, "succeeded");
});

function startInput(task) {
  return {
    type: "start_run",
    payload: {
      workspaceId: "workspace-primary",
      profileId: "codex-subscription",
      sessionName: "Run",
      agentName: "Agent",
      task,
      kind: "writer",
    },
  };
}

function decode(value) {
  if (value == null || typeof value !== "string") return value;
  try { return JSON.parse(value); } catch { return value; }
}
