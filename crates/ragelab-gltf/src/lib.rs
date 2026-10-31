mod decode;
mod images;
mod parser;
pub use decode::*;
pub use images::*;
pub use parser::*;

use std::collections::BTreeSet;

/// Creates a versioned editable material profile with standard glTF slots
/// mapped to explicit semantic roles. This is an authoring preset, not proof
/// that every intent already has a verified GTA shader implementation.
pub fn default_authoring_profile(report: &GlbInspectionReport) -> GtaAuthoringProfile {
    let materials = report
        .materials
        .iter()
        .map(|material| {
            let emissive_nonzero = material.emissive_factor.iter().any(|v| *v > 0.0)
                || material.emissive_texture.is_some()
                || material.emissive_strength > 1.0;
            let shader_intent = if material.unlit {
                ShaderIntent::Unlit
            } else if emissive_nonzero {
                ShaderIntent::Emissive
            } else {
                ShaderIntent::Lit
            };
            let surface_mode = match material.alpha_mode.as_str() {
                "MASK" => SurfaceMode::Masked,
                "BLEND" => SurfaceMode::Blended,
                _ => SurfaceMode::Opaque,
            };
            MaterialAuthoringSettings {
                source_material_index: material.index,
                source_name: material.name.clone(),
                shader_intent,
                surface_mode,
                texture_roles: TextureRoleMap {
                    albedo: material.base_color_texture.clone(),
                    normal: material.normal_texture.clone(),
                    metallic_roughness: material.metallic_roughness_texture.clone(),
                    specular: material.specular_texture.clone(),
                    specular_color: material.specular_color_texture.clone(),
                    tint_palette: None,
                    occlusion: material.occlusion_texture.clone(),
                    emissive: material.emissive_texture.clone(),
                },
                parameters: MaterialParameters {
                    base_color_factor: material.base_color_factor,
                    metallic: material.metallic_factor,
                    roughness: material.roughness_factor,
                    emissive_factor: material.emissive_factor,
                    emissive_strength: material.emissive_strength,
                    alpha_cutoff: material.alpha_cutoff,
                    double_sided: material.double_sided,
                    transmission_factor: material.transmission_factor,
                    ior: material.ior,
                },
            }
        })
        .collect();

    GtaAuthoringProfile {
        schema: PROFILE_SCHEMA.to_string(),
        schema_version: SCHEMA_VERSION,
        source: ProfileSource {
            file_name: report.source.file_name.clone(),
            byte_length: report.source.byte_length,
            sha256: report.source.sha256.clone(),
            material_count: report.counts.materials,
            texture_count: report.counts.textures,
        },
        settings: ExportSettings {
            target: "gta_v_legacy_gen8".into(),
            embed_textures: true,
            generate_mipmaps: true,
            texture_encoding: TextureEncoding::Auto,
            normal_map_convention: NormalMapConvention::OpenGl,
        },
        materials,
    }
}

