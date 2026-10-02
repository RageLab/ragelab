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
