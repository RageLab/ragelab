use std::{
    fs,
    path::PathBuf,
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use ragelab_ydr::YdrDocument;
use ragelab_ytd::TextureFormat;
use serde_json::{json, Value};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ragelab"))
}

fn temp_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("ragelab-glb-cli-{}-{nonce}", process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn minimal_glb() -> Vec<u8> {
    let document = json!({
        "asset": {
            "version": "2.0",
            "generator": "RageLab CLI integration fixture"
        }
    });
    let mut json_chunk = serde_json::to_vec(&document).unwrap();
    while json_chunk.len() % 4 != 0 {
        json_chunk.push(b' ');
    }
    let total_length = 12 + 8 + json_chunk.len();
    let mut bytes = Vec::with_capacity(total_length);
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(total_length as u32).to_le_bytes());
    bytes.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
    bytes.extend_from_slice(&json_chunk);
    bytes
}

fn png_1x1_rgba() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 128, 16, 255]).unwrap();
    }
    bytes
}

fn triangle_glb_with_png() -> Vec<u8> {
    let image = png_1x1_rgba();
    let image_length = image.len();
    let mut binary = image;
    while binary.len() % 4 != 0 {
        binary.push(0);
    }
    let positions_offset = binary.len();
    for vertex in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        for component in vertex {
            binary.extend_from_slice(&component.to_le_bytes());
        }
    }
    let indices_offset = binary.len();
    for index in [0u16, 1, 2] {
        binary.extend_from_slice(&index.to_le_bytes());
    }
    while binary.len() % 4 != 0 {
        binary.push(0);
    }
    let uv1_offset = binary.len();
    for uv in [[0.25f32, 0.25], [0.75, 0.25], [0.25, 0.75]] {
        for component in uv {
            binary.extend_from_slice(&component.to_le_bytes());
        }
    }

    let document = json!({
        "asset": {"version": "2.0", "generator": "RageLab GLB export integration"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "Pumpkin", "mesh": 0}],
        "meshes": [{
            "name": "PumpkinMesh",
            "primitives": [{
                "attributes": {"POSITION": 0, "TEXCOORD_1": 2},
                "indices": 1,
                "material": 0
            }]
        }],
        "materials": [{
            "name": "PumpkinOrange",
            "pbrMetallicRoughness": {
                "baseColorFactor": [1.0, 1.0, 1.0, 1.0],
                "baseColorTexture": {"index": 0, "texCoord": 1},
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0
            }
        }],
        "textures": [{"source": 0}],
        "images": [{
            "name": "pumpkin_albedo",
            "mimeType": "image/png",
            "bufferView": 0
        }],
        "accessors": [
            {
                "bufferView": 1,
                "componentType": 5126,
                "count": 3,
                "type": "VEC3",
                "min": [0.0, 0.0, 0.0],
                "max": [1.0, 1.0, 0.0]
            },
            {
                "bufferView": 2,
                "componentType": 5123,
                "count": 3,
                "type": "SCALAR"
            },
            {
                "bufferView": 3,
                "componentType": 5126,
                "count": 3,
                "type": "VEC2"
            }
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": image_length},
            {"buffer": 0, "byteOffset": positions_offset, "byteLength": 36},
            {"buffer": 0, "byteOffset": indices_offset, "byteLength": 6},
            {"buffer": 0, "byteOffset": uv1_offset, "byteLength": 24}
        ],
        "buffers": [{"byteLength": binary.len()}]
    });
    let mut json_chunk = serde_json::to_vec(&document).unwrap();
    while json_chunk.len() % 4 != 0 {
        json_chunk.push(b' ');
    }
    let total_length = 12 + 8 + json_chunk.len() + 8 + binary.len();
    let mut bytes = Vec::with_capacity(total_length);
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(total_length as u32).to_le_bytes());
    bytes.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
    bytes.extend_from_slice(&json_chunk);
    bytes.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x004E4942u32.to_le_bytes());
    bytes.extend_from_slice(&binary);
    bytes
}

fn rewrite_glb_document(bytes: &[u8], mutate: impl FnOnce(&mut Value)) -> Vec<u8> {
    let json_length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let json_start = 20usize;
    let json_end = json_start + json_length;
    let mut document: Value = serde_json::from_slice(&bytes[json_start..json_end]).unwrap();
    mutate(&mut document);

    let mut json_chunk = serde_json::to_vec(&document).unwrap();
    while json_chunk.len() % 4 != 0 {
        json_chunk.push(b' ');
    }

    let mut result = Vec::new();
    result.extend_from_slice(b"glTF");
    result.extend_from_slice(&2u32.to_le_bytes());
    if bytes.len() > json_end {
        let bin_length =
            u32::from_le_bytes(bytes[json_end..json_end + 4].try_into().unwrap()) as usize;
        let bin_type = u32::from_le_bytes(bytes[json_end + 4..json_end + 8].try_into().unwrap());
        assert_eq!(bin_type, 0x004E4942);
        let bin_start = json_end + 8;
        let bin_end = bin_start + bin_length;
        let binary = &bytes[bin_start..bin_end];
        let total_length = 12 + 8 + json_chunk.len() + 8 + binary.len();
        result.extend_from_slice(&(total_length as u32).to_le_bytes());
        result.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
        result.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
        result.extend_from_slice(&json_chunk);
        result.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        result.extend_from_slice(&0x004E4942u32.to_le_bytes());
        result.extend_from_slice(binary);
    } else {
        let total_length = 12 + 8 + json_chunk.len();
        result.extend_from_slice(&(total_length as u32).to_le_bytes());
        result.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
        result.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
        result.extend_from_slice(&json_chunk);
    }
    result
}

fn two_material_triangles_glb_with_png() -> Vec<u8> {
    let original = triangle_glb_with_png();
    let json_length = u32::from_le_bytes(original[12..16].try_into().unwrap()) as usize;
    let json_start = 20usize;
    let json_end = json_start + json_length;
    let mut document: Value = serde_json::from_slice(&original[json_start..json_end]).unwrap();
    let mut second_primitive = document["meshes"][0]["primitives"][0].clone();
    second_primitive["material"] = json!(1);
    document["meshes"][0]["primitives"]
        .as_array_mut()
        .unwrap()
        .push(second_primitive);
    let mut unmaterialed_primitive = document["meshes"][0]["primitives"][0].clone();
    unmaterialed_primitive
        .as_object_mut()
        .unwrap()
        .remove("material");
    document["meshes"][0]["primitives"]
        .as_array_mut()
        .unwrap()
        .push(unmaterialed_primitive);
    let mut second_material = document["materials"][0].clone();
    second_material["name"] = json!("PumpkinOrangeSecond");
    document["materials"]
        .as_array_mut()
        .unwrap()
        .push(second_material);

    let bin_header = json_end;
    let bin_length =
        u32::from_le_bytes(original[bin_header..bin_header + 4].try_into().unwrap()) as usize;
    let bin_start = bin_header + 8;
    let bin_end = bin_start + bin_length;
    let binary = &original[bin_start..bin_end];

    let mut json_chunk = serde_json::to_vec(&document).unwrap();
    while json_chunk.len() % 4 != 0 {
        json_chunk.push(b' ');
    }
    let total_length = 12 + 8 + json_chunk.len() + 8 + binary.len();
    let mut bytes = Vec::with_capacity(total_length);
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(total_length as u32).to_le_bytes());
    bytes.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
    bytes.extend_from_slice(&json_chunk);
    bytes.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x004E4942u32.to_le_bytes());
    bytes.extend_from_slice(binary);
    bytes
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout should contain valid JSON")
}

