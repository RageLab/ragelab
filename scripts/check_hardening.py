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

# Test tiers are separate contracts. In particular, the previously waived
# standalone Legacy runtime gate must never become an implicit CI dependency.
gates = {gate["id"]: gate for gate in matrix.get("validationGates", [])}
required_gates = {
    "repository-tests",
    "static-analysis",
    "format-and-compile",
    "semantic-round-trip",
    "codewalker-core-parser",
    "codewalker-gui",
    "gta-v-legacy-runtime",
    "fivem-runtime",
}
assert required_gates <= gates.keys(), (
    f"validation matrix missing gates: {sorted(required_gates - gates.keys())}"
)
assert gates["repository-tests"]["status"] == "required_ci"
assert gates["static-analysis"]["status"] == "required_ci"
assert gates["format-and-compile"]["status"] == "required_ci"
assert gates["semantic-round-trip"]["status"] == "required_ci"
assert gates["codewalker-core-parser"]["status"] == "external_optional"
codewalker_evidence = gates["codewalker-core-parser"]["lastRecordedEvidence"]
assert codewalker_evidence["tool"] == "CodeWalker Core parser"
assert codewalker_evidence["revision"] == "485d56b"
assert codewalker_evidence["accepted"] == 5
assert codewalker_evidence["rejected"] == 0
assert codewalker_evidence["automation"] == "manual_external_not_ci"
assert gates["codewalker-gui"]["status"] == "manual_external"
gui_evidence = gates["codewalker-gui"]["lastRecordedEvidence"]
assert gui_evidence["result"] == "user_confirmed_visible"
assert gui_evidence["drawnGeometryCount"] == 140
assert gui_evidence["automation"] == "manual_gui_user_confirmed"
assert gates["gta-v-legacy-runtime"]["status"] == "waived"
assert gates["gta-v-legacy-runtime"]["requiresExplicitApprovalToReopen"] is True
assert gates["fivem-runtime"]["status"] == "manual_external"
fivem_evidence = gates["fivem-runtime"]["lastRecordedEvidence"]
assert fivem_evidence["result"] == "PASS_USER_CONFIRMED"
assert set(fivem_evidence["presets"]) == {
    "normal.sps",
    "spec.sps",
    "normal_spec_tnt.sps",
    "emissivenight.sps",
    "emissive.sps",
}
assert fivem_evidence["logsOrScreenshotsArchived"] is False
assert fivem_evidence["runtimeRenderableScope"] == "FiveM_only"

# Assert parser/resource budgets remain explicit and the deterministic
# mutation suites stay wired into the normal workspace test command.
def source(relative: str) -> str:
    return (ROOT / relative).read_text(encoding="utf-8")

budget_contracts = {
    "crates/ragelab-gltf/src/parser.rs": [
        "MAX_GLB_BYTES: usize = 512 * 1024 * 1024",
    ],
    "crates/ragelab-gltf/src/decode.rs": [
        "MAX_VERTICES_PER_PRIMITIVE: usize = 2_000_000",
        "MAX_INDICES_PER_PRIMITIVE: usize = 6_000_000",
    ],
    "crates/ragelab-gltf/src/images.rs": [
        "MAX_ENCODED_IMAGE_BYTES: usize = 128 * 1024 * 1024",
        "MAX_TOTAL_IMAGE_BYTES: usize = 512 * 1024 * 1024",
        "MAX_IMAGE_PIXELS: usize = 64 * 1024 * 1024",
        "MAX_IMAGE_DIMENSION: usize = 16_384",
    ],
    "crates/ragelab-rbf/src/lib.rs": [
        "const MAX_DEPTH: usize = 256",
        "const MAX_NODES: usize = 1_000_000",
        "parser_handles_deterministic_mutation_and_random_corpus_without_panicking",
    ],
    "crates/ragelab-ydr/src/lib.rs": [
        "const MAX_VERTEX_BYTES: usize = 256 * 1024 * 1024",
        "const MAX_MODELS: usize = 4_096",
        "const MAX_NAME_LENGTH: usize = 1_024",
    ],
    "crates/ragelab-ybn/src/lib.rs": [
        "const MAX_TOTAL_VERTICES: usize = 4_000_000",
        "const MAX_TOTAL_INDICES: usize = 24_000_000",
    ],
    "crates/ragelab-ytd/src/lib.rs": [
        "const MAX_DECODED_RGBA_BYTES: usize = 256 * 1024 * 1024",
        "const MAX_GENERATED_RGBA_MIP_BYTES: usize = 512 * 1024 * 1024",
        "const MAX_TEXTURE_NAME_BYTES: usize = 4_096",
    ],
    "crates/ragelab-engine/src/glb.rs": [
        "glb_inspection_and_geometry_handle_deterministic_mutation_corpus",
    ],
    "crates/ragelab-engine/src/discovery.rs": [
        "MAX_STEAM_METADATA_BYTES: usize = 1024 * 1024",
        "MAX_CITIZENFX_INI_BYTES: usize = 256 * 1024",
        "steam_manifest_parsers_handle_deterministic_mutation_corpus",
        "bounded_metadata_reader_rejects_oversized_manifest_files",
    ],
    "crates/ragelab-resource/src/lib.rs": [
        "rsc7_header_probe_and_resource_parser_handle_bounded_mutation_corpus",
    ],
}
for relative, required in budget_contracts.items():
    body = source(relative)
    for token in required:
        assert token in body, f"missing hardening contract {token!r} in {relative}"

ci_workflow = source(".github/workflows/ci.yml")
for required_command in (
    "cargo fmt --all -- --check",
    "cargo check --locked --workspace",
    "cargo clippy --locked --workspace --all-targets -- -D warnings",
    "cargo test --locked --workspace",
):
    assert required_command in ci_workflow, (
        f"CI workflow is missing locked quality gate: {required_command}"
    )
for fixture_gate in (
    "python3 scripts/validate_synthetic.py",
    "python3 scripts/validate_synthetic_ymf.py",
    "python3 scripts/check_hardening.py",
):
    assert fixture_gate in ci_workflow, (
        f"CI workflow is missing fixture gate: {fixture_gate}"
    )

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
