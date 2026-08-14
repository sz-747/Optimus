import { ContractError } from "./contracts.js";
import { isAllowedDesktopOrigin } from "./auth.js";

const MAX_BODY_BYTES = 256 * 1_024;

export async function readJson(request, maximum = MAX_BODY_BYTES) {
  const contentType = String(request.headers?.["content-type"] || "").split(";")[0].trim().toLowerCase();
  if (contentType !== "application/json") throw new ContractError("application/json is required", "unsupported_media_type", 415);
  if (request.body && typeof request.body === "object" && !Buffer.isBuffer(request.body)) return request.body;
  let total = 0;
  const chunks = [];
  for await (const chunk of request) {
    total += chunk.length;
    if (total > maximum) throw new ContractError("request body is too large", "payload_too_large", 413);
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw new ContractError("request body is not valid JSON", "invalid_json", 400);
  }
}

export function sendJson(response, status, body, extraHeaders = {}) {
  applyApiHeaders(response, extraHeaders);
  response.statusCode = status;
  response.setHeader("Content-Type", "application/json; charset=utf-8");
  response.end(JSON.stringify(body));
}

export function sendEmpty(response, status, extraHeaders = {}) {
  applyApiHeaders(response, extraHeaders);
  response.statusCode = status;
  response.end();
}

export function sendError(response, error) {
  if (response.headersSent) {
    response.end();
    return;
  }
  const status = error instanceof ContractError ? error.status : 500;
  const code = error instanceof ContractError ? error.code : "internal_error";
  const message = error instanceof ContractError ? error.message : "relay request failed";
  sendJson(response, status, { error: { code, message } });
}

export function methodNotAllowed(response, methods) {
  sendJson(response, 405, { error: { code: "method_not_allowed", message: "method not allowed" } }, { Allow: methods.join(", ") });
}

export function handleDesktopOptions(request, response, methods) {
  const origin = isAllowedDesktopOrigin(request);
  if (!origin) {
    sendJson(response, 403, { error: { code: "bad_origin", message: "origin is not allowed" } });
    return;
  }
  sendEmpty(response, 204, {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": methods.join(", "),
    "Access-Control-Allow-Headers": "Authorization, Content-Type, X-Optimus-Device",
    "Access-Control-Max-Age": "600",
    Vary: "Origin",
  });
}

export function desktopCorsHeaders(request) {
  const origin = isAllowedDesktopOrigin(request);
  return origin ? { "Access-Control-Allow-Origin": origin, Vary: "Origin" } : {};
}

export function query(request, key) {
  const url = new URL(request.url || "/", `http://${request.headers?.host || "localhost"}`);
  return url.searchParams.get(key) || "";
}

export function applyApiHeaders(response, extra = {}) {
  const headers = {
    "Cache-Control": "private, no-store, max-age=0",
    "Content-Security-Policy": "default-src 'none'; frame-ancestors 'none'",
    "Referrer-Policy": "no-referrer",
    "X-Content-Type-Options": "nosniff",
    "X-Frame-Options": "DENY",
    ...extra,
  };
  for (const [name, value] of Object.entries(headers)) response.setHeader(name, value);
}
