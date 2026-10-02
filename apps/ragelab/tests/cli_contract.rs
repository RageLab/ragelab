use std::{
    fs,
    path::PathBuf,
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

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
    assert!(canonical.iter().any(|value| value == "workspace.scene"));
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
        .any(|value| value == "workspace.scene"));
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