pub fn validate_authoring_profile(
    report: &GlbInspectionReport,
    profile: &GtaAuthoringProfile,
) -> Result<ProfileValidationReport, GlbError> {
    if profile.schema != PROFILE_SCHEMA {
        return Err(GlbError::InvalidProfile(format!(
            "schema must be {PROFILE_SCHEMA}"
        )));
    }
    if profile.schema_version != SCHEMA_VERSION {
        return Err(GlbError::InvalidProfile(format!(
            "schemaVersion {} is unsupported; expected {}",
            profile.schema_version, SCHEMA_VERSION
        )));
    }
    if profile.source.sha256 != report.source.sha256 {
        return Err(GlbError::InvalidProfile(
            "profile source SHA-256 does not match the supplied GLB".into(),
        ));
    }
    if profile.source.byte_length != report.source.byte_length {
        return Err(GlbError::InvalidProfile(
            "profile source byteLength does not match the supplied GLB".into(),
        ));
    }
    if profile.source.material_count != report.counts.materials
        || profile.source.texture_count != report.counts.textures
    {
        return Err(GlbError::InvalidProfile(
            "profile material/texture counts do not match the supplied GLB".into(),
        ));
    }
    if profile.settings.target != "gta_v_legacy_gen8" {
        return Err(GlbError::InvalidProfile(
            "only target gta_v_legacy_gen8 is supported by this profile schema".into(),
        ));
    }
    if profile.materials.len() != report.materials.len() {
        return Err(GlbError::InvalidProfile(format!(
            "profile has {} material entries but source has {}",
            profile.materials.len(),
            report.materials.len()
        )));
    }

    let mut seen_materials = BTreeSet::new();
    let mut mapped_texture_slots = 0usize;
    for material in &profile.materials {
        idx(
            material.source_material_index,
            report.materials.len(),
            "profile material",
        )
        .map_err(|error| GlbError::InvalidProfile(error.to_string()))?;
        if !seen_materials.insert(material.source_material_index) {
            return Err(GlbError::InvalidProfile(format!(
                "material {} appears more than once in the profile",
                material.source_material_index
            )));
        }
        for (label, value) in [
            ("metallic", material.parameters.metallic),
            ("roughness", material.parameters.roughness),
            ("alphaCutoff", material.parameters.alpha_cutoff),
            (
                "baseColorFactor[0]",
                material.parameters.base_color_factor[0],
            ),
            (
                "baseColorFactor[1]",
                material.parameters.base_color_factor[1],
            ),
            (
                "baseColorFactor[2]",
                material.parameters.base_color_factor[2],
            ),
            (
                "baseColorFactor[3]",
                material.parameters.base_color_factor[3],
            ),
        ] {
            validate_unit_interval(value, label)?;
        }
        for (index, value) in material.parameters.emissive_factor.iter().enumerate() {
            if !value.is_finite() || *value < 0.0 {
                return Err(GlbError::InvalidProfile(format!(
                    "emissiveFactor[{index}] must be finite and non-negative"
                )));
            }
        }
        if !material.parameters.emissive_strength.is_finite()
            || material.parameters.emissive_strength < 0.0
        {
            return Err(GlbError::InvalidProfile(
                "emissiveStrength must be finite and non-negative".into(),
            ));
        }
        if let Some(transmission) = material.parameters.transmission_factor {
            validate_unit_interval(transmission, "transmissionFactor")?;
        }
        if let Some(ior) = material.parameters.ior {
            if !ior.is_finite() || ior <= 0.0 {
                return Err(GlbError::InvalidProfile(
                    "ior must be finite and greater than zero".into(),
                ));
            }
        }

        for binding in [
            material.texture_roles.albedo.as_ref(),
            material.texture_roles.normal.as_ref(),
            material.texture_roles.metallic_roughness.as_ref(),
            material.texture_roles.specular.as_ref(),
            material.texture_roles.specular_color.as_ref(),
            material.texture_roles.tint_palette.as_ref(),
            material.texture_roles.occlusion.as_ref(),
            material.texture_roles.emissive.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            idx(
                binding.texture_index,
                report.textures.len(),
                "profile texture role",
            )
            .map_err(|error| GlbError::InvalidProfile(error.to_string()))?;
            mapped_texture_slots += 1;
        }
    }

    let mut notes = vec![
        "profile validation checks source identity and references; it does not certify a GTA shader mapping".into(),
        "glTF metallicRoughness textures use glTF channel conventions; a future GTA writer must explicitly repack or reinterpret channels".into(),
    ];
    if !profile.settings.embed_textures {
        notes.push("the initial embedded-YDR writer will require embedTextures=true".into());
    }
    if report.images.iter().any(|image| !image.embedded) {
        notes.push("one or more source images are external and must be resolved or embedded before a self-contained YDR can be produced".into());
    }
    if report
        .textures
        .iter()
        .any(|texture| texture.source_image_index.is_none())
    {
        notes.push("one or more texture sources are supplied through an extension or are otherwise unresolved; the future writer must resolve them explicitly".into());
    }
    if !report.unsupported_required_extensions.is_empty() {
        notes.push(
            "required glTF extensions are not fully supported; conversion must remain blocked"
                .into(),
        );
    }

    Ok(ProfileValidationReport {
        schema: PROFILE_VALIDATION_SCHEMA,
        schema_version: SCHEMA_VERSION,
        valid: true,
        source_sha256: report.source.sha256.clone(),
        material_count: profile.materials.len(),
        mapped_texture_slots,
        embed_textures: profile.settings.embed_textures,
        notes,
    })
}

