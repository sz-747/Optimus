import { randomUUID } from "node:crypto";
import { Redis } from "@upstash/redis";
import { ContractError, normalizeRemoteSnapshot } from "./contracts.js";
import { RedisUrlAdapter } from "./redis-url-adapter.js";

const PREFIX = "op:cp:v1";
const SNAPSHOT_TTL_SECONDS = 86_400;
const PRESENCE_TTL_SECONDS = 30;
const COMMAND_TTL_SECONDS = 120;
const RUNNING_COMMAND_TTL_SECONDS = 15 * 60;
const COMMAND_STATUS_TTL_SECONDS = 86_400;
const IDEMPOTENCY_TTL_SECONDS = 600;
const LEASE_TTL_SECONDS = 30;
const MAX_QUEUE = 1_000;

export const SNAPSHOT_CAS_SCRIPT = `-- optimus:snapshot-cas:v1
local source = redis.call("GET", KEYS[1])
if source then
  local current = cjson.decode(source)
  if current.instanceId == ARGV[1] then
    local incoming = tonumber(ARGV[2])
    local previous = tonumber(current.sourceRevision)
    if incoming < previous then return {-1, tonumber(current.revision)} end
    if incoming == previous then
      redis.call("SET", KEYS[4], ARGV[4], "EX", ARGV[6])
      return {0, tonumber(current.revision)}
    end
  elseif redis.call("GET", KEYS[4]) then
    return {-2, tonumber(current.revision)}
  end
end
local revision = redis.call("INCR", KEYS[2])
local snapshot = cjson.decode(ARGV[5])
snapshot.revision = revision
snapshot.publishedAt = tonumber(ARGV[4])
redis.call("SET", KEYS[1], cjson.encode({instanceId=ARGV[1], sourceRevision=tonumber(ARGV[2]), capturedAt=tonumber(ARGV[3]), revision=revision}), "EX", ARGV[7])
redis.call("SET", KEYS[3], cjson.encode(snapshot), "EX", ARGV[7])
redis.call("SET", KEYS[4], ARGV[4], "EX", ARGV[6])
return {1, revision}`;

export const ENQUEUE_SCRIPT = `-- optimus:enqueue:v1
local existing = redis.call("GET", KEYS[1])
if existing then return {0, existing} end
redis.call("ZREMRANGEBYSCORE", KEYS[4], "-inf", tonumber(ARGV[3]) - tonumber(ARGV[8]))
if redis.call("ZCARD", KEYS[4]) >= tonumber(ARGV[7]) then return {-1, ""} end
redis.call("SET", KEYS[2], ARGV[4], "EX", ARGV[9])
redis.call("SET", KEYS[3], ARGV[5], "EX", ARGV[6])
redis.call("ZADD", KEYS[4], ARGV[3], ARGV[1])
redis.call("SET", KEYS[1], ARGV[1], "EX", ARGV[10])
return {1, ARGV[1]}`;

export const ACKNOWLEDGE_SCRIPT = `-- optimus:acknowledge:v1
local rawCommand = redis.call("GET", KEYS[1])
if not rawCommand then return {-1, ""} end
local command = cjson.decode(rawCommand)
if command.deviceId ~= ARGV[1] then return {-1, ""} end
local rawLease = redis.call("GET", KEYS[2])
if not rawLease then return {-2, ""} end
local lease = cjson.decode(rawLease)
if lease.consumerId ~= ARGV[2] or lease.leaseId ~= ARGV[3] then return {-2, ""} end
command.status = ARGV[4]
command.errorCode = ARGV[5] == "" and cjson.null or ARGV[5]
command.updatedAt = tonumber(ARGV[6])
local isTerminal = ARGV[4] == "succeeded" or ARGV[4] == "failed" or ARGV[4] == "rejected" or ARGV[4] == "expired"
if isTerminal then
  command.leaseId = cjson.null
  command.leaseOwner = cjson.null
  command.leaseUntil = cjson.null
  redis.call("ZREM", KEYS[3], command.commandId)
  redis.call("DEL", KEYS[4])
  redis.call("DEL", KEYS[2])
else
  command.expiresAt = tonumber(ARGV[6]) + tonumber(ARGV[8]) * 1000
  command.leaseUntil = command.expiresAt
  redis.call("SET", KEYS[2], rawLease, "EX", ARGV[8])
end
local encoded = cjson.encode(command)
redis.call("SET", KEYS[1], encoded, "EX", ARGV[7])
return {1, encoded}`;

export const EXPIRE_COMMAND_SCRIPT = `-- optimus:expire-command:v1
local rawCommand = redis.call("GET", KEYS[1])
if not rawCommand then return {0, ""} end
local command = cjson.decode(rawCommand)
local isTerminal = command.status == "succeeded" or command.status == "failed" or command.status == "rejected" or command.status == "expired"
if isTerminal or tonumber(command.expiresAt) > tonumber(ARGV[1]) then return {0, rawCommand} end
command.status = "expired"
command.errorCode = "command_expired"
command.updatedAt = tonumber(ARGV[1])
command.leaseId = cjson.null
command.leaseOwner = cjson.null
command.leaseUntil = cjson.null
redis.call("ZREM", KEYS[2], command.commandId)
redis.call("DEL", KEYS[3])
redis.call("DEL", KEYS[4])
local encoded = cjson.encode(command)
redis.call("SET", KEYS[1], encoded, "EX", ARGV[2])
return {1, encoded}`;

