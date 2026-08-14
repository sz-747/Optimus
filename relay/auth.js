import { createHmac, createHash, timingSafeEqual } from "node:crypto";
import { ContractError, parseDeviceId } from "./contracts.js";

const COOKIE_NAME = "optimus_dashboard";
const SESSION_AGE_MS = 12 * 60 * 60 * 1_000;

export function verifyDashboardToken(candidate) {
  return verifySecret(candidate, process.env.OPTIMUS_DASHBOARD_TOKEN);
}

export function authorizeDesktop(request) {
  const token = bearerToken(request);
  if (!verifySecret(token, process.env.OPTIMUS_DESKTOP_TOKEN)) {
    throw new ContractError("authentication required", "unauthorized", 401);
  }
  return parseDeviceId(header(request, "x-optimus-device"));
}

export function authorizeDashboard(request, now = Date.now()) {
  const token = process.env.OPTIMUS_DASHBOARD_TOKEN;
  if (!token) throw new ContractError("relay authentication is not configured", "not_configured", 503);
  const encoded = cookieValue(request, COOKIE_NAME);
  if (!encoded) throw new ContractError("authentication required", "unauthorized", 401);
  const [payload, signature] = encoded.split(".");
  if (!payload || !signature || !safeEqual(signature, sign(payload, token))) {
    throw new ContractError("authentication required", "unauthorized", 401);
  }
  let session;
  try {
    session = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
  } catch {
    throw new ContractError("authentication required", "unauthorized", 401);
  }
  if (!Number.isSafeInteger(session.expiresAt) || session.expiresAt <= now) {
    throw new ContractError("session expired", "session_expired", 401);
  }
  return { deviceId: parseDeviceId(session.deviceId), expiresAt: session.expiresAt };
}

export function createDashboardCookie(deviceId, request, now = Date.now()) {
  const secret = process.env.OPTIMUS_DASHBOARD_TOKEN;
  if (!secret) throw new ContractError("relay authentication is not configured", "not_configured", 503);
  const payload = Buffer.from(JSON.stringify({ deviceId: parseDeviceId(deviceId), expiresAt: now + SESSION_AGE_MS })).toString("base64url");
  const secure = isSecureRequest(request) ? "; Secure" : "";
  return `${COOKIE_NAME}=${payload}.${sign(payload, secret)}; Path=/; Max-Age=${Math.floor(SESSION_AGE_MS / 1_000)}; HttpOnly; SameSite=Strict${secure}`;
}

export function clearDashboardCookie(request) {
  const secure = isSecureRequest(request) ? "; Secure" : "";
  return `${COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict${secure}`;
}

export function requireSameOrigin(request) {
  const origin = header(request, "origin");
  if (!origin) return;
  const forwardedHost = header(request, "x-forwarded-host") || header(request, "host");
  const forwardedProto = header(request, "x-forwarded-proto") || "http";
  if (!forwardedHost || origin !== `${forwardedProto}://${forwardedHost}`) {
    throw new ContractError("cross-origin mutation rejected", "bad_origin", 403);
  }
}

export function isAllowedDesktopOrigin(request) {
  const origin = header(request, "origin");
  if (!origin) return null;
  const configured = (process.env.OPTIMUS_DESKTOP_ORIGINS || "tauri://localhost,http://tauri.localhost")
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
  return configured.includes(origin) ? origin : null;
}

export function clientKey(request) {
  const forwarded = header(request, "x-forwarded-for").split(",")[0].trim();
  const source = forwarded || request.socket?.remoteAddress || "unknown";
  return createHash("sha256").update(source).digest("hex").slice(0, 24);
}

export function header(request, name) {
  const value = request.headers?.[name] ?? request.headers?.[name.toLowerCase()];
  return Array.isArray(value) ? value[0] || "" : String(value || "");
}

function bearerToken(request) {
  const authorization = header(request, "authorization");
  const match = /^Bearer ([^\s]+)$/.exec(authorization);
  return match?.[1] || "";
}

function verifySecret(candidate, expected) {
  if (!candidate || !expected || expected.length < 24) return false;
  return safeEqual(hash(candidate), hash(expected));
}

function hash(value) {
  return createHash("sha256").update(value).digest("base64url");
}

function sign(value, secret) {
  return createHmac("sha256", secret).update(value).digest("base64url");
}

function safeEqual(left, right) {
  const a = Buffer.from(String(left));
  const b = Buffer.from(String(right));
  return a.length === b.length && timingSafeEqual(a, b);
}

function cookieValue(request, name) {
  const cookies = header(request, "cookie").split(";");
  for (const cookie of cookies) {
    const [key, ...rest] = cookie.trim().split("=");
    if (key === name) return rest.join("=");
  }
  return "";
}

function isSecureRequest(request) {
  return header(request, "x-forwarded-proto") === "https" || Boolean(process.env.VERCEL);
}