fn validate_unit_interval(value: f64, label: &str) -> Result<(), GlbError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(GlbError::InvalidProfile(format!(
            "{label} must be finite and between 0 and 1"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn sample_glb() -> Vec<u8> {
        let document = json!({
            "asset": {"version": "2.0", "generator": "RageLab synthetic fixture"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": "Pumpkin", "mesh": 0}],
            "meshes": [{
                "name": "PumpkinMesh",
                "primitives": [{
                    "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2},
                    "indices": 3,
                    "material": 0
                }]
            }],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [-1.0, -1.0, -1.0], "max": [1.0, 1.0, 1.0]},
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC2"},
                {"bufferView": 0, "componentType": 5123, "count": 3, "type": "SCALAR"}
            ],
            "buffers": [{"byteLength": 60}],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 24}
            ],
            "images": [{"name": "pumpkin_orange", "bufferView": 1, "mimeType": "image/png"}],
            "textures": [{"name": "pumpkin_albedo", "source": 0}],
            "materials": [{
                "name": "OrangeShell",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [1.0, 0.45, 0.08, 1.0],
                    "baseColorTexture": {"index": 0, "texCoord": 0},
                    "metallicFactor": 0.0,
                    "roughnessFactor": 0.82
                },
                "alphaMode": "OPAQUE",
                "doubleSided": false
            }]
        });
        let mut binary = vec![0u8; 60];
        binary[36..44].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        binary[44..48].copy_from_slice(&13u32.to_be_bytes());
        binary[48..52].copy_from_slice(b"IHDR");
        binary[52..56].copy_from_slice(&512u32.to_be_bytes());
        binary[56..60].copy_from_slice(&256u32.to_be_bytes());
        build_glb(document, binary)
    }

    fn build_glb(document: Value, mut binary: Vec<u8>) -> Vec<u8> {
        let mut json_bytes = serde_json::to_vec(&document).unwrap();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        while binary.len() % 4 != 0 {
            binary.push(0);
        }
        let total = 12 + 8 + json_bytes.len() + 8 + binary.len();
        let mut glb = Vec::with_capacity(total);
        glb.extend_from_slice(GLB_MAGIC);
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(&JSON_CHUNK_TYPE.to_le_bytes());
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        glb.extend_from_slice(&BIN_CHUNK_TYPE.to_le_bytes());
        glb.extend_from_slice(&binary);
        glb
    }

    fn geometry_glb(mode: u32, scale: [f64; 3]) -> Vec<u8> {
        let document = json!({
            "asset": {"version": "2.0", "generator": "RageLab geometry fixture"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": "Triangle", "mesh": 0, "scale": scale}],
            "meshes": [{
                "name": "Geometry",
                "primitives": [{
                    "mode": mode,
                    "attributes": {
                        "POSITION": 0,
                        "NORMAL": 1,
                        "TANGENT": 2,
                        "TEXCOORD_0": 3,
                        "COLOR_0": 4
                    }
                }]
            }],
            "buffers": [{"byteLength": 162}],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 36},
                {"buffer": 0, "byteOffset": 72, "byteLength": 48},
                {"buffer": 0, "byteOffset": 120, "byteLength": 24},
                {"buffer": 0, "byteOffset": 144, "byteLength": 9},
                {"buffer": 0, "byteOffset": 156, "byteLength": 6}
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4"},
                {"bufferView": 3, "componentType": 5126, "count": 3, "type": "VEC2"},
                {"bufferView": 4, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC3"},
                {"bufferView": 5, "componentType": 5123, "count": 3, "type": "SCALAR"}
            ]
        });
        let mut binary = Vec::new();
        let positions = [[0.0f32, 0.0, 0.0], [1.0f32, 0.0, 0.0], [0.0f32, 1.0, 0.0]];
        for vector in positions {
            for component in vector {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        for _ in 0..3 {
            for component in [0.0f32, 1.0, 0.0] {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        for _ in 0..3 {
            for component in [1.0f32, 0.0, 0.0, 1.0] {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        for uv in [[0.25f32, 0.75], [1.0, 0.0], [0.0, 1.0]] {
            for component in uv {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        binary.extend_from_slice(&[255, 128, 0, 0, 255, 0, 0, 0, 255]);
        binary.extend_from_slice(&[0, 0, 0]);
        for index in [0u16, 1, 2] {
            binary.extend_from_slice(&index.to_le_bytes());
        }
        assert_eq!(binary.len(), 162);
        build_glb(document, binary)
    }

    #[test]
    fn decodes_geometry_attributes_and_converts_gltf_y_up_to_gta_z_up() {
        let bytes = geometry_glb(4, [1.0, 1.0, 1.0]);
        let report = decode_glb_geometry(&bytes, "triangle.glb").unwrap();
        assert_eq!(report.primitives.len(), 1);
        let primitive = &report.primitives[0];
        assert_eq!(primitive.vertex_count, 3);
        assert_eq!(primitive.index_count, 3);
        assert_eq!(primitive.bounds_min, [0.0, 0.0, 0.0]);
        assert_eq!(primitive.bounds_max, [1.0, 0.0, 1.0]);
        assert_eq!(primitive.indices, vec![0, 1, 2]);
        assert_eq!(primitive.positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(primitive.positions[1], [1.0, 0.0, 0.0]);
        assert_eq!(primitive.positions[2], [0.0, 0.0, 1.0]);
        assert_eq!(primitive.normals.as_ref().unwrap()[0], [0.0, 0.0, 1.0]);
        assert_eq!(
            primitive.tangents.as_ref().unwrap()[0],
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(primitive.texcoord_0.as_ref().unwrap()[0], [0.25, 0.75]);
        assert_eq!(
            primitive.colors_0.as_ref().unwrap()[0],
            [1.0, 128.0 / 255.0, 0.0, 1.0]
        );
    }

    #[test]
    fn negative_node_scale_reverses_triangle_winding() {
        let bytes = geometry_glb(4, [-1.0, 1.0, 1.0]);
        let report = decode_glb_geometry(&bytes, "mirrored.glb").unwrap();
        assert_eq!(report.primitives[0].indices, vec![0, 2, 1]);
        assert_eq!(report.primitives[0].positions[1], [-1.0, 0.0, 0.0]);
        assert_eq!(report.primitives[0].bounds_min, [-1.0, 0.0, 0.0]);
        assert_eq!(report.primitives[0].bounds_max, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn triangle_strip_and_fan_are_expanded_to_triangle_lists() {
        assert_eq!(
            decode::triangulate(5, &[0, 1, 2, 3], 0, 0).unwrap(),
            vec![0, 1, 2, 2, 1, 3]
        );
        assert_eq!(
            decode::triangulate(6, &[0, 1, 2, 3], 0, 0).unwrap(),
            vec![0, 1, 2, 0, 2, 3]
        );
    }

    #[test]
    fn compressed_required_extensions_block_geometry_decode() {
        let document = json!({
            "asset": {"version": "2.0"},
            "extensionsRequired": ["KHR_draco_mesh_compression"]
        });
        let bytes = build_glb(document, Vec::new());
        assert!(matches!(
            decode_glb_geometry(&bytes, "compressed.glb"),
            Err(GlbError::Unsupported(_))
        ));
    }

    #[test]
    fn inspects_glb_mesh_material_slots_and_embedded_png_dimensions() {
        let report = inspect_glb(&sample_glb(), "pumpkin.glb").unwrap();
        assert_eq!(report.counts.meshes, 1);
        assert_eq!(report.counts.primitives, 1);
        assert_eq!(
            report.meshes[0].primitives[0].attributes["POSITION"].count,
            3
        );
        assert_eq!(
            report.meshes[0].primitives[0]
                .indices
                .as_ref()
                .unwrap()
                .count,
            3
        );
        assert_eq!(report.materials[0].name.as_deref(), Some("OrangeShell"));
        assert_eq!(
            report.materials[0]
                .base_color_texture
                .as_ref()
                .unwrap()
                .texture_index,
            0
        );
        assert_eq!(report.images[0].dimensions.unwrap().width, 512);
        assert_eq!(report.images[0].dimensions.unwrap().height, 256);
        assert!(report.images[0].embedded);
    }

    #[test]
    fn builds_and_validates_material_profile_with_distinct_roles_and_surface() {
        let report = inspect_glb(&sample_glb(), "pumpkin.glb").unwrap();
        let profile = default_authoring_profile(&report);
        assert!(profile.settings.embed_textures);
        assert_eq!(profile.materials[0].surface_mode, SurfaceMode::Opaque);
        assert_eq!(
            profile.materials[0]
                .texture_roles
                .albedo
                .as_ref()
                .unwrap()
                .texture_index,
            0
        );
        assert_eq!(
            validate_authoring_profile(&report, &profile)
                .unwrap()
                .mapped_texture_slots,
            1
        );

        let mut changed = profile;
        changed.materials[0].surface_mode = SurfaceMode::Glass;
        changed.materials[0].texture_roles.normal = Some(TextureBindingInfo {
            texture_index: 0,
            tex_coord: 0,
            scale: Some(1.0),
            strength: None,
        });
        assert_eq!(
            validate_authoring_profile(&report, &changed)
                .unwrap()
                .mapped_texture_slots,
            2
        );
    }

    #[test]
    fn profile_rejects_wrong_source_and_invalid_texture_mapping() {
        let report = inspect_glb(&sample_glb(), "pumpkin.glb").unwrap();
        let mut profile = default_authoring_profile(&report);
        profile.source.sha256 = "0".repeat(64);
        assert!(validate_authoring_profile(&report, &profile).is_err());

        let mut profile = default_authoring_profile(&report);
        profile.materials[0].texture_roles.normal = Some(TextureBindingInfo {
            texture_index: 99,
            tex_coord: 0,
            scale: None,
            strength: None,
        });
        assert!(validate_authoring_profile(&report, &profile).is_err());
    }

    #[test]
    fn profiles_without_optional_tint_palette_remain_backward_compatible() {
        let report = inspect_glb(&sample_glb(), "pumpkin.glb").unwrap();
        let profile = default_authoring_profile(&report);
        let mut value = serde_json::to_value(profile).unwrap();
        value["materials"][0]["textureRoles"]
            .as_object_mut()
            .unwrap()
            .remove("tintPalette");

        let restored: GtaAuthoringProfile = serde_json::from_value(value).unwrap();
        assert_eq!(restored.materials[0].texture_roles.tint_palette, None);
        assert!(validate_authoring_profile(&report, &restored).is_ok());
    }

    #[test]
    fn rejects_invalid_glb_header_and_bad_buffer_view_bounds() {
        assert!(matches!(
            inspect_glb(b"not a glb", "bad.glb"),
            Err(GlbError::InvalidContainer(_))
        ));
        let document = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 60}],
            "bufferViews": [{"buffer": 0, "byteLength": 128}]
        });
        let invalid = build_glb(document, vec![0u8; 60]);
        assert!(inspect_glb(&invalid, "bad-range.glb").is_err());
    }

    #[test]
    fn external_buffer_image_is_not_reported_as_embedded() {
        let document = json!({
            "asset": {"version": "2.0"},
            "buffers": [
                {"byteLength": 4},
                {"uri": "images.bin", "byteLength": 4}
            ],
            "bufferViews": [{"buffer": 1, "byteLength": 4}],
            "images": [{"bufferView": 0, "mimeType": "image/png"}],
            "textures": [{"source": 0}]
        });
        let bytes = build_glb(document, vec![0, 0, 0, 0]);
        let report = inspect_glb(&bytes, "external-image.glb").unwrap();
        assert!(!report.images[0].embedded);
        let profile = default_authoring_profile(&report);
        let validation = validate_authoring_profile(&report, &profile).unwrap();
        assert!(validation
            .notes
            .iter()
            .any(|note| note.contains("external")));
    }

    #[test]
    fn unsupported_required_material_extension_is_reported() {
        let document = json!({
            "asset": {"version": "2.0"},
            "extensionsRequired": ["KHR_materials_clearcoat"]
        });
        let bytes = build_glb(document, Vec::new());
        let report = inspect_glb(&bytes, "clearcoat.glb").unwrap();
        assert_eq!(
            report.unsupported_required_extensions,
            vec!["KHR_materials_clearcoat"]
        );
        assert!(!report.warnings.is_empty());
    }

    #[test]
    fn profile_rejects_invalid_transmission_and_ior_values() {
        let report = inspect_glb(&sample_glb(), "pumpkin.glb").unwrap();

        let mut transmission = default_authoring_profile(&report);
        transmission.materials[0].parameters.transmission_factor = Some(1.2);
        assert!(validate_authoring_profile(&report, &transmission).is_err());

        let mut ior = default_authoring_profile(&report);
        ior.materials[0].parameters.ior = Some(0.0);
        assert!(validate_authoring_profile(&report, &ior).is_err());
    }

    #[test]
    fn alpha_modes_become_explicit_surface_intents() {
        let report = inspect_glb(&sample_glb(), "pumpkin.glb").unwrap();
        let mut masked = report.clone();
        masked.materials[0].alpha_mode = "MASK".into();
        assert_eq!(
            default_authoring_profile(&masked).materials[0].surface_mode,
            SurfaceMode::Masked
        );
        let mut blended = report;
        blended.materials[0].alpha_mode = "BLEND".into();
        assert_eq!(
            default_authoring_profile(&blended).materials[0].surface_mode,
            SurfaceMode::Blended
        );
    }
}
