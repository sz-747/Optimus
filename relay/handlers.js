import {
  authorizeDashboard,
  authorizeDesktop,
  clearDashboardCookie,
  clientKey,
  createDashboardCookie,
  header,
  requireSameOrigin,
  verifyDashboardToken,
} from "./auth.js";
import {
  ContractError,
  parseCommandInput,
  parseConsumerId,
  parseDesktopAck,
  parseDeviceId,
  parseSnapshotEnvelope,
} from "./contracts.js";
import {
  applyApiHeaders,
  desktopCorsHeaders,
  handleDesktopOptions,
  methodNotAllowed,
  query,
  readJson,
  sendEmpty,
  sendError,
  sendJson,
} from "./http.js";

export async function handleHealth(request, response) {
  if (request.method !== "GET") return methodNotAllowed(response, ["GET"]);
  sendJson(response, 200, { ok: true });
}

export async function handleAuth(request, response, store, now = Date.now) {
  try {
    if (request.method === "GET") {
      const session = authorizeDashboard(request, now());
      return sendJson(response, 200, { authenticated: true, deviceId: session.deviceId, expiresAt: session.expiresAt });
    }
    if (request.method === "DELETE") {
      requireSameOrigin(request);
      return sendEmpty(response, 204, { "Set-Cookie": clearDashboardCookie(request) });
    }
    if (request.method !== "POST") return methodNotAllowed(response, ["GET", "POST", "DELETE"]);
    requireSameOrigin(request);
    await enforceRate(store, `login:${clientKey(request)}`, 5, 600);
    const body = await readJson(request, 4_096);
    exactLogin(body);
    if (!verifyDashboardToken(body.token)) throw new ContractError("authentication required", "unauthorized", 401);
    const deviceId = parseDeviceId(body.deviceId);
    sendEmpty(response, 204, { "Set-Cookie": createDashboardCookie(deviceId, request, now()) });
  } catch (error) {
    sendError(response, error);
  }
}

export async function handleSnapshot(request, response, store, now = Date.now) {
  try {
    if (request.method !== "GET") return methodNotAllowed(response, ["GET"]);
    const session = authorizeDashboard(request, now());
    await enforceRate(store, `read:${session.deviceId}:${clientKey(request)}`, 180, 60);
    const [snapshot, desktop] = await Promise.all([
      store.snapshot(session.deviceId),
      store.presence(session.deviceId),
    ]);
    if (!snapshot) throw new ContractError("desktop snapshot is not available", "snapshot_unavailable", 404);
    sendJson(response, 200, { ...snapshot, desktop });
  } catch (error) {
    sendError(response, error);
  }
}

export async function handleDesktopSnapshot(request, response, store) {
  try {
    if (request.method === "OPTIONS") return handleDesktopOptions(request, response, ["PUT", "OPTIONS"]);
    if (request.method !== "PUT") return methodNotAllowed(response, ["PUT", "OPTIONS"]);
    const deviceId = authorizeDesktop(request);
    await enforceRate(store, `publish:${deviceId}`, 600, 60);
    const envelope = parseSnapshotEnvelope(await readJson(request));
    const result = await store.publishSnapshot(deviceId, envelope);
    sendJson(response, result.duplicate ? 200 : 202, result, desktopCorsHeaders(request));
  } catch (error) {
    sendError(response, error);
  }
}

