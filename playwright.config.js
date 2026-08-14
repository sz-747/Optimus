import { defineConfig } from "@playwright/test";

const localBrowser = process.env.PLAYWRIGHT_CHANNEL || "chrome";

export default defineConfig({
  testDir: "./ui/e2e",
  testIgnore: ["control-plane-relay.spec.js", "control-plane-live.spec.js"],
  outputDir: "./artifacts/playwright/test-results",
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  globalSetup: "./ui/e2e/global-setup.js",
  reporter: [
    ["list"],
    ["html", { outputFolder: "./artifacts/playwright/report", open: "never" }],
  ],
  use: {
    baseURL: "http://127.0.0.1:4173",
    ...(process.env.CI ? {} : { channel: localBrowser }),
    headless: true,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "retain-on-failure",
  },
});
