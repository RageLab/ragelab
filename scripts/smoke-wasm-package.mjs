import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { pathToFileURL } from "node:url";

const repo = resolve(import.meta.dirname, "..");
const packageDir = resolve(repo, process.argv[2] ?? "target/ragelab-wasm-pkg");
const api = await import(pathToFileURL(join(packageDir, "ragelab_wasm.js")).href);
const wasm = readFileSync(join(packageDir, "ragelab_wasm_bg.wasm"));
api.initSync({ module: wasm });

const caps = api.capabilities();
assert.equal(caps.schema, "ragelab.wasm.capabilities");
assert.ok(caps.desktopOnly.includes("rpf"));
assert.ok(caps.desktopOnly.includes("gtaKeys"));

const fixture = (...parts) => readFileSync(join(repo, "fixtures", "synthetic", ...parts));
const ymap = fixture("stream", "simple.ymap");
const ytyp = fixture("mlo.ytyp");
const ydr = fixture("ydr", "simple.ydr");
const ydd = fixture("ydd", "editable.ydd");
const ytd = fixture("ytd", "simple.ytd");

for (const [format, bytes] of [["ymap", ymap], ["ytyp", ytyp], ["ydr", ydr], ["ydd", ydd], ["ytd", ytd]]) {
  const report = api.validateAsset(format, bytes);
  assert.equal(report.valid, true);
}

const model = api.ydrModel(ydr);
assert.ok(model.positions().length > 0);
assert.ok(model.indices().length > 0);
const texture = api.ytdTexture(ytd, 0);
assert.ok(texture.rgba().length > 0);

const png = api.exportYtdTexturePng(ytd, 0);
const rewritten = api.replaceYtdTexturePng(ytd, 0, png);
assert.equal(api.validateAsset("ytd", rewritten).valid, true);

const mip1 = api.ytdTextureMip(rewritten, 0, 1);
assert.equal(mip1.metadata().mipIndex, 1);
assert.equal(mip1.metadata().width, 2);
assert.equal(mip1.metadata().height, 2);

let mipRejected = false;
try {
  api.ytdTextureMip(rewritten, 0, 99);
} catch (error) {
  mipRejected = error?.schema === "ragelab.wasm.error" && error?.code === "indexOutOfBounds";
}
assert.equal(mipRejected, true);

let rejected = false;
try {
  api.validateAsset("rpf", new Uint8Array());
} catch (error) {
  rejected = error?.schema === "ragelab.wasm.error" && error?.code === "unsupportedFormat";
}
assert.equal(rejected, true);

console.log(JSON.stringify({
  ok: true,
  packageDir,
  modelPositions: model.positions().length,
  textureBytes: texture.rgba().length,
  rewrittenYtdBytes: rewritten.length
}, null, 2));
