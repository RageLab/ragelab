import { readFile, readdir, stat } from "node:fs/promises";
import { resolve, relative } from "node:path";

const appRoot = process.cwd();
const repoRoot = resolve(appRoot, "..", "..");
const budgets = JSON.parse(
  await readFile(resolve(repoRoot, "fixtures", "hardening", "budgets.json"), "utf8"),
).web;
const dist = resolve(appRoot, "dist");
const vendorWasm = resolve(appRoot, "vendor", "wasm", "ragelab_wasm_bg.wasm");

async function filesRecursive(root) {
  const out = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) out.push(...await filesRecursive(path));
    else out.push(path);
  }
  return out;
}

const files = await filesRecursive(dist);
const js = files.filter((path) => path.endsWith(".js"));
const css = files.filter((path) => path.endsWith(".css"));
const sum = async (paths) => {
  let total = 0;
  for (const path of paths) total += (await stat(path)).size;
  return total;
};

const report = {
  schema: "ragelab.web.budget-report",
  schemaVersion: 1,
  wasmBytes: (await stat(vendorWasm)).size,
  javascriptBytes: await sum(js),
  cssBytes: await sum(css),
  files: files.map((path) => relative(appRoot, path).replaceAll("\\", "/")),
  budgets,
};
const failures = [];
for (const key of ["wasmBytes", "javascriptBytes", "cssBytes"]) {
  if (report[key] > budgets[key]) failures.push(`${key} ${report[key]} > ${budgets[key]}`);
}
console.log(JSON.stringify(report));
if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}
