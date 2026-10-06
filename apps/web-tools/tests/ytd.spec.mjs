import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");

test("opens a YTD by drop, previews channels/mips, replaces PNG and downloads only after semantic reopen", async ({ page }, testInfo) => {
  await page.goto("/");
  await expect(page.locator("#core-status")).toHaveText("Rust/WASM Core ready");

  await page.evaluate(async () => {
    const bytes = await fetch("/fixtures/simple.ytd").then((response) => response.arrayBuffer());
    const transfer = new DataTransfer();
    transfer.items.add(new File([bytes], "dropped-simple.ytd", { type: "application/octet-stream" }));
    const event = new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer });
    document.querySelector("#drop-zone").dispatchEvent(event);
  });

  await expect(page.locator("#asset-name")).toHaveText("dropped-simple.ytd");
  await expect(page.locator("#texture-count")).toHaveText("1 texture");
  await expect(page.locator("#texture-table tbody tr")).toHaveCount(1);
  await expect(page.locator("#texture-table tbody tr")).toContainText("RGBA8");
  await expect(page.locator("#texture-table tbody tr")).toContainText("4×4");
  await expect(page.locator("#texture-table tbody tr")).toContainText("1");

  const pngDownload = page.waitForEvent("download");
  await page.locator("#export-button").click();
  const png = await pngDownload;
  const pngPath = testInfo.outputPath("exported.png");
  await png.saveAs(pngPath);
  expect((await readFile(pngPath)).subarray(1, 4).toString("ascii")).toBe("PNG");

  await page.locator("#replace-input").setInputFiles(pngPath);
  await expect(page.locator("#semantic-status")).toHaveText("PASS — reopened by Rust Core");
  await expect(page.locator("#success-panel")).toContainText("semantic reopen PASS");
  await expect(page.locator("#mip-select option")).toHaveCount(3);

  await page.locator("#mip-select").selectOption("1");
  await expect(page.locator("#preview-dimensions")).toContainText("2 × 2");
  await expect(page.locator("#texture-canvas")).toHaveAttribute("width", "2");
  await expect(page.locator("#texture-canvas")).toHaveAttribute("height", "2");

  await page.locator('button[data-channel="a"]').click();
  await expect(page.locator('button[data-channel="a"]')).toHaveClass(/active/);

  const probe = await page.evaluate(() => window.__ragelabWebTools.probeMip(0, 99));
  expect(probe).toEqual(expect.objectContaining({ ok: false, code: "indexOutOfBounds" }));

  await expect(page.locator("#download-button")).toBeEnabled();
  const ytdDownload = page.waitForEvent("download");
  await page.locator("#download-button").click();
  const ytd = await ytdDownload;
  const ytdPath = testInfo.outputPath("rebuilt.ytd");
  await ytd.saveAs(ytdPath);
  expect((await readFile(ytdPath)).byteLength).toBeGreaterThan(158);
});

test("unsupported replacement format stays inspect-only with Core reason", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#core-status")).toHaveText("Rust/WASM Core ready");

  const fixture = resolve(
    repoRoot,
    "apps",
    "web-tools",
    "public",
    "fixtures",
    "inspect-only-bc2.ytd",
  );
  await page.locator("#asset-input").setInputFiles(fixture);

  await expect(page.locator("#texture-table tbody tr")).toHaveCount(1);
  await expect(page.locator("#texture-table tbody tr")).toContainText("BC2");
  await expect(page.locator("#texture-table tbody tr")).toContainText("inspect/export-only");
  await expect(page.locator("#write-policy")).toContainText("inspect/export-only");
  await expect(page.locator("#replace-input")).toBeDisabled();
  await expect(page.locator("#export-button")).toBeEnabled();
  await expect(page.locator("#texture-canvas")).toHaveAttribute("width", "4");
  await expect(page.locator("#texture-canvas")).toHaveAttribute("height", "4");
});

test("invalid YTD open fails closed and does not replace the current document", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#core-status")).toHaveText("Rust/WASM Core ready");
  await page.locator("#fixture-button").click();
  await expect(page.locator("#asset-name")).toHaveText("simple.ytd");

  await page.locator("#asset-input").setInputFiles({
    name: "broken.ytd",
    mimeType: "application/octet-stream",
    buffer: Buffer.from([1, 2, 3, 4, 5, 6, 7, 8]),
  });

  await expect(page.locator("#error-panel")).toBeVisible();
  await expect(page.locator("#asset-name")).toHaveText("simple.ytd");
  await expect(page.locator("#texture-table tbody tr")).toHaveCount(1);
});
