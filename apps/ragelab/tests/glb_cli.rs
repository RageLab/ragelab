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
