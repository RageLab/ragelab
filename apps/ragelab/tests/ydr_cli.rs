use serde_json::Value;
use std::{
    fs,
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ragelab"))
}

fn fixture(path: &str) -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic")
        .join(path)
        .to_string_lossy()
        .into_owned()
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout should contain valid JSON")
}

fn temp_file(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("ragelab-ydr-{label}-{}-{nonce}.ydr", process::id()))
}

#[test]
fn ydr_inspect_reports_drawable_lod_geometry_and_semantic_reopen() {
    let output = binary()
        .args(["ydr", "inspect", &fixture("ydr/simple.ydr"), "--json"])
        .output()
        .expect("ragelab should run");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["ok"], true);
    assert_eq!(body["command"], "ydr.inspect");
    let report = &body["data"];
    assert_eq!(report["schema"], "ragelab.ydr.inspection");
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["container"]["magic"], "RSC7");
    assert_eq!(report["container"]["resourceVersion"], 165);
    assert_eq!(report["drawable"]["name"], "test_drawable");
    assert_eq!(report["drawable"]["lodCount"], 1);
    assert_eq!(report["lods"][0]["level"], "high");
    assert_eq!(report["lods"][0]["geometries"][0]["vertexCount"], 3);
    assert!(report["lods"][0]["geometries"][0]["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|attribute| attribute == "TEXCOORD_0"));
    assert_eq!(report["structuralAudit"]["valid"], true);
    assert_eq!(report["semanticReopen"], true);
    assert_eq!(report["runtimeRenderable"], false);
}

#[test]
fn ydr_validate_decodes_embedded_texture_mips_and_reports_hash_mismatch() {
    let output = binary()
        .args(["ydr", "validate", &fixture("ydr/embedded.ydr"), "--json"])
        .output()
        .expect("ragelab should run");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["ok"], true);
    assert_eq!(body["command"], "ydr.validate");
    let report = &body["data"];
    assert_eq!(report["schema"], "ragelab.ydr.validation");
    assert_eq!(report["valid"], true);
    assert_eq!(report["strict"], false);
    assert_eq!(report["inspection"]["embeddedTextures"]["textureCount"], 1);
    assert_eq!(
        report["inspection"]["embeddedTextures"]["textures"][0]["format"],
        "BC1"
    );
    assert_eq!(
        report["inspection"]["embeddedTextures"]["textures"][0]["decodedMipLevels"],
        1
    );
    assert_eq!(
        report["inspection"]["embeddedTextures"]["textures"][0]["nameHashMatchesName"],
        false
    );
    assert!(report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "texture_name_hash_mismatch"));
    assert_eq!(report["semanticReopen"], true);
    assert_eq!(report["codewalkerGuiAccepted"], false);
    assert_eq!(report["runtimeRenderable"], false);
}

#[test]
fn ydr_validate_strict_promotes_warnings_and_returns_report_with_exit_four() {
    let output = binary()
        .args([
            "ydr",
            "validate",
            &fixture("ydr/simple.ydr"),
            "--strict",
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["ok"], false);
    assert_eq!(body["command"], "ydr.validate");
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(body["data"]["strict"], true);
    assert_eq!(body["data"]["valid"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| {
            diagnostic["severity"] == "error" && diagnostic["promotedFrom"] == "warning"
        }));
}

#[test]
fn ydr_validate_invalid_container_returns_structured_report_and_exit_four() {
    let path = temp_file("invalid");
    fs::write(&path, b"not an RSC7 drawable").unwrap();

    let output = binary()
        .args(["ydr", "validate", path.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    let _ = fs::remove_file(&path);
    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(body["data"]["containerValid"], false);
    assert_eq!(body["data"]["formatValid"], false);
    assert_eq!(
        body["data"]["diagnostics"][0]["code"],
        "invalid_rsc7_resource"
    );
}

#[test]
fn ydr_namespaced_inspect_rejects_unknown_options() {
    let output = binary()
        .args([
            "ydr",
            "inspect",
            &fixture("ydr/simple.ydr"),
            "--strict",
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(2));
    let body = stdout_json(&output);
    assert_eq!(body["ok"], false);
    assert_eq!(body["command"], "ydr.inspect");
    assert_eq!(body["error"]["code"], "invalid_input");
}
