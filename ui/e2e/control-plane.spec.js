import { expect, test } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

const artifactDir = resolve("artifacts/playwright");

test.beforeAll(async () => {
  await mkdir(artifactDir, { recursive: true });
});

function captureRuntimeErrors(page) {
  const errors = [];
  page.on("pageerror", (error) => errors.push(`pageerror: ${error.message}`));
  page.on("console", (message) => {
    if (message.type() === "error") {
      const location = message.location().url || page.url();
      errors.push(`console: ${message.text()} (${location})`);
    }
  });
  return errors;
}

async function openControlPlane(page, query = "?demo=1") {
  const response = await page.goto(`/control-plane${query}`, { waitUntil: "domcontentloaded" });
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId("control-plane")).toBeVisible();
  await expect(page.getByTestId("connection-state")).toHaveAttribute("data-state", "connected");
  await page.evaluate(async () => {
    await document.fonts.ready;
    await new Promise((resolveFrame) => requestAnimationFrame(resolveFrame));
    await new Promise((resolveFrame) => requestAnimationFrame(resolveFrame));
  });
}

async function expectNoHorizontalOverflow(page) {
  const dimensions = await page.evaluate(() => ({
    viewport: window.innerWidth,
    document: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
  }));
  expect(dimensions.document).toBeLessThanOrEqual(dimensions.viewport);
  expect(dimensions.body).toBeLessThanOrEqual(dimensions.viewport);
}

test("renders a revisioned multi-agent snapshot with stable hierarchy", async ({ page }) => {
  const runtimeErrors = captureRuntimeErrors(page);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await openControlPlane(page);

  await expect(page).toHaveTitle("Optimus Control Plane");
  await expect(page.locator("[data-agent-id]")).toHaveCount(6);
  await expect(page.locator("[data-agent-id='agent-3']")).toHaveCSS("--agent-depth", "1");
  await expect(page.getByTestId("capacity-meter")).toContainText("5 / 8");
  await expect(page.getByTestId("status-summary")).toContainText("3Running");
  await expect(page.getByTestId("status-summary")).toContainText("1Stalled");

  const inspector = page.getByTestId("record-inspector");
  await expect(inspector).toBeHidden();
  await page.getByLabel("Toggle inspector").click();
  await expect(inspector).toBeVisible();
  await page.getByLabel("Close inspector").click();
  await expect(inspector).toBeHidden();

  const collapse = page.getByRole("button", { name: "Collapse optimus" });
  await collapse.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-agent-id='agent-1']")).toBeHidden();
  const expand = page.getByRole("button", { name: "Expand optimus" });
  await expand.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-agent-id='agent-1']")).toBeVisible();

  const recordIds = await page.locator("[data-record-id]").evaluateAll((rows) =>
    rows.map((row) => Number(row.dataset.recordId)),
  );
  expect(recordIds).toEqual([42, 41, 40, 39, 38, 37, 36, 35, 34]);
  await expectNoHorizontalOverflow(page);
  await page.screenshot({ path: resolve(artifactDir, "control-plane-desktop.png"), fullPage: true, animations: "disabled" });
  expect(runtimeErrors).toEqual([]);
});

test("scopes activity and drills into a same-file contradiction", async ({ page }) => {
  const runtimeErrors = captureRuntimeErrors(page);
  await openControlPlane(page);

  await page.locator("[data-agent-id='agent-3']").click();
  await expect(page).toHaveURL(/agent=agent-3/);
  await expect(page).toHaveURL(/workspace=ws-1/);
  await expect(page.locator("[data-record-id]")).toHaveCount(2);
  await expect(page.getByTestId("record-inspector")).toContainText("Build the product dashboard");

  await page.locator("[data-relation='contradicts']").click();
  await expect(page).toHaveURL(/kind=contradiction/);
  await expect(page.getByTestId("activity-search")).toHaveValue("shell/src/control_plane/read_model.rs");
  await expect(page.locator("[data-record-id]")).toHaveCount(2);
  await expect(page.locator("[data-record-id='39']")).toBeVisible();
  await expect(page.locator("[data-record-id='38']")).toBeVisible();
  await expect(page.getByTestId("record-inspector")).toContainText("contradicts:v1:trusted-window-200");
  expect(runtimeErrors).toEqual([]);
});

