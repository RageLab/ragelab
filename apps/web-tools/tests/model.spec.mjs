import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const publicFixture = (name) =>
  resolve(repoRoot, "apps", "web-tools", "public", "fixtures", name);

async function openModelTool(page) {
  await page.goto("/");
  await expect(page.locator("#core-status")).toHaveText("Rust/WASM Core ready");
  await page.locator("#tool-model").click();
  await expect(page.locator("#model-tool")).toBeVisible();
  await expect(page.locator("#model-canvas")).toHaveAttribute("data-renderer", "three");
}

test("renders embedded YDR from typed Core buffers with controls and PNG screenshot", async ({ page }, testInfo) => {
  await openModelTool(page);
  await page.locator("#model-fixture-embedded").click();

  await expect(page.locator("#model-asset-name")).toHaveText("model-embedded.ydr");
  await expect(page.locator("#model-summary")).toContainText("YDR · 1 primitive");
  await expect(page.locator("#model-summary")).toContainText("3 vertices");
  await expect(page.locator("#model-material-table tbody tr")).toHaveCount(1);
  await expect(page.locator("#model-material-table tbody tr")).toContainText("embedded_diff");
  await expect(page.locator("#model-material-table tbody tr")).toContainText("Embedded");
  await expect(page.locator("#model-diagnostics")).toHaveClass(/ok/);

  const snapshot = await page.evaluate(() => window.__ragelabWebTools.modelSnapshot());
  expect(snapshot).toEqual(expect.objectContaining({
    format: "ydr",
    primitiveCount: 1,
    vertexCount: 3,
    indexCount: 3,
    dependencyCount: 0,
    diagnostics: [],
  }));

  await page.locator("#model-view-select").selectOption("top");
  await page.locator("#model-bounds-toggle").check();
  await page.locator("#model-wire-toggle").check();
  await page.locator("#model-turntable-button").click();
  await expect(page.locator("#model-turntable-button")).toHaveAttribute("aria-pressed", "true");

  const screenshotDownload = page.waitForEvent("download");
  await page.locator("#model-screenshot-button").click();
  const screenshot = await screenshotDownload;
  const screenshotPath = testInfo.outputPath("embedded-model.png");
  await screenshot.saveAs(screenshotPath);
  const png = await readFile(screenshotPath);
  expect([...png.subarray(0, 8)]).toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
  expect(png.byteLength).toBeGreaterThan(500);
});

test("resolves proven external diffuse only after caller supplies matching YTD and supports YDD selection", async ({ page }) => {
  await openModelTool(page);

  await page.locator("#model-input").setInputFiles(publicFixture("model-external-diffuse.ydr"));
  await expect(page.locator("#model-asset-name")).toHaveText("model-external-diffuse.ydr");
  await expect(page.locator("#model-material-table tbody tr")).toContainText("test_diffuse");
  await expect(page.locator("#model-material-table tbody tr")).toContainText("Unresolved");
  await expect(page.locator("#model-diagnostics")).toContainText("textureNotFound");

  await page.locator("#model-dependency-input").setInputFiles([
    publicFixture("model-test-diffuse.ytd"),
  ]);
  await expect(page.locator("#model-material-table tbody tr")).toContainText(
    "model-test-diffuse.ytd",
  );
  await expect(page.locator("#model-material-table tbody tr")).toContainText("RGBA8");
  await expect(page.locator("#model-diagnostics")).toHaveClass(/ok/);
  const externalSnapshot = await page.evaluate(() => window.__ragelabWebTools.modelSnapshot());
  expect(externalSnapshot).toEqual(expect.objectContaining({
    format: "ydr",
    dependencyCount: 1,
    diagnostics: [],
  }));

  await page.locator("#model-fixture-ydd").click();
  await expect(page.locator("#model-asset-name")).toHaveText("model-editable.ydd");
  await expect(page.locator("#model-drawable-wrap")).toBeVisible();
  await expect(page.locator("#model-drawable-select option")).toHaveCount(1);
  await expect(page.locator("#model-summary")).toContainText("YDD · 1 primitive");
  await expect(page.locator("#model-material-table tbody tr")).toHaveCount(2);
  await expect(page.locator("#model-material-table tbody tr").first()).toContainText(
    "No proven diffuse binding",
  );
  const yddSnapshot = await page.evaluate(() => window.__ragelabWebTools.modelSnapshot());
  expect(yddSnapshot).toEqual(expect.objectContaining({
    format: "ydd",
    drawableIndex: 0,
    primitiveCount: 1,
  }));
});

test("YFT without pristine main drawable and invalid model both fail closed without replacing current preview", async ({ page }) => {
  await openModelTool(page);
  await page.locator("#model-fixture-embedded").click();
  await expect(page.locator("#model-asset-name")).toHaveText("model-embedded.ydr");

  await page.locator("#model-input").setInputFiles(publicFixture("model-no-main.yft"));
  await expect(page.locator("#error-panel")).toContainText("unsupportedAsset");
  await expect(page.locator("#error-panel")).toContainText("pristine main drawable");
  await expect(page.locator("#model-asset-name")).toHaveText("model-embedded.ydr");

  await page.locator("#model-input").setInputFiles({
    name: "broken.ydr",
    mimeType: "application/octet-stream",
    buffer: Buffer.from([1, 2, 3, 4, 5, 6, 7, 8]),
  });
  await expect(page.locator("#error-panel")).toBeVisible();
  await expect(page.locator("#model-asset-name")).toHaveText("model-embedded.ydr");
  const snapshot = await page.evaluate(() => window.__ragelabWebTools.modelSnapshot());
  expect(snapshot).toEqual(expect.objectContaining({ name: "model-embedded.ydr", format: "ydr" }));
});