#[test]
fn glb_tree_lists_the_selectable_node_hierarchy_in_json() {
    let root = temp_root();
    let source = root.join("group.glb");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let output = binary()
        .args(["glb", "tree", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.tree");
    assert_eq!(body["data"]["schema"], "ragelab.glb.node-tree");
    assert_eq!(body["data"]["schemaVersion"], 1);
    assert_eq!(body["data"]["nodeCount"], 1);
    assert_eq!(body["data"]["meshNodeCount"], 1);
    assert_eq!(body["data"]["primitiveCount"], 1);
    assert_eq!(body["data"]["roots"][0]["nodeIndex"], 0);
    assert_eq!(body["data"]["roots"][0]["name"], "Pumpkin");
    assert_eq!(body["data"]["roots"][0]["meshIndex"], 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_accepts_node_selection_and_reports_included_nodes() {
    let root = temp_root();
    let source = root.join("group.glb");
    let output_path = root.join("selected.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--node",
            "0",
            "--output",
            output_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.export");
    assert_eq!(body["data"]["schemaVersion"], 2);
    assert_eq!(body["data"]["geometryCount"], 1);
    assert_eq!(body["data"]["semanticReopen"], true);
    assert_eq!(body["data"]["runtimeRenderable"], false);
    assert_eq!(
        body["data"]["selection"]["schema"],
        "ragelab.glb.node-selection"
    );
    assert_eq!(
        body["data"]["selection"]["selectedNodes"][0]["nodeIndex"],
        0
    );
    assert_eq!(body["data"]["selection"]["includedNodeIndices"], json!([0]));
    assert_eq!(body["data"]["selection"]["includedMeshIndices"], json!([0]));
    assert_eq!(body["data"]["selection"]["primitiveCount"], 1);
    assert!(output_path.is_file());
    YdrDocument::from_bytes(&fs::read(&output_path).unwrap()).unwrap();

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rejects_invalid_and_duplicate_node_selection_without_output() {
    let root = temp_root();
    let source = root.join("group.glb");
    let invalid_output = root.join("invalid-node.ydr");
    let duplicate_output = root.join("duplicate-node.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let invalid = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--node",
            "999",
            "--output",
            invalid_output.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(!invalid.status.success());
    assert!(!invalid_output.exists());
    let invalid_body = stdout_json(&invalid);
    assert_eq!(invalid_body["error"]["code"], "invalid_input");
    assert!(invalid_body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("selected node index 999 is out of range"));

    let duplicate = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--node",
            "0",
            "--node",
            "0",
            "--output",
            duplicate_output.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(!duplicate.status.success());
    assert!(!duplicate_output.exists());
    let duplicate_body = stdout_json(&duplicate);
    assert_eq!(duplicate_body["error"]["code"], "invalid_input");
    assert!(duplicate_body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("duplicate node indices"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn namespaced_glb_inspect_returns_structured_inventory() {
    let root = temp_root();
    let source = root.join("empty-model.glb");
    fs::write(&source, minimal_glb()).unwrap();

    let output = binary()
        .args(["glb", "inspect", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["schema"], "ragelab.cli.response");
    assert_eq!(body["command"], "glb.inspect");
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["schema"], "ragelab.glb.inspection");
    assert_eq!(body["data"]["asset"]["version"], "2.0");
    assert_eq!(body["data"]["counts"]["meshes"], 0);
    assert!(body["data"]["source"]["sha256"].as_str().unwrap().len() == 64);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn namespaced_glb_geometry_decodes_and_reports_triangle_data() {
    let root = temp_root();
    let source = root.join("geometry.glb");
    let bytes = {
        let document = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "accessors": [{
                "bufferView": 0,
                "componentType": 5126,
                "count": 3,
                "type": "VEC3",
                "min": [0.0, 0.0, 0.0],
                "max": [1.0, 1.0, 0.0]
            }],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
            "buffers": [{"byteLength": 36}]
        });
        let mut json_chunk = serde_json::to_vec(&document).unwrap();
        while json_chunk.len() % 4 != 0 {
            json_chunk.push(b' ');
        }
        let mut binary = Vec::new();
        for vertex in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for component in vertex {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        let total_length = 12 + 8 + json_chunk.len() + 8 + binary.len();
        let mut bytes = Vec::with_capacity(total_length);
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&(total_length as u32).to_le_bytes());
        bytes.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
        bytes.extend_from_slice(&json_chunk);
        bytes.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0x004E4942u32.to_le_bytes());
        bytes.extend_from_slice(&binary);
        bytes
    };
    fs::write(&source, bytes).unwrap();

    let output = binary()
        .args(["glb", "geometry", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.geometry");
    assert_eq!(body["data"]["schema"], "ragelab.glb.geometry");
    assert_eq!(body["data"]["primitives"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["primitives"][0]["vertexCount"], 3);
    assert_eq!(body["data"]["primitives"][0]["indexCount"], 3);
    assert_eq!(body["data"]["primitives"][0]["indices"], json!([0, 1, 2]));
    assert_eq!(
        body["data"]["primitives"][0]["boundsMin"],
        json!([0.0, 0.0, 0.0])
    );
    assert_eq!(
        body["data"]["primitives"][0]["boundsMax"],
        json!([1.0, 0.0, 1.0])
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_writes_structural_ydr_with_embedded_png_and_no_shader_claim() {
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    let output_path = root.join("pumpkin.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.export");
    assert_eq!(body["data"]["schema"], "ragelab.glb.ydr-export");
    assert_eq!(body["data"]["ydrVersion"], 165);
    assert_eq!(body["data"]["geometryCount"], 1);
    assert_eq!(body["data"]["vertexCount"], 3);
    assert_eq!(body["data"]["triangleCount"], 1);
    assert_eq!(body["data"]["embeddedTextureCount"], 1);
    assert_eq!(body["data"]["profileValidated"], true);
    assert_eq!(body["data"]["runtimeRenderable"], false);
    assert_eq!(body["data"]["materialShaderBindings"], 0);
    assert!(body["data"]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item
            .as_str()
            .unwrap()
            .contains("CodeWalker Model Viewer is not safe for shaderless drawables")));
    assert!(output_path.is_file());

    let bytes = fs::read(&output_path).unwrap();
    let document = YdrDocument::from_bytes(&bytes).unwrap();
    assert_eq!(document.model.name.as_deref(), Some("pumpkin"));
    assert_eq!(document.model.primitives.len(), 1);
    assert_eq!(
        document.model.primitives[0].uv1,
        Some(vec![[0.25, 0.25], [0.75, 0.25], [0.25, 0.75]])
    );
    assert!(document.model.shaders.is_empty());
    let dictionary = document.embedded_textures.as_ref().unwrap();
    assert_eq!(dictionary.textures().len(), 1);
    assert_eq!(
        dictionary.textures()[0].name.as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    assert_eq!(
        (
            dictionary.textures()[0].width,
            dictionary.textures()[0].height
        ),
        (1, 1)
    );
    assert_eq!(dictionary.textures()[0].format, TextureFormat::Bc1);

    let refused_overwrite = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(!refused_overwrite.status.success());
    assert_eq!(fs::read(&output_path).unwrap(), bytes);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rebinds_legacy_shader_template_when_environment_path_is_set() {
    let Ok(template_path) = std::env::var("RAGELAB_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    let output_path = root.join("pumpkin-shader.ydr");
    let report_path = root.join("pumpkin-shader.report.json");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--report",
            report_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.export");
    assert_eq!(body["data"]["shaderGroup"], "populated");
    assert_eq!(body["data"]["shaderCount"], 1);
    assert_eq!(body["data"]["materialShaderBindings"], 1);
    assert_eq!(body["data"]["shaderTextureBindings"], 1);
    assert_eq!(body["data"]["runtimeRenderable"], false);
    assert_eq!(body["data"]["report"], report_path.to_str().unwrap());
    let report: Value = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["schema"], "ragelab.glb.export-report");
    assert_eq!(
        report["provenance"]["shaderTemplate"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        report["decisions"]["shaderParameters"]["outputShaderCount"],
        1
    );
    assert!(
        !report["decisions"]["shaderParameters"]["shaders"][0]["parameters"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        report["decisions"]["shaderParameters"]["shaders"][0]["textureReferences"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let bytes = fs::read(&output_path).unwrap();
    let document = YdrDocument::from_bytes(&bytes).unwrap();
    assert_eq!(document.model.shaders.len(), 1);
    assert_eq!(document.model.shaders[0].name_hash, 0x38DD_00DF);
    assert_eq!(document.model.shaders[0].file_hash, 0x14A7_80FD);
    assert_eq!(document.model.primitives[0].shader_index, Some(0));
    assert_eq!(
        document.model.shaders[0].parameters[0]
            .texture_name
            .as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    assert!(document.model.shaders[0].parameters[1]
        .texture_name
        .is_none());
    assert!(document.model.shaders[0].parameters[2]
        .texture_name
        .is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_applies_normal_sampler_and_scale_to_normal_sps_template() {
    let Ok(template_path) = std::env::var("RAGELAB_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    let profile_path = root.join("pumpkin.gta-materials.json");
    let output_path = root.join("pumpkin-normal.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["textureRoles"]["normal"] =
        json!({"textureIndex": 0, "texCoord": 1, "scale": 0.5});
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["data"]["shaderTextureBindings"], 2);

    let document = YdrDocument::from_bytes(&fs::read(&output_path).unwrap()).unwrap();
    let shader = &document.model.shaders[0];
    assert_eq!(
        shader.parameters[1].texture_name.as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    assert_eq!(f32::from_bits(shader.parameters[6].vector_data[0][0]), 0.5);
    assert_eq!(document.model.primitives[0].shader_index, Some(0));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_clones_one_normal_shader_per_material_and_maps_each_geometry() {
    let Ok(template_path) = std::env::var("RAGELAB_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin-multi.glb");
    let output_path = root.join("pumpkin-multi.ydr");
    let report_path = root.join("pumpkin-multi.report.json");
    fs::write(&source, two_material_triangles_glb_with_png()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--report",
            report_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["data"]["geometryCount"], 3);
    assert_eq!(body["data"]["shaderCount"], 3);
    assert_eq!(body["data"]["materialShaderBindings"], 2);
    assert_eq!(body["data"]["geometryShaderMappings"], 3);
    assert_eq!(body["data"]["shaderTextureBindings"], 2);
    assert_eq!(body["data"]["report"], report_path.to_str().unwrap());
    let report: Value = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(
        report["decisions"]["shaderParameters"]["outputShaderCount"],
        3
    );
    assert_eq!(report["fallbacks"].as_array().unwrap().len(), 1);
    assert_eq!(
        report["fallbacks"][0]["kind"],
        "neutral_shader_for_unmaterialed_geometry"
    );
    assert_eq!(report["fallbacks"][0]["authorizedBy"], "--shader-template");
    assert!(!report["fallbacks"][0]["affectedGeometryIndices"]
        .as_array()
        .unwrap()
        .is_empty());

    let document = YdrDocument::from_bytes(&fs::read(&output_path).unwrap()).unwrap();
    assert_eq!(document.model.shaders.len(), 3);
    assert_eq!(document.model.primitives.len(), 3);
    assert_eq!(document.model.primitives[0].shader_index, Some(0));
    assert_eq!(document.model.primitives[1].shader_index, Some(1));
    assert_eq!(document.model.primitives[2].shader_index, Some(2));
    assert_eq!(document.model.shaders[0].name_hash, 0x38DD_00DF);
    assert_eq!(document.model.shaders[1].name_hash, 0x38DD_00DF);
    assert_eq!(document.model.shaders[2].name_hash, 0x38DD_00DF);
    assert!(document.model.shaders[2].parameters[..3]
        .iter()
        .all(|parameter| parameter.texture_name.is_none() && parameter.texture_base.is_none()));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rebinds_verified_spec_sps_diffuse_and_specular_samplers() {
    let Ok(template_path) = std::env::var("RAGELAB_SPEC_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin-spec.glb");
    let profile_path = root.join("pumpkin-spec.gta-materials.json");
    let output_path = root.join("pumpkin-spec.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should initialize the authoring profile");
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["textureRoles"]["specular"] = json!({"textureIndex": 0, "texCoord": 1});
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--shader-index",
            "0",
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let body = stdout_json(&output);
    assert_eq!(body["data"]["shaderTemplate"]["preset"], "spec.sps");
    assert_eq!(body["data"]["shaderGroup"], "populated");
    assert_eq!(body["data"]["shaderCount"], 1);
    assert_eq!(body["data"]["shaderTextureBindings"], 2);
    assert_eq!(body["data"]["runtimeRenderable"], false);

    let document = YdrDocument::from_bytes(&fs::read(&output_path).unwrap()).unwrap();
    let shader = &document.model.shaders[0];
    assert_eq!(shader.name_hash, 0x7204_A391);
    assert_eq!(shader.file_hash, 0x85F5_0BE9);
    assert_eq!(document.model.primitives[0].shader_index, Some(0));
    assert_eq!(
        shader.parameters[0].texture_name.as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    assert_eq!(
        shader.parameters[1].texture_name.as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    assert_eq!(f32::from_bits(shader.parameters[2].vector_data[0][0]), 1.0);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rebinds_normal_spec_tint_palette_and_preserves_sampler_usage_data() {
    let Ok(template_path) = std::env::var("RAGELAB_TINT_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("bollard-palette.glb");
    let profile_path = root.join("bollard-palette.gta-materials.json");
    let output_path = root.join("bollard-palette.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should initialize the authoring profile");
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["textureRoles"]["tintPalette"] =
        json!({"textureIndex": 0, "texCoord": 1});
    profile["materials"][0]["textureRoles"]["normal"] = json!({"textureIndex": 0, "texCoord": 1});
    profile["materials"][0]["textureRoles"]["specular"] = json!({"textureIndex": 0, "texCoord": 1});
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--shader-index",
            "0",
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let body = stdout_json(&output);
    assert_eq!(
        body["data"]["shaderTemplate"]["preset"],
        "normal_spec_tnt.sps"
    );
    assert_eq!(body["data"]["shaderGroup"], "populated");
    assert_eq!(body["data"]["shaderCount"], 1);
    assert_eq!(body["data"]["shaderTextureBindings"], 4);
    assert_eq!(body["data"]["runtimeRenderable"], false);

    let document = YdrDocument::from_bytes(&fs::read(&output_path).unwrap()).unwrap();
    let shader = &document.model.shaders[0];
    assert_eq!(shader.name_hash, 0x6CD4_735B);
    assert_eq!(shader.file_hash, 0x304A_B527);
    assert_eq!(document.model.primitives[0].shader_index, Some(0));
    assert_eq!(shader.parameters.len(), 13);
    assert_eq!(
        shader.parameters[0].texture_name.as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    assert_eq!(
        shader.parameters[1].texture_name.as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    assert!(shader.parameters[2..4]
        .iter()
        .all(|parameter| parameter.texture_name.as_deref() == Some("img_0_pumpkin_albedo")));
    assert_eq!(
        [0, 1, 2, 3].map(|index| shader.parameters[index]
            .texture_base
            .as_ref()
            .unwrap()
            .usage_data),
        [0x2001_5874, 0x2000_2019, 0x2001_5876, 0x2001_5877]
    );
    assert_eq!(shader.parameters[12].vector_data[0], [0; 4]);
    assert_eq!(f32::from_bits(shader.parameters[7].vector_data[0][0]), 2.0);

    fs::remove_dir_all(root).unwrap();
}

fn assert_emissive_sps_ydr_template(
    template_path: &str,
    shader_index: &str,
    expected_vft: u32,
    expected_unknown32: u16,
    expected_usage_data: u32,
) {
    let root = temp_root();
    let source = root.join("arena-emissive.glb");
    let profile_path = root.join("arena-emissive.gta-materials.json");
    let output_path = root.join("arena-emissive.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should initialize the authoring profile");
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["shaderIntent"] = json!("emissive");
    profile["materials"][0]["textureRoles"]["albedo"] = Value::Null;
    profile["materials"][0]["textureRoles"]["emissive"] = json!({"textureIndex": 0, "texCoord": 1});
    profile["materials"][0]["parameters"]["emissiveFactor"] = json!([0.125, 0.5, 0.875]);
    profile["materials"][0]["parameters"]["emissiveStrength"] = json!(2.5);
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path,
            "--shader-index",
            shader_index,
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let body = stdout_json(&output);
    assert_eq!(body["data"]["shaderTemplate"]["preset"], "emissive.sps");
    assert_eq!(body["data"]["shaderGroup"], "populated");
    assert_eq!(body["data"]["shaderCount"], 1);
    assert_eq!(body["data"]["shaderTextureBindings"], 1);
    assert_eq!(body["data"]["runtimeRenderable"], false);

    let document = YdrDocument::from_bytes(&fs::read(&output_path).unwrap()).unwrap();
    let shader = &document.model.shaders[0];
    assert_eq!(shader.name_hash, 0x8F24_5469);
    assert_eq!(shader.file_hash, 0x9D18_3303);
    assert_eq!(document.model.primitives[0].shader_index, Some(0));
    assert_eq!(
        shader.parameters[0].texture_name.as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    let base = shader.parameters[0].texture_base.as_ref().unwrap();
    assert_eq!(base.vft, expected_vft);
    assert_eq!(base.unknown_32, expected_unknown32);
    assert_eq!(base.usage_data, expected_usage_data);
    assert_eq!(
        shader.parameters[1].vector_data[0].map(f32::from_bits),
        [0.125, 0.5, 0.875, 1.0]
    );
    assert_eq!(f32::from_bits(shader.parameters[4].vector_data[0][0]), 2.5);
    assert_eq!(
        shader.parameters[5].vector_data[0].map(f32::from_bits),
        [0.0, 1.0, 0.0, 0.0]
    );
    assert_eq!(
        shader.parameters[6].vector_data[0].map(f32::from_bits),
        [1.0, 0.0, 0.0, 0.0]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_accepts_emissive_sps_from_three_verified_ydr_texture_base_variants() {
    let Ok(variant_a) = std::env::var("RAGELAB_EMISSIVE_SHADER_YDR_TEMPLATE") else {
        return;
    };
    let Ok(variant_b) = std::env::var("RAGELAB_EMISSIVE_ALT_SHADER_YDR_TEMPLATE") else {
        return;
    };
    let Ok(variant_c) = std::env::var("RAGELAB_EMISSIVE_ALT2_SHADER_YDR_TEMPLATE") else {
        return;
    };

    // Independent real resources in the Arena light props RPF:
    // - arena_light_a shader 0: VFT 40619668 / UsageData 2000B414
    // - arena_lamps_01a shader 1: VFT 40619668 / UsageData 2000B014
    // - arena_light_a shader 3: VFT 4061A8F8 / UsageData 0
    assert_emissive_sps_ydr_template(&variant_a, "0", 0x4061_9668, 0x80, 0x2000_B414);
    assert_emissive_sps_ydr_template(&variant_b, "1", 0x4061_9668, 0x80, 0x2000_B014);
    assert_emissive_sps_ydr_template(&variant_c, "3", 0x4061_A8F8, 0x0002, 0);
}

#[test]
fn glb_export_rebinds_emissivenight_and_maps_emission_factor_and_strength() {
    let Ok(template_path) = std::env::var("RAGELAB_EMISSIVE_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin-emissive.glb");
    let profile_path = root.join("pumpkin-emissive.gta-materials.json");
    let output_path = root.join("pumpkin-emissive.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should initialize the authoring profile");
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["shaderIntent"] = json!("emissive");
    profile["materials"][0]["textureRoles"]["albedo"] = Value::Null;
    profile["materials"][0]["textureRoles"]["emissive"] = json!({"textureIndex": 0, "texCoord": 1});
    profile["materials"][0]["parameters"]["emissiveFactor"] = json!([0.25, 0.5, 0.75]);
    profile["materials"][0]["parameters"]["emissiveStrength"] = json!(3.5);
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--shader-index",
            "2",
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let body = stdout_json(&output);
    assert_eq!(
        body["data"]["shaderTemplate"]["preset"],
        "emissivenight.sps"
    );
    assert_eq!(body["data"]["shaderGroup"], "populated");
    assert_eq!(body["data"]["shaderCount"], 1);
    assert_eq!(body["data"]["shaderTextureBindings"], 1);
    assert_eq!(body["data"]["runtimeRenderable"], false);

    let document = YdrDocument::from_bytes(&fs::read(&output_path).unwrap()).unwrap();
    let shader = &document.model.shaders[0];
    assert_eq!(shader.name_hash, 0xB08C_6435);
    assert_eq!(shader.file_hash, 0x557D_B6E4);
    assert_eq!(document.model.primitives[0].shader_index, Some(0));
    assert_eq!(
        shader.parameters[0].texture_name.as_deref(),
        Some("img_0_pumpkin_albedo")
    );
    assert_eq!(
        shader.parameters[0].texture_base.as_ref().unwrap().vft,
        0x4061_A8F8
    );
    let color_scale = shader.parameters[1].vector_data[0].map(f32::from_bits);
    assert_eq!(color_scale, [0.25, 0.5, 0.75, 1.0]);
    assert_eq!(f32::from_bits(shader.parameters[4].vector_data[0][0]), 3.5);
    // The un-authored vectors are carried over exactly from the YFT template.
    assert_eq!(
        shader.parameters[2].vector_data[0].map(f32::from_bits),
        [1.0, 0.0, 0.0, 0.0]
    );
    assert_eq!(shader.parameters[3].vector_data[0], [0; 4]);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rejects_emissivenight_nonwhite_base_color_before_creating_output() {
    let Ok(template_path) = std::env::var("RAGELAB_EMISSIVE_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin-emissive.glb");
    let profile_path = root.join("pumpkin-emissive.gta-materials.json");
    let output_path = root.join("unsupported-emissive-base-color.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should initialize the authoring profile");
    assert!(initialized.status.success());
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["shaderIntent"] = json!("emissive");
    profile["materials"][0]["parameters"]["emissiveFactor"] = json!([1.0, 0.0, 0.0]);
    profile["materials"][0]["parameters"]["emissiveStrength"] = json!(6.0);
    profile["materials"][0]["parameters"]["baseColorFactor"] = json!([0.5, 1.0, 1.0, 1.0]);
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--shader-index",
            "2",
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(!output.status.success());
    assert!(!output_path.exists());
    let body = stdout_json(&output);
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("baseColorFactor, metallic or roughness"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rejects_unimplemented_default_sps_template_before_creating_output() {
    let Ok(template_path) = std::env::var("RAGELAB_SPEC_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin-default.glb");
    let output_path = root.join("unsupported-default.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--shader-index",
            "1",
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(!output.status.success());
    assert!(!output_path.exists());
    let body = stdout_json(&output);
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"].as_str().unwrap().contains(
        "supported presets are normal.sps, spec.sps, normal_spec_tnt.sps, emissivenight.sps and emissive.sps"
    ));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rejects_normal_map_for_spec_sps_before_creating_output() {
    let Ok(template_path) = std::env::var("RAGELAB_SPEC_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin-spec.glb");
    let profile_path = root.join("pumpkin-spec.gta-materials.json");
    let output_path = root.join("unsupported-spec-normal.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should initialize the authoring profile");
    assert!(initialized.status.success());
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["textureRoles"]["normal"] =
        json!({"textureIndex": 0, "texCoord": 1, "scale": 0.5});
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--shader-index",
            "0",
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(!output.status.success());
    assert!(!output_path.exists());
    let body = stdout_json(&output);
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("spec.sps only exposes DiffuseSampler and SpecSampler"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rejects_tint_palette_for_spec_sps_before_creating_output() {
    let Ok(template_path) = std::env::var("RAGELAB_SPEC_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin-spec.glb");
    let profile_path = root.join("pumpkin-spec.gta-materials.json");
    let output_path = root.join("unsupported-spec-tint.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should initialize the authoring profile");
    assert!(initialized.status.success());
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["textureRoles"]["tintPalette"] =
        json!({"textureIndex": 0, "texCoord": 1});
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--shader-index",
            "0",
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(!output.status.success());
    assert!(!output_path.exists());
    let body = stdout_json(&output);
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("spec.sps only exposes DiffuseSampler and SpecSampler"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_rejects_unmapped_pbr_factors_before_creating_output() {
    let Ok(template_path) = std::env::var("RAGELAB_SHADER_TEMPLATE_YFT") else {
        return;
    };
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    let profile_path = root.join("pumpkin.gta-materials.json");
    let output_path = root.join("unsupported-pbr.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(initialized.status.success());
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile["materials"][0]["parameters"]["baseColorFactor"] = json!([1.0, 0.5, 0.25, 1.0]);
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--profile",
            profile_path.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--shader-template",
            template_path.as_str(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert!(!output.status.success());
    assert!(!output_path.exists());
    let body = stdout_json(&output);
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("baseColorFactor"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_profile_cli_creates_and_validates_versioned_sidecar() {
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    let profile = root.join("pumpkin.gta-materials.json");
    fs::write(&source, minimal_glb()).unwrap();

    let initialized = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let init_body = stdout_json(&initialized);
    assert_eq!(init_body["command"], "glb.profile.init");
    assert_eq!(init_body["data"]["embedTextures"], true);
    assert!(profile.is_file());

    let validated = binary()
        .args([
            "glb",
            "profile",
            "validate",
            source.to_str().unwrap(),
            profile.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert!(
        validated.status.success(),
        "stdout: {}\\nstderr: {}",
        String::from_utf8_lossy(&validated.stdout),
        String::from_utf8_lossy(&validated.stderr)
    );
    let validation_body = stdout_json(&validated);
    assert_eq!(validation_body["command"], "glb.profile.validate");
    assert_eq!(validation_body["data"]["valid"], true);

    let source_bytes = fs::read(&source).unwrap();
    let mut changed = source_bytes.clone();
    changed.push(0);
    fs::write(&source, changed).unwrap();
    let stale = binary()
        .args([
            "glb",
            "profile",
            "validate",
            source.to_str().unwrap(),
            profile.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");
    assert_eq!(stale.status.code(), Some(4));
    let stale_body = stdout_json(&stale);
    assert_eq!(stale_body["error"]["code"], "validation_failed");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_profile_init_does_not_overwrite_existing_profile_without_flag() {
    let root = temp_root();
    let source = root.join("model.glb");
    let profile = root.join("profile.json");
    fs::write(&source, minimal_glb()).unwrap();
    fs::write(&profile, b"keep").unwrap();

    let output = binary()
        .args([
            "glb",
            "profile",
            "init",
            source.to_str().unwrap(),
            "--output",
            profile.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read(&profile).unwrap(), b"keep");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_runs_in_memory_semantic_preflight_and_reports_warnings() {
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert!(
        output.status.success(),
        "stdout: {}\\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.validate");
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["schema"], "ragelab.glb.validation");
    assert_eq!(body["data"]["schemaVersion"], 1);
    assert_eq!(body["data"]["valid"], true);
    assert_eq!(body["data"]["structuralValid"], true);
    assert_eq!(body["data"]["exportCompatible"], true);
    assert_eq!(body["data"]["geometryDecoded"], true);
    assert_eq!(body["data"]["imagesDecoded"], true);
    assert_eq!(body["data"]["semanticPreflight"], true);
    assert_eq!(body["data"]["semanticReopen"], true);
    assert_eq!(body["data"]["runtimeRenderable"], false);
    assert_eq!(body["data"]["counts"]["decodedPrimitives"], 1);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["code"] == "ydr_semantic_preflight_passed"));
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["code"] == "missing_normals" && item["severity"] == "warning"));
    assert_eq!(
        fs::read_dir(&root).unwrap().count(),
        1,
        "validation must not write a YDR or any other output"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_strict_promotes_warnings_without_misreporting_structural_invalidity() {
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let output = binary()
        .args([
            "glb",
            "validate",
            source.to_str().unwrap(),
            "--strict",
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.validate");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(body["data"]["valid"], false);
    assert_eq!(body["data"]["structuralValid"], true);
    assert_eq!(body["data"]["exportCompatible"], true);
    assert_eq!(body["data"]["strict"], true);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["code"] == "missing_normals"
                && item["severity"] == "error"
                && item["promotedFrom"] == "warning"
        }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_accounts_for_matrix_column_padding_in_unreferenced_accessors() {
    let root = temp_root();
    let source = root.join("short-mat3.glb");
    let bytes = rewrite_glb_document(&triangle_glb_with_png(), |document| {
        let view_index = document["bufferViews"].as_array().unwrap().len();
        document["bufferViews"]
            .as_array_mut()
            .unwrap()
            .push(json!({"buffer": 0, "byteOffset": 0, "byteLength": 9}));
        document["accessors"].as_array_mut().unwrap().push(json!({
            "bufferView": view_index,
            "componentType": 5121,
            "count": 1,
            "type": "MAT3"
        }));
    });
    fs::write(&source, bytes).unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["data"]["structuralValid"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["code"] == "glb_invalid"
                && item["message"]
                    .as_str()
                    .unwrap()
                    .contains("byte range exceeds")
        }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_checks_sparse_accessor_indices_and_values_ranges() {
    let root = temp_root();
    let source = root.join("short-sparse-indices.glb");
    let bytes = rewrite_glb_document(&triangle_glb_with_png(), |document| {
        let indices_view = document["bufferViews"].as_array().unwrap().len();
        document["bufferViews"]
            .as_array_mut()
            .unwrap()
            .push(json!({"buffer": 0, "byteOffset": 0, "byteLength": 1}));
        let values_view = document["bufferViews"].as_array().unwrap().len();
        document["bufferViews"]
            .as_array_mut()
            .unwrap()
            .push(json!({"buffer": 0, "byteOffset": 4, "byteLength": 12}));
        document["accessors"].as_array_mut().unwrap().push(json!({
            "componentType": 5126,
            "count": 3,
            "type": "VEC3",
            "sparse": {
                "count": 1,
                "indices": {
                    "bufferView": indices_view,
                    "componentType": 5121,
                    "byteOffset": 2
                },
                "values": {"bufferView": values_view}
            }
        }));
    });
    fs::write(&source, bytes).unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["data"]["structuralValid"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["code"] == "glb_invalid"
                && item["message"].as_str().unwrap().contains("sparse.indices")
                && item["message"]
                    .as_str()
                    .unwrap()
                    .contains("byte range exceeds")
        }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_rejects_unreferenced_accessor_range_outside_buffer_view() {
    let root = temp_root();
    let source = root.join("invalid-unused-accessor.glb");
    let bytes = rewrite_glb_document(&triangle_glb_with_png(), |document| {
        let position_accessor = document["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
            .as_u64()
            .expect("POSITION accessor index") as usize;
        let mut unused = document["accessors"][position_accessor].clone();
        unused["byteOffset"] = json!(1024);
        document["accessors"]
            .as_array_mut()
            .expect("accessor array")
            .push(unused);
    });
    fs::write(&source, bytes).unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["data"]["structuralValid"], false);
    assert_eq!(body["data"]["geometryDecoded"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["code"] == "glb_invalid"
                && item["severity"] == "error"
                && item["message"].as_str().unwrap().contains("accessors[")
                && item["message"]
                    .as_str()
                    .unwrap()
                    .contains("byte range exceeds")
        }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_rejects_referenced_accessor_range_outside_buffer_view() {
    let root = temp_root();
    let source = root.join("invalid-accessor-range.glb");
    let bytes = rewrite_glb_document(&triangle_glb_with_png(), |document| {
        let position_accessor = document["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
            .as_u64()
            .expect("POSITION accessor index") as usize;
        document["accessors"][position_accessor]["byteOffset"] = json!(1024);
    });
    fs::write(&source, bytes).unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.validate");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(body["data"]["structuralValid"], false);
    assert_eq!(body["data"]["geometryDecoded"], false);
    assert_eq!(body["data"]["semanticPreflight"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["code"] == "glb_invalid"
                && item["severity"] == "error"
                && item["message"].as_str().unwrap().contains("accessors[")
                && item["message"]
                    .as_str()
                    .unwrap()
                    .contains("byte range exceeds")
        }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_reports_export_profile_failure_without_falsifying_structure() {
    let root = temp_root();
    let source = root.join("valid-structure.glb");
    let profile = root.join("invalid-profile.json");
    fs::write(&source, triangle_glb_with_png()).unwrap();
    fs::write(
        &profile,
        br#"{"schema":"unknown.profile","schemaVersion":99}"#,
    )
    .unwrap();

    let output = binary()
        .args([
            "glb",
            "validate",
            source.to_str().unwrap(),
            "--profile",
            profile.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(body["data"]["structuralValid"], true);
    assert_eq!(body["data"]["geometryDecoded"], true);
    assert_eq!(body["data"]["imagesDecoded"], true);
    assert_eq!(body["data"]["exportCompatible"], false);
    assert_eq!(body["data"]["semanticPreflight"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| { item["code"] == "export_preflight_invalid" && item["severity"] == "error" }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_rejects_invalid_sampler_enum() {
    let root = temp_root();
    let source = root.join("invalid-sampler.glb");
    let bytes = rewrite_glb_document(&triangle_glb_with_png(), |document| {
        document["samplers"] = json!([{"wrapS": 12345}]);
        document["textures"][0]["sampler"] = json!(0);
    });
    fs::write(&source, bytes).unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["data"]["structuralValid"], false);
    assert_eq!(body["data"]["exportCompatible"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["code"] == "invalid_sampler"
                && item["severity"] == "error"
                && item["path"] == "samplers[0]"
        }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_returns_versioned_report_for_invalid_container() {
    let root = temp_root();
    let source = root.join("broken.glb");
    fs::write(&source, b"not a GLB container").unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.validate");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(body["data"]["schema"], "ragelab.glb.validation");
    assert_eq!(body["data"]["schemaVersion"], 1);
    assert_eq!(body["data"]["valid"], false);
    assert_eq!(body["data"]["structuralValid"], false);
    assert_eq!(body["data"]["exportCompatible"], false);
    assert_eq!(body["data"]["semanticPreflight"], false);
    assert_eq!(body["data"]["runtimeRenderable"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["code"] == "glb_invalid" && item["severity"] == "error"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_rejects_required_extension_missing_from_extensions_used() {
    let root = temp_root();
    let source = root.join("required-extension-not-used.glb");
    let bytes = rewrite_glb_document(&triangle_glb_with_png(), |document| {
        document["extensionsRequired"] = json!(["EXT_ragelab_validation_fixture"]);
    });
    fs::write(&source, bytes).unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(4));
    let body = stdout_json(&output);
    assert_eq!(body["data"]["structuralValid"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["code"] == "required_extension_not_used"
                && item["severity"] == "error"
                && item["path"] == "extensionsRequired"
        }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_writes_deterministic_versioned_report_sidecar() {
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    let output_path = root.join("pumpkin.ydr");
    let report_a = root.join("pumpkin.export-report.json");
    let report_b = root.join("pumpkin.export-report-repeat.json");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let first = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--report",
            report_a.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should export YDR and report");
    assert!(
        first.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_envelope = stdout_json(&first);
    assert_eq!(first_envelope["command"], "glb.export");
    assert_eq!(first_envelope["data"]["report"], report_a.to_str().unwrap());
    let first_bytes = fs::read(&report_a).unwrap();
    let first_report: Value = serde_json::from_slice(&first_bytes).unwrap();

    assert_eq!(first_report["schema"], "ragelab.glb.export-report");
    assert_eq!(first_report["schemaVersion"], 1);
    assert_eq!(first_report["tool"]["name"], "ragelab");
    assert!(!first_report["tool"]["coreRevision"]
        .as_str()
        .unwrap()
        .is_empty());
    assert!(first_report["tool"]["coreDirty"].is_boolean());
    assert_eq!(
        first_report["provenance"]["input"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        first_report["provenance"]["authoringProfile"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        first_report["provenance"]["output"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(first_report["summary"]["geometryCount"], 1);
    assert_eq!(first_report["summary"]["embeddedTextureCount"], 1);
    assert_eq!(
        first_report["decisions"]["materials"][0]["sourceMaterialIndex"],
        0
    );
    assert_eq!(
        first_report["decisions"]["geometries"][0]["geometryIndex"],
        0
    );
    assert_eq!(
        first_report["decisions"]["textureSlotMappings"][0]["role"],
        "albedo"
    );
    assert_eq!(
        first_report["decisions"]["shaderParameters"]["outputShaderCount"],
        0
    );
    assert!(first_report["decisions"]["shaderParameters"]["shaders"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(first_report["transformations"][1]["from"], "glTF_Y_up");
    assert_eq!(first_report["transformations"][1]["to"], "GTA_Z_up");
    assert_eq!(
        first_report["textureConversions"][0]["decodedPixelFormat"],
        "rgba8"
    );
    assert_eq!(first_report["textureConversions"][0]["outputFormat"], "bc1");
    assert_eq!(
        first_report["validationStages"][5]["stage"],
        "ydr_semantic_reopen"
    );
    assert_eq!(first_report["validationStages"][5]["status"], "passed");
    assert_eq!(first_report["runtimeRenderable"], false);
    assert_eq!(
        first_report["runtimeAcceptance"]["codewalkerGui"],
        "not_run"
    );
    assert_eq!(first_report["runtimeAcceptance"]["gtaV"], "not_run");

    let material = &first_report["decisions"]["materials"][0];
    let geometry = &first_report["decisions"]["geometries"][0];
    let texture_slot = &first_report["decisions"]["textureSlotMappings"][0];
    let texture = &first_report["textureConversions"][0];
    let golden_snapshot = json!({
        "schema": first_report["schema"],
        "schemaVersion": first_report["schemaVersion"],
        "tool": {
            "name": first_report["tool"]["name"],
            "version": first_report["tool"]["version"]
        },
        "summary": first_report["summary"],
        "material": {
            "sourceMaterialIndex": material["sourceMaterialIndex"],
            "sourceName": material["sourceName"],
            "shaderIntent": material["shaderIntent"],
            "surfaceMode": material["surfaceMode"],
            "shaderDecision": material["shader"]["decision"],
            "textureSlotMappings": [{
                "materialIndex": texture_slot["materialIndex"],
                "role": texture_slot["role"],
                "textureIndex": texture_slot["textureIndex"],
                "texCoord": texture_slot["texCoord"],
                "imageIndex": texture_slot["imageIndex"],
                "outputTextureName": texture_slot["outputTextureName"]
            }]
        },
        "geometry": {
            "geometryIndex": geometry["geometryIndex"],
            "nodeIndex": geometry["source"]["nodeIndex"],
            "nodeName": geometry["source"]["nodeName"],
            "meshIndex": geometry["source"]["meshIndex"],
            "meshName": geometry["source"]["meshName"],
            "primitiveIndex": geometry["source"]["primitiveIndex"],
            "primitiveModeName": geometry["source"]["primitiveModeName"],
            "materialIndex": geometry["materialIndex"],
            "materialProfileIndex": geometry["materialProfileIndex"],
            "shaderIndex": geometry["shaderIndex"],
            "vertexCount": geometry["vertexCount"],
            "indexCount": geometry["indexCount"],
            "triangleCount": geometry["triangleCount"],
            "attributes": geometry["attributes"]
        },
        "shaderParameters": first_report["decisions"]["shaderParameters"],
        "transformations": [
            {
                "id": first_report["transformations"][0]["id"],
                "status": first_report["transformations"][0]["status"]
            },
            {
                "id": first_report["transformations"][1]["id"],
                "status": first_report["transformations"][1]["status"],
                "from": first_report["transformations"][1]["from"],
                "to": first_report["transformations"][1]["to"]
            },
            {
                "id": first_report["transformations"][2]["id"],
                "status": first_report["transformations"][2]["status"],
                "outputMode": first_report["transformations"][2]["outputMode"]
            },
            {
                "id": first_report["transformations"][3]["id"],
                "status": first_report["transformations"][3]["status"]
            }
        ],
        "textureConversion": {
            "imageIndex": texture["imageIndex"],
            "sourceName": texture["sourceName"],
            "sourceMimeType": texture["sourceMimeType"],
            "width": texture["width"],
            "height": texture["height"],
            "decodedPixelFormat": texture["decodedPixelFormat"],
            "outputTextureName": texture["outputTextureName"],
            "outputFormat": texture["outputFormat"],
            "hasTransparency": texture["hasTransparency"],
            "hasPartialAlpha": texture["hasPartialAlpha"],
            "mipmapLevelsGenerated": texture["mipmapLevelsGenerated"],
            "lossyBlockCompression": texture["lossyBlockCompression"],
            "compressionSelection": texture["compressionSelection"]
        },
        "resources": first_report["resources"],
        "losses": first_report["losses"],
        "fallbacks": first_report["fallbacks"],
        "runtimeRenderable": first_report["runtimeRenderable"],
        "runtimeAcceptance": first_report["runtimeAcceptance"]
    });
    let golden: Value = serde_json::from_str(include_str!(
        "../../../fixtures/synthetic/glb-export-report-v1.golden.json"
    ))
    .unwrap();
    assert_eq!(
        golden_snapshot, golden,
        "GLB export v1 golden snapshot changed"
    );

    fs::remove_file(&output_path).unwrap();
    let second = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--report",
            report_b.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should reproduce the report");
    assert!(
        second.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(fs::read(&report_b).unwrap(), first_bytes);

    fs::remove_file(&output_path).unwrap();
    fs::remove_file(&report_b).unwrap();
    let human = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--report",
            report_b.to_str().unwrap(),
        ])
        .output()
        .expect("ragelab should produce the same sidecar in human mode");
    assert!(
        human.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&human.stdout),
        String::from_utf8_lossy(&human.stderr)
    );
    assert!(
        String::from_utf8_lossy(&human.stdout).contains(&format!("report: {}", report_b.display()))
    );
    assert_eq!(fs::read(&report_b).unwrap(), first_bytes);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_report_never_overwrites_existing_sidecar_or_output() {
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    let output_path = root.join("pumpkin.ydr");
    let report_path = root.join("sentinel.report.json");
    fs::write(&source, triangle_glb_with_png()).unwrap();
    fs::write(&report_path, b"preserve-this-report").unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--report",
            report_path.to_str().unwrap(),
            "--overwrite",
            "--json",
        ])
        .output()
        .expect("ragelab should reject an existing report path");

    assert!(!output.status.success());
    assert_eq!(fs::read(&report_path).unwrap(), b"preserve-this-report");
    assert!(!output_path.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_export_report_must_not_alias_any_export_input_or_output() {
    let root = temp_root();
    let source = root.join("pumpkin.glb");
    let output_path = root.join("pumpkin.ydr");
    fs::write(&source, triangle_glb_with_png()).unwrap();

    let output = binary()
        .args([
            "glb",
            "export",
            source.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--report",
            output_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab should reject report/output path aliasing");

    assert!(!output.status.success());
    assert!(!output_path.exists());

    #[cfg(windows)]
    {
        let case_alias = root.join("PUMPKIN.YDR");
        let case_alias_output = binary()
            .args([
                "glb",
                "export",
                source.to_str().unwrap(),
                "--output",
                output_path.to_str().unwrap(),
                "--report",
                case_alias.to_str().unwrap(),
                "--json",
            ])
            .output()
            .expect("ragelab should reject case-insensitive path aliasing on Windows");
        assert!(!case_alias_output.status.success());
        assert!(!output_path.exists());
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn glb_validate_reports_unsupported_extensions_in_failure_envelope() {
    let root = temp_root();
    let source = root.join("unsupported-extension.glb");
    let bytes = rewrite_glb_document(&triangle_glb_with_png(), |document| {
        document["extensionsUsed"] = json!(["EXT_ragelab_validation_fixture"]);
    });
    fs::write(&source, bytes).unwrap();

    let output = binary()
        .args(["glb", "validate", source.to_str().unwrap(), "--json"])
        .output()
        .expect("ragelab should run");

    assert_eq!(output.status.code(), Some(3));
    let body = stdout_json(&output);
    assert_eq!(body["command"], "glb.validate");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "unsupported");
    assert_eq!(body["data"]["structuralValid"], true);
    assert_eq!(body["data"]["exportCompatible"], false);
    assert_eq!(body["data"]["semanticPreflight"], false);
    assert!(body["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["code"] == "unsupported_extension"
                && item["severity"] == "unsupported"
                && item["message"]
                    .as_str()
                    .unwrap()
                    .contains("EXT_ragelab_validation_fixture")
        }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn namespaced_glb_batch_writes_item_and_aggregate_reports() {
    let root = temp_root();
    for directory in ["input", "profiles", "output", "reports"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }

    let source = triangle_glb_with_png();
    fs::write(root.join("input/triangle.glb"), &source).unwrap();
    let profile = ragelab_engine::default_glb_authoring_profile(&source, "triangle.glb").unwrap();
    fs::write(
        root.join("profiles/triangle.json"),
        serde_json::to_vec_pretty(&profile).unwrap(),
    )
    .unwrap();

    let manifest = json!({
        "schema": "ragelab.glb.batch-manifest",
        "schemaVersion": 1,
        "failurePolicy": "continue",
        "concurrency": 1,
        "report": "reports/batch.json",
        "items": [{
            "id": "triangle",
            "source": "input/triangle.glb",
            "output": "output/triangle.ydr",
            "profile": "profiles/triangle.json",
            "shaderTemplate": null,
            "shaderIndex": 0,
            "nodes": [],
            "report": "reports/triangle.export-report.json"
        }]
    });
    fs::write(
        root.join("batch.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let output = binary()
        .args([
            "glb",
            "batch",
            root.join("batch.json").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("ragelab batch should run");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("[glb.batch] 1/1"));
    let body = stdout_json(&output);
    assert_eq!(body["schema"], "ragelab.cli.response");
    assert_eq!(body["command"], "glb.batch");
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["schema"], "ragelab.glb.batch-report");
    assert_eq!(body["data"]["summary"]["succeeded"], 1);
    assert_eq!(body["data"]["items"][0]["id"], "triangle");
    assert_eq!(body["data"]["items"][0]["status"], "succeeded");
    assert_eq!(
        PathBuf::from(body["data"]["aggregateReport"].as_str().unwrap())
            .file_name()
            .unwrap(),
        std::ffi::OsStr::new("batch.json")
    );
    assert!(root.join("output/triangle.ydr").is_file());
    assert!(root.join("reports/triangle.export-report.json").is_file());
    assert!(root.join("reports/batch.json").is_file());

    let aggregate: Value =
        serde_json::from_slice(&fs::read(root.join("reports/batch.json")).unwrap()).unwrap();
    assert_eq!(aggregate["schema"], "ragelab.glb.batch-report");
    assert_eq!(
        aggregate["items"][0]["outputSha256"],
        body["data"]["items"][0]["outputSha256"]
    );

    fs::remove_dir_all(root).unwrap();
}
