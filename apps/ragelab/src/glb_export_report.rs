use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

use ragelab_engine::{
    decode_glb_geometry_bytes, default_glb_authoring_profile, inspect_glb_bytes, inspect_ydr_bytes,
    GlbYdrExportResult,
};
use ragelab_gltf::{decode_glb_images, GtaAuthoringProfile, TextureBindingInfo, TextureEncoding};
use serde_json::{json, Value};

pub(crate) struct GlbExportReportRequest<'a> {
    pub source_path: &'a Path,
    pub source_bytes: &'a [u8],
    pub output_path: &'a Path,
    pub profile_path: Option<&'a Path>,
    pub profile_bytes: Option<&'a [u8]>,
    pub shader_template_path: Option<&'a Path>,
    pub selected_node_indices: &'a [usize],
    pub output_overwrite: bool,
    pub result: &'a GlbYdrExportResult,
}

pub(crate) fn build_report(request: GlbExportReportRequest<'_>) -> Result<Value, Box<dyn Error>> {
    let GlbExportReportRequest {
        source_path,
        source_bytes,
        output_path,
        profile_path,
        profile_bytes,
        shader_template_path,
        selected_node_indices,
        output_overwrite,
        result,
    } = request;
    let source_name = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb");
    let inspection = inspect_glb_bytes(source_bytes, source_name)?;
    let decoded_geometry = decode_glb_geometry_bytes(source_bytes, source_name)?;
    let decoded_images = decode_glb_images(source_bytes)?;
    let output_name = output_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("output.ydr");
    let output_inspection = inspect_ydr_bytes(&result.output_bytes, output_name)?;
    let profile: GtaAuthoringProfile = match profile_bytes {
        Some(bytes) => serde_json::from_slice(bytes)?,
        None => default_glb_authoring_profile(source_bytes, source_name)?,
    };

    let included_nodes = result.report.selection.as_ref().map(|selection| {
        selection
            .included_node_indices
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
    });
    let included_primitives = decoded_geometry
        .primitives
        .iter()
        .filter(|primitive| {
            included_nodes
                .as_ref()
                .map_or(true, |nodes| nodes.contains(&primitive.node_index))
        })
        .collect::<Vec<_>>();
    let omitted_primitives = decoded_geometry
        .primitives
        .iter()
        .filter(|primitive| {
            included_nodes
                .as_ref()
                .is_some_and(|nodes| !nodes.contains(&primitive.node_index))
        })
        .map(|primitive| {
            json!({
                "nodeIndex": primitive.node_index,
                "meshIndex": primitive.mesh_index,
                "primitiveIndex": primitive.primitive_index,
                "materialIndex": primitive.material_index,
                "reason": "excluded_by_node_selection"
            })
        })
        .collect::<Vec<_>>();

    let image_names = decoded_images
        .iter()
        .map(|image| (image.index, embedded_texture_name(image.index, &image.name)))
        .collect::<BTreeMap<_, _>>();

    let mut material_decisions = Vec::with_capacity(profile.materials.len());
    let mut texture_slot_mappings = Vec::new();
    let mut referenced_images = BTreeSet::new();
    for (profile_index, material) in profile.materials.iter().enumerate() {
        let source_material = inspection
            .materials
            .iter()
            .find(|candidate| candidate.index == material.source_material_index);
        let mut slots = Vec::new();
        for (role, binding) in material_texture_roles(material) {
            if let Some(binding) = binding {
                let texture = inspection
                    .textures
                    .iter()
                    .find(|candidate| candidate.index == binding.texture_index);
                let image_index = texture.and_then(|candidate| candidate.source_image_index);
                if let Some(index) = image_index {
                    referenced_images.insert(index);
                }
                let output_texture_name =
                    image_index.and_then(|index| image_names.get(&index).cloned());
                let mapping = json!({
                    "materialIndex": material.source_material_index,
                    "role": role,
                    "textureIndex": binding.texture_index,
                    "texCoord": binding.tex_coord,
                    "textureName": texture.and_then(|item| item.name.clone()),
                    "samplerIndex": texture.and_then(|item| item.sampler_index),
                    "imageIndex": image_index,
                    "outputTextureName": output_texture_name
                });
                slots.push(mapping.clone());
                texture_slot_mappings.push(mapping);
            }
        }

        let geometry_indices = included_primitives
            .iter()
            .enumerate()
            .filter(|(_, primitive)| {
                primitive.material_index == Some(material.source_material_index)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let shader_index = result
            .report
            .shader_template
            .as_ref()
            .map(|_| profile_index);

        material_decisions.push(json!({
            "profileIndex": profile_index,
            "sourceMaterialIndex": material.source_material_index,
            "sourceName": material.source_name.clone().or_else(|| source_material.and_then(|item| item.name.clone())),
            "shaderIntent": material.shader_intent,
            "surfaceMode": material.surface_mode,
            "parameters": material.parameters,
            "textureSlotMappings": slots,
            "shader": {
                "preset": result.report.shader_template.as_ref().map(|template| template.preset),
                "shaderIndex": shader_index,
                "decision": if shader_index.is_some() { "clone_verified_template_for_material" } else { "no_template_structural_geometry_only" }
            },
            "includedGeometryIndices": geometry_indices
        }));
    }

    let has_unmaterialed_geometry = included_primitives
        .iter()
        .any(|primitive| primitive.material_index.is_none());
    let default_shader_index =
        if has_unmaterialed_geometry && result.report.shader_template.is_some() {
            Some(profile.materials.len())
        } else {
            None
        };
    let unmaterialed_geometry_indices = included_primitives
        .iter()
        .enumerate()
        .filter_map(|(index, primitive)| primitive.material_index.is_none().then_some(index))
        .collect::<Vec<_>>();
    let fallback_decisions = default_shader_index
        .map(|shader_index| {
            vec![json!({
                "kind": "neutral_shader_for_unmaterialed_geometry",
                "shaderIndex": shader_index,
                "affectedGeometryIndices": unmaterialed_geometry_indices,
                "method": "clone_verified_template_and_clear_sampler_bindings",
                "authorizedBy": "--shader-template",
                "templateSha256": result.report.shader_template.as_ref().map(|template| template.sha256.as_str())
            })]
        })
        .unwrap_or_default();

    let geometry_decisions = included_primitives
        .iter()
        .enumerate()
        .map(|(geometry_index, primitive)| {
            let material_profile_index = primitive.material_index.and_then(|source_index| {
                profile
                    .materials
                    .iter()
                    .position(|material| material.source_material_index == source_index)
            });
            let shader_index = if result.report.shader_template.is_none() {
                None
            } else {
                material_profile_index.or(default_shader_index)
            };
            json!({
                "geometryIndex": geometry_index,
                "source": {
                    "nodeIndex": primitive.node_index,
                    "nodeName": primitive.node_name,
                    "meshIndex": primitive.mesh_index,
                    "meshName": primitive.mesh_name,
                    "primitiveIndex": primitive.primitive_index,
                    "primitiveMode": primitive.source_mode,
                    "primitiveModeName": primitive_mode_name(primitive.source_mode)
                },
                "materialIndex": primitive.material_index,
                "materialProfileIndex": material_profile_index,
                "shaderIndex": shader_index,
                "vertexCount": primitive.vertex_count,
                "indexCount": primitive.index_count,
                "triangleCount": primitive.index_count / 3,
                "bounds": {
                    "min": primitive.bounds_min,
                    "max": primitive.bounds_max
                },
                "attributes": {
                    "position": true,
                    "normal": primitive.normals.is_some(),
                    "tangent": primitive.tangents.is_some(),
                    "color0": primitive.colors_0.is_some(),
                    "texcoord0": primitive.texcoord_0.is_some(),
                    "texcoord1": primitive.texcoord_1.is_some(),
                    "texcoord2": primitive.texcoord_2.is_some()
                },
                "unsupportedAttributes": primitive.unsupported_attributes
            })
        })
        .collect::<Vec<_>>();

    let texture_conversions = decoded_images
        .iter()
        .map(|image| {
            let has_transparency = image.rgba8.iter().skip(3).step_by(4).any(|alpha| *alpha != 255);
            let has_partial_alpha = image
                .rgba8
                .iter()
                .skip(3)
                .step_by(4)
                .any(|alpha| *alpha != 0 && *alpha != 255);
            let format = match profile.settings.texture_encoding {
                TextureEncoding::Auto => {
                    if has_transparency { "bc3" } else { "bc1" }
                }
                TextureEncoding::Bc1 => "bc1",
                TextureEncoding::Bc3 => "bc3",
                TextureEncoding::Rgba8 => "rgba8",
            };
            let mip_levels = u32::BITS - image.width.max(image.height).max(1).leading_zeros();
            json!({
                "imageIndex": image.index,
                "sourceName": image.name,
                "sourceMimeType": image.mime_type,
                "sourceStorage": inspection.images.get(image.index).map(|item| item.storage.clone()),
                "sourceByteLength": inspection.images.get(image.index).and_then(|item| item.byte_length),
                "width": image.width,
                "height": image.height,
                "decodedPixelFormat": "rgba8",
                "outputTextureName": image_names.get(&image.index),
                "outputFormat": format,
                "hasTransparency": has_transparency,
                "hasPartialAlpha": has_partial_alpha,
                "mipmapLevelsGenerated": mip_levels,
                "lossyBlockCompression": format == "bc1" || format == "bc3",
                "compressionSelection": match profile.settings.texture_encoding {
                    TextureEncoding::Auto => "profile_auto_alpha_aware",
                    TextureEncoding::Bc1 => "explicit_profile_bc1",
                    TextureEncoding::Bc3 => "explicit_profile_bc3",
                    TextureEncoding::Rgba8 => "explicit_profile_rgba8",
                }
            })
        })
        .collect::<Vec<_>>();

    let mut warnings = inspection.warnings.clone();
    warnings.extend(decoded_geometry.warnings.clone());
    warnings.extend(result.report.limitations.clone());
    if profile.settings.normal_map_convention == ragelab_gltf::NormalMapConvention::DirectX {
        warnings.push(
            "normalMapConvention=direct_x is recorded from the profile, but this export path does not apply a per-pixel normal-map channel swizzle"
                .to_owned(),
        );
    }
    warnings.sort();
    warnings.dedup();

    let losses = texture_conversions
        .iter()
        .filter(|conversion| {
            conversion["lossyBlockCompression"]
                .as_bool()
                .unwrap_or(false)
        })
        .map(|conversion| {
            json!({
                "kind": "block_texture_compression",
                "imageIndex": conversion["imageIndex"],
                "outputFormat": conversion["outputFormat"],
                "authorizedBy": "profile.settings.textureEncoding",
                "selection": conversion["compressionSelection"]
            })
        })
        .collect::<Vec<_>>();

    let unused_images = decoded_images
        .iter()
        .filter(|image| !referenced_images.contains(&image.index))
        .map(|image| image.index)
        .collect::<Vec<_>>();

    let validation_stages = json!([
        {"stage": "glb_inspection", "status": "passed"},
        {"stage": "geometry_decode_and_transform", "status": "passed"},
        {"stage": "embedded_image_decode", "status": "passed"},
        {"stage": "authoring_profile_validation", "status": if result.report.profile_validated { "passed" } else { "failed" }},
        {"stage": "ydr_serialization", "status": "passed"},
        {"stage": "ydr_semantic_reopen", "status": if result.report.semantic_reopen { "passed" } else { "failed" }},
        {"stage": "codewalker_core_validation", "status": "not_run"},
        {"stage": "codewalker_gui_acceptance", "status": "not_run"},
        {"stage": "gta_v_runtime_acceptance", "status": "not_run"},
        {"stage": "fivem_runtime_acceptance", "status": "not_run"}
    ]);

    Ok(json!({
        "schema": "ragelab.glb.export-report",
        "schemaVersion": 1,
        "tool": {
            "name": "ragelab",
            "version": env!("CARGO_PKG_VERSION"),
            "coreRevision": ragelab_engine::CORE_REVISION,
            "coreDirty": ragelab_engine::CORE_DIRTY
        },
        "provenance": {
            "input": {
                "fileName": source_name,
                "path": source_path,
                "byteLength": result.report.source_byte_length,
                "sha256": result.report.source_sha256
            },
            "authoringProfile": {
                "origin": if profile_path.is_some() { "provided_file" } else { "generated_default" },
                "fileName": profile_path.and_then(Path::file_name).and_then(|value| value.to_str()),
                "path": profile_path,
                "schema": profile.schema,
                "schemaVersion": profile.schema_version,
                "sha256": result.report.profile_sha256,
                "sourceSha256": profile.source.sha256,
                "source": profile.source,
                "settings": profile.settings
            },
            "shaderTemplate": result.report.shader_template.as_ref().map(|template| json!({
                "fileName": shader_template_path.and_then(Path::file_name).and_then(|value| value.to_str()),
                "path": shader_template_path,
                "format": template.format,
                "shaderIndex": template.index,
                "preset": template.preset,
                "nameHash": template.name_hash,
                "fileHash": template.file_hash,
                "sha256": template.sha256
            })),
            "output": {
                "fileName": output_path.file_name().and_then(|value| value.to_str()),
                "path": output_path,
                "byteLength": result.report.output_byte_length,
                "sha256": result.report.output_sha256
            }
        },
        "options": {
            "selectedNodeIndices": selected_node_indices,
            "shaderIndex": result.report.shader_template.as_ref().map(|template| template.index).unwrap_or(0),
            "outputOverwrite": output_overwrite,
            "textureEncoding": profile.settings.texture_encoding,
            "generateMipmaps": profile.settings.generate_mipmaps,
            "normalMapConvention": profile.settings.normal_map_convention
        },
        "summary": {
            "ydrVersion": result.report.ydr_version,
            "geometryCount": result.report.geometry_count,
            "vertexCount": result.report.vertex_count,
            "triangleCount": result.report.triangle_count,
            "materialCount": result.report.material_count,
            "embeddedTextureCount": result.report.embedded_texture_count,
            "textureFormats": result.report.texture_formats,
            "shaderCount": result.report.shader_count,
            "materialShaderBindings": result.report.material_shader_bindings,
            "geometryShaderMappings": result.report.geometry_shader_mappings,
            "shaderTextureBindings": result.report.shader_texture_bindings
        },
        "decisions": {
            "materials": material_decisions,
            "geometries": geometry_decisions,
            "textureSlotMappings": texture_slot_mappings,
            "shaderParameters": {
                "outputShaderCount": output_inspection.shaders.len(),
                "shaders": output_inspection.shaders
            }
        },
        "transformations": [
            {
                "id": "node_hierarchy_transform",
                "status": "applied",
                "method": "composed_glTF_node_TRS_or_matrix_hierarchy"
            },
            {
                "id": "coordinate_system_conversion",
                "status": "applied",
                "from": "glTF_Y_up",
                "to": "GTA_Z_up",
                "attributes": ["POSITION", "NORMAL", "TANGENT"]
            },
            {
                "id": "primitive_topology",
                "status": "applied",
                "sourceModes": ["TRIANGLES", "TRIANGLE_STRIP", "TRIANGLE_FAN"],
                "outputMode": "triangle_list",
                "mirroredTransformWindingCorrection": true
            },
            {
                "id": "primitive_bounds",
                "status": "applied",
                "method": "post_transform_axis_converted_aabb"
            }
        ],
        "textureConversions": texture_conversions,
        "resources": {
            "included": {
                "activeSceneIndex": decoded_geometry.selected_scene,
                "nodeIndices": result.report.selection.as_ref().map(|selection| selection.included_node_indices.clone()).unwrap_or_else(|| {
                    included_primitives.iter().map(|primitive| primitive.node_index).collect::<BTreeSet<_>>().into_iter().collect()
                }),
                "meshIndices": result.report.selection.as_ref().map(|selection| selection.included_mesh_indices.clone()).unwrap_or_else(|| {
                    included_primitives.iter().map(|primitive| primitive.mesh_index).collect::<BTreeSet<_>>().into_iter().collect()
                }),
                "geometryCount": result.report.geometry_count,
                "imageIndices": decoded_images.iter().map(|image| image.index).collect::<Vec<_>>(),
                "imagesNotReferencedByConfiguredTextureRoles": unused_images
            },
            "omitted": {
                "primitivesExcludedByNodeSelection": omitted_primitives,
                "uninstantiatedMeshIndices": decoded_geometry.uninstantiated_meshes
            }
        },
        "losses": losses,
        "fallbacks": fallback_decisions,
        "warnings": warnings,
        "errors": [],
        "pipelineLimitations": result.report.limitations,
        "validationStages": validation_stages,
        "runtimeRenderable": false,
        "runtimeAcceptance": {
            "codewalkerCore": "not_run",
            "codewalkerGui": "not_run",
            "gtaV": "not_run",
            "fivem": "not_run"
        },
        "engineReport": result.report
    }))
}

fn material_texture_roles(
    material: &ragelab_gltf::MaterialAuthoringSettings,
) -> [(&'static str, &Option<TextureBindingInfo>); 8] {
    [
        ("albedo", &material.texture_roles.albedo),
        ("normal", &material.texture_roles.normal),
        (
            "metallicRoughness",
            &material.texture_roles.metallic_roughness,
        ),
        ("specular", &material.texture_roles.specular),
        ("specularColor", &material.texture_roles.specular_color),
        ("tintPalette", &material.texture_roles.tint_palette),
        ("occlusion", &material.texture_roles.occlusion),
        ("emissive", &material.texture_roles.emissive),
    ]
}

fn primitive_mode_name(mode: u32) -> &'static str {
    match mode {
        4 => "triangles",
        5 => "triangle_strip",
        6 => "triangle_fan",
        _ => "unknown",
    }
}

fn embedded_texture_name(index: usize, source_name: &str) -> String {
    let safe = source_name
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || value == '_' {
                value
            } else {
                '_'
            }
        })
        .take(96)
        .collect::<String>();
    if safe.is_empty() {
        format!("img_{index}")
    } else {
        format!("img_{index}_{safe}")
    }
}

/// Publishes a complete report atomically. The final path is created with
/// hard-link semantics, so an existing report is never replaced.
pub(crate) fn write_report_atomic(path: &Path, body: &[u8]) -> io::Result<()> {
    write_bytes_atomic(path, body)
}

pub(crate) fn write_bytes_atomic(path: &Path, body: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "report path must name a file"))?
        .to_string_lossy();

    for attempt in 0..64u32 {
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };

        let write_result = (|| {
            file.write_all(body)?;
            file.flush()?;
            file.sync_all()
        })();
        drop(file);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }

        let publish_result = fs::hard_link(&temporary, path);
        let _ = fs::remove_file(&temporary);
        publish_result?;
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary report file",
    ))
}
