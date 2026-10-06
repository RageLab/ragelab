import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const publicFixture = (name) =>
  resolve(repoRoot, "apps", "web-tools", "public", "fixtures", name);

async function openYmapTool(page) {
  await page.goto("/");
  await expect(page.locator("#core-status")).toHaveText("Rust/WASM Core ready");
  await page.locator("#tool-ymap").click();
  await expect(page.locator("#ymap-tool")).toBeVisible();
  await expect(page.locator("#ymap-canvas")).toHaveAttribute("data-renderer", "three");
}

test("resolves a supplied-only YMAP catalog and fails closed for missing or ambiguous dependencies", async ({ page }) => {
  await openYmapTool(page);

  await page.locator("#ymap-dependency-input").setInputFiles([
    publicFixture("ymap-simple.ytyp"),
  ]);
  await page.locator("#ymap-input").setInputFiles(publicFixture("ymap-simple.ymap"));

  await expect(page.locator("#ymap-asset-name")).toHaveText("ymap-simple.ymap");
  await expect(page.locator("#ymap-diagnostics")).toContainText("assetMissing");
  let snapshot = await page.evaluate(() => window.__ragelabWebTools.ymapSnapshot());
  expect(snapshot).toEqual(expect.objectContaining({
    dependencyCount: 1,
    entityCount: 1,
    renderedEntityCount: 0,
    diagnosticCodes: ["assetMissing"],
  }));

  const ytyp = await readFile(publicFixture("ymap-simple.ytyp"));
  const ydr = await readFile(publicFixture("test_drawable.ydr"));
  await page.locator("#ymap-dependency-input").setInputFiles([
    { name: "first.ytyp", mimeType: "application/octet-stream", buffer: ytyp },
    { name: "second.ytyp", mimeType: "application/octet-stream", buffer: ytyp },
    { name: "test_drawable.ydr", mimeType: "application/octet-stream", buffer: ydr },
  ]);
  await expect(page.locator("#ymap-diagnostics")).toContainText("providerAmbiguous");
  snapshot = await page.evaluate(() => window.__ragelabWebTools.ymapSnapshot());
  expect(snapshot).toEqual(expect.objectContaining({
    dependencyCount: 3,
    renderedEntityCount: 0,
    diagnosticCodes: ["providerAmbiguous"],
  }));

  await page.locator("#ymap-dependency-input").setInputFiles([
    publicFixture("ymap-simple.ytyp"),
    publicFixture("test_drawable.ydr"),
  ]);
  await expect(page.locator("#ymap-diagnostics")).toHaveClass(/ok/);
  await expect(page.locator("#ymap-entity-list .ymap-entity-row")).toHaveCount(1);
  await expect(page.locator("#ymap-entity-list .ymap-entity-row")).toContainText("YDR");
  await expect(page.locator("#ymap-bounds")).toContainText("rendered:");
  snapshot = await page.evaluate(() => window.__ragelabWebTools.ymapSnapshot());
  expect(snapshot).toEqual(expect.objectContaining({
    dependencyCount: 2,
    entityCount: 1,
    renderedEntityCount: 1,
    diagnosticCodes: [],
  }));
  expect(snapshot.bounds.empty).toBe(false);
});

test("selects, focuses, isolates, safely edits and explicitly downloads a semantically reopened YMAP", async ({ page }, testInfo) => {
  await openYmapTool(page);
  await page.locator("#ymap-fixture-button").click();

  await expect(page.locator("#ymap-asset-name")).toHaveText("ymap-simple.ymap");
  await expect(page.locator("#ymap-diagnostics")).toHaveClass(/ok/);
  await page.locator("#ymap-entity-list .ymap-entity-row").click();

  await expect(page.locator("#ymap-inspector")).toBeVisible();
  await expect(page.locator("#ymap-inspector-index")).toHaveText("#0");
  await expect(page.locator("#ymap-inspector-resolution")).toContainText("YDR");
  await expect(page.locator("#ymap-focus-button")).toBeEnabled();
  await page.locator("#ymap-focus-button").click();
  await page.locator("#ymap-isolate-toggle").check();

  await page.locator("#ymap-position-x").fill("12.5");
  await page.locator("#ymap-flags").fill("123");
  await page.locator("#ymap-apply-button").click();

  await expect(page.locator("#ymap-semantic-status")).toContainText("PASS");
  await expect(page.locator("#ymap-download-button")).toBeEnabled();
  let snapshot = await page.evaluate(() => window.__ragelabWebTools.ymapSnapshot());
  expect(snapshot).toEqual(expect.objectContaining({
    selectedIndex: 0,
    dirty: true,
    semanticValidated: true,
    renderedEntityCount: 1,
  }));

  const downloadEvent = page.waitForEvent("download");
  await page.locator("#ymap-download-button").click();
  const download = await downloadEvent;
  const output = testInfo.outputPath("edited.ymap");
  await download.saveAs(output);
  const bytes = await readFile(output);
  expect(bytes.byteLength).toBeGreaterThan(32);

  await page.locator("#ymap-input").setInputFiles(output);
  await expect(page.locator("#ymap-asset-name")).toHaveText("edited.ymap");
  await page.locator("#ymap-entity-list .ymap-entity-row").click();
  await expect(page.locator("#ymap-position-x")).toHaveValue("12.5");
  await expect(page.locator("#ymap-flags")).toHaveValue("123");
  await expect(page.locator("#ymap-semantic-status")).toHaveText("Original bytes");
  snapshot = await page.evaluate(() => window.__ragelabWebTools.ymapSnapshot());
  expect(snapshot).toEqual(expect.objectContaining({
    dirty: false,
    semanticValidated: false,
    renderedEntityCount: 1,
  }));
});
