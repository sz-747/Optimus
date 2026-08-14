import { defineConfig } from "@playwright/test";

if (!process.env.PLAYWRIGHT_BASE_URL) {
  throw new Error("PLAYWRIGHT_BASE_URL is required for live relay tests");
}

export default defineConfig({
  testDir: "./ui/e2e",
  testMatch: "control-plane-live.spec.js",
  outputDir: "./artifacts/playwright/live-results",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [
    ["list"],
    ["html", { outputFolder: "./artifacts/playwright/live-report", open: "never" }],
  ],
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL,
    channel: process.env.CI ? undefined : (process.env.PLAYWRIGHT_CHANNEL || "chrome"),
    headless: true,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "retain-on-failure",
  },
});
