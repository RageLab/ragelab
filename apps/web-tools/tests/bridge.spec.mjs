import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
const publicFixture = (name) =>
  resolve(repoRoot, "apps", "web-tools", "public", "fixtures", name);

test("bridge stays disconnected until explicit connect and opens a bounded YMAP bundle", async ({ page }) => {
  const token = "a".repeat(64);
  const assetId = "asset-0123456789abcdef0123456789abcdef";
  const ytdAssetId = "asset-33333333333333333333333333333333";
  const requests = [];

  const ymap = await readFile(publicFixture("ymap-simple.ymap"));
  const ytyp = await readFile(publicFixture("ymap-simple.ytyp"));
  const ydr = await readFile(publicFixture("test_drawable.ydr"));
  const ytd = await readFile(publicFixture("simple.ytd"));
  const totalBytes = ymap.byteLength + ytyp.byteLength + ydr.byteLength;

  await page.route("http://127.0.0.1:32191/**", async (route) => {
    const request = route.request();
    requests.push({
      url: request.url(),
      authorization: request.headers().authorization ?? null,
    });
    const url = new URL(request.url());
    const json = (body) => route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(body),
    });

    if (url.pathname === "/v1/health") {
      await json({
        schema: "ragelab.bridge.health",
        schemaVersion: 1,
        connected: true,
        readOnly: true,
        writesEnabled: false,
        capabilities: ["search", "asset.read", "ymap.bundle"],
        limits: {
          maxAssetBytes: 32 * 1024 * 1024,
          maxBundleAssets: 96,
          maxBundleBytes: 64 * 1024 * 1024,
          maxSearchResults: 50,
        },
      });
      return;
    }
    if (url.pathname === "/v1/search") {
      await json({
        schema: "ragelab.bridge.search",
        schemaVersion: 1,
        query: url.searchParams.get("q"),
        results: [
          {
            kind: "ymap",
            hash: "0x12345678",
            label: "ymap-simple.ymap",
            assetId,
            format: "ymap",
            mapHash: "0x12345678",
            entityIndex: null,
            position: null,
            bounds: null,
            entityCount: 1,
          },
          {
            kind: "asset",
            hash: "0x87654321",
            label: "simple.ytd",
            assetId: ytdAssetId,
            format: "ytd",
            mapHash: null,
            entityIndex: null,
            position: null,
            bounds: null,
            entityCount: null,
          },
        ],
        truncated: false,
      });
      return;
    }
    if (url.pathname === `/v1/assets/${ytdAssetId}`) {
      await route.fulfill({
        status: 200,
        contentType: "application/octet-stream",
        body: ytd,
      });
      return;
    }
    if (url.pathname === `/v1/ymap-bundle/${assetId}`) {
      await json({
        schema: "ragelab.bridge.ymap-bundle",
        schemaVersion: 1,
        mapHash: "0x12345678",
        files: [
          {
            role: "ymap",
            assetId,
            name: "ymap-simple.ymap",
            format: "ymap",
            hash: "0x12345678",
            byteLength: ymap.byteLength,
            bytesBase64: ymap.toString("base64"),
          },
          {
            role: "archetypeProvider",
            assetId: "asset-11111111111111111111111111111111",
            name: "ymap-simple.ytyp",
            format: "ytyp",
            hash: "0x11111111",
            byteLength: ytyp.byteLength,
            bytesBase64: ytyp.toString("base64"),
          },
          {
            role: "model",
            assetId: "asset-22222222222222222222222222222222",
            name: "test_drawable.ydr",
            format: "ydr",
            hash: "0x22222222",
            byteLength: ydr.byteLength,
            bytesBase64: ydr.toString("base64"),
          },
        ],
        limits: {
          assetCount: 3,
          declaredBytes: totalBytes,
          actualBytes: totalBytes,
          maxAssets: 96,
          maxBytes: 64 * 1024 * 1024,
        },
        diagnostics: {
          requestedArchetypes: 1,
          unresolvedArchetypes: [],
          ambiguousArchetypes: [],
          unresolvedAssets: [],
          ambiguousAssets: [],
          ambiguousTextureParents: [],
          textureParentCycles: [],
          collisionEntriesOmitted: 0,
        },
      });
      return;
    }
    await route.fulfill({ status: 404, body: "unexpected route" });
  });

  await page.goto("/");
  await expect(page.locator("#core-status")).toHaveText("Rust/WASM Core ready");
  await expect(page.locator("#bridge-status")).toContainText("Disconnected");
  expect(requests).toHaveLength(0);

  await page.locator("#bridge-token").fill(token);
  await page.locator("#bridge-connect").click();
  await expect(page.locator("#bridge-status")).toHaveText("Connected · read-only");
  await expect(page.locator("#bridge-token")).toHaveValue("");
  expect(requests).toHaveLength(1);
  expect(requests[0].authorization).toBe(`Bearer ${token}`);

  let snapshot = await page.evaluate(() => window.__ragelabWebTools.bridgeSnapshot());
  expect(snapshot).toEqual({
    connected: true,
    endpoint: "http://127.0.0.1:32191",
    readOnly: true,
    writesEnabled: false,
    resultCount: 0,
  });
  const persisted = await page.evaluate(() => ({
    local: Object.values(localStorage),
    session: Object.values(sessionStorage),
  }));
  expect(JSON.stringify(persisted)).not.toContain(token);

  await page.locator("#bridge-search-input").fill("ymap-simple");
  await page.locator("#bridge-search-form").evaluate((form) => form.requestSubmit());
  await expect(page.locator("#bridge-search-results .bridge-result")).toHaveCount(2);
  await expect(page.locator("#bridge-search-results")).toContainText("ymap-simple.ymap");
  await expect(page.locator("#bridge-search-results")).toContainText("simple.ytd");
  snapshot = await page.evaluate(() => window.__ragelabWebTools.bridgeSnapshot());
  expect(snapshot.resultCount).toBe(2);

  const ymapRow = page.locator("#bridge-search-results .bridge-result").filter({
    hasText: "ymap-simple.ymap",
  });
  await ymapRow.locator("button").click();
  await expect(page.locator("#ymap-tool")).toBeVisible();
  await expect(page.locator("#ymap-asset-name")).toHaveText("ymap-simple.ymap");
  await expect(page.locator("#ymap-diagnostics")).toHaveClass(/ok/);
  const ymapSnapshot = await page.evaluate(() => window.__ragelabWebTools.ymapSnapshot());
  expect(ymapSnapshot).toEqual(expect.objectContaining({
    dependencyCount: 2,
    entityCount: 1,
    renderedEntityCount: 1,
    dirty: false,
    semanticValidated: false,
  }));

  const ytdRow = page.locator("#bridge-search-results .bridge-result").filter({
    hasText: "simple.ytd",
  });
  await ytdRow.locator("button").click();
  await expect(page.locator("#ytd-tool")).toBeVisible();
  await expect(page.locator("#asset-name")).toHaveText("simple.ytd");
  await expect(page.locator("#texture-count")).not.toHaveText("0 textures");

  expect(requests.every((request) => request.authorization === `Bearer ${token}`)).toBe(true);
  await page.locator("#bridge-disconnect").click();
  await expect(page.locator("#bridge-status")).toContainText("Disconnected");
  snapshot = await page.evaluate(() => window.__ragelabWebTools.bridgeSnapshot());
  expect(snapshot.connected).toBe(false);
});
