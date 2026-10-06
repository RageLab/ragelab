import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  use: {
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