export const RATE_LIMIT_SCRIPT = `-- optimus:rate-limit:v1
local count = redis.call("INCR", KEYS[1])
if count == 1 then redis.call("EXPIRE", KEYS[1], ARGV[1]) end
return count`;

export class UpstashRelayStore {
  constructor(redis = redisFromEnvironment(), { now = () => Date.now(), uuid = () => randomUUID() } = {}) {
    this.redis = redis;
    this.now = now;
    this.uuid = uuid;
  }

  async publishSnapshot(deviceId, envelope) {
    const now = this.now();
    const candidate = normalizeRemoteSnapshot(envelope, 0, now);
    const result = await this.redis.eval(
      SNAPSHOT_CAS_SCRIPT,
      [key(deviceId, "source"), key(deviceId, "revision"), key(deviceId, "snapshot"), key(deviceId, "presence")],
      [
        envelope.instanceId,
        String(envelope.sourceRevision),
        String(envelope.capturedAt),
        String(now),
        JSON.stringify(candidate),
        String(PRESENCE_TTL_SECONDS),
        String(SNAPSHOT_TTL_SECONDS),
      ],
    );
    const disposition = Number(result?.[0]);
    const revision = Number(result?.[1]);
    if (disposition === -1) throw new ContractError("snapshot revision is stale", "stale_revision", 409);
    if (disposition === -2) throw new ContractError("another desktop instance is online", "instance_conflict", 409);
    if (!Number.isSafeInteger(revision) || revision < 1) throw new Error("invalid snapshot CAS result");
    return { revision, duplicate: disposition === 0 };
  }

  async snapshot(deviceId) {
    return decode(await this.redis.get(key(deviceId, "snapshot")));
  }

  async presence(deviceId) {
    const [rawPresence, snapshot] = await Promise.all([
      this.redis.get(key(deviceId, "presence")),
      this.snapshot(deviceId),
    ]);
    const liveAt = rawPresence == null ? Number.NaN : Number(rawPresence);
    return {
      online: Number.isFinite(liveAt) && this.now() - liveAt < PRESENCE_TTL_SECONDS * 1_000,
      lastSeenAt: Number.isFinite(liveAt) ? liveAt : snapshot?.publishedAt ?? null,
    };
  }

  async revision(deviceId) {
    return Number(await this.redis.get(key(deviceId, "revision"))) || 0;
  }

  async enqueue(deviceId, input, idempotencyKey) {
    const commandId = this.uuid();
    const now = this.now();
    const command = {
      commandId,
      deviceId,
      type: input.type,
      status: "queued",
      issuedAt: now,
      expiresAt: now + COMMAND_TTL_SECONDS * 1_000,
      leaseId: null,
      leaseOwner: null,
      leaseUntil: null,
      errorCode: null,
      updatedAt: now,
    };
    const result = await this.redis.eval(
      ENQUEUE_SCRIPT,
      [
        key(deviceId, `idempotency:${idempotencyKey}`),
        key(deviceId, `command:${commandId}`),
        key(deviceId, `payload:${commandId}`),
        key(deviceId, "commands"),
      ],
      [
        commandId,
        deviceId,
        String(now),
        JSON.stringify(command),
        JSON.stringify(input.payload),
        String(COMMAND_TTL_SECONDS),
        String(MAX_QUEUE),
        String(COMMAND_TTL_SECONDS * 1_000),
        String(COMMAND_STATUS_TTL_SECONDS),
        String(IDEMPOTENCY_TTL_SECONDS),
      ],
    );
    const disposition = Number(result?.[0]);
    const acceptedId = String(result?.[1] || "");
    if (disposition === -1) throw new ContractError("command queue is full", "queue_full", 429);
    if (disposition === 0) {
      const existing = await this.command(deviceId, acceptedId);
      if (!existing) throw new ContractError("idempotent command is unavailable", "command_unavailable", 409);
      return existing;
    }
    if (acceptedId !== commandId) throw new Error("invalid enqueue CAS result");
    return publicCommand(command);
  }

  async command(deviceId, commandId) {
    const command = decode(await this.redis.get(key(deviceId, `command:${commandId}`)));
    if (!command || command.deviceId !== deviceId) return null;
    if (command.expiresAt <= this.now() && !terminal(command.status)) {
      await this.#expireCommand(deviceId, command);
    }
    return publicCommand(command);
  }

