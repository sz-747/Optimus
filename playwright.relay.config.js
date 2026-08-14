import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./ui/e2e",
  testMatch: "control-plane-relay.spec.js",
  outputDir: "./artifacts/playwright/relay-results",
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: [
    ["list"],
    ["html", { outputFolder: "./artifacts/playwright/relay-report", open: "never" }],
  ],
  globalSetup: "./ui/e2e/relay-global-setup.js",
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL || "http://127.0.0.1:4174",
    channel: process.env.CI ? undefined : (process.env.PLAYWRIGHT_CHANNEL || "chrome"),
    headless: true,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "retain-on-failure",
  },
});
