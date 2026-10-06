#!/usr/bin/env python3
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BUDGETS = ROOT / "fixtures" / "hardening" / "budgets.json"
MATRIX = ROOT / "fixtures" / "hardening" / "compatibility-matrix.json"

budgets = json.loads(BUDGETS.read_text(encoding="utf-8"))
matrix = json.loads(MATRIX.read_text(encoding="utf-8"))

assert budgets["schema"] == "ragelab.hardening.budgets"
assert budgets["schemaVersion"] == 1
for section in ("native", "web", "release"):
    assert section in budgets and budgets[section], f"missing budget section {section}"
    for key, value in budgets[section].items():
        assert isinstance(value, int) and value > 0, f"invalid budget {section}.{key}"

assert matrix["schema"] == "ragelab.hardening.compatibility-matrix"
assert matrix["schemaVersion"] == 1
layers = {case["layer"] for case in matrix["cases"]}
required = {"base", "update", "dlc", "world", "interior"}
assert required <= layers, f"compatibility matrix missing layers: {sorted(required - layers)}"

seen = set()
for case in matrix["cases"]:
    case_id = case["id"]
    assert case_id not in seen, f"duplicate case id: {case_id}"
    seen.add(case_id)
    relative = Path(case["path"])
    assert not relative.is_absolute(), f"fixture path must be repository-relative: {relative}"
    normalized = relative.as_posix()
    assert normalized.startswith("fixtures/synthetic/"), (
        f"committed compatibility bytes must be synthetic: {relative}"
    )
    assert "private" not in relative.parts, f"private fixture cannot enter public matrix: {relative}"
    full = ROOT / relative
    assert full.is_file(), f"missing fixture: {relative}"
    assert full.stat().st_size > 0, f"empty fixture is not representative: {relative}"
    assert case.get("checks"), f"case has no checks: {case_id}"

profiles = matrix.get("legacyBuildProfiles", [])
assert len(profiles) >= 2, "at least two synthetic Legacy build profiles are required"
assert len({profile["id"] for profile in profiles}) == len(profiles)

renderer_source = (ROOT / "crates" / "ragelab-render" / "src" / "lib.rs").read_text(
    encoding="utf-8"
)
assert "max_asset_bytes: 384 * 1024 * 1024" in renderer_source
assert "max_texture_bytes: 256 * 1024 * 1024" in renderer_source
assert budgets["native"]["gpuAssetCacheBytes"] == 384 * 1024 * 1024
assert budgets["native"]["gpuTextureCacheBytes"] == 256 * 1024 * 1024

print(json.dumps({
    "schema": "ragelab.hardening.fixture-check",
    "schemaVersion": 1,
    "cases": len(matrix["cases"]),
    "layers": sorted(layers),
    "legacyBuildProfiles": len(profiles),
    "budgets": budgets,
}, separators=(",", ":")))
