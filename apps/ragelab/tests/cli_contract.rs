use std::{
    fs,
    path::PathBuf,
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use ragelab_ytd::Ytd;
use serde_json::{json, Value};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ragelab"))
}

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic")
        .join(path)
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout should contain valid JSON")
}

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("ragelab-cli-{label}-{}-{nonce}", process::id()))
}

fn operation_document(source: &str, output: &str) -> String {
    json!({
        "schema": "ragelab.operation",
        "schemaVersion": 1,
        "source": source,
        "output": output,
        "operations": [
            {
                "type": "ydr.translate",
                "delta": [1.0, 2.0, 3.0]
            }
        ]
    })
    .to_string()
}

#[test]
fn inspect_json_uses_versioned_response_envelope() {
    let output = binary()
        .args([
            "inspect",
            fixture("simple.ymap").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body["schema"], "ragelab.cli.response");
    assert_eq!(body["schemaVersion"], 1);
    assert_eq!(body["ok"], true);
    assert_eq!(body["command"], "inspect");
    assert_eq!(body["data"]["type"], "YMAP");
    assert_eq!(body["data"]["details"]["entities"], 1);
}

#[test]
fn corrupt_asset_fails_closed_with_structured_validation_error() {
    let root = temp_root("corrupt-asset");
    fs::create_dir_all(&root).unwrap();
    let source = fixture("ydr/editable.ydr");
    let corrupt = root.join("corrupt.ydr");
    let bytes = fs::read(source).unwrap();
    fs::write(&corrupt, &bytes[..32]).unwrap();

    let output = binary()
        .args(["inspect", corrupt.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab inspect corrupt asset should run");

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stderr.is_empty());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "inspect");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "validation_failed");
    assert!(body["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("decompression")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsupported_preview_layout_is_distinct_from_corruption() {
    let output = binary()
        .args([
            "preview",
            fixture("yft/no-main-drawable.yft").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab preview unsupported YFT should run");

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stderr.is_empty());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "preview");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "unsupported");
    assert!(body["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("pristine main drawable")));
}

#[test]
fn validate_json_reports_checks() {
    let output = binary()
        .args([
            "validate",
            fixture("ytd/simple.ytd").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "validate");
    assert_eq!(body["data"]["valid"], true);
    assert!(body["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check == "ytd.parse"));
}

#[test]
fn structured_invalid_input_uses_exit_code_two() {
    let output = binary()
        .args(["inspect", "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());

    let body = stdout_json(&output);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "invalid_input");
}

#[test]
fn structured_validation_failure_uses_exit_code_four() {
    let output = binary()
        .args([
            "validate",
            fixture("stream/test_drawable.ydr").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stderr.is_empty());

    let body = stdout_json(&output);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "validation_failed");
}

#[test]
fn namespaced_ymap_command_routes_to_existing_implementation() {
    let output = binary()
        .args(["ymap", "info", fixture("simple.ymap").to_str().unwrap()])
        .output()
        .expect("ragelab should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("entities: 1"));
}

#[test]
fn global_capabilities_preserve_legacy_ids_and_advertise_canonical_ids() {
    let output = binary()
        .args(["capabilities", "--json"])
        .output()
        .expect("ragelab should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    let commands = body["commands"].as_array().unwrap();
    let canonical = body["canonicalCommands"].as_array().unwrap();

    assert!(commands.iter().any(|value| value == "ydr-info"));
    assert!(commands.iter().any(|value| value == "ydr.info"));
    assert!(canonical.iter().any(|value| value == "ydr.info"));
    assert!(canonical.iter().any(|value| value == "spatial"));
    assert!(canonical.iter().any(|value| value == "preview"));
    assert!(canonical.iter().any(|value| value == "workspace.preflight"));
    assert!(canonical.iter().any(|value| value == "workspace.export"));
    assert!(canonical.iter().any(|value| value == "workspace.scene"));
    assert!(canonical.iter().any(|value| value == "render.asset"));
    assert!(canonical.iter().any(|value| value == "render.scene"));
    assert!(canonical.iter().any(|value| value == "render.compare"));
    assert!(canonical.iter().any(|value| value == "gta.discover"));
    assert!(canonical.iter().any(|value| value == "gta.catalog"));
    assert!(canonical.iter().any(|value| value == "gta.world-query"));
    assert!(canonical.iter().any(|value| value == "fivem.discover"));
    assert_eq!(body["responseEnvelope"]["schema"], "ragelab.cli.response");
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "plan"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "apply"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "spatial"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "preview"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "workspace.preflight"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "workspace.export"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "workspace.scene"));
    for command in ["render.asset", "render.scene", "render.compare"] {
        assert!(body["structuredOutput"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == command));
    }
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "gta.discover"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "gta.catalog"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "gta.world-query"));
    assert!(body["structuredOutput"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "fivem.discover"));
}

#[test]
fn declarative_plan_and_apply_are_structured_and_non_destructive() {
    let root = temp_root("plan-apply");
    fs::create_dir_all(&root).unwrap();

    let source = root.join("source.ydr");
    let output = root.join("output.ydr");
    let operation = root.join("operation.json");
    fs::copy(fixture("ydr/simple.ydr"), &source).unwrap();
    let source_before = fs::read(&source).unwrap();
    fs::write(&operation, operation_document("source.ydr", "output.ydr")).unwrap();

    let plan_output = binary()
        .args(["plan", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab plan should run");
    assert!(plan_output.status.success());

    let plan = stdout_json(&plan_output);
    assert_eq!(plan["command"], "plan");
    assert_eq!(plan["data"]["schema"], "ragelab.operation.plan");
    assert_eq!(plan["data"]["allowed"], true);
    assert_eq!(plan["data"]["nonDestructive"], true);
    assert!(!output.exists());

    let apply_output = binary()
        .args(["apply", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab apply should run");
    assert!(apply_output.status.success());

    let apply = stdout_json(&apply_output);
    assert_eq!(apply["command"], "apply");
    assert_eq!(apply["data"]["schema"], "ragelab.operation.apply");
    assert_eq!(apply["data"]["nonDestructive"], true);
    assert_eq!(apply["data"]["validation"]["semanticReopen"], true);
    assert_eq!(apply["data"]["validation"]["sourceUnchanged"], true);
    assert!(output.is_file());
    assert_eq!(fs::read(&source).unwrap(), source_before);

    let validate_output = binary()
        .args(["validate", output.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab validate should run");
    assert!(validate_output.status.success());
    assert_eq!(stdout_json(&validate_output)["data"]["valid"], true);

    let second_apply = binary()
        .args(["apply", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab second apply should run");
    assert_eq!(second_apply.status.code(), Some(3));
    assert_eq!(stdout_json(&second_apply)["error"]["code"], "unsupported");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ybn_capabilities_and_declarative_polygon_edit_are_agent_accessible() {
    let root = temp_root("ybn-plan-apply");
    fs::create_dir_all(&root).unwrap();

    let source = root.join("source.ybn");
    let output = root.join("output.ybn");
    let operation = root.join("operation.json");
    fs::copy(fixture("ybn/simple.ybn"), &source).unwrap();
    let source_before = fs::read(&source).unwrap();

    let capabilities_output = binary()
        .args(["capabilities", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab capabilities should run");
    assert!(capabilities_output.status.success());
    let capabilities = stdout_json(&capabilities_output);
    let operations = capabilities["data"]["operations"].as_array().unwrap();
    let polygon_edit = operations
        .iter()
        .find(|operation| operation["id"] == "ybn.edit-polygon")
        .unwrap();
    assert_eq!(polygon_edit["availability"], "parameterized");
    let plan_capability = operations
        .iter()
        .find(|operation| operation["id"] == "plan")
        .unwrap();
    assert_eq!(plan_capability["availability"], "available");
    assert!(operations
        .iter()
        .any(|operation| operation["id"] == "apply"));

    let body = json!({
        "schema": "ragelab.operation",
        "schemaVersion": 1,
        "source": "source.ybn",
        "output": "output.ybn",
        "operations": [
            {
                "type": "ybn.edit-polygon",
                "childIndex": 0,
                "polygonIndex": 0,
                "kind": "sphere",
                "radius": 2.75,
                "materialIndex": 1
            }
        ]
    });
    fs::write(&operation, body.to_string()).unwrap();

    let plan_output = binary()
        .args(["plan", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab plan should run");
    assert!(plan_output.status.success());
    assert_eq!(stdout_json(&plan_output)["data"]["allowed"], true);
    assert!(!output.exists());

    let apply_output = binary()
        .args(["apply", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab apply should run");
    assert!(apply_output.status.success());
    let apply = stdout_json(&apply_output);
    assert_eq!(apply["data"]["assetType"], "YBN");
    assert_eq!(apply["data"]["validation"]["semanticReopen"], true);
    assert_eq!(apply["data"]["validation"]["sourceUnchanged"], true);
    assert_eq!(fs::read(&source).unwrap(), source_before);

    let validate_output = binary()
        .args(["validate", output.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab validate should run");
    assert!(validate_output.status.success());
    assert_eq!(stdout_json(&validate_output)["data"]["valid"], true);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn spatial_json_preserves_fail_closed_world_semantics() {
    let ymap_output = binary()
        .args([
            "spatial",
            fixture("simple.ymap").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab spatial should run");
    assert!(ymap_output.status.success());
    let ymap = stdout_json(&ymap_output);
    assert_eq!(ymap["command"], "spatial");
    assert_eq!(ymap["data"]["type"], "YMAP");
    assert_eq!(ymap["data"]["context"]["classification"], "worldSpatial");

    let ydr_output = binary()
        .args([
            "spatial",
            fixture("ydr/simple.ydr").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab spatial should run");
    assert!(ydr_output.status.success());
    let ydr = stdout_json(&ydr_output);
    assert_eq!(ydr["data"]["type"], "YDR");
    assert_eq!(ydr["data"]["context"]["classification"], "localOnly");
    assert_eq!(
        ydr["data"]["context"]["reason"]["code"],
        "localCoordinatesOnly"
    );
    assert!(ydr["data"]["context"]["worldTransform"].is_null());
}

#[test]
fn rpf_keys_reuses_seeded_cache_with_structured_report() {
    let root = temp_root("rpf-keys");
    let exe = root.join("GTA5.exe");
    let cache_root = root.join("cache");
    fs::create_dir_all(&root).unwrap();
    fs::write(&exe, b"synthetic-executable").unwrap();

    let metadata = fs::metadata(&exe).unwrap();
    let modified = metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let cache_dir = cache_root.join(format!("{}-{modified}", metadata.len()));
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(cache_dir.join("gtav_aes_key.dat"), vec![0x11_u8; 32]).unwrap();
    fs::write(cache_dir.join("gtav_ng_key.dat"), vec![0x22_u8; 101 * 272]).unwrap();
    fs::write(
        cache_dir.join("gtav_ng_decrypt_tables.dat"),
        vec![0x33_u8; 17 * 16 * 256 * 4],
    )
    .unwrap();

    let output = binary()
        .args([
            "rpf",
            "keys",
            exe.to_str().unwrap(),
            "--cache-root",
            cache_root.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab rpf keys should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body["schema"], "ragelab.gta.rpf-keys");
    assert_eq!(body["schemaVersion"], 1);
    assert_eq!(body["cacheHit"], true);
    assert_eq!(body["cache"], cache_dir.display().to_string());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workspace_scene_json_exposes_resolved_assets_and_local_collision() {
    let output = binary()
        .args([
            "workspace",
            "scene",
            fixture("stream").to_str().unwrap(),
            "simple.ymap",
            "--max-nodes",
            "100",
            "--json",
        ])
        .output()
        .expect("ragelab workspace scene should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "scene");
    assert_eq!(body["data"]["summary"]["totalEntities"], 1);
    assert_eq!(body["data"]["summary"]["resolvedNodes"], 1);
    assert_eq!(body["data"]["nodes"][0]["assetKind"], "YDR");
    assert_eq!(body["data"]["nodes"][0]["collision"]["state"], "localOnly");
    assert_eq!(body["data"]["limits"]["truncated"], false);
}

#[test]
fn workspace_scene_accepts_read_only_fallback_root() {
    let root = temp_root("scene-fallback");
    let primary = root.join("primary");
    let fallback = root.join("fallback");
    fs::create_dir_all(&primary).unwrap();
    fs::create_dir_all(&fallback).unwrap();

    fs::copy(fixture("stream/simple.ymap"), primary.join("simple.ymap")).unwrap();
    for name in ["simple.ytyp", "test_drawable.ydr", "test_collision.ybn"] {
        fs::copy(fixture(&format!("stream/{name}")), fallback.join(name)).unwrap();
    }

    let output = binary()
        .args([
            "workspace",
            "scene",
            primary.to_str().unwrap(),
            "simple.ymap",
            "--fallback-root",
            fallback.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab workspace scene with fallback should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body["data"]["summary"]["resolvedNodes"], 1);
    let asset_path = body["data"]["assets"][0]["path"]
        .as_str()
        .expect("resolved asset path");
    assert!(PathBuf::from(asset_path).starts_with(&fallback));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn namespaced_structured_errors_use_canonical_command_id() {
    let output = binary()
        .args([
            "workspace",
            "scene",
            fixture("stream").to_str().unwrap(),
            "simple.ymap",
            "--max-nodes",
            "0",
            "--json",
        ])
        .output()
        .expect("ragelab workspace scene should run");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "scene");
    assert_eq!(body["error"]["code"], "invalid_input");
}

#[test]
fn ydr_capabilities_use_writer_evidence_for_translation() {
    let output = binary()
        .args([
            "capabilities",
            fixture("ydr/simple.ydr").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab capabilities should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    let operations = body["data"]["operations"].as_array().unwrap();
    let translate = operations
        .iter()
        .find(|operation| operation["id"] == "ydr.translate")
        .unwrap();
    assert_eq!(translate["availability"], "available");
    assert_eq!(translate["requiresParameters"][0], "delta");
}

#[test]
fn ymap_capabilities_mark_workspace_scene_as_context_required() {
    let output = binary()
        .args([
            "capabilities",
            fixture("simple.ymap").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab capabilities should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    let operations = body["data"]["operations"].as_array().unwrap();
    let scene = operations
        .iter()
        .find(|operation| operation["id"] == "workspace.scene")
        .unwrap();
    assert_eq!(scene["availability"], "contextRequired");
    assert_eq!(scene["requiresWorkspace"], true);
}

#[test]
fn ydr_rebinds_are_declarative_and_non_destructive() {
    let root = temp_root("ydr-rebind");
    fs::create_dir_all(&root).unwrap();

    let source = root.join("source.ydr");
    let output = root.join("output.ydr");
    let operation = root.join("operation.json");
    fs::copy(fixture("ydr/editable.ydr"), &source).unwrap();
    let source_before = fs::read(&source).unwrap();

    let capabilities_output = binary()
        .args(["capabilities", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab capabilities should run");
    assert!(capabilities_output.status.success());
    let capabilities = stdout_json(&capabilities_output);
    let operations = capabilities["data"]["operations"].as_array().unwrap();
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "ydr.rebind-texture")
            .unwrap()["availability"],
        "parameterized"
    );
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "ydr.rebind-shader")
            .unwrap()["availability"],
        "parameterized"
    );
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "plan")
            .unwrap()["availability"],
        "available"
    );

    let body = json!({
        "schema": "ragelab.operation",
        "schemaVersion": 1,
        "source": "source.ydr",
        "output": "output.ydr",
        "operations": [
            {
                "type": "ydr.rebind-texture",
                "sourceShader": 0,
                "sourceParameter": 0,
                "targetShader": 0,
                "targetParameter": 1
            },
            {
                "type": "ydr.rebind-shader",
                "modelIndex": 0,
                "geometryIndex": 0,
                "targetShaderIndex": 1
            }
        ]
    });
    fs::write(&operation, body.to_string()).unwrap();

    let plan_output = binary()
        .args(["plan", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab plan should run");
    assert!(plan_output.status.success());
    let plan = stdout_json(&plan_output);
    assert_eq!(plan["data"]["allowed"], true);
    assert_eq!(
        plan["data"]["operations"][0]["details"]["targetTexture"],
        "test_normal"
    );
    assert_eq!(
        plan["data"]["operations"][1]["details"]["targetShaderIndex"],
        1
    );
    assert!(!output.exists());

    let apply_output = binary()
        .args(["apply", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab apply should run");
    assert!(apply_output.status.success());
    let apply = stdout_json(&apply_output);
    assert_eq!(apply["data"]["assetType"], "YDR");
    assert_eq!(apply["data"]["details"]["textureRebinds"], 1);
    assert_eq!(apply["data"]["details"]["shaderRebinds"], 1);
    assert_eq!(apply["data"]["validation"]["semanticReopen"], true);
    assert_eq!(apply["data"]["validation"]["sourceUnchanged"], true);
    assert_eq!(fs::read(&source).unwrap(), source_before);

    let validate_output = binary()
        .args(["validate", output.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab validate should run");
    assert!(validate_output.status.success());
    assert_eq!(stdout_json(&validate_output)["data"]["valid"], true);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ydd_writers_are_declarative_and_non_destructive() {
    let root = temp_root("ydd-plan-apply");
    fs::create_dir_all(&root).unwrap();

    let source = root.join("source.ydd");
    let output = root.join("output.ydd");
    let operation = root.join("operation.json");
    fs::copy(fixture("ydd/editable.ydd"), &source).unwrap();
    let source_before = fs::read(&source).unwrap();

    let capabilities_output = binary()
        .args(["capabilities", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab capabilities should run");
    assert!(capabilities_output.status.success());
    let capabilities = stdout_json(&capabilities_output);
    let operations = capabilities["data"]["operations"].as_array().unwrap();
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "plan")
            .unwrap()["availability"],
        "available"
    );
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "ydd.translate")
            .unwrap()["availability"],
        "parameterized"
    );
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "ydd.rebind-texture")
            .unwrap()["availability"],
        "parameterized"
    );
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "ydd.rebind-shader")
            .unwrap()["availability"],
        "parameterized"
    );

    let body = json!({
        "schema": "ragelab.operation",
        "schemaVersion": 1,
        "source": "source.ydd",
        "output": "output.ydd",
        "operations": [
            {
                "type": "ydd.translate",
                "drawableIndex": 0,
                "delta": [1.0, 0.0, 0.0]
            },
            {
                "type": "ydd.rebind-texture",
                "drawableIndex": 0,
                "sourceShader": 0,
                "sourceParameter": 0,
                "targetShader": 0,
                "targetParameter": 1
            },
            {
                "type": "ydd.rebind-shader",
                "drawableIndex": 0,
                "modelIndex": 0,
                "geometryIndex": 0,
                "targetShaderIndex": 1
            }
        ]
    });
    fs::write(&operation, body.to_string()).unwrap();

    let plan_output = binary()
        .args(["plan", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab plan should run");
    assert!(plan_output.status.success());
    let plan = stdout_json(&plan_output);
    assert_eq!(plan["data"]["allowed"], true);
    assert_eq!(plan["data"]["assetType"], "YDD");
    assert_eq!(plan["data"]["operations"][0]["details"]["drawableIndex"], 0);
    assert_eq!(
        plan["data"]["operations"][1]["details"]["targetTexture"],
        "dict_normal"
    );
    assert_eq!(
        plan["data"]["operations"][2]["details"]["targetShaderIndex"],
        1
    );
    assert!(!output.exists());

    let apply_output = binary()
        .args(["apply", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab apply should run");
    assert!(apply_output.status.success());
    let apply = stdout_json(&apply_output);
    assert_eq!(apply["data"]["assetType"], "YDD");
    assert_eq!(apply["data"]["details"]["translations"], 1);
    assert_eq!(apply["data"]["details"]["textureRebinds"], 1);
    assert_eq!(apply["data"]["details"]["shaderRebinds"], 1);
    assert_eq!(apply["data"]["details"]["editedDrawableIndices"][0], 0);
    assert_eq!(apply["data"]["validation"]["semanticReopen"], true);
    assert_eq!(apply["data"]["validation"]["sourceUnchanged"], true);
    assert_eq!(fs::read(&source).unwrap(), source_before);

    let validate_output = binary()
        .args(["validate", output.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab validate should run");
    assert!(validate_output.status.success());
    assert_eq!(stdout_json(&validate_output)["data"]["valid"], true);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ytd_inventory_and_writers_are_structured_and_non_destructive() {
    let root = temp_root("ytd-plan-apply");
    fs::create_dir_all(&root).unwrap();

    let source = root.join("source.ytd");
    let output = root.join("output.ytd");
    let operation = root.join("operation.json");
    fs::copy(fixture("ytd/simple.ytd"), &source).unwrap();
    let source_before = fs::read(&source).unwrap();

    let inspect_output = binary()
        .args(["inspect", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab inspect should run");
    assert!(inspect_output.status.success());
    let inspect = stdout_json(&inspect_output);
    assert_eq!(inspect["data"]["type"], "YTD");
    assert_eq!(inspect["data"]["details"]["textureCount"], 1);
    assert_eq!(
        inspect["data"]["details"]["textures"][0]["name"],
        "synthetic_diffuse"
    );
    assert_eq!(
        inspect["data"]["details"]["textures"][0]["preview"]["topMipRgba"],
        true
    );
    assert_eq!(
        inspect["data"]["details"]["textures"][0]["preview"]["classicDds"],
        true
    );
    assert_eq!(
        inspect["data"]["details"]["textures"][0]["writers"]["rgbaRepack"],
        true
    );

    let capabilities_output = binary()
        .args(["capabilities", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab capabilities should run");
    assert!(capabilities_output.status.success());
    let capabilities = stdout_json(&capabilities_output);
    let operations = capabilities["data"]["operations"].as_array().unwrap();
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "plan")
            .unwrap()["availability"],
        "available"
    );
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "ytd.repack-rgba")
            .unwrap()["availability"],
        "parameterized"
    );
    assert_eq!(
        operations
            .iter()
            .find(|operation| operation["id"] == "ytd.rebuild-compact")
            .unwrap()["availability"],
        "available"
    );

    let original_dds = Ytd::texture_dds(&source_before, 0).unwrap();
    fs::write(root.join("same.dds"), &original_dds).unwrap();

    let resized_rgba = vec![
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let resized_dds = Ytd::rgba8_dds_with_generated_mips(2, 2, &resized_rgba).unwrap();
    fs::write(root.join("resized.dds"), &resized_dds).unwrap();

    let rgba = vec![64_u8; 4 * 4 * 4];
    fs::write(root.join("replacement.rgba"), &rgba).unwrap();

    let body = json!({
        "schema": "ragelab.operation",
        "schemaVersion": 1,
        "source": "source.ytd",
        "output": "output.ytd",
        "operations": [
            {
                "type": "ytd.replace-dds",
                "textureIndex": 0,
                "replacement": "same.dds"
            },
            {
                "type": "ytd.repack-dds",
                "textureIndex": 0,
                "replacement": "resized.dds"
            },
            {
                "type": "ytd.repack-rgba",
                "textureIndex": 0,
                "width": 4,
                "height": 4,
                "replacement": "replacement.rgba"
            },
            {
                "type": "ytd.rebuild-compact"
            }
        ]
    });
    fs::write(&operation, body.to_string()).unwrap();

    let plan_output = binary()
        .args(["plan", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab plan should run");
    assert!(plan_output.status.success());
    let plan = stdout_json(&plan_output);
    assert_eq!(plan["data"]["allowed"], true);
    assert_eq!(plan["data"]["assetType"], "YTD");
    assert_eq!(
        plan["data"]["operations"][0]["details"]["layoutPreserved"],
        true
    );
    assert_eq!(
        plan["data"]["operations"][1]["details"]["sizeAfter"],
        json!([2, 2])
    );
    assert_eq!(
        plan["data"]["operations"][2]["details"]["sizeAfter"],
        json!([4, 4])
    );
    assert_eq!(
        plan["data"]["operations"][3]["details"]["layoutRebuilt"],
        true
    );
    assert!(!output.exists());

    let apply_output = binary()
        .args(["apply", operation.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab apply should run");
    assert!(apply_output.status.success());
    let apply = stdout_json(&apply_output);
    assert_eq!(apply["data"]["assetType"], "YTD");
    assert_eq!(apply["data"]["details"]["replaceDds"], 1);
    assert_eq!(apply["data"]["details"]["repackDds"], 1);
    assert_eq!(apply["data"]["details"]["repackRgba"], 1);
    assert_eq!(apply["data"]["details"]["compactRebuilds"], 1);
    assert_eq!(apply["data"]["validation"]["semanticReopen"], true);
    assert_eq!(apply["data"]["validation"]["sourceUnchanged"], true);
    assert_eq!(fs::read(&source).unwrap(), source_before);

    let validate_output = binary()
        .args(["validate", output.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab validate should run");
    assert!(validate_output.status.success());
    assert_eq!(stdout_json(&validate_output)["data"]["valid"], true);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workspace_preflight_is_structured_for_single_and_combined_maps() {
    let single = binary()
        .args([
            "workspace",
            "preflight",
            fixture("stream").to_str().unwrap(),
            "simple.ymap",
            "--json",
        ])
        .output()
        .expect("ragelab workspace preflight should run");
    assert!(single.status.success());
    let single = stdout_json(&single);
    assert_eq!(single["command"], "preflight");
    assert_eq!(single["data"]["selectedRoots"][0], "simple.ymap");
    assert_eq!(single["data"]["closureYmaps"].as_array().unwrap().len(), 1);
    assert_eq!(single["data"]["unresolved"]["unknown"], 0);
    assert_eq!(single["data"]["exportGate"]["allowedWithoutOverride"], true);

    let combined = binary()
        .args([
            "workspace",
            "preflight",
            fixture("ymap-relations").to_str().unwrap(),
            "relation_parent.ymap",
            "relation_child_a.ymap",
            "--json",
        ])
        .output()
        .expect("ragelab combined preflight should run");
    assert!(combined.status.success());
    let combined = stdout_json(&combined);
    assert_eq!(combined["command"], "preflight");
    assert_eq!(
        combined["data"]["selectedRoots"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        combined["data"]["closureYmaps"].as_array().unwrap().len(),
        4
    );
    assert!(combined["data"]["unresolved"]["unknown"].as_u64().unwrap() > 0);
    assert_eq!(
        combined["data"]["exportGate"]["requiresAllowUnresolved"],
        true
    );
}

#[test]
fn workspace_export_is_structured_and_validates_single_resource() {
    let root = temp_root("workspace-export-single");
    fs::create_dir_all(&root).unwrap();
    let output = root.join("single-resource");

    let result = binary()
        .args([
            "workspace",
            "export",
            fixture("stream").to_str().unwrap(),
            "simple.ymap",
            "--output",
            output.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab workspace export should run");

    assert!(result.status.success());
    let body = stdout_json(&result);
    assert_eq!(body["command"], "export");
    assert_eq!(body["data"]["resourceName"], "single-resource");
    assert_eq!(body["data"]["selectedRoots"][0], "simple.ymap");
    assert_eq!(body["data"]["unresolved"]["unknown"], 0);
    assert_eq!(body["data"]["validation"]["valid"], true);
    assert_eq!(body["data"]["validation"]["manifestValid"], true);
    assert_eq!(body["data"]["validation"]["fxmanifestPresent"], true);
    assert!(output.join("fxmanifest.lua").is_file());
    assert!(output.join("stream").join("_manifest.ymf").is_file());
    assert!(output.join(".ragelab-export.json").is_file());
    assert!(output.join("stream").join("simple.ymap").is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn combined_workspace_export_respects_unresolved_gate_and_override() {
    let root = temp_root("workspace-export-combined");
    fs::create_dir_all(&root).unwrap();
    let blocked_output = root.join("blocked");
    let allowed_output = root.join("allowed");
    let workspace = fixture("ymap-relations");

    let blocked = binary()
        .args([
            "workspace",
            "export",
            workspace.to_str().unwrap(),
            "relation_parent.ymap",
            "relation_child_a.ymap",
            "--output",
            blocked_output.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab combined export should run");
    assert_eq!(blocked.status.code(), Some(3));
    assert!(blocked.stderr.is_empty());
    let blocked_body = stdout_json(&blocked);
    assert_eq!(blocked_body["command"], "export");
    assert_eq!(blocked_body["error"]["code"], "unsupported");
    assert!(!blocked_output.exists());

    let allowed = binary()
        .args([
            "workspace",
            "export",
            workspace.to_str().unwrap(),
            "relation_parent.ymap",
            "relation_child_a.ymap",
            "--output",
            allowed_output.to_str().unwrap(),
            "--allow-unresolved",
            "--json",
        ])
        .output()
        .expect("ragelab combined export with override should run");
    assert!(allowed.status.success());
    let allowed_body = stdout_json(&allowed);
    assert_eq!(allowed_body["command"], "export");
    assert_eq!(
        allowed_body["data"]["selectedRoots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(allowed_body["data"]["output"]["manifestMaps"], 4);
    assert_eq!(allowed_body["data"]["unresolved"]["allowUnresolved"], true);
    assert!(
        allowed_body["data"]["unresolved"]["unknown"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(allowed_body["data"]["validation"]["valid"], true);
    assert!(allowed_output
        .join("stream")
        .join("_manifest.ymf")
        .is_file());
    assert!(allowed_output.join(".ragelab-export.json").is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ydr_preview_is_structured_renderer_neutral_and_bounded() {
    let output = binary()
        .args([
            "preview",
            fixture("ydr/editable.ydr").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab preview should run");
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "preview");
    assert_eq!(body["data"]["type"], "YDR");
    assert_eq!(body["data"]["spatial"]["classification"], "localOnly");
    assert_eq!(
        body["data"]["preview"]["coordinateConvention"],
        "sourceXyzZUp"
    );
    assert_eq!(body["data"]["preview"]["counts"]["primitives"], 1);
    assert_eq!(body["data"]["preview"]["counts"]["vertices"], 3);
    assert_eq!(
        body["data"]["preview"]["primitives"][0]["geometryIncluded"],
        true
    );
    assert_eq!(
        body["data"]["preview"]["primitives"][0]["geometry"]["indices"],
        json!([0, 1, 2])
    );
    assert_eq!(
        body["data"]["preview"]["shaders"][0]["textureReferences"][0]["textureName"],
        "test_diffuse"
    );

    let bounded = binary()
        .args([
            "preview",
            fixture("ydr/editable.ydr").to_str().unwrap(),
            "--max-vertices",
            "2",
            "--max-indices",
            "3",
            "--json",
        ])
        .output()
        .expect("bounded ragelab preview should run");
    assert!(bounded.status.success());
    let bounded = stdout_json(&bounded);
    assert_eq!(
        bounded["data"]["preview"]["primitives"][0]["geometryIncluded"],
        false
    );
    assert!(bounded["data"]["preview"]["primitives"][0]["geometry"].is_null());
    assert_eq!(bounded["data"]["preview"]["emitted"]["vertices"], 0);
    assert_eq!(
        bounded["data"]["preview"]["truncated"]["geometryOmittedPrimitives"],
        1
    );
}

#[test]
fn ydd_preview_requires_explicit_zero_based_drawable_selector() {
    let missing = binary()
        .args([
            "preview",
            fixture("ydd/editable.ydd").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab preview should run");
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stderr.is_empty());
    let missing = stdout_json(&missing);
    assert_eq!(missing["command"], "preview");
    assert_eq!(missing["error"]["code"], "invalid_input");

    let selected = binary()
        .args([
            "preview",
            fixture("ydd/editable.ydd").to_str().unwrap(),
            "--drawable-index",
            "0",
            "--json",
        ])
        .output()
        .expect("selected YDD preview should run");
    assert!(selected.status.success());
    let selected = stdout_json(&selected);
    assert_eq!(selected["data"]["type"], "YDD");
    assert_eq!(selected["data"]["preview"]["selector"]["drawableIndex"], 0);
    assert_eq!(
        selected["data"]["preview"]["selector"]["nameHash"],
        "0x12345678"
    );
    assert_eq!(
        selected["data"]["preview"]["shaders"][0]["textureReferences"][0]["textureName"],
        "dict_diffuse"
    );
}

#[test]
fn preview_rejects_limits_above_hard_caps() {
    let output = binary()
        .args([
            "preview",
            fixture("ydr/editable.ydr").to_str().unwrap(),
            "--max-vertices",
            "100001",
            "--json",
        ])
        .output()
        .expect("ragelab preview should run");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "preview");
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("hard limit 100000"));
}

#[test]
fn ybn_preview_exposes_bounded_mesh_shapes_and_local_spatial_semantics() {
    let capabilities_output = binary()
        .args([
            "capabilities",
            fixture("ybn/preview.ybn").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab capabilities should run");
    assert!(capabilities_output.status.success());
    let capabilities = stdout_json(&capabilities_output);
    let operations = capabilities["data"]["operations"].as_array().unwrap();
    let preview = operations
        .iter()
        .find(|operation| operation["id"] == "preview")
        .unwrap();
    assert_eq!(preview["availability"], "available");
    assert_eq!(preview["structuredOutput"], true);
    assert_eq!(preview["requiresParameters"], json!([]));

    let output = binary()
        .args([
            "preview",
            fixture("ybn/preview.ybn").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab preview should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "preview");
    assert_eq!(body["data"]["type"], "YBN");
    assert_eq!(body["data"]["spatial"]["classification"], "localOnly");
    assert_eq!(
        body["data"]["spatial"]["coordinateConvention"],
        "sourceXyzZUp"
    );
    assert_eq!(body["data"]["preview"]["kind"], "collision");
    assert_eq!(body["data"]["preview"]["counts"]["children"], 1);
    assert_eq!(body["data"]["preview"]["counts"]["materials"], 2);
    assert_eq!(body["data"]["preview"]["counts"]["meshPrimitives"], 1);
    assert_eq!(body["data"]["preview"]["counts"]["shapePrimitives"], 1);
    assert_eq!(body["data"]["preview"]["counts"]["vertices"], 3);
    assert_eq!(body["data"]["preview"]["counts"]["indices"], 3);
    assert_eq!(
        body["data"]["preview"]["mesh"]["positions"],
        json!([[10.0, 20.0, 30.0], [11.0, 20.0, 30.0], [10.0, 21.0, 30.0]])
    );
    assert_eq!(
        body["data"]["preview"]["mesh"]["primitives"][0]["indices"],
        json!([0, 1, 2])
    );
    assert_eq!(
        body["data"]["preview"]["shapePrimitives"][0]["shape"]["kind"],
        "sphere"
    );
    assert_eq!(
        body["data"]["preview"]["shapePrimitives"][0]["shape"]["center"],
        json!([11.0, 20.0, 30.0])
    );
    assert_eq!(
        body["data"]["preview"]["shapePrimitives"][0]["materialIndex"],
        1
    );

    let bounded = binary()
        .args([
            "preview",
            fixture("ybn/preview.ybn").to_str().unwrap(),
            "--max-vertices",
            "2",
            "--max-materials",
            "1",
            "--json",
        ])
        .output()
        .expect("bounded YBN preview should run");

    assert!(bounded.status.success());
    let bounded = stdout_json(&bounded);
    assert_eq!(
        bounded["data"]["preview"]["mesh"]["positionsIncluded"],
        false
    );
    assert!(bounded["data"]["preview"]["mesh"]["positions"].is_null());
    assert_eq!(
        bounded["data"]["preview"]["mesh"]["primitives"][0]["indicesIncluded"],
        false
    );
    assert!(bounded["data"]["preview"]["mesh"]["primitives"][0]["indices"].is_null());
    assert_eq!(bounded["data"]["preview"]["emitted"]["vertices"], 0);
    assert_eq!(bounded["data"]["preview"]["emitted"]["materials"], 1);
    assert_eq!(bounded["data"]["preview"]["truncated"]["vertices"], true);
    assert_eq!(bounded["data"]["preview"]["truncated"]["materials"], true);
    assert_eq!(bounded["data"]["preview"]["truncated"]["indices"], true);
}

#[test]
fn ybn_preview_rejects_drawable_selector() {
    let output = binary()
        .args([
            "preview",
            fixture("ybn/preview.ybn").to_str().unwrap(),
            "--drawable-index",
            "0",
            "--json",
        ])
        .output()
        .expect("ragelab preview should run");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "preview");
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("--drawable-index is only valid for YDD preview"));
}

#[test]
fn gta_discover_is_structured_and_validates_legacy_override() {
    let root = temp_root("gta-legacy-discovery");
    fs::create_dir_all(root.join("update")).unwrap();
    for relative in [
        "GTA5.exe",
        "PlayGTAV.exe",
        "common.rpf",
        "x64a.rpf",
        "update/update.rpf",
    ] {
        fs::write(root.join(relative), []).unwrap();
    }

    let output = binary()
        .env("RAGELAB_GTA5_LEGACY", &root)
        .args(["gta", "discover", "--json"])
        .output()
        .expect("ragelab gta discover should run");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "gta.discover");
    assert_eq!(body["data"]["schema"], "ragelab.gta.discovery");
    assert_eq!(body["data"]["schemaVersion"], 1);
    assert_eq!(body["data"]["target"]["product"], "gtaV");
    assert_eq!(body["data"]["target"]["edition"], "legacy");
    assert_eq!(body["data"]["target"]["steamAppId"], 271590);

    let candidates = body["data"]["candidates"].as_array().unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| {
            candidate["provenance"]
                .as_array()
                .unwrap()
                .iter()
                .any(|source| source["source"] == "environment")
        })
        .expect("environment candidate should be present");
    assert_eq!(candidate["edition"], "legacy");
    assert_eq!(candidate["valid"], true);
    assert!(candidate["checks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|check| !check["required"].as_bool().unwrap() || check["passed"] == true));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fivem_discover_is_structured_and_links_citizenfx_ivpath_to_legacy_gta() {
    let base = temp_root("fivem-discovery");
    let gta_root = base.join("gta");
    let fivem_root = base.join("FiveM");
    let app_root = fivem_root.join("FiveM.app");

    fs::create_dir_all(gta_root.join("update")).unwrap();
    for relative in [
        "GTA5.exe",
        "PlayGTAV.exe",
        "common.rpf",
        "x64a.rpf",
        "update/update.rpf",
    ] {
        fs::write(gta_root.join(relative), []).unwrap();
    }

    fs::create_dir_all(app_root.join("data").join("game-storage")).unwrap();
    fs::create_dir_all(app_root.join("citizen")).unwrap();
    fs::write(fivem_root.join("FiveM.exe"), []).unwrap();
    fs::write(
        app_root.join("CitizenFX.ini"),
        format!(
            "[Game]\nIVPath={}\nSavedBuildNumber=3095\nUpdateChannel=production\n",
            gta_root.display()
        ),
    )
    .unwrap();

    let output = binary()
        .env("RAGELAB_FIVEM", &fivem_root)
        .args(["fivem", "discover", "--json"])
        .output()
        .expect("ragelab fivem discover should run");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "fivem.discover");
    assert_eq!(body["data"]["schema"], "ragelab.fivem.discovery");
    assert_eq!(body["data"]["schemaVersion"], 1);

    let candidates = body["data"]["candidates"].as_array().unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| {
            candidate["provenance"]
                .as_array()
                .unwrap()
                .iter()
                .any(|source| source["source"] == "environment")
        })
        .expect("environment FiveM candidate should be present");

    assert_eq!(candidate["valid"], true);
    assert_eq!(candidate["savedBuildNumber"], "3095");
    assert_eq!(candidate["updateChannel"], "production");
    assert_eq!(candidate["gta"]["status"], "validLegacy");
    assert_eq!(candidate["gta"]["installation"]["edition"], "legacy");
    assert_eq!(candidate["gta"]["installation"]["valid"], true);
    assert!(candidate["storagePaths"]
        .as_array()
        .unwrap()
        .iter()
        .find(|path| path["id"] == "gameStorage")
        .is_some_and(|path| path["exists"] == true));

    fs::remove_dir_all(base).unwrap();
}

#[test]
fn gta_catalog_builds_complete_loose_tree_index_with_structured_status() {
    let root = temp_root("gta-catalog-loose");
    let output_path = root.join("out").join("paths.txt");
    fs::create_dir_all(root.join("stream")).unwrap();
    fs::write(root.join("stream").join("z_asset.ytd"), []).unwrap();
    fs::write(root.join("stream").join("a_asset.ydr"), []).unwrap();
    fs::write(root.join("stream").join("readme.txt"), b"ignored").unwrap();

    let output = binary()
        .args([
            "gta",
            "catalog",
            root.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab gta catalog should run");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body = stdout_json(&output);
    assert_eq!(body["command"], "gta.catalog");
    assert_eq!(body["data"]["schema"], "ragelab.gta.catalog");
    assert_eq!(body["data"]["schemaVersion"], 1);
    assert_eq!(body["data"]["sourceKind"], "filesystemTree");
    assert_eq!(body["data"]["coverage"], "filesystemTreeComplete");
    assert_eq!(body["data"]["complete"], true);
    assert_eq!(body["data"]["pathEntries"], 2);
    assert_eq!(body["data"]["uniqueCatalogEntries"], 2);
    assert_eq!(body["data"]["archiveBoundary"]["rpfArchivesFound"], 0);
    assert_eq!(
        fs::read_to_string(&output_path).unwrap(),
        "stream/a_asset.ydr\nstream/z_asset.ytd\n"
    );

    let second = binary()
        .args([
            "gta",
            "catalog",
            root.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("second ragelab gta catalog should run");
    assert_eq!(second.status.code(), Some(1));
    let second_body = stdout_json(&second);
    assert_eq!(second_body["command"], "gta.catalog");
    assert_eq!(second_body["error"]["code"], "operation_failed");

    let overwrite = binary()
        .args([
            "gta",
            "catalog",
            root.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--overwrite",
            "--json",
        ])
        .output()
        .expect("ragelab gta catalog overwrite should run");
    assert!(overwrite.status.success());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn gta_catalog_reports_raw_rpf_boundary_for_valid_legacy_installation() {
    let root = temp_root("gta-catalog-legacy");
    let output_path = root.join("catalog.txt");
    fs::create_dir_all(root.join("update")).unwrap();
    for relative in [
        "GTA5.exe",
        "PlayGTAV.exe",
        "common.rpf",
        "x64a.rpf",
        "update/update.rpf",
    ] {
        fs::write(root.join(relative), []).unwrap();
    }
    fs::create_dir_all(root.join("mods")).unwrap();
    fs::write(root.join("mods").join("loose_prop.ydr"), []).unwrap();

    let output = binary()
        .args([
            "gta",
            "catalog",
            root.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab gta catalog should run");

    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body["data"]["sourceKind"], "gtaLegacyInstallation");
    assert_eq!(body["data"]["coverage"], "partialRpfBoundary");
    assert_eq!(body["data"]["complete"], false);
    assert_eq!(body["data"]["archiveBoundary"]["rpfArchivesFound"], 3);
    assert_eq!(
        body["data"]["archiveBoundary"]["enumerationSupported"],
        false
    );
    assert!(body["data"]["archiveBoundary"]["reason"]
        .as_str()
        .unwrap()
        .contains("does not enumerate raw/encrypted RPF"));
    assert_eq!(body["data"]["pathEntries"], 1);
    assert_eq!(
        fs::read_to_string(&output_path).unwrap(),
        "mods/loose_prop.ydr\n"
    );

    fs::remove_dir_all(root).unwrap();
}