  async lease(deviceId, consumerId, limit = 10) {
    const now = this.now();
    const queueKey = key(deviceId, "commands");
    const ids = await this.redis.zrange(queueKey, 0, -1);
    const leased = [];
    for (const commandId of ids) {
      if (leased.length >= Math.min(limit, 25)) break;
      const commandKey = key(deviceId, `command:${commandId}`);
      const payloadKey = key(deviceId, `payload:${commandId}`);
      const leaseKey = key(deviceId, `lease:${commandId}`);
      const command = decode(await this.redis.get(commandKey));
      if (!command) {
        await this.#removeQueueEntry(queueKey, commandId, payloadKey, leaseKey);
        continue;
      }
      if (command.expiresAt <= now || terminal(command.status)) {
        if (!terminal(command.status)) await this.#expireCommand(deviceId, command);
        else await this.#removeQueueEntry(queueKey, commandId, payloadKey, leaseKey);
        continue;
      }
      const leaseId = this.uuid();
      const won = await this.redis.set(
        leaseKey,
        JSON.stringify({ consumerId, leaseId }),
        { nx: true, ex: LEASE_TTL_SECONDS },
      );
      if (!won) continue;
      const payload = decode(await this.redis.get(payloadKey));
      if (!payload) {
        await this.#expireCommand(deviceId, command);
        continue;
      }
      Object.assign(command, {
        status: "leased",
        leaseId,
        leaseOwner: consumerId,
        leaseUntil: now + LEASE_TTL_SECONDS * 1_000,
        updatedAt: now,
      });
      await this.redis.set(commandKey, JSON.stringify(command), { ex: COMMAND_STATUS_TTL_SECONDS });
      leased.push({ ...command, payload });
    }
    return leased;
  }

  async acknowledge(deviceId, consumerId, ack) {
    const commandKey = key(deviceId, `command:${ack.commandId}`);
    const leaseKey = key(deviceId, `lease:${ack.commandId}`);
    const result = await this.redis.eval(
      ACKNOWLEDGE_SCRIPT,
      [commandKey, leaseKey, key(deviceId, "commands"), key(deviceId, `payload:${ack.commandId}`)],
      [
        deviceId,
        consumerId,
        ack.leaseId,
        ack.status,
        ack.errorCode || "",
        String(this.now()),
        String(COMMAND_STATUS_TTL_SECONDS),
        String(RUNNING_COMMAND_TTL_SECONDS),
      ],
    );
    const disposition = Number(result?.[0]);
    if (disposition === -1) throw new ContractError("command not found", "command_not_found", 404);
    if (disposition === -2) throw new ContractError("command lease is not owned by this consumer", "lease_mismatch", 409);
    const command = decode(result?.[1]);
    if (disposition !== 1 || !command) throw new Error("invalid acknowledgement result");
    return publicCommand(command);
  }

  async consumeRateLimit(rateKey, limit, windowSeconds) {
    const redisKey = `${PREFIX}:rate:${rateKey}`;
    const count = Number(await this.redis.eval(RATE_LIMIT_SCRIPT, [redisKey], [String(windowSeconds)]));
    return { allowed: count <= limit, remaining: Math.max(0, limit - count) };
  }

  async #expireCommand(deviceId, command) {
    const result = await this.redis.eval(
      EXPIRE_COMMAND_SCRIPT,
      [
        key(deviceId, `command:${command.commandId}`),
        key(deviceId, "commands"),
        key(deviceId, `payload:${command.commandId}`),
        key(deviceId, `lease:${command.commandId}`),
      ],
      [String(this.now()), String(COMMAND_STATUS_TTL_SECONDS)],
    );
    const current = decode(result?.[1]);
    if (current) Object.assign(command, current);
  }

  async #removeQueueEntry(queueKey, commandId, payloadKey, leaseKey) {
    const transaction = this.redis.multi();
    transaction.zrem(queueKey, commandId);
    transaction.del(payloadKey);
    transaction.del(leaseKey);
    await transaction.exec();
  }
}

export function redisFromEnvironment() {
  const url = process.env.UPSTASH_REDIS_REST_URL || process.env.KV_REST_API_URL;
  const token = process.env.UPSTASH_REDIS_REST_TOKEN || process.env.KV_REST_API_TOKEN;
  if (url && token) return new Redis({ url, token });
  if (process.env.REDIS_URL) return new RedisUrlAdapter(process.env.REDIS_URL);
  throw new ContractError("relay storage is not configured", "not_configured", 503);
}

function key(deviceId, suffix) {
  return `${PREFIX}:${deviceId}:${suffix}`;
}

function decode(value) {
  if (value == null) return null;
  if (typeof value === "string") {
    try { return JSON.parse(value); } catch { return value; }
  }
  return value;
}

function publicCommand(command) {
  if (!command) return null;
  const { commandId, type, status, issuedAt, expiresAt, updatedAt, errorCode } = command;
  return { commandId, type, status, issuedAt, expiresAt, updatedAt, errorCode };
}

function terminal(status) {
  return ["succeeded", "failed", "rejected", "expired"].includes(status);
}
