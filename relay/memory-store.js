import { randomUUID } from "node:crypto";
import { ContractError, normalizeRemoteSnapshot } from "./contracts.js";

const SNAPSHOT_TTL_MS = 24 * 60 * 60 * 1_000;
const COMMAND_TTL_MS = 2 * 60 * 1_000;
const RUNNING_COMMAND_TTL_MS = 15 * 60 * 1_000;
const LEASE_TTL_MS = 30 * 1_000;
const IDEMPOTENCY_TTL_MS = 10 * 60 * 1_000;
const MAX_QUEUE = 1_000;

export class MemoryRelayStore {
  constructor({ now = () => Date.now(), uuid = () => randomUUID() } = {}) {
    this.now = now;
    this.uuid = uuid;
    this.snapshots = new Map();
    this.presences = new Map();
    this.revisions = new Map();
    this.commands = new Map();
    this.idempotency = new Map();
    this.rateLimits = new Map();
  }

  async publishSnapshot(deviceId, envelope) {
    const now = this.now();
    const current = this.snapshots.get(deviceId);
    if (current && current.expiresAt > now) {
      if (current.instanceId === envelope.instanceId) {
        if (envelope.sourceRevision < current.sourceRevision) {
          throw new ContractError("snapshot revision is stale", "stale_revision", 409);
        }
        if (envelope.sourceRevision === current.sourceRevision) {
          this.presences.set(deviceId, now);
          return { revision: current.revision, duplicate: true };
        }
      } else if (this.presences.has(deviceId) && now - this.presences.get(deviceId) < LEASE_TTL_MS) {
        throw new ContractError("another desktop instance is online", "instance_conflict", 409);
      }
    }
    const revision = (this.revisions.get(deviceId) || 0) + 1;
    const snapshot = normalizeRemoteSnapshot(envelope, revision, now);
    this.revisions.set(deviceId, revision);
    this.snapshots.set(deviceId, {
      revision,
      instanceId: envelope.instanceId,
      sourceRevision: envelope.sourceRevision,
      capturedAt: envelope.capturedAt,
      snapshot,
      publishedAt: now,
      expiresAt: now + SNAPSHOT_TTL_MS,
    });
    this.presences.set(deviceId, now);
    return { revision, duplicate: false };
  }

  async snapshot(deviceId) {
    const row = this.snapshots.get(deviceId);
    if (!row || row.expiresAt <= this.now()) return null;
    return structuredClone(row.snapshot);
  }

  async presence(deviceId) {
    const lastSeenAt = this.presences.get(deviceId) ?? this.snapshots.get(deviceId)?.publishedAt ?? null;
    return {
      online: lastSeenAt != null && this.now() - lastSeenAt < LEASE_TTL_MS,
      lastSeenAt,
    };
  }

  async revision(deviceId) {
    return this.revisions.get(deviceId) || 0;
  }

  async enqueue(deviceId, input, idempotencyKey) {
    const now = this.now();
    this.#prune(now);
    const key = `${deviceId}:${idempotencyKey}`;
    const existingId = this.idempotency.get(key)?.commandId;
    if (existingId) return this.#publicCommand(this.commands.get(existingId));
    const queued = [...this.commands.values()].filter((command) => command.deviceId === deviceId && !terminal(command.status) && command.expiresAt > now);
    if (queued.length >= MAX_QUEUE) throw new ContractError("command queue is full", "queue_full", 429);
    const command = {
      commandId: this.uuid(),
      deviceId,
      type: input.type,
      payload: structuredClone(input.payload),
      status: "queued",
      issuedAt: now,
      expiresAt: now + COMMAND_TTL_MS,
      leaseId: null,
      leaseOwner: null,
      leaseUntil: null,
      errorCode: null,
      updatedAt: now,
    };
    this.commands.set(command.commandId, command);
    this.idempotency.set(key, { commandId: command.commandId, expiresAt: now + IDEMPOTENCY_TTL_MS });
    return this.#publicCommand(command);
  }

  async command(deviceId, commandId) {
    const command = this.commands.get(commandId);
    return command?.deviceId === deviceId ? this.#publicCommand(command) : null;
  }

  async lease(deviceId, consumerId, limit = 10) {
    const now = this.now();
    this.#prune(now);
    const leased = [];
    const candidates = [...this.commands.values()]
      .filter((command) => command.deviceId === deviceId && command.expiresAt > now && (command.status === "queued" || (command.status === "leased" && command.leaseUntil <= now)))
      .sort((left, right) => left.issuedAt - right.issuedAt);
    for (const command of candidates.slice(0, Math.min(limit, 25))) {
      command.status = "leased";
      command.leaseId = this.uuid();
      command.leaseOwner = consumerId;
      command.leaseUntil = now + LEASE_TTL_MS;
      command.updatedAt = now;
      leased.push(structuredClone(command));
    }
    return leased;
  }

  async acknowledge(deviceId, consumerId, ack) {
    const command = this.commands.get(ack.commandId);
    if (!command || command.deviceId !== deviceId) throw new ContractError("command not found", "command_not_found", 404);
    if (command.leaseId !== ack.leaseId || command.leaseOwner !== consumerId || command.leaseUntil <= this.now()) {
      throw new ContractError("command lease is not owned by this consumer", "lease_mismatch", 409);
    }
    command.status = ack.status;
    command.errorCode = ack.errorCode;
    command.updatedAt = this.now();
    if (terminal(command.status)) {
      command.payload = null;
      command.leaseId = null;
      command.leaseOwner = null;
      command.leaseUntil = null;
    } else {
      command.expiresAt = this.now() + RUNNING_COMMAND_TTL_MS;
      command.leaseUntil = command.expiresAt;
    }
    return this.#publicCommand(command);
  }

  async consumeRateLimit(key, limit, windowSeconds) {
    const now = this.now();
    const row = this.rateLimits.get(key);
    if (!row || row.expiresAt <= now) {
      this.rateLimits.set(key, { count: 1, expiresAt: now + windowSeconds * 1_000 });
      return { allowed: true, remaining: limit - 1 };
    }
    row.count += 1;
    return { allowed: row.count <= limit, remaining: Math.max(0, limit - row.count) };
  }

  #publicCommand(command) {
    if (!command) return null;
    return {
      commandId: command.commandId,
      type: command.type,
      status: command.status,
      issuedAt: command.issuedAt,
      expiresAt: command.expiresAt,
      updatedAt: command.updatedAt,
      errorCode: command.errorCode,
    };
  }

  #prune(now) {
    for (const [key, value] of this.idempotency) if (value.expiresAt <= now) this.idempotency.delete(key);
    for (const command of this.commands.values()) {
      if (command.expiresAt <= now && !terminal(command.status)) {
        command.status = "expired";
        command.payload = null;
        command.updatedAt = now;
      }
    }
  }
}

function terminal(status) {
  return new Set(["succeeded", "failed", "rejected", "expired"]).has(status);
}
