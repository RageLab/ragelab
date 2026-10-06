import { defineConfig } from "@playwright/test";

const matrixBrowser = process.env.RAGELAB_BROWSER;

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  workers: matrixBrowser ? 1 : undefined,
  use: matrixBrowser
    ? {
        baseURL: "http://127.0.0.1:4173",
        browserName: matrixBrowser,
        headless: true,
      }
    : {
        baseURL: "http://127.0.0.1:4173",
        channel: "chrome",
        headless: true,
      },
  webServer: {
    command: "bun run dev -- --host 127.0.0.1 --port 4173",
    port: 4173,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