test("applies live revisions without reload and preserves paused follow", async ({ page }) => {
  const runtimeErrors = captureRuntimeErrors(page);
  await page.setViewportSize({ width: 1280, height: 480 });
  await openControlPlane(page, "");
  await expect(page.getByTestId("transport-mode")).toHaveText("DESKTOP");

  await page.evaluate(() => {
    window.__OPTIMUS_MOCK__.updateAgent("agent-1", { state: "stalled", status: "Waiting for review" });
  });
  await expect(page.locator("[data-agent-id='agent-1']")).toHaveAttribute("data-state", "stalled");
  await expect(page.getByTestId("status-summary")).toContainText("2Running");
  await expect(page.getByTestId("status-summary")).toContainText("2Stalled");

  const feed = page.getByTestId("activity-stream");
  await feed.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event("scroll"));
  });
  await expect.poll(() => feed.evaluate((element) => element.scrollTop)).toBeGreaterThan(12);
  await page.evaluate(() => {
    window.__OPTIMUS_MOCK__.appendActivity({ fact: "Playwright observed the live revision" });
  });
  await expect(page.getByTestId("live-follow")).toHaveText("1 new");
  await page.getByTestId("live-follow").click();
  await expect(page.locator("[data-record-id='43']")).toContainText("Playwright observed the live revision");
  expect(runtimeErrors).toEqual([]);
});

test("starts and stops an agent through the product controls", async ({ page }) => {
  const runtimeErrors = captureRuntimeErrors(page);
  await openControlPlane(page, "");

  await page.getByRole("button", { name: "New run", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Start agent run" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("Agent name").fill("Playwright runner");
  await dialog.getByLabel("Task").fill("Verify the browser control plane");
  await dialog.getByLabel("Branch").fill("test/playwright-control-plane");
  await dialog.getByRole("button", { name: "Start", exact: true }).click();

  const agent = page.locator("[data-agent-id='agent-7']");
  await expect(agent).toContainText("Playwright runner");
  await expect(agent).toHaveAttribute("data-state", "running");
  await agent.click();
  await page.getByRole("button", { name: "Stop agent" }).click();
  await expect(agent).toHaveAttribute("data-state", "done");
  await expect(page.getByTestId("status-summary")).toContainText("2Done");
  expect(runtimeErrors).toEqual([]);
});

test("supports keyboard search, feed navigation, and the command palette", async ({ page }) => {
  const runtimeErrors = captureRuntimeErrors(page);
  await openControlPlane(page);

  await page.locator("[data-record-id='42']").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("record-inspector")).toContainText("Added responsive control-plane layout");

  await page.keyboard.press("/");
  await expect(page.getByTestId("activity-search")).toBeFocused();
  await page.getByTestId("activity-search").fill("SQLite");
  await expect(page.locator("[data-record-id]")).toHaveCount(1);
  await page.getByTestId("activity-search").fill("");

  await page.getByTestId("activity-stream").focus();
  await page.keyboard.press("j");
  await expect(page.locator("[data-record-id='41']")).toHaveClass(/selected/);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("record-inspector")).toContainText("Reviewing correlation output");

  await page.keyboard.press("Control+Shift+P");
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await palette.getByRole("searchbox", { name: "Command search" }).fill("contradictions");
  await page.keyboard.press("Enter");
  await expect(palette).toBeHidden();
  await expect(page.locator(".cp-segment[data-kind='contradiction']")).toHaveAttribute("aria-selected", "true");
  expect(runtimeErrors).toEqual([]);
});

test("keeps the desktop, tablet, and mobile surfaces usable", async ({ page }) => {
  const runtimeErrors = captureRuntimeErrors(page);
  await page.setViewportSize({ width: 1024, height: 768 });
  await openControlPlane(page);
  await page.locator("[data-record-id='42']").click();
  await expect(page.getByTestId("record-inspector")).toHaveClass(/open/);
  await expectNoHorizontalOverflow(page);
  await page.screenshot({ path: resolve(artifactDir, "control-plane-tablet.png"), fullPage: true, animations: "disabled" });

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole("button", { name: "Activity" })).toBeVisible();
  await expect(page.getByTestId("record-inspector")).toBeVisible();
  await page.getByLabel("Close inspector").click();
  await expect.poll(() => page.getByTestId("record-inspector").evaluate((element) => element.getBoundingClientRect().left)).toBeGreaterThanOrEqual(390);
  await expectNoHorizontalOverflow(page);
  await page.screenshot({ path: resolve(artifactDir, "control-plane-mobile-activity.png"), fullPage: true, animations: "disabled" });

  await page.locator(".cp-mobile-tab[data-view='agents']").click();
  await expect(page.getByTestId("workspace-tree")).toBeVisible();
  await expect(page.getByTestId("capacity-meter")).toBeVisible();
  await expectNoHorizontalOverflow(page);
  await page.screenshot({ path: resolve(artifactDir, "control-plane-mobile-agents.png"), fullPage: true, animations: "disabled" });
  expect(runtimeErrors).toEqual([]);
});
