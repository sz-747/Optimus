import { expect, test } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { safeEnvelope } from "../../relay/tests/fixture.js";

const dashboardToken = process.env.OPTIMUS_DASHBOARD_TOKEN || "playwright-dashboard-token-32-chars";
const desktopToken = process.env.OPTIMUS_DESKTOP_TOKEN || "playwright-desktop-token-32-chars";
const artifactDir = resolve("artifacts/playwright");

test.beforeAll(async () => {
  await mkdir(artifactDir, { recursive: true });
});

test("production WEB mode authenticates without storing the dashboard secret", async ({ page, request, context }) => {
  const deviceId = "relay-auth";
  await publish(request, deviceId, envelope(1, "running"));
  await selectDevice(page, deviceId);
  const errors = captureErrors(page);
  await page.goto("/control-plane", { waitUntil: "domcontentloaded" });

  expect(await page.evaluate(() => window.__TAURI__)).toBeUndefined();
  expect(await page.evaluate(() => window.__OPTIMUS_MOCK__)).toBeUndefined();
  await expect(page.getByTestId("transport-mode")).toHaveText("WEB");
  const dialog = page.getByRole("dialog", { name: "Dashboard connection" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("Access token").fill("incorrect-token-at-least-24-chars");
  await dialog.getByRole("button", { name: "Connect" }).click();
  await expect(page.locator(".cp-status-message")).toHaveText("authentication required");

  await dialog.getByLabel("Access token").fill(dashboardToken);
  await dialog.getByRole("button", { name: "Connect" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("connection-state")).toHaveAttribute("data-state", "connected");
  await expect(page.locator("[data-agent-id='agent-1']")).toHaveAttribute("data-state", "running");
  expect(await page.evaluate(() => localStorage.getItem("optimus.dashboardToken"))).toBeNull();
  const sessionCookie = (await context.cookies()).find((cookie) => cookie.name === "optimus_dashboard");
  expect(sessionCookie?.httpOnly).toBe(true);
  expect(sessionCookie?.sameSite).toBe("Strict");
  await page.screenshot({ path: resolve(artifactDir, "control-plane-relay-desktop.png"), fullPage: true, animations: "disabled" });
  expect(errors.filter((entry) => !entry.includes("401"))).toEqual([]);
});

test("production ignores the unauthenticated demo query", async ({ page }) => {
  await selectDevice(page, "relay-no-production-demo");
  await page.goto("/control-plane?demo=1", { waitUntil: "domcontentloaded" });
  await expect(page.getByRole("dialog", { name: "Dashboard connection" })).toBeVisible();
  await expect(page.getByTestId("connection-state")).not.toHaveAttribute("data-state", "connected");
  await expect(page.getByRole("button", { name: "New run", exact: true }).first()).toBeDisabled();
});

test("start and stop travel through the real command queue and return over SSE", async ({ page, request }) => {
  const deviceId = "relay-lifecycle";
  const consumerId = "22222222-2222-4222-8222-222222222222";
  await publish(request, deviceId, envelope(1, "waiting", "spawn"));
  await selectDevice(page, deviceId);
  const errors = captureErrors(page);
  await page.goto("/control-plane");
  await login(page);

  await page.getByRole("button", { name: "New run", exact: true }).first().click();
  const runDialog = page.getByRole("dialog", { name: "Start agent run" });
  await expect(runDialog.getByLabel("Repository path")).toBeHidden();
  await expect(runDialog.getByLabel("Agent command")).toBeHidden();
  await expect(runDialog.getByLabel("Workspace")).toHaveValue("workspace-primary");
  await expect(runDialog.getByLabel("Model profile")).toHaveValue("codex-subscription");
  await runDialog.getByLabel("Run name").fill("Relay verification");
  await runDialog.getByLabel("Agent name").fill("Remote runner");
  await runDialog.getByLabel("Task").fill("Verify the typed remote lifecycle");
  await runDialog.getByRole("button", { name: "Start", exact: true }).click();
  await expect(runDialog).toBeHidden();

  const start = await pollCommand(request, deviceId, consumerId);
  expect(start.type).toBe("start_run");
  expect(start.payload.workspaceId).toBe("workspace-primary");
  expect(start.payload.profileId).toBe("codex-subscription");
  expect(start.payload.agents).toEqual([{
    agentName: "Remote runner",
    task: "Verify the typed remote lifecycle",
    kind: "writer",
    fileScope: ["ui"],
  }]);
  expect(JSON.stringify(start.payload)).not.toMatch(/repoRoot|command|branch|cwd|env/i);
  await acknowledge(request, deviceId, consumerId, start, "succeeded");
  await expect(page.locator(".cp-status-message")).toHaveText("Run started", { timeout: 5_000 });
  await publish(request, deviceId, envelope(2, "running", "status"));
  await expect(page.locator("[data-agent-id='agent-1']")).toHaveAttribute("data-state", "running", { timeout: 5_000 });

  await page.locator("[data-agent-id='agent-1']").click();
  await page.getByRole("button", { name: "Stop agent" }).click();
  const stop = await pollCommand(request, deviceId, consumerId);
  expect(stop.type).toBe("stop_agent");
  expect(stop.payload).toEqual({ agentId: "agent-1" });
  await acknowledge(request, deviceId, consumerId, stop, "succeeded");
  await expect(page.locator(".cp-status-message")).toHaveText("Agent stopped", { timeout: 5_000 });
  await publish(request, deviceId, envelope(3, "done", "status"));
  await expect(page.locator("[data-agent-id='agent-1']")).toHaveAttribute("data-state", "done", { timeout: 5_000 });
  expect(errors.filter((entry) => !entry.includes("401"))).toEqual([]);
});

test("the web run dialog submits parallel agents with disjoint file ownership", async ({ page, request }) => {
  const deviceId = "relay-parallel";
  const consumerId = "44444444-4444-4444-8444-444444444444";
  await publish(request, deviceId, envelope(1, "waiting"));
  await selectDevice(page, deviceId);
  await page.goto("/control-plane");
  await login(page);
  await page.getByRole("button", { name: "New run", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Start agent run" });
  await dialog.getByLabel("Run name").fill("Parallel release");
  await dialog.getByLabel("Agent name").fill("Web agent");
  await dialog.getByLabel("Task").fill("Build the web dashboard");
  await dialog.getByLabel("File scope").fill("ui");
  await dialog.getByRole("button", { name: "Add parallel agent" }).click();
  await dialog.getByLabel("Agent 2 name").fill("Shell agent");
  await dialog.getByLabel("Agent 2 task").fill("Build the desktop bridge");
  await dialog.getByLabel("Agent 2 file scope").fill("shell/src");
  await dialog.screenshot({ path: resolve(artifactDir, "control-plane-parallel-run-dialog.png"), animations: "disabled" });
  await dialog.getByRole("button", { name: "Start", exact: true }).click();
  const command = await pollCommand(request, deviceId, consumerId);
  expect(command.payload.agents.map((agent) => agent.fileScope)).toEqual([["ui"], ["shell/src"]]);
  await acknowledge(request, deviceId, consumerId, command, "succeeded");
  await expect(page.locator(".cp-status-message")).toHaveText("Run started", { timeout: 5_000 });
});

test("desktop presence goes offline, blocks commands, and recovers on a duplicate heartbeat", async ({ page, request }) => {
  const deviceId = "relay-presence";
  await publish(request, deviceId, envelope(1, "running"));
  await selectDevice(page, deviceId);
  await page.goto("/control-plane");
  await login(page);

  await request.get("/__test__/advance?ms=31000");
  await expect(page.getByTestId("connection-state")).toHaveAttribute("data-state", "offline", { timeout: 5_000 });
  await page.getByRole("button", { name: "New run", exact: true }).first().click();
  const runDialog = page.getByRole("dialog", { name: "Start agent run" });
  await runDialog.getByLabel("Run name").fill("Offline run");
  await runDialog.getByLabel("Agent name").fill("Offline agent");
  await runDialog.getByLabel("Task").fill("This command must not queue while the desktop is offline");
  await runDialog.getByRole("button", { name: "Start", exact: true }).click();
  await expect(page.locator(".cp-status-message")).toHaveText("desktop is offline");
  await expect(runDialog).toBeVisible();
  await runDialog.getByLabel("Close").click();

  await publish(request, deviceId, envelope(1, "running"));
  await expect(page.getByTestId("connection-state")).toHaveAttribute("data-state", "connected", { timeout: 5_000 });
});

test("terminal command failures are visible in the dashboard", async ({ page, request }) => {
  const deviceId = "relay-command-failure";
  const consumerId = "33333333-3333-4333-8333-333333333333";
  await publish(request, deviceId, envelope(1, "waiting"));
  await selectDevice(page, deviceId);
  await page.goto("/control-plane");
  await login(page);
  await page.getByRole("button", { name: "New run", exact: true }).first().click();
  const runDialog = page.getByRole("dialog", { name: "Start agent run" });
  await runDialog.getByLabel("Run name").fill("Failure feedback");
  await runDialog.getByLabel("Agent name").fill("Rejected agent");
  await runDialog.getByLabel("Task").fill("Surface the desktop execution failure");
  await runDialog.getByRole("button", { name: "Start", exact: true }).click();
  const command = await pollCommand(request, deviceId, consumerId);
  await acknowledge(request, deviceId, consumerId, command, "failed", "provider_unavailable");
  await expect(page.locator(".cp-status-message")).toHaveText("Run failed: provider_unavailable", { timeout: 5_000 });
});

test("a transient command-status failure retries instead of reporting a false terminal failure", async ({ page, request }) => {
  const deviceId = "relay-command-retry";
  const consumerId = "55555555-5555-4555-8555-555555555555";
  await publish(request, deviceId, envelope(1, "waiting"));
  await selectDevice(page, deviceId);
  await page.goto("/control-plane");
  await login(page);
  let failedOnce = false;
  await page.route("**/api/control-plane/commands?id=*", async (route) => {
    if (!failedOnce) {
      failedOnce = true;
      await route.abort("failed");
      return;
    }
    await route.continue();
  });
  await page.getByRole("button", { name: "New run", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Start agent run" });
  await dialog.getByLabel("Run name").fill("Retry feedback");
  await dialog.getByLabel("Agent name").fill("Retry agent");
  await dialog.getByLabel("Task").fill("Retry command status without inventing a failure");
  await dialog.getByRole("button", { name: "Start", exact: true }).click();
  const command = await pollCommand(request, deviceId, consumerId);
  await acknowledge(request, deviceId, consumerId, command, "succeeded");
  await expect(page.locator(".cp-status-message")).toHaveText("Run started", { timeout: 10_000 });
  expect(failedOnce).toBe(true);
});

test("SSE reconnects keep exactly one live browser stream", async ({ page, request }) => {
  const deviceId = "relay-sse-singleton";
  await publish(request, deviceId, envelope(1, "running"));
  await selectDevice(page, deviceId);
  await page.goto("/control-plane");
  await login(page);
  await delay(7_000);
  const response = await request.get("/__test__/metrics");
  const metrics = await response.json();
  expect(metrics.totalSse).toBeGreaterThanOrEqual(2);
  expect(metrics.maxActiveSse).toBe(1);
  expect(metrics.activeSse).toBeLessThanOrEqual(1);
});

test("a delayed older snapshot cannot overwrite a newer revision or presence", async ({ page, request }) => {
  const deviceId = "relay-ordering";
  await publish(request, deviceId, envelope(1, "waiting"));
  await selectDevice(page, deviceId);
  await page.goto("/control-plane");
  await login(page);

  let snapshotRequests = 0;
  await page.route("**/api/control-plane/snapshot", async (route) => {
    snapshotRequests += 1;
    const response = await route.fetch();
    const body = await response.json();
    body.desktop.online = snapshotRequests === 1;
    if (snapshotRequests === 1) await delay(900);
    await route.fulfill({ response, json: body });
  });
  await publish(request, deviceId, envelope(2, "running"));
  await expect.poll(() => snapshotRequests).toBe(1);
  await publish(request, deviceId, envelope(3, "done"));
  await expect(page.locator("[data-agent-id='agent-1']")).toHaveAttribute("data-state", "done", { timeout: 5_000 });
  await delay(1_000);
  await expect(page.locator("[data-agent-id='agent-1']")).toHaveAttribute("data-state", "done");
  await expect(page.locator(".cp-revision")).toContainText("3");
  await expect(page.getByTestId("connection-state")).toHaveAttribute("data-state", "offline");
});

test("the authenticated remote dashboard remains usable on tablet and mobile", async ({ page, request }) => {
  const deviceId = "relay-responsive";
  await publish(request, deviceId, envelope(1, "running"));
  await selectDevice(page, deviceId);
  const errors = captureErrors(page);
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.goto("/control-plane");
  await login(page);
  await expectNoOverflow(page);
  await page.locator("[data-record-id='1']").click();
  await expect(page.getByTestId("record-inspector")).toHaveClass(/open/);
  await page.screenshot({ path: resolve(artifactDir, "control-plane-relay-tablet.png"), fullPage: true, animations: "disabled" });

  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByLabel("Close inspector").click();
  await page.locator(".cp-mobile-tab[data-view='agents']").click();
  await expect(page.getByTestId("workspace-tree")).toBeVisible();
  await expectNoOverflow(page);
  await page.screenshot({ path: resolve(artifactDir, "control-plane-relay-mobile.png"), fullPage: true, animations: "disabled" });
  expect(errors.filter((entry) => !entry.includes("401"))).toEqual([]);
});

async function login(page) {
  const dialog = page.getByRole("dialog", { name: "Dashboard connection" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("Access token").fill(dashboardToken);
  await dialog.getByRole("button", { name: "Connect" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("connection-state")).toHaveAttribute("data-state", "connected");
}

async function selectDevice(page, deviceId) {
  await page.setExtraHTTPHeaders({ "X-Test-Client": deviceId });
  await page.addInitScript((id) => localStorage.setItem("optimus.deviceId", id), deviceId);
}

async function publish(request, deviceId, body) {
  const response = await request.put("/api/control-plane/desktop/snapshot", {
    headers: { Authorization: `Bearer ${desktopToken}`, "X-Optimus-Device": deviceId, "Content-Type": "application/json" },
    data: body,
  });
  expect(response.status()).toBeLessThan(300);
}

async function pollCommand(request, deviceId, consumerId) {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const response = await request.get(`/api/control-plane/desktop/commands?consumer=${consumerId}`, {
      headers: { Authorization: `Bearer ${desktopToken}`, "X-Optimus-Device": deviceId },
    });
    expect(response.ok()).toBe(true);
    const { commands } = await response.json();
    if (commands.length) return commands[0];
    await delay(100);
  }
  throw new Error("desktop did not receive a command");
}

async function acknowledge(request, deviceId, consumerId, command, status, errorCode = null) {
  const response = await request.patch(`/api/control-plane/desktop/commands?consumer=${consumerId}`, {
    headers: { Authorization: `Bearer ${desktopToken}`, "X-Optimus-Device": deviceId, "Content-Type": "application/json" },
    data: { commandId: command.commandId, leaseId: command.leaseId, status, errorCode },
  });
  expect(response.ok()).toBe(true);
}

function envelope(revision, state, activityKind = "status") {
  const value = structuredClone(safeEnvelope(revision));
  value.instanceId = "99999999-9999-4999-8999-999999999999";
  value.snapshot.agents[0].state = state;
  value.snapshot.activity[0].kind = activityKind;
  value.snapshot.totals = { running: state === "running" ? 1 : 0, stalled: state === "stalled" ? 1 : 0, waiting: ["waiting", "ready", "planned"].includes(state) ? 1 : 0, done: state === "done" ? 1 : 0, failed: ["failed", "interrupted"].includes(state) ? 1 : 0 };
  return value;
}

function captureErrors(page) {
  const errors = [];
  page.on("pageerror", (error) => errors.push(`pageerror: ${error.message}`));
  page.on("console", (message) => { if (message.type() === "error") errors.push(`console: ${message.text()}`); });
  return errors;
}

async function expectNoOverflow(page) {
  const width = await page.evaluate(() => ({ viewport: innerWidth, document: document.documentElement.scrollWidth, body: document.body.scrollWidth }));
  expect(width.document).toBeLessThanOrEqual(width.viewport);
  expect(width.body).toBeLessThanOrEqual(width.viewport);
}

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
