import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, resolve, sep } from "node:path";
import { MemoryRelayStore } from "../../relay/memory-store.js";
import {
  handleAuth,
  handleCommands,
  handleDesktopCommands,
  handleDesktopSnapshot,
  handleEvents,
  handleHealth,
  handleSnapshot,
} from "../../relay/handlers.js";

const host = "127.0.0.1";
const port = 4174;
const dist = resolve("ui/dist");
let relayTimeOffset = 0;
const store = new MemoryRelayStore({ now: () => Date.now() + relayTimeOffset });
const metrics = { activeSse: 0, maxActiveSse: 0, totalSse: 0 };

process.env.OPTIMUS_DASHBOARD_TOKEN ||= "playwright-dashboard-token-32-chars";
process.env.OPTIMUS_DESKTOP_TOKEN ||= "playwright-desktop-token-32-chars";
process.env.OPTIMUS_SSE_MAX_MS ||= "3000";

const routes = new Map([
  ["/api/control-plane/health", (request, response) => handleHealth(request, response)],
  ["/api/control-plane/auth", (request, response) => handleAuth(request, response, store)],
  ["/api/control-plane/snapshot", (request, response) => handleSnapshot(request, response, store)],
  ["/api/control-plane/commands", (request, response) => handleCommands(request, response, store)],
  ["/api/control-plane/desktop/snapshot", (request, response) => handleDesktopSnapshot(request, response, store)],
  ["/api/control-plane/desktop/commands", (request, response) => handleDesktopCommands(request, response, store)],
]);

const server = createServer(async (request, response) => {
  const url = new URL(request.url || "/", `http://${request.headers.host || `${host}:${port}`}`);
  if (request.headers["x-test-client"]) request.headers["x-forwarded-for"] = String(request.headers["x-test-client"]);
  if (url.pathname === "/api/control-plane/events") {
    metrics.activeSse += 1;
    metrics.totalSse += 1;
    metrics.maxActiveSse = Math.max(metrics.maxActiveSse, metrics.activeSse);
    response.once("close", () => { metrics.activeSse -= 1; });
    await handleEvents(request, response, store);
    return;
  }
  if (url.pathname === "/__test__/metrics") {
    response.setHeader("Content-Type", "application/json");
    response.end(JSON.stringify(metrics));
    return;
  }
  if (url.pathname === "/__test__/advance") {
    relayTimeOffset += Number(url.searchParams.get("ms")) || 0;
    response.setHeader("Content-Type", "application/json");
    response.end(JSON.stringify({ now: Date.now() + relayTimeOffset }));
    return;
  }
  const route = routes.get(url.pathname);
  if (route) {
    await route(request, response);
    return;
  }
  serveStatic(url.pathname, response);
});

export async function startRelayServer() {
  if (server.listening) return;
  await new Promise((resolveStart, rejectStart) => {
    const onError = (error) => {
      server.off("listening", onListening);
      rejectStart(new Error(`Optimus relay test server could not bind ${host}:${port}: ${error.code || error.message}`));
    };
    const onListening = () => {
      server.off("error", onError);
      console.log(`Optimus relay test server ready at http://${host}:${port}`);
      resolveStart();
    };
    server.once("error", onError);
    server.once("listening", onListening);
    server.listen(port, host);
  });
}

export async function closeRelayServer() {
  if (!server.listening) return;
  server.closeAllConnections?.();
  await new Promise((resolveClose) => server.close(resolveClose));
}

function serveStatic(pathname, response) {
  const requested = pathname === "/" || pathname === "/control-plane" || pathname === "/control-plane/"
    ? resolve(dist, "index.html")
    : resolve(dist, `.${decodeURIComponent(pathname)}`);
  if (!requested.startsWith(`${dist}${sep}`) || !existsSync(requested) || !statSync(requested).isFile()) {
    response.statusCode = 404;
    response.end("Not found");
    return;
  }
  response.setHeader("Content-Type", contentType(requested));
  response.setHeader("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self' 'sha256-x/rNMgILcHGVDVe5r701jOKnqPB35jz097I4GS59mxc='; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; object-src 'none'");
  response.setHeader("Referrer-Policy", "no-referrer");
  response.setHeader("X-Content-Type-Options", "nosniff");
  response.setHeader("X-Frame-Options", "DENY");
  createReadStream(requested).pipe(response);
}

function contentType(path) {
  return ({ ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".png": "image/png", ".ico": "image/x-icon" })[extname(path)] || "application/octet-stream";
}