export async function handleEvents(request, response, store, now = Date.now) {
  try {
    if (request.method !== "GET") return methodNotAllowed(response, ["GET"]);
    const session = authorizeDashboard(request, now());
    await enforceRate(store, `events:${session.deviceId}:${clientKey(request)}`, 30, 60);
    applyApiHeaders(response, {
      "Content-Type": "text/event-stream; charset=utf-8",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    });
    response.statusCode = 200;
    response.flushHeaders?.();
    response.write("retry: 1000\n\n");
    let closed = false;
    response.on("close", () => { closed = true; });
    let lastRevision = Math.max(numericHeader(request, "last-event-id"), numericQuery(request, "after"));
    let lastOnline = null;
    let heartbeatAt = now();
    const startedAt = now();
    const maximumMs = clamp(Number(process.env.OPTIMUS_SSE_MAX_MS) || 55_000, 1_000, 55_000);
    while (!closed && now() - startedAt < maximumMs) {
      const revision = await store.revision(session.deviceId);
      if (revision > lastRevision) {
        lastRevision = revision;
        response.write(`id: ${revision}\nevent: revision\ndata: ${JSON.stringify({ revision })}\n\n`);
      }
      const presence = await store.presence(session.deviceId);
      if (presence.online !== lastOnline) {
        lastOnline = presence.online;
        response.write(`event: presence\ndata: ${JSON.stringify(presence)}\n\n`);
      }
      if (now() - heartbeatAt >= 10_000) {
        response.write(`: heartbeat ${now()}\n\n`);
        heartbeatAt = now();
      }
      await delay(750);
    }
    if (!closed) response.end();
  } catch (error) {
    sendError(response, error);
  }
}

export async function handleCommands(request, response, store, now = Date.now) {
  try {
    const session = authorizeDashboard(request, now());
    if (request.method === "GET") {
      const commandId = uuid(query(request, "id"), "command id");
      const command = await store.command(session.deviceId, commandId);
      if (!command) throw new ContractError("command not found", "command_not_found", 404);
      return sendJson(response, 200, command);
    }
    if (request.method !== "POST") return methodNotAllowed(response, ["GET", "POST"]);
    requireSameOrigin(request);
    await enforceRate(store, `commands:${session.deviceId}:${clientKey(request)}`, 30, 60);
    const idempotencyKey = uuid(header(request, "idempotency-key"), "Idempotency-Key");
    const [snapshot, presence] = await Promise.all([
      store.snapshot(session.deviceId),
      store.presence(session.deviceId),
    ]);
    if (!snapshot) throw new ContractError("desktop snapshot is not available", "snapshot_unavailable", 409);
    if (!presence.online) throw new ContractError("desktop is offline", "desktop_offline", 409);
    const command = parseCommandInput(await readJson(request, 8_192), snapshot.catalog);
    sendJson(response, 202, await store.enqueue(session.deviceId, command, idempotencyKey));
  } catch (error) {
    sendError(response, error);
  }
}

export async function handleDesktopCommands(request, response, store) {
  try {
    if (request.method === "OPTIONS") return handleDesktopOptions(request, response, ["GET", "PATCH", "OPTIONS"]);
    const deviceId = authorizeDesktop(request);
    const consumerId = parseConsumerId(query(request, "consumer"));
    await enforceRate(store, `desktop-commands:${deviceId}`, 240, 60);
    if (request.method === "GET") {
      const commands = await store.lease(deviceId, consumerId, 10);
      return sendJson(response, 200, { commands }, desktopCorsHeaders(request));
    }
    if (request.method !== "PATCH") return methodNotAllowed(response, ["GET", "PATCH", "OPTIONS"]);
    const ack = parseDesktopAck(await readJson(request, 4_096));
    const command = await store.acknowledge(deviceId, consumerId, ack);
    sendJson(response, 200, command, desktopCorsHeaders(request));
  } catch (error) {
    sendError(response, error);
  }
}

async function enforceRate(store, key, limit, windowSeconds) {
  const result = await store.consumeRateLimit(key, limit, windowSeconds);
  if (!result.allowed) throw new ContractError("rate limit exceeded", "rate_limited", 429);
}

function exactLogin(body) {
  if (!body || typeof body !== "object" || Array.isArray(body)) throw new ContractError("body must be an object");
  const keys = Object.keys(body).sort().join(",");
  if (keys !== "deviceId,token" || typeof body.token !== "string" || body.token.length > 512) throw new ContractError("login body is invalid");
}

function uuid(value, label) {
  if (!/^[0-9a-f]{8}-[0-9a-f-]{27,35}$/i.test(value)) throw new ContractError(`${label} is invalid`);
  return value;
}

function numericHeader(request, name) {
  const value = Number(header(request, name));
  return Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

function numericQuery(request, name) {
  const value = Number(query(request, name));
  return Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

function clamp(value, minimum, maximum) {
  return Math.min(Math.max(value, minimum), maximum);
}

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
