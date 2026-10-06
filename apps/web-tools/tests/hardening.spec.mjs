import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { expect, test } from "@playwright/test";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const budgets = JSON.parse(
  await readFile(resolve(repoRoot, "fixtures", "hardening", "budgets.json"), "utf8"),
).web;

test("Rust/WASM core reaches ready state within the hardening budget", async ({ page }) => {
  const started = performance.now();
  await page.goto("/");
  await expect(page.locator("#core-status")).toHaveText("Rust/WASM Core ready", {
    timeout: budgets.coreReadyMs,
  });
  const elapsedMs = performance.now() - started;
  expect(elapsedMs).toBeLessThanOrEqual(budgets.coreReadyMs);
  console.log(JSON.stringify({
    schema: "ragelab.web.readiness-budget",
    schemaVersion: 1,
    elapsedMs,
    budgetMs: budgets.coreReadyMs,
  }));
});
