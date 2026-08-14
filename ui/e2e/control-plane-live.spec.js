import { expect, test } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { randomUUID } from "node:crypto";
import { safeEnvelope } from "../../relay/tests/fixture.js";

const dashboardToken = process.env.OPTIMUS_DASHBOARD_TOKEN;
const desktopToken = process.env.OPTIMUS_DESKTOP_TOKEN;
const artifactDir = resolve("artifacts/playwright");

test.beforeAll(async () => {
  expect(dashboardToken, "OPTIMUS_DASHBOARD_TOKEN is required").toBeTruthy();
  expect(desktopToken, "OPTIMUS_DESKTOP_TOKEN is required").toBeTruthy();
  await mkdir(artifactDir, { recursive: true });
});

test("production relay completes an authenticated browser-to-desktop round trip", async ({ page, request, context }) => {
  const deviceId = `live-${randomUUID()}`;
  const consumerId = randomUUID();
  const first = envelope(1, "waiting");

  const health = await request.get("/api/control-plane/health");
  expect(health.ok()).toBe(true);
  expect(await health.json()).toMatchObject({ ok: true });

  const unauthenticated = await request.get("/api/control-plane/snapshot", {
    headers: { "X-Optimus-Device": deviceId },
  });
  expect(unauthenticated.status()).toBe(401);

  await publish(request, deviceId, first);
  await page.addInitScript((id) => localStorage.setItem("optimus.deviceId", id), deviceId);
  const documentResponse = await page.goto("/control-plane", { waitUntil: "domcontentloaded" });
  expect(documentResponse?.ok()).toBe(true);
  const headers = await documentResponse.allHeaders();
  expect(headers["content-security-policy"]).toContain("default-src 'self'");
  expect(headers["content-security-policy"]).toContain("frame-ancestors 'none'");
  expect(headers["referrer-policy"]).toBe("no-referrer");
  expect(headers["x-content-type-options"]).toBe("nosniff");
  expect(headers["x-frame-options"]).toBe("DENY");
  expect(headers["permissions-policy"]).toContain("camera=()");

  const dialog = page.getByRole("dialog", { name: "Dashboard connection" });
  await dialog.getByLabel("Access token").fill(dashboardToken);
  await dialog.getByRole("button", { name: "Connect" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("connection-state")).toHaveAttribute("data-state", "connected");
  await expect(page.locator("[data-agent-id='agent-1']")).toHaveAttribute("data-state", "waiting");

  const sessionCookie = (await context.cookies()).find((cookie) => cookie.name === "optimus_dashboard");
  expect(sessionCookie).toMatchObject({ httpOnly: true, sameSite: "Strict", secure: true });
  expect(await page.evaluate(() => localStorage.getItem("optimus.dashboardToken"))).toBeNull();

  await page.getByRole("button", { name: "New run", exact: true }).first().click();
  const runDialog = page.getByRole("dialog", { name: "Start agent run" });
  await runDialog.getByLabel("Run name").fill("Live relay verification");
  await runDialog.getByLabel("Agent name").fill("Vercel smoke agent");
  await runDialog.getByLabel("Task").fill("Verify the production control-plane relay");
  await runDialog.getByLabel("File scope").fill("ui");
  await runDialog.getByRole("button", { name: "Start", exact: true }).click();

  const command = await pollCommand(request, deviceId, consumerId);
  expect(command).toMatchObject({
    type: "start_run",
    payload: {
      sessionName: "Live relay verification",
      agents: [{
        agentName: "Vercel smoke agent",
        task: "Verify the production control-plane relay",
        fileScope: ["ui"],
      }],
    },
  });
  expect(JSON.stringify(command.payload)).not.toMatch(/repoRoot|command|branch|cwd|env/i);
  await acknowledge(request, deviceId, consumerId, command);
  await expect(page.locator(".cp-status-message")).toHaveText("Run started", { timeout: 10_000 });

  await publish(request, deviceId, envelope(2, "running"));
  await expect(page.locator("[data-agent-id='agent-1']")).toHaveAttribute("data-state", "running", { timeout: 10_000 });
  await expect(page.locator(".cp-revision")).toContainText("2");
  await page.screenshot({
    path: resolve(artifactDir, "control-plane-live-production.png"),
    fullPage: true,
    animations: "disabled",
  });
});

async function publish(request, deviceId, data) {
  const response = await request.put("/api/control-plane/desktop/snapshot", {
    headers: desktopHeaders(deviceId),
    data,
  });
  expect(response.ok()).toBe(true);
}

async function pollCommand(request, deviceId, consumerId) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const response = await request.get(`/api/control-plane/desktop/commands?consumer=${consumerId}`, {
      headers: desktopHeaders(deviceId),
    });
    expect(response.ok()).toBe(true);
    const { commands } = await response.json();
    if (commands.length) return commands[0];
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 200));
  }
  throw new Error("production desktop relay did not receive the command");
}

async function acknowledge(request, deviceId, consumerId, command) {
  const response = await request.patch(`/api/control-plane/desktop/commands?consumer=${consumerId}`, {
    headers: desktopHeaders(deviceId),
    data: { commandId: command.commandId, leaseId: command.leaseId, status: "succeeded", errorCode: null },
  });
  expect(response.ok()).toBe(true);
}

function desktopHeaders(deviceId) {
  return {
    Authorization: `Bearer ${desktopToken}`,
    "X-Optimus-Device": deviceId,
    "Content-Type": "application/json",
  };
}

function envelope(revision, state) {
  const value = structuredClone(safeEnvelope(revision));
  value.instanceId = "99999999-9999-4999-8999-999999999999";
  value.snapshot.agents[0].state = state;
  value.snapshot.totals = {
    running: state === "running" ? 1 : 0,
    stalled: 0,
    waiting: state === "waiting" ? 1 : 0,
    done: 0,
    failed: 0,
  };
  return value;
}
