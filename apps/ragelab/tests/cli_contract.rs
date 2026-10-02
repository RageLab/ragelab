use std::{path::PathBuf, process::Command};

use serde_json::Value;

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
}
