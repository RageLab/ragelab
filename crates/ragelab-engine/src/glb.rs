//! Byte-first GLB authoring use cases shared by the CLI and RageLab Studio.
//!
//! This module coordinates the format crates. GLB parsing/decoding remains in
//! ragelab-gltf, YFT template parsing in ragelab-yft, and YDR serialization and
//! semantic reopen in ragelab-ydr. No filesystem or process discovery occurs here.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use ragelab_gltf::{
    decode_glb_geometry as decode_geometry, decode_glb_images, default_authoring_profile,
    inspect_glb as inspect_document, validate_authoring_profile, GlbError,
    MaterialAuthoringSettings, ShaderIntent, SurfaceMode, TextureBindingInfo, TextureEncoding,
};
pub use ragelab_gltf::{
    DecodedGeometryReport, GlbInspectionReport, GtaAuthoringProfile, ProfileValidationReport,
};
use ragelab_ydr::{
    build_ydr_with_embedded_textures, build_ydr_with_embedded_textures_and_shaders, ShaderInfo,
    ShaderTextureReference, YdrDocument, YdrGeometryInput, YdrTextureInput,
};
use ragelab_yft::YftDocument;
use ragelab_ytd::TextureFormat;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShaderTemplateKind {
    Normal,
    Spec,
    NormalSpecTint,
    EmissiveNight,
    Emissive,
}

impl ShaderTemplateKind {
    fn preset_name(self) -> &'static str {
        match self {
            Self::Normal => "normal.sps",
            Self::Spec => "spec.sps",
            Self::NormalSpecTint => "normal_spec_tnt.sps",
            Self::EmissiveNight => "emissivenight.sps",
            Self::Emissive => "emissive.sps",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlbShaderTemplateFormat {
    LegacyYft,
    LegacyYdr,
}

#[derive(Debug, Clone)]
pub struct GlbShaderTemplateInput {
    pub format: GlbShaderTemplateFormat,
    /// Informational source label for provenance only; the engine never opens it.
    pub source_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct GlbYdrExportRequest {
    pub source_name: String,
    pub source_bytes: Vec<u8>,
    pub profile_bytes: Option<Vec<u8>>,
    pub shader_template: Option<GlbShaderTemplateInput>,
    pub shader_index: usize,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GlbShaderTemplateReport {
    pub source: String,
    pub index: usize,
    pub preset: &'static str,
    pub name_hash: String,
    pub file_hash: String,
    #[ts(type = "\"yft\" | \"ydr\"")]
    pub format: &'static str,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GlbYdrExportReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub source_name: String,
    pub source_sha256: String,
    pub source_byte_length: usize,
    pub profile_sha256: String,
    pub output_sha256: String,
    pub output_byte_length: usize,
    pub ydr_version: u32,
    pub geometry_count: usize,
    pub vertex_count: usize,
    pub triangle_count: usize,
    pub embedded_texture_count: usize,
    pub texture_formats: BTreeMap<String, usize>,
    pub profile_validated: bool,
    pub material_count: usize,
    #[ts(type = "\"not_emitted\" | \"empty\" | \"populated\"")]
    pub shader_group: &'static str,
    pub shader_count: usize,
    pub shader_template: Option<GlbShaderTemplateReport>,
    pub material_shader_bindings: usize,
    pub geometry_shader_mappings: usize,
    pub shader_texture_bindings: usize,
    pub semantic_reopen: bool,
    #[ts(type = "false")]
    pub runtime_renderable: bool,
    pub limitations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub selection: Option<GlbNodeSelectionReport>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GlbNodeSelectionReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub scene_index: Option<usize>,
    pub selected_nodes: Vec<GlbNodeSelectionEntry>,
    pub included_node_indices: Vec<usize>,
    pub included_mesh_indices: Vec<usize>,
    pub primitive_count: usize,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GlbNodeSelectionEntry {
    pub node_index: usize,
    pub node_name: Option<String>,
    pub included_node_indices: Vec<usize>,
    pub mesh_indices: Vec<usize>,
    pub primitive_count: usize,
}

#[derive(Debug)]
pub struct GlbYdrExportResult {
    pub output_bytes: Vec<u8>,
    pub report: GlbYdrExportReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum GlbValidationSeverity {
    Error,
    Unsupported,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GlbValidationDiagnostic {
    pub severity: GlbValidationSeverity,
    pub code: String,
    pub path: Option<String>,
    pub message: String,
    pub promoted_from: Option<GlbValidationSeverity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GlbValidationCounts {
    pub scenes: usize,
    pub nodes: usize,
    pub meshes: usize,
    pub primitives: usize,
    pub decoded_primitives: usize,
    pub accessors: usize,
    pub materials: usize,
    pub textures: usize,
    pub samplers: usize,
    pub images: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GlbValidationReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub source_name: String,
    pub source_sha256: String,
    pub source_byte_length: usize,
    pub strict: bool,
    pub valid: bool,
    pub structural_valid: bool,
    pub export_compatible: bool,
    pub geometry_decoded: bool,
    pub images_decoded: bool,
    pub semantic_preflight: bool,
    pub semantic_reopen: bool,
    pub runtime_renderable: bool,
    pub shader_preset: Option<String>,
    pub counts: GlbValidationCounts,
    pub diagnostics: Vec<GlbValidationDiagnostic>,
}

pub fn inspect_glb_bytes(
    source_bytes: &[u8],
    source_name: &str,
) -> Result<GlbInspectionReport, io::Error> {
    inspect_document(source_bytes, source_name).map_err(glb_validation_error)
}

pub fn decode_glb_geometry_bytes(
    source_bytes: &[u8],
    source_name: &str,
) -> Result<DecodedGeometryReport, io::Error> {
    decode_geometry(source_bytes, source_name).map_err(glb_validation_error)
}

/// Validates a GLB against the current static Legacy YDR export pipeline.
/// The YDR preflight is built and semantically reopened in memory; no file is written.
pub fn validate_glb_export_bytes(
    source_bytes: &[u8],
    source_name: &str,
    profile_bytes: Option<Vec<u8>>,
    shader_template: Option<GlbShaderTemplateInput>,
    shader_index: usize,
    strict: bool,
) -> Result<GlbValidationReport, io::Error> {
    let inspection = match inspect_document(source_bytes, source_name) {
        Ok(report) => report,
        Err(error) => {
            let (severity, code) = match &error {
                GlbError::Unsupported(_) => (GlbValidationSeverity::Unsupported, "glb_unsupported"),
                _ => (GlbValidationSeverity::Error, "glb_invalid"),
            };
            let mut diagnostics = Vec::new();
            push_glb_validation_diagnostic(
                &mut diagnostics,
                severity,
                code,
                Some("document".into()),
                error.to_string(),
            );
            let digest = Sha256::digest(source_bytes);
            return Ok(GlbValidationReport {
                schema: "ragelab.glb.validation",
                schema_version: 1,
                source_name: source_name.to_owned(),
                source_sha256: format!("{digest:x}"),
                source_byte_length: source_bytes.len(),
                strict,
                valid: false,
                structural_valid: severity == GlbValidationSeverity::Unsupported,
                export_compatible: false,
                geometry_decoded: false,
                images_decoded: false,
                semantic_preflight: false,
                semantic_reopen: false,
                runtime_renderable: false,
                shader_preset: None,
                counts: GlbValidationCounts {
                    scenes: 0,
                    nodes: 0,
                    meshes: 0,
                    primitives: 0,
                    decoded_primitives: 0,
                    accessors: 0,
                    materials: 0,
                    textures: 0,
                    samplers: 0,
                    images: 0,
                },
                diagnostics,
            });
        }
    };
    let mut diagnostics = Vec::new();

    for chunk in &inspection.container.unknown_chunks {
        push_glb_validation_diagnostic(
            &mut diagnostics,
            GlbValidationSeverity::Warning,
            "unknown_container_chunk",
            Some("container".into()),
            format!(
                "GLB contains unrecognized chunk {} ({} bytes); the YDR pipeline does not interpret it",
                chunk.chunk_type, chunk.byte_length
            ),
        );
    }

    const UNDERSTOOD_EXTENSIONS: &[&str] = &[
        "KHR_materials_emissive_strength",
        "KHR_materials_specular",
        "KHR_materials_transmission",
        "KHR_materials_ior",
        "KHR_materials_unlit",
    ];
    for extension in &inspection.extensions_used {
        if !UNDERSTOOD_EXTENSIONS.contains(&extension.as_str()) {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Unsupported,
                "unsupported_extension",
                Some("extensionsUsed".into()),
                format!(
                    "extension {extension} is used by this document but is not interpreted by the current GLB-to-YDR pipeline"
                ),
            );
        }
    }

    for extension in &inspection.extensions_required {
        if !inspection.extensions_used.contains(extension) {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Error,
                "required_extension_not_used",
                Some("extensionsRequired".into()),
                format!("required extension {extension} must also appear in extensionsUsed"),
            );
        }
    }

    for sampler in &inspection.samplers {
        let invalid_mag_filter = sampler
            .mag_filter
            .is_some_and(|value| !matches!(value, 9728 | 9729));
        let invalid_min_filter = sampler
            .min_filter
            .is_some_and(|value| !matches!(value, 9728 | 9729 | 9984 | 9985 | 9986 | 9987));
        let invalid_wrap = !matches!(sampler.wrap_s, 33071 | 33648 | 10497)
            || !matches!(sampler.wrap_t, 33071 | 33648 | 10497);
        if invalid_mag_filter || invalid_min_filter || invalid_wrap {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Error,
                "invalid_sampler",
                Some(format!("samplers[{}]", sampler.index)),
                format!(
                    "sampler {} contains an invalid magFilter/minFilter/wrapS/wrapT enum",
                    sampler.index
                ),
            );
        }
    }
    for texture in &inspection.textures {
        let has_extensions = texture
            .extensions
            .as_object()
            .is_some_and(|value| !value.is_empty());
        if texture.source_image_index.is_none() && !has_extensions {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Error,
                "texture_missing_source",
                Some(format!("textures[{}].source", texture.index)),
                format!(
                    "texture {} has neither a core image source nor a source-bearing extension",
                    texture.index
                ),
            );
        }
    }
    for material in &inspection.materials {
        if !matches!(material.alpha_mode.as_str(), "OPAQUE" | "MASK" | "BLEND") {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Error,
                "invalid_alpha_mode",
                Some(format!("materials[{}].alphaMode", material.index)),
                format!(
                    "material {} has invalid alphaMode {}",
                    material.index, material.alpha_mode
                ),
            );
        }
    }

    for node in &inspection.nodes {
        if node.skin_index.is_some() {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Unsupported,
                "skinned_node",
                Some(format!("nodes[{}].skin", node.index)),
                format!(
                    "node {} uses a skin; the current YDR importer supports static geometry only",
                    node.index
                ),
            );
        }
    }
    for mesh in &inspection.meshes {
        for primitive in &mesh.primitives {
            let path = format!("meshes[{}].primitives[{}]", mesh.index, primitive.index);
            if primitive.morph_target_count > 0 {
                push_glb_validation_diagnostic(
                    &mut diagnostics,
                    GlbValidationSeverity::Unsupported,
                    "morph_targets",
                    Some(format!("{path}.targets")),
                    format!(
                        "{path} contains {} morph targets; morph targets are not represented by the static YDR writer",
                        primitive.morph_target_count
                    ),
                );
            }
            if primitive
                .extensions
                .as_object()
                .is_some_and(|value| !value.is_empty())
            {
                push_glb_validation_diagnostic(
                    &mut diagnostics,
                    GlbValidationSeverity::Unsupported,
                    "primitive_extension",
                    Some(format!("{path}.extensions")),
                    format!("{path} uses primitive extensions that are not converted by the current YDR pipeline"),
                );
            }
        }
    }
    for image in &inspection.images {
        if !image.embedded {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Unsupported,
                "external_image",
                Some(format!("images[{}]", image.index)),
                format!(
                    "image {} is not embedded in the GLB; the CLI does not resolve external image paths during export",
                    image.index
                ),
            );
        }
    }

    let geometry = match decode_geometry(source_bytes, source_name) {
        Ok(report) => Some(report),
        Err(error) => {
            let (severity, code) = match &error {
                GlbError::Unsupported(_) => {
                    (GlbValidationSeverity::Unsupported, "geometry_unsupported")
                }
                GlbError::InvalidContainer(_)
                | GlbError::InvalidDocument(_)
                | GlbError::Json(_) => (GlbValidationSeverity::Error, "geometry_invalid"),
                GlbError::InvalidProfile(_) => {
                    (GlbValidationSeverity::Error, "geometry_invalid_profile")
                }
                GlbError::Io(_) => (GlbValidationSeverity::Error, "geometry_io_error"),
            };
            push_glb_validation_diagnostic(
                &mut diagnostics,
                severity,
                code,
                Some("geometry".into()),
                error.to_string(),
            );
            None
        }
    };

    if let Some(geometry) = &geometry {
        for mesh_index in &geometry.uninstantiated_meshes {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Warning,
                "mesh_not_in_active_scene",
                Some(format!("meshes[{mesh_index}]")),
                format!(
                    "mesh {mesh_index} is not instantiated by the active scene and is not included in the default YDR export"
                ),
            );
        }
        if geometry.primitives.is_empty() {
            push_glb_validation_diagnostic(
                &mut diagnostics,
                GlbValidationSeverity::Unsupported,
                "no_exportable_geometry",
                Some("scenes".into()),
                "the active scene contains no static triangle geometry exportable to YDR",
            );
        }
        for primitive in &geometry.primitives {
            let path = format!(
                "nodes[{}].mesh[{}].primitives[{}]",
                primitive.node_index, primitive.mesh_index, primitive.primitive_index
            );
            if !primitive.unsupported_attributes.is_empty() {
                push_glb_validation_diagnostic(
                    &mut diagnostics,
                    GlbValidationSeverity::Unsupported,
                    "unsupported_vertex_attributes",
                    Some(format!("{path}.attributes")),
                    format!(
                        "vertex attributes are not preserved by the YDR writer: {}",
                        primitive.unsupported_attributes.join(", ")
                    ),
                );
            }
            if primitive.normals.is_none() {
                push_glb_validation_diagnostic(
                    &mut diagnostics,
                    GlbValidationSeverity::Warning,
                    "missing_normals",
                    Some(format!("{path}.attributes.NORMAL")),
                    "primitive has no NORMAL attribute; downstream shading may be incomplete",
                );
            }
        }
    }

    let images_decoded = match decode_glb_images(source_bytes) {
        Ok(_) => true,
        Err(error) => {
            let (severity, code) = match &error {
                GlbError::Unsupported(_) => {
                    (GlbValidationSeverity::Unsupported, "image_unsupported")
                }
                _ => (GlbValidationSeverity::Error, "image_invalid"),
            };
            push_glb_validation_diagnostic(
                &mut diagnostics,
                severity,
                code,
                Some("images".into()),
                error.to_string(),
            );
            false
        }
    };

    let blocked_before_preflight = diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.severity,
            GlbValidationSeverity::Error | GlbValidationSeverity::Unsupported
        )
    });
    let (semantic_preflight, semantic_reopen, shader_preset) = if blocked_before_preflight {
        (false, false, None)
    } else {
        let request = GlbYdrExportRequest {
            source_name: source_name.to_owned(),
            source_bytes: source_bytes.to_vec(),
            profile_bytes,
            shader_template,
            shader_index,
        };
        match export_glb_to_ydr(request) {
            Ok(result) => {
                push_glb_validation_diagnostic(
                    &mut diagnostics,
                    GlbValidationSeverity::Info,
                    "ydr_semantic_preflight_passed",
                    Some("export".into()),
                    "in-memory YDR serialization and semantic reopen passed; no output file was written",
                );
                if result.report.shader_template.is_none() && !inspection.materials.is_empty() {
                    push_glb_validation_diagnostic(
                        &mut diagnostics,
                        GlbValidationSeverity::Info,
                        "shader_template_not_supplied",
                        Some("materials".into()),
                        "no ShaderFX template was supplied; this preflight certifies structural YDR export only, not material shader bindings",
                    );
                }
                (
                    true,
                    result.report.semantic_reopen,
                    result
                        .report
                        .shader_template
                        .map(|template| template.preset.to_owned()),
                )
            }
            Err(error) => {
                let (severity, code) = match error.kind() {
                    io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput => {
                        (GlbValidationSeverity::Unsupported, "export_unsupported")
                    }
                    io::ErrorKind::InvalidData => {
                        (GlbValidationSeverity::Error, "export_preflight_invalid")
                    }
                    _ => (GlbValidationSeverity::Error, "export_preflight_failed"),
                };
                push_glb_validation_diagnostic(
                    &mut diagnostics,
                    severity,
                    code,
                    Some("export".into()),
                    error.to_string(),
                );
                (false, false, None)
            }
        }
    };

    if strict {
        for diagnostic in &mut diagnostics {
            if diagnostic.severity == GlbValidationSeverity::Warning {
                diagnostic.promoted_from = Some(GlbValidationSeverity::Warning);
                diagnostic.severity = GlbValidationSeverity::Error;
            }
        }
    }

    let has_structural_errors = diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == GlbValidationSeverity::Error
            && diagnostic.promoted_from.is_none()
            && matches!(
                diagnostic.code.as_str(),
                "geometry_invalid"
                    | "geometry_invalid_profile"
                    | "geometry_io_error"
                    | "image_invalid"
                    | "invalid_sampler"
                    | "texture_missing_source"
                    | "invalid_alpha_mode"
                    | "required_extension_not_used"
            )
    });
    let has_export_errors = diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == GlbValidationSeverity::Error
            && diagnostic.promoted_from.is_none()
            && diagnostic.code.starts_with("export_")
    });
    let has_unsupported = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == GlbValidationSeverity::Unsupported);
    let has_strict_warnings = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.promoted_from == Some(GlbValidationSeverity::Warning));
    let structural_valid = !has_structural_errors;
    let export_compatible =
        semantic_preflight && structural_valid && !has_unsupported && !has_export_errors;
    let valid = structural_valid && export_compatible && !has_strict_warnings;
    Ok(GlbValidationReport {
        schema: "ragelab.glb.validation",
        schema_version: 1,
        source_name: inspection.source.file_name.clone(),
        source_sha256: inspection.source.sha256.clone(),
        source_byte_length: inspection.source.byte_length,
        strict,
        valid,
        structural_valid,
        export_compatible,
        geometry_decoded: geometry.is_some(),
        images_decoded,
        semantic_preflight,
        semantic_reopen,
        runtime_renderable: false,
        shader_preset,
        counts: GlbValidationCounts {
            scenes: inspection.counts.scenes,
            nodes: inspection.counts.nodes,
            meshes: inspection.counts.meshes,
            primitives: inspection.counts.primitives,
            decoded_primitives: geometry.as_ref().map_or(0, |value| value.primitives.len()),
            accessors: inspection.counts.accessors,
            materials: inspection.counts.materials,
            textures: inspection.counts.textures,
            samplers: inspection.counts.samplers,
            images: inspection.counts.images,
        },
        diagnostics,
    })
}

fn push_glb_validation_diagnostic(
    diagnostics: &mut Vec<GlbValidationDiagnostic>,
    severity: GlbValidationSeverity,
    code: &str,
    path: Option<String>,
    message: impl Into<String>,
) {
    diagnostics.push(GlbValidationDiagnostic {
        severity,
        code: code.to_owned(),
        path,
        message: message.into(),
        promoted_from: None,
    });
}

pub fn default_glb_authoring_profile(
    source_bytes: &[u8],
    source_name: &str,
) -> Result<GtaAuthoringProfile, io::Error> {
    let inspection = inspect_document(source_bytes, source_name).map_err(glb_validation_error)?;
    if !inspection.unsupported_required_extensions.is_empty() {
        return Err(glb_validation_error(format!(
            "required glTF extensions block profile creation: {}",
            inspection.unsupported_required_extensions.join(", ")
        )));
    }
    Ok(default_authoring_profile(&inspection))
}

pub fn validate_glb_authoring_profile_bytes(
    source_bytes: &[u8],
    source_name: &str,
    profile_bytes: &[u8],
) -> Result<ProfileValidationReport, io::Error> {
    let inspection = inspect_document(source_bytes, source_name).map_err(glb_validation_error)?;
    if !inspection.unsupported_required_extensions.is_empty() {
        return Err(glb_validation_error(format!(
            "required glTF extensions block profile validation: {}",
            inspection.unsupported_required_extensions.join(", ")
        )));
    }
    let profile: GtaAuthoringProfile = serde_json::from_slice(profile_bytes)
        .map_err(|error| invalid_data(format!("cannot parse profile JSON: {error}")))?;
    validate_authoring_profile(&inspection, &profile).map_err(glb_validation_error)
}

pub fn export_glb_to_ydr(request: GlbYdrExportRequest) -> Result<GlbYdrExportResult, io::Error> {
    export_glb_to_ydr_internal(request, None)
}

/// Exports only the selected node subtrees; each selected node includes its descendants.
pub fn export_glb_to_ydr_with_nodes(
    request: GlbYdrExportRequest,
    selected_node_indices: &[usize],
) -> Result<GlbYdrExportResult, io::Error> {
    export_glb_to_ydr_internal(request, Some(selected_node_indices))
}

fn export_glb_to_ydr_internal(
    request: GlbYdrExportRequest,
    selected_node_indices: Option<&[usize]>,
) -> Result<GlbYdrExportResult, io::Error> {
    let source_name = request.source_name.as_str();
    let source_bytes = request.source_bytes.as_slice();
    let inspection = inspect_document(source_bytes, source_name).map_err(glb_validation_error)?;
    if !inspection.unsupported_required_extensions.is_empty() {
        return Err(glb_validation_error(format!(
            "required glTF extensions block structural export: {}",
            inspection.unsupported_required_extensions.join(", ")
        )));
    }

    let profile = if let Some(bytes) = request.profile_bytes.as_deref() {
        serde_json::from_slice::<GtaAuthoringProfile>(bytes)
            .map_err(|error| invalid_data(format!("cannot parse profile JSON: {error}")))?
    } else {
        default_authoring_profile(&inspection)
    };
    let profile_validation =
        validate_authoring_profile(&inspection, &profile).map_err(glb_validation_error)?;
    let effective_profile_bytes = match request.profile_bytes.as_deref() {
        Some(bytes) => bytes.to_vec(),
        None => serde_json::to_vec(&profile).map_err(|error| {
            invalid_data(format!("cannot serialize effective profile: {error}"))
        })?,
    };
    let profile_sha256 = sha256_hex(&effective_profile_bytes);

    let shader_template_input = request.shader_template.as_ref();
    let shader_template = if let Some(template_input) = shader_template_input {
        let shaders = match template_input.format {
            GlbShaderTemplateFormat::LegacyYft => {
                let template_yft =
                    YftDocument::from_bytes(&template_input.bytes).map_err(|error| {
                        invalid_data(format!("cannot parse Legacy YFT shader template: {error}"))
                    })?;
                let drawable = template_yft
                    .main_drawable
                    .ok_or_else(|| invalid_data("shader template YFT has no main Drawable"))?;
                drawable.model.shaders.clone()
            }
            GlbShaderTemplateFormat::LegacyYdr => {
                let template_ydr =
                    YdrDocument::from_bytes(&template_input.bytes).map_err(|error| {
                        invalid_data(format!("cannot parse Legacy YDR shader template: {error}"))
                    })?;
                template_ydr.model.shaders
            }
        };
        let shader = shaders.get(request.shader_index).cloned().ok_or_else(|| {
            invalid_input(format!(
                "shader index {} does not exist in template resource ({} shaders)",
                request.shader_index,
                shaders.len()
            ))
        })?;
        let template_kind = validate_supported_shader_template(&shader)?;
        Some((shader, template_kind))
    } else {
        if request.shader_index != 0 {
            return Err(invalid_input("--shader-index requires --shader-template"));
        }
        None
    };
    if !profile.settings.embed_textures {
        return Err(invalid_input(
            "structural YDR export requires profile setting embedTextures=true",
        ));
    }
    if !profile.settings.generate_mipmaps {
        return Err(invalid_input(
            "structural YDR export currently requires generateMipmaps=true",
        ));
    }

    for material in &profile.materials {
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
            if binding.tex_coord > 2 {
                return Err(invalid_input(format!(
                    "material {} texture {} uses TEXCOORD_{}, but the structural YDR writer supports UV sets 0, 1 and 2 only",
                    material.source_material_index,
                    binding.texture_index,
                    binding.tex_coord
                )));
            }
            let texture = inspection
                .textures
                .get(binding.texture_index)
                .ok_or_else(|| {
                    invalid_data(format!(
                        "material {} references missing texture {}",
                        material.source_material_index, binding.texture_index
                    ))
                })?;
            if texture.source_image_index.is_none() {
                return Err(invalid_input(format!(
                    "texture {} is supplied through an unresolved extension; structural export cannot embed it",
                    binding.texture_index
                )));
            }
        }
    }

    let mut decoded_geometry =
        decode_geometry(source_bytes, source_name).map_err(glb_validation_error)?;
    let selection_report = if let Some(selected_indices) = selected_node_indices {
        let selection =
            build_node_selection_report(&inspection, &decoded_geometry, selected_indices)?;
        let included_nodes = selection
            .included_node_indices
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        decoded_geometry
            .primitives
            .retain(|primitive| included_nodes.contains(&primitive.node_index));
        if decoded_geometry.primitives.is_empty() {
            return Err(invalid_input(
                "selected node subtrees contain no exportable geometry in the active scene",
            ));
        }
        Some(selection)
    } else {
        None
    };
    if decoded_geometry.primitives.is_empty() {
        return Err(invalid_input(
            "selected GLB scene contains no exportable geometry",
        ));
    }
    for material in &profile.materials {
        let bindings = [
            material.texture_roles.albedo.as_ref(),
            material.texture_roles.normal.as_ref(),
            material.texture_roles.metallic_roughness.as_ref(),
            material.texture_roles.specular.as_ref(),
            material.texture_roles.specular_color.as_ref(),
            material.texture_roles.tint_palette.as_ref(),
            material.texture_roles.occlusion.as_ref(),
            material.texture_roles.emissive.as_ref(),
        ];
        for binding in bindings.into_iter().flatten() {
            for primitive in decoded_geometry.primitives.iter().filter(|primitive| {
                primitive.material_index == Some(material.source_material_index)
            }) {
                let available = match binding.tex_coord {
                    0 => primitive.texcoord_0.is_some(),
                    1 => primitive.texcoord_1.is_some(),
                    2 => primitive.texcoord_2.is_some(),
                    _ => false,
                };
                if !available {
                    return Err(invalid_input(format!(
                        "material {} maps texture {} to TEXCOORD_{}, but mesh {} primitive {} has no matching UV attribute",
                        material.source_material_index,
                        binding.texture_index,
                        binding.tex_coord,
                        primitive.mesh_index,
                        primitive.primitive_index
                    )));
                }
            }
        }
    }

    let mut geometries = Vec::with_capacity(decoded_geometry.primitives.len());
    let mut geometry_material_indices = Vec::with_capacity(decoded_geometry.primitives.len());
    for primitive in decoded_geometry.primitives {
        if !primitive.unsupported_attributes.is_empty() {
            return Err(invalid_input(format!(
                "mesh {} primitive {} contains unsupported vertex attributes: {}",
                primitive.mesh_index,
                primitive.primitive_index,
                primitive.unsupported_attributes.join(", ")
            )));
        }
        geometry_material_indices.push(primitive.material_index);
        geometries.push(YdrGeometryInput {
            positions: primitive.positions,
            normals: primitive.normals,
            colors0: primitive.colors_0,
            uv0: primitive.texcoord_0,
            uv1: primitive.texcoord_1,
            uv2: primitive.texcoord_2,
            tangents: primitive.tangents,
            indices: primitive.indices,
            shader_index: None,
        });
    }

    let decoded_images = decode_glb_images(source_bytes).map_err(glb_validation_error)?;
    if decoded_images.len() != inspection.images.len() {
        return Err(invalid_data(
            "embedded image decoder count does not match GLB inspection",
        ));
    }
    let image_names = decoded_images
        .iter()
        .map(|image| (image.index, embedded_texture_name(image.index, &image.name)))
        .collect::<BTreeMap<_, _>>();
    let mut textures = Vec::with_capacity(decoded_images.len());
    for image in decoded_images {
        let width = u16::try_from(image.width).map_err(|_| {
            invalid_input(format!(
                "image {} width {} exceeds the Legacy YDR 16-bit texture dimension",
                image.index, image.width
            ))
        })?;
        let height = u16::try_from(image.height).map_err(|_| {
            invalid_input(format!(
                "image {} height {} exceeds the Legacy YDR 16-bit texture dimension",
                image.index, image.height
            ))
        })?;
        let has_partial_alpha = image
            .rgba8
            .iter()
            .skip(3)
            .step_by(4)
            .any(|alpha| *alpha != 0 && *alpha != 255);
        let has_transparency = image
            .rgba8
            .iter()
            .skip(3)
            .step_by(4)
            .any(|alpha| *alpha != 255);
        let format = match profile.settings.texture_encoding {
            TextureEncoding::Auto => {
                if has_transparency {
                    TextureFormat::Bc3
                } else {
                    TextureFormat::Bc1
                }
            }
            TextureEncoding::Bc1 if has_partial_alpha => {
                return Err(invalid_input(format!(
                    "image {} has fractional alpha and cannot be losslessly encoded as BC1; choose BC3 or RGBA8 in the authoring profile",
                    image.index
                )));
            }
            TextureEncoding::Bc1 => TextureFormat::Bc1,
            TextureEncoding::Bc3 => TextureFormat::Bc3,
            TextureEncoding::Rgba8 => TextureFormat::Rgba8,
        };
        textures.push(YdrTextureInput {
            name: embedded_texture_name(image.index, &image.name),
            width,
            height,
            rgba8: image.rgba8,
            format,
        });
    }

    let mut shaders = Vec::new();
    let mut material_shader_indices = BTreeMap::new();
    if let Some((template, template_kind)) = &shader_template {
        for (shader_slot, material) in profile.materials.iter().enumerate() {
            let shader_index = u16::try_from(shader_slot)
                .map_err(|_| invalid_input("material count exceeds the Legacy u16 shader index"))?;
            let shader = bind_shader_template_to_material(
                template,
                *template_kind,
                material,
                &inspection,
                &image_names,
            )?;
            material_shader_indices.insert(material.source_material_index, shader_index);
            shaders.push(shader);
        }
        let has_unmaterialed_geometry = geometry_material_indices.iter().any(Option::is_none);
        let default_shader_index = if has_unmaterialed_geometry {
            let index = u16::try_from(shaders.len()).map_err(|_| {
                invalid_input("default material shader exceeds the Legacy u16 index")
            })?;
            let mut default_shader = template.clone();
            let sampler_hashes = default_shader
                .parameters
                .iter()
                .filter(|parameter| parameter.data_type == 0)
                .map(|parameter| parameter.parameter_hash)
                .collect::<Vec<_>>();
            for hash in sampler_hashes {
                set_shader_sampler(&mut default_shader, hash, None)?;
            }
            default_shader.texture_references = default_shader
                .parameters
                .iter()
                .filter(|parameter| parameter.data_type == 0)
                .map(|parameter| ShaderTextureReference {
                    parameter_hash: parameter.parameter_hash,
                    texture_name: parameter.texture_name.clone(),
                })
                .collect();
            shaders.push(default_shader);
            Some(index)
        } else {
            None
        };
        for (geometry_index, (geometry, material_index)) in geometries
            .iter_mut()
            .zip(geometry_material_indices.iter().copied())
            .enumerate()
        {
            let shader_index = match material_index {
                Some(source_material_index) => *material_shader_indices
                    .get(&source_material_index)
                    .ok_or_else(|| invalid_data(format!(
                        "geometry {geometry_index} references material {source_material_index}, but no shader was produced for it"
                    )))?,
                None => default_shader_index.expect("unmaterialed geometry creates a default shader"),
            };
            geometry.shader_index = Some(shader_index);
        }
    }

    let drawable_name = drawable_name_from_source_name(source_name);
    let ydr_bytes = if shaders.is_empty() {
        build_ydr_with_embedded_textures(&drawable_name, &geometries, &textures)
            .map_err(glb_validation_error)?
    } else {
        build_ydr_with_embedded_textures_and_shaders(
            &drawable_name,
            &geometries,
            &textures,
            &shaders,
        )
        .map_err(glb_validation_error)?
    };

    // A successful writer call is not enough: reopen the exact output bytes
    // through the format reader before exposing them to an adapter.
    YdrDocument::from_bytes(&ydr_bytes)
        .map_err(|error| invalid_data(format!("generated YDR failed semantic reopen: {error}")))?;

    let vertex_count = geometries
        .iter()
        .map(|geometry| geometry.positions.len())
        .sum::<usize>();
    let triangle_count = geometries
        .iter()
        .map(|geometry| geometry.indices.len() / 3)
        .sum::<usize>();
    let texture_formats = textures
        .iter()
        .map(|texture| match texture.format {
            TextureFormat::Rgba8 => "rgba8",
            TextureFormat::Bc1 => "bc1",
            TextureFormat::Bc3 => "bc3",
            _ => "other",
        })
        .fold(BTreeMap::<String, usize>::new(), |mut counts, format| {
            *counts.entry(format.to_owned()).or_default() += 1;
            counts
        });

    let geometry_shader_mapping_count = if shaders.is_empty() {
        0
    } else {
        geometries.len()
    };
    let material_shader_binding_count = if shaders.is_empty() {
        0
    } else {
        geometry_material_indices
            .iter()
            .filter(|material| material.is_some())
            .count()
    };
    let shader_texture_binding_count = shaders
        .iter()
        .flat_map(|shader| shader.parameters.iter())
        .filter(|parameter| parameter.data_type == 0 && parameter.texture_name.is_some())
        .count();
    let shader_template_report = shader_template_input.map(|template_input| {
        let (shader, kind) = shader_template
            .as_ref()
            .expect("validated template is present");
        GlbShaderTemplateReport {
            source: template_input.source_name.clone(),
            index: request.shader_index,
            preset: kind.preset_name(),
            name_hash: format!("0x{:08X}", shader.name_hash),
            file_hash: format!("0x{:08X}", shader.file_hash),
            format: match template_input.format {
                GlbShaderTemplateFormat::LegacyYft => "yft",
                GlbShaderTemplateFormat::LegacyYdr => "ydr",
            },
            sha256: sha256_hex(&template_input.bytes),
        }
    });
    let limitations = if shaders.is_empty() {
        vec![
            "YDR contains structural geometry and embedded textures only".to_owned(),
            "no ShaderFX templates or material bindings were emitted".to_owned(),
            "CodeWalker Model Viewer is not safe for shaderless drawables: its BasicShader path dereferences DrawableGeometry.Shader; supply a verified --shader-template for interactive viewing".to_owned(),
            "output has not been accepted by GTA V runtime or CodeWalker".to_owned(),
        ]
    } else {
        let preset = shader_template
            .as_ref()
            .map(|(_, kind)| kind.preset_name())
            .unwrap_or("unknown");
        let sampler_note = match shader_template.as_ref().map(|(_, kind)| kind) {
            Some(ShaderTemplateKind::Normal) => {
                "normal.sps: DiffuseSampler, BumpSampler and SpecSampler were rebound; bumpiness may be set from normalTexture.scale"
            }
            Some(ShaderTemplateKind::Spec) => {
                "spec.sps: only DiffuseSampler and SpecSampler were rebound; all other shader parameters remain copied from the source template"
            }
            Some(ShaderTemplateKind::NormalSpecTint) => {
                "normal_spec_tnt.sps: DiffuseSampler, TintPaletteSampler, BumpSampler and SpecSampler were rebound; tintPaletteSelector remains fixed to the verified source value"
            }
            Some(ShaderTemplateKind::EmissiveNight) => {
                "emissivenight.sps: DiffuseSampler is bound to one combined albedo/emissive source; emissiveFactor maps to matMaterialColorScale RGB and emissiveStrength maps to emissiveMultiplier"
            }
            Some(ShaderTemplateKind::Emissive) => {
                "emissive.sps: DiffuseSampler is bound to one combined albedo/emissive source; emissiveFactor maps to matMaterialColorScale RGB and emissiveStrength maps to emissiveMultiplier"
            }
            None => "ShaderFX template identity is unavailable",
        };
        vec![
            format!("ShaderFX preset {preset} was cloned from a Legacy resource template"),
            sampler_note.to_owned(),
            "baseColorFactor, metallic, roughness, emissive, alpha and other PBR/surface semantics are not generalized by this template path".to_owned(),
            "output has not been accepted by GTA V runtime or CodeWalker".to_owned(),
        ]
    };

    let report = GlbYdrExportReport {
        schema: "ragelab.glb.ydr-export",
        schema_version: if selection_report.is_some() { 2 } else { 1 },
        source_name: source_name.to_owned(),
        source_sha256: inspection.source.sha256.clone(),
        source_byte_length: source_bytes.len(),
        profile_sha256,
        output_sha256: sha256_hex(&ydr_bytes),
        output_byte_length: ydr_bytes.len(),
        ydr_version: 165,
        geometry_count: geometries.len(),
        vertex_count,
        triangle_count,
        embedded_texture_count: textures.len(),
        texture_formats,
        profile_validated: profile_validation.valid,
        material_count: profile.materials.len(),
        shader_group: if shaders.is_empty() {
            if textures.is_empty() {
                "not_emitted"
            } else {
                "empty"
            }
        } else {
            "populated"
        },
        shader_count: shaders.len(),
        shader_template: shader_template_report,
        material_shader_bindings: material_shader_binding_count,
        geometry_shader_mappings: geometry_shader_mapping_count,
        shader_texture_bindings: shader_texture_binding_count,
        semantic_reopen: true,
        runtime_renderable: false,
        limitations,
        selection: selection_report,
    };

    Ok(GlbYdrExportResult {
        output_bytes: ydr_bytes,
        report,
    })
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

fn drawable_name_from_source_name(source_name: &str) -> String {
    let stem = Path::new(source_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("glb_model");
    let normalized = stem
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || value == '_' {
                value
            } else {
                '_'
            }
        })
        .take(64)
        .collect::<String>();
    if normalized.is_empty() {
        "glb_model".into()
    } else {
        normalized
    }
}

fn build_node_selection_report(
    inspection: &GlbInspectionReport,
    geometry: &DecodedGeometryReport,
    selected_node_indices: &[usize],
) -> Result<GlbNodeSelectionReport, io::Error> {
    if selected_node_indices.is_empty() {
        return Err(invalid_input(
            "node selection must contain at least one node index",
        ));
    }

    let nodes_by_index = inspection
        .nodes
        .iter()
        .map(|node| (node.index, node))
        .collect::<BTreeMap<_, _>>();
    let requested = selected_node_indices
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if requested.len() != selected_node_indices.len() {
        return Err(invalid_input(
            "node selection contains duplicate node indices",
        ));
    }
    for index in &requested {
        if !nodes_by_index.contains_key(index) {
            return Err(invalid_input(format!(
                "selected node index {index} is out of range ({} nodes)",
                inspection.nodes.len()
            )));
        }
    }

    let mut parents = BTreeMap::new();
    for node in &inspection.nodes {
        for child_index in &node.children {
            if parents.insert(*child_index, node.index).is_some() {
                return Err(invalid_data(format!(
                    "GLB node {child_index} is referenced by more than one parent"
                )));
            }
        }
    }

    let scene_index = inspection
        .default_scene
        .or_else(|| (!inspection.scenes.is_empty()).then_some(0));
    let roots = if let Some(index) = scene_index {
        inspection
            .scenes
            .get(index)
            .ok_or_else(|| invalid_data(format!("active scene index {index} is out of range")))?
            .nodes
            .clone()
    } else {
        inspection
            .nodes
            .iter()
            .filter(|node| !parents.contains_key(&node.index))
            .map(|node| node.index)
            .collect()
    };

    let mut scene_nodes = BTreeSet::new();
    let mut pending = roots;
    while let Some(index) = pending.pop() {
        if !scene_nodes.insert(index) {
            return Err(invalid_data(format!(
                "active scene contains a cycle or repeated node reference at {index}"
            )));
        }
        let node = nodes_by_index
            .get(&index)
            .ok_or_else(|| invalid_data(format!("active scene references missing node {index}")))?;
        pending.extend(node.children.iter().copied());
    }

    for index in &requested {
        if !scene_nodes.contains(index) {
            return Err(invalid_input(format!(
                "selected node {index} is not reachable from the active scene"
            )));
        }
        let mut ancestor = parents.get(index).copied();
        while let Some(parent_index) = ancestor {
            if requested.contains(&parent_index) {
                return Err(invalid_input(format!(
                    "selected node {index} is already included by selected ancestor {parent_index}; select only the highest group"
                )));
            }
            ancestor = parents.get(&parent_index).copied();
        }
    }

    let mut selected_entries = Vec::with_capacity(requested.len());
    let mut included_node_indices = BTreeSet::new();
    let mut included_mesh_indices = BTreeSet::new();
    for root_index in requested {
        let mut subtree_nodes = BTreeSet::new();
        let mut pending = vec![root_index];
        while let Some(index) = pending.pop() {
            if !subtree_nodes.insert(index) {
                return Err(invalid_data(format!(
                    "selected subtree rooted at node {root_index} repeats node {index}"
                )));
            }
            let node = nodes_by_index.get(&index).ok_or_else(|| {
                invalid_data(format!("selected subtree references missing node {index}"))
            })?;
            pending.extend(node.children.iter().copied());
        }
        for index in &subtree_nodes {
            if !included_node_indices.insert(*index) {
                return Err(invalid_input(format!(
                    "selected node subtrees overlap at node {index}"
                )));
            }
            if let Some(mesh_index) = nodes_by_index[index].mesh_index {
                included_mesh_indices.insert(mesh_index);
            }
        }
        let node = nodes_by_index[&root_index];
        let mesh_indices = subtree_nodes
            .iter()
            .filter_map(|index| nodes_by_index[index].mesh_index)
            .collect::<BTreeSet<_>>();
        let primitive_count = geometry
            .primitives
            .iter()
            .filter(|primitive| subtree_nodes.contains(&primitive.node_index))
            .count();
        selected_entries.push(GlbNodeSelectionEntry {
            node_index: root_index,
            node_name: node.name.clone(),
            included_node_indices: subtree_nodes.into_iter().collect(),
            mesh_indices: mesh_indices.into_iter().collect(),
            primitive_count,
        });
    }

    let primitive_count = geometry
        .primitives
        .iter()
        .filter(|primitive| included_node_indices.contains(&primitive.node_index))
        .count();
    if primitive_count == 0 {
        return Err(invalid_input(
            "selected nodes contain no mesh primitives in the active scene",
        ));
    }

    Ok(GlbNodeSelectionReport {
        schema: "ragelab.glb.node-selection",
        schema_version: 1,
        scene_index,
        selected_nodes: selected_entries,
        included_node_indices: included_node_indices.into_iter().collect(),
        included_mesh_indices: included_mesh_indices.into_iter().collect(),
        primitive_count,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing into String cannot fail");
    }
    output
}

fn glb_validation_error(error: impl std::fmt::Display) -> io::Error {
    invalid_data(error.to_string())
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn validate_supported_shader_template(
    shader: &ShaderInfo,
) -> Result<ShaderTemplateKind, io::Error> {
    match (shader.name_hash, shader.file_hash) {
        (0x38DD_00DF, 0x14A7_80FD) => {
            validate_supported_normal_shader_template(shader)?;
            Ok(ShaderTemplateKind::Normal)
        }
        (0x7204_A391, 0x85F5_0BE9) => {
            validate_supported_spec_shader_template(shader)?;
            Ok(ShaderTemplateKind::Spec)
        }
        (0x6CD4_735B, 0x304A_B527) => {
            validate_supported_normal_spec_tint_shader_template(shader)?;
            Ok(ShaderTemplateKind::NormalSpecTint)
        }
        (0xB08C_6435, 0x557D_B6E4) => {
            validate_supported_emissive_night_shader_template(shader)?;
            Ok(ShaderTemplateKind::EmissiveNight)
        }
        (0x8F24_5469, 0x9D18_3303) => {
            validate_supported_emissive_shader_template(shader)?;
            Ok(ShaderTemplateKind::Emissive)
        }
        _ => Err(invalid_input(format!(
            "unsupported Legacy ShaderFX template name/file hashes 0x{:08X}/0x{:08X}; supported presets are normal.sps, spec.sps, normal_spec_tnt.sps, emissivenight.sps and emissive.sps",
            shader.name_hash, shader.file_hash
        ))),
    }
}

fn validate_supported_normal_shader_template(shader: &ShaderInfo) -> Result<(), io::Error> {
    const EXPECTED: [(u32, u8); 11] = [
        (0xF1FE_2B71, 0),
        (0x46B7_C64F, 0),
        (0x6087_99C6, 0),
        (0xE943_7406, 1),
        (0x4620_A35D, 1),
        (0x32DD_9FF5, 1),
        (0xF671_2B81, 1),
        (0xFF11_711D, 1),
        (0xF418_334F, 1),
        (0x8774_4680, 1),
        (0x27B9_B2FA, 1),
    ];
    if shader.name_hash != 0x38DD_00DF
        || shader.file_hash != 0x14A7_80FD
        || shader.render_bucket != 0
        || shader.render_bucket_mask != 0x0000_FF01
        || shader.unknown_12 != 0x8000
        || shader.parameter_size != 304
        || shader.parameter_data_size != 384
        || shader.texture_parameters_count != 3
        || shader.parameters.len() != EXPECTED.len()
        || shader
            .parameters
            .iter()
            .zip(EXPECTED)
            .any(|(parameter, (hash, data_type))| {
                parameter.parameter_hash != hash || parameter.data_type != data_type
            })
    {
        return Err(invalid_input(
            "the selected YFT shader is not the supported Legacy normal.sps template (name/file hashes and 11-parameter signature must match the verified sample)",
        ));
    }
    for (index, parameter) in shader.parameters.iter().take(3).enumerate() {
        validate_shader_texture_base(parameter, 0x4061_87F8, "normal.sps", index)?;
    }
    Ok(())
}

fn validate_supported_spec_shader_template(shader: &ShaderInfo) -> Result<(), io::Error> {
    const EXPECTED: [(u32, u8, u8); 11] = [
        (0xF1FE_2B71, 0, 0),
        (0x6087_99C6, 0, 2),
        (0xE943_7406, 1, 168),
        (0x4620_A35D, 1, 167),
        (0x32DD_9FF5, 1, 166),
        (0xFF11_711D, 1, 165),
        (0xF418_334F, 1, 164),
        (0x8774_4680, 1, 163),
        (0x27B9_B2FA, 1, 162),
        (0xBA54_C190, 1, 161),
        (0xD79B_FC1E, 1, 160),
    ];
    if shader.name_hash != 0x7204_A391
        || shader.file_hash != 0x85F5_0BE9
        || shader.render_bucket != 0
        || shader.render_bucket_mask != 0x0000_FF01
        || shader.unknown_0c != 0
        || shader.unknown_12 != 0x8000
        || shader.unknown_1c != 0
        || shader.unknown_24 != 0
        || shader.unknown_26 != 0
        || shader.unknown_28 != 0
        || shader.parameter_size != 320
        || shader.parameter_data_size != 400
        || shader.texture_parameters_count != 2
        || shader.parameters.len() != EXPECTED.len()
        || shader.parameters.iter().zip(EXPECTED).any(
            |(parameter, (hash, data_type, unknown_1))| {
                parameter.parameter_hash != hash
                    || parameter.data_type != data_type
                    || parameter.unknown_1 != unknown_1
                    || parameter.unknown_2 != 0
                    || parameter.unknown_4 != 0
                    || (data_type == 1 && parameter.vector_data.len() != 1)
            },
        )
    {
        return Err(invalid_input(
            "the selected YFT shader is not the supported Legacy spec.sps template (header and ordered 11-parameter descriptor signature must match the verified road-pole sample)",
        ));
    }
    for (index, parameter) in shader.parameters.iter().take(2).enumerate() {
        validate_shader_texture_base(parameter, 0x4061_87F8, "spec.sps", index)?;
    }
    Ok(())
}

fn validate_supported_normal_spec_tint_shader_template(
    shader: &ShaderInfo,
) -> Result<(), io::Error> {
    const EXPECTED: [(u32, u8, u8); 13] = [
        (0xF1FE_2B71, 0, 0),
        (0xF648_A067, 0, 2),
        (0x46B7_C64F, 0, 3),
        (0x6087_99C6, 0, 4),
        (0xE943_7406, 1, 168),
        (0x4620_A35D, 1, 167),
        (0x32DD_9FF5, 1, 166),
        (0xF671_2B81, 1, 165),
        (0xFF11_711D, 1, 164),
        (0xF418_334F, 1, 163),
        (0x8774_4680, 1, 162),
        (0x27B9_B2FA, 1, 161),
        (0xFDD7_96CF, 1, 160),
    ];
    if shader.name_hash != 0x6CD4_735B
        || shader.file_hash != 0x304A_B527
        || shader.render_bucket != 0
        || shader.render_bucket_mask != 0x0000_FF01
        || shader.unknown_0c != 0
        || shader.unknown_12 != 0x8000
        || shader.unknown_1c != 0
        || shader.unknown_24 != 0
        || shader.unknown_26 != 0
        || shader.unknown_28 != 0
        || shader.parameter_size != 352
        || shader.parameter_data_size != 448
        || shader.texture_parameters_count != 4
        || shader.parameters.len() != EXPECTED.len()
        || shader.parameters.iter().zip(EXPECTED).any(
            |(parameter, (hash, data_type, unknown_1))| {
                parameter.parameter_hash != hash
                    || parameter.data_type != data_type
                    || parameter.unknown_1 != unknown_1
                    || parameter.unknown_2 != 0
                    || parameter.unknown_4 != 0
                    || (data_type == 1 && parameter.vector_data.len() != 1)
            },
        )
    {
        return Err(invalid_input(
            "the selected YFT shader is not the supported Legacy normal_spec_tnt.sps template (header and ordered 13-parameter descriptor signature must match the verified bollard sample)",
        ));
    }

    // The observed selector is exactly zero. Keep it source-owned until the
    // runtime semantics of non-zero palette selection are independently proven.
    let selector = &shader.parameters[12];
    if selector.vector_data.as_slice() != [[0; 4]] {
        return Err(invalid_input(
            "normal_spec_tnt.sps template has a non-default tintPaletteSelector; only the verified zero selector is currently supported",
        ));
    }

    const USAGE_DATA: [u32; 4] = [
        0x2001_5874, // DIFFUSE
        0x2000_2019, // TINTPALETTE
        0x2001_5876, // NORMAL
        0x2001_5877, // SPECULAR
    ];
    for (index, (parameter, usage_data)) in
        shader.parameters.iter().take(4).zip(USAGE_DATA).enumerate()
    {
        validate_tint_texture_base(parameter, usage_data, index)?;
    }
    Ok(())
}

fn validate_tint_texture_base(
    parameter: &ragelab_ydr::ShaderParameterInfo,
    expected_usage_data: u32,
    index: usize,
) -> Result<(), io::Error> {
    let Some(base) = parameter.texture_base.as_ref() else {
        return Err(invalid_input(format!(
            "normal_spec_tnt.sps sampler {index} has no TextureBase template"
        )));
    };
    if base.vft != 0x4061_7568
        || base.unknown_4 != 1
        || base.unknown_8 != 0
        || base.unknown_c != 0
        || base.unknown_10 != 0
        || base.unknown_14 != 0
        || base.unknown_18 != 0
        || base.unknown_1c != 0
        || base.unknown_20 != 0
        || base.unknown_24 != 0
        || base.unknown_30 != 1
        || base.unknown_32 != 0x80
        || base.unknown_34 != 0
        || base.unknown_38 != 0
        || base.unknown_3c != 0
        || base.usage_data != expected_usage_data
        || base.unknown_44 != 0
        || base.extra_flags != 0
        || base.unknown_4c != 0
        || base.name.as_deref() != parameter.texture_name.as_deref()
    {
        return Err(invalid_input(format!(
            "normal_spec_tnt.sps sampler {index} TextureBase does not match the verified bollard sample (including role-specific UsageData)"
        )));
    }
    Ok(())
}

fn validate_supported_emissive_night_shader_template(shader: &ShaderInfo) -> Result<(), io::Error> {
    const EXPECTED: [(u32, u8, u8); 5] = [
        (0xF1FE_2B71, 0, 0),
        (0x948A_54F2, 1, 163),
        (0xE943_7406, 1, 162),
        (0x4620_A35D, 1, 161),
        (0x5EEB_ED48, 1, 160),
    ];
    if shader.name_hash != 0xB08C_6435
        || shader.file_hash != 0x557D_B6E4
        || shader.render_bucket != 0
        || shader.render_bucket_mask != 0x0000_FF01
        || shader.unknown_0c != 0
        || shader.unknown_12 != 0x8000
        || shader.unknown_1c != 0
        || shader.unknown_24 != 0
        || shader.unknown_26 != 0
        || shader.unknown_28 != 0
        || shader.parameter_size != 144
        || shader.parameter_data_size != 208
        || shader.texture_parameters_count != 1
        || shader.parameters.len() != EXPECTED.len()
        || shader.parameters.iter().zip(EXPECTED).any(
            |(parameter, (hash, data_type, unknown_1))| {
                parameter.parameter_hash != hash
                    || parameter.data_type != data_type
                    || parameter.unknown_1 != unknown_1
                    || parameter.unknown_2 != 0
                    || parameter.unknown_4 != 0
                    || (data_type == 1 && parameter.vector_data.len() != 1)
                    || (data_type == 0 && !parameter.vector_data.is_empty())
            },
        )
    {
        return Err(invalid_input(
            "the selected YFT shader is not the supported Legacy emissivenight.sps template (header and ordered 5-parameter descriptor signature must match the verified generator bulb sample)",
        ));
    }
    validate_shader_texture_base(&shader.parameters[0], 0x4061_A8F8, "emissivenight.sps", 0)?;
    Ok(())
}

fn validate_supported_emissive_shader_template(shader: &ShaderInfo) -> Result<(), io::Error> {
    const EXPECTED: [(u32, u8, u8); 7] = [
        (0xF1FE_2B71, 0, 0),
        (0x948A_54F2, 1, 165),
        (0xE943_7406, 1, 164),
        (0x4620_A35D, 1, 163),
        (0x5EEB_ED48, 1, 162),
        (0xBA54_C190, 1, 161),
        (0xD79B_FC1E, 1, 160),
    ];
    if shader.name_hash != 0x8F24_5469
        || shader.file_hash != 0x9D18_3303
        || shader.render_bucket != 0
        || shader.render_bucket_mask != 0x0000_FF01
        || shader.unknown_0c != 0
        || shader.unknown_12 != 0x8000
        || shader.unknown_1c != 0
        || shader.unknown_24 != 0
        || shader.unknown_26 != 0
        || shader.unknown_28 != 0
        || shader.parameter_size != 208
        || shader.parameter_data_size != 272
        || shader.texture_parameters_count != 1
        || shader.parameters.len() != EXPECTED.len()
        || shader.parameters.iter().zip(EXPECTED).any(
            |(parameter, (hash, data_type, unknown_1))| {
                parameter.parameter_hash != hash
                    || parameter.data_type != data_type
                    || parameter.unknown_1 != unknown_1
                    || parameter.unknown_2 != 0
                    || parameter.unknown_4 != 0
                    || (data_type == 1 && parameter.vector_data.len() != 1)
                    || (data_type == 0 && !parameter.vector_data.is_empty())
            },
        )
    {
        return Err(invalid_input(
            "the selected YDR/YFT shader is not the supported Legacy emissive.sps template (header and ordered 7-parameter descriptor signature must match the verified Arena light samples)",
        ));
    }

    let parameter = &shader.parameters[0];
    let Some(base) = parameter.texture_base.as_ref() else {
        return Err(invalid_input(
            "emissive.sps DiffuseSampler has no TextureBase template",
        ));
    };
    let texture_base_variant = matches!(
        (base.vft, base.unknown_32, base.usage_data),
        (0x4061_9668, 0x80, 0x2000_B414)
            | (0x4061_9668, 0x80, 0x2000_B014)
            | (0x4061_A8F8, 0x0002, 0x0000_0000)
    );
    if !texture_base_variant
        || base.unknown_4 != 1
        || base.unknown_8 != 0
        || base.unknown_c != 0
        || base.unknown_10 != 0
        || base.unknown_14 != 0
        || base.unknown_18 != 0
        || base.unknown_1c != 0
        || base.unknown_20 != 0
        || base.unknown_24 != 0
        || base.unknown_30 != 1
        || base.unknown_34 != 0
        || base.unknown_38 != 0
        || base.unknown_3c != 0
        || base.unknown_44 != 0
        || base.extra_flags != 0
        || base.unknown_4c != 0
        || base.name.as_deref() != parameter.texture_name.as_deref()
    {
        return Err(invalid_input(
            "emissive.sps DiffuseSampler TextureBase does not match one of the three verified Legacy variants (VFT/flags/UsageData)",
        ));
    }
    Ok(())
}

fn validate_shader_texture_base(
    parameter: &ragelab_ydr::ShaderParameterInfo,
    expected_vft: u32,
    preset: &str,
    index: usize,
) -> Result<(), io::Error> {
    let Some(base) = parameter.texture_base.as_ref() else {
        return Err(invalid_input(format!(
            "{preset} sampler {index} has no TextureBase template"
        )));
    };
    if base.vft != expected_vft
        || base.unknown_4 != 1
        || base.unknown_8 != 0
        || base.unknown_c != 0
        || base.unknown_10 != 0
        || base.unknown_14 != 0
        || base.unknown_18 != 0
        || base.unknown_1c != 0
        || base.unknown_20 != 0
        || base.unknown_24 != 0
        || base.unknown_30 != 1
        || base.unknown_32 != 2
        || base.unknown_34 != 0
        || base.unknown_38 != 0
        || base.unknown_3c != 0
        || base.usage_data != 0
        || base.unknown_44 != 0
        || base.extra_flags != 0
        || base.unknown_4c != 0
        || base.name.as_deref() != parameter.texture_name.as_deref()
    {
        return Err(invalid_input(format!(
            "{preset} sampler {index} TextureBase does not match the verified Legacy layout"
        )));
    }
    Ok(())
}

fn bind_shader_template_to_material(
    template: &ShaderInfo,
    kind: ShaderTemplateKind,
    material: &MaterialAuthoringSettings,
    inspection: &ragelab_gltf::GlbInspectionReport,
    image_names: &BTreeMap<usize, String>,
) -> Result<ShaderInfo, io::Error> {
    match kind {
        ShaderTemplateKind::Normal => {
            bind_normal_shader_to_material(template, material, inspection, image_names)
        }
        ShaderTemplateKind::Spec => {
            bind_spec_shader_to_material(template, material, inspection, image_names)
        }
        ShaderTemplateKind::NormalSpecTint => {
            bind_normal_spec_tint_shader_to_material(template, material, inspection, image_names)
        }
        ShaderTemplateKind::EmissiveNight | ShaderTemplateKind::Emissive => {
            bind_emissive_shader_to_material(
                template,
                kind.preset_name(),
                material,
                inspection,
                image_names,
            )
        }
    }
}

fn bind_normal_shader_to_material(
    template: &ShaderInfo,
    material: &MaterialAuthoringSettings,
    inspection: &ragelab_gltf::GlbInspectionReport,
    image_names: &BTreeMap<usize, String>,
) -> Result<ShaderInfo, io::Error> {
    if material.shader_intent != ShaderIntent::Lit {
        return Err(invalid_input(format!(
            "material {} requests {:?}; the verified normal.sps template supports lit surfaces only",
            material.source_material_index, material.shader_intent
        ))
        );
    }
    if material.surface_mode != SurfaceMode::Opaque {
        return Err(invalid_input(format!(
            "material {} requests {:?}; the verified normal.sps template currently supports opaque surfaces only",
            material.source_material_index, material.surface_mode
        ))
        );
    }
    if material.parameters.double_sided
        || material
            .parameters
            .transmission_factor
            .is_some_and(|value| value > 0.0)
        || material.parameters.ior.is_some()
    {
        return Err(invalid_input(format!(
            "material {} requests double-sided, transmission or IOR behavior that is not represented by the verified normal.sps template",
            material.source_material_index
        ))
        );
    }
    if material.parameters.base_color_factor != [1.0; 4]
        || material.parameters.metallic != 0.0
        || material.parameters.roughness != 1.0
        || material
            .parameters
            .emissive_factor
            .iter()
            .any(|value| *value != 0.0)
        || material.parameters.emissive_strength != 1.0
    {
        return Err(invalid_input(format!(
            "material {} has non-default baseColorFactor, metallic, roughness or emissive factors that normal.sps does not map; bake the intended color into the albedo image and set baseColorFactor=[1,1,1,1], metallic=0, roughness=1, emissiveFactor=[0,0,0], emissiveStrength=1 in the profile",
            material.source_material_index
        ))
        );
    }
    if material.texture_roles.metallic_roughness.is_some()
        || material.texture_roles.occlusion.is_some()
        || material.texture_roles.emissive.is_some()
        || material.texture_roles.tint_palette.is_some()
    {
        return Err(invalid_input(format!(
            "material {} maps metallic-roughness, occlusion, emissive or tintPalette textures; this normal.sps template only supports albedo, normal and specular samplers",
            material.source_material_index
        ))
        );
    }
    if material.texture_roles.specular.is_some() && material.texture_roles.specular_color.is_some()
    {
        return Err(invalid_input(format!(
            "material {} assigns both specular and specularColor to the same SpecSampler",
            material.source_material_index
        )));
    }

    let resolve = |binding: Option<&TextureBindingInfo>| -> Result<Option<String>, io::Error> {
        let Some(binding) = binding else {
            return Ok(None);
        };
        let texture = inspection
            .textures
            .get(binding.texture_index)
            .ok_or_else(|| invalid_data("material texture index is missing"))?;
        let image_index = texture
            .source_image_index
            .ok_or_else(|| invalid_input("normal.sps cannot bind an unresolved texture source"))?;
        let name = image_names.get(&image_index).cloned().ok_or_else(|| {
            invalid_input(format!(
                "texture {} image {} was not decoded as an embedded PNG",
                binding.texture_index, image_index
            ))
        })?;
        Ok(Some(name))
    };

    let albedo_name = resolve(material.texture_roles.albedo.as_ref())?;
    let normal_name = resolve(material.texture_roles.normal.as_ref())?;
    let specular_binding = material
        .texture_roles
        .specular
        .as_ref()
        .or(material.texture_roles.specular_color.as_ref());
    let specular_name = resolve(specular_binding)?;

    let mut shader = template.clone();
    set_shader_sampler(&mut shader, 0xF1FE_2B71, albedo_name)?;
    set_shader_sampler(&mut shader, 0x46B7_C64F, normal_name)?;
    set_shader_sampler(&mut shader, 0x6087_99C6, specular_name)?;

    if let Some(scale) = material
        .texture_roles
        .normal
        .as_ref()
        .and_then(|binding| binding.scale)
    {
        if !scale.is_finite() || scale < 0.0 || !(scale as f32).is_finite() {
            return Err(invalid_input(format!(
                "material {} normal scale must be finite, non-negative and representable as f32",
                material.source_material_index
            )));
        }
        let bumpiness = shader
            .parameters
            .iter_mut()
            .find(|parameter| parameter.parameter_hash == 0xF671_2B81)
            .ok_or_else(|| invalid_data("normal.sps template has no bumpiness parameter"))?;
        if bumpiness.data_type != 1 || bumpiness.vector_data.len() != 1 {
            return Err(invalid_data(
                "normal.sps bumpiness parameter has an unsupported layout",
            ));
        }
        bumpiness.vector_data[0][0] = (scale as f32).to_bits();
    }

    shader.texture_references = shader
        .parameters
        .iter()
        .filter(|parameter| parameter.data_type == 0)
        .map(|parameter| ShaderTextureReference {
            parameter_hash: parameter.parameter_hash,
            texture_name: parameter.texture_name.clone(),
        })
        .collect();
    Ok(shader)
}

fn bind_spec_shader_to_material(
    template: &ShaderInfo,
    material: &MaterialAuthoringSettings,
    inspection: &ragelab_gltf::GlbInspectionReport,
    image_names: &BTreeMap<usize, String>,
) -> Result<ShaderInfo, io::Error> {
    if material.shader_intent != ShaderIntent::Lit {
        return Err(invalid_input(format!(
            "material {} requests {:?}; the verified spec.sps template supports lit surfaces only",
            material.source_material_index, material.shader_intent
        )));
    }
    if material.surface_mode != SurfaceMode::Opaque {
        return Err(invalid_input(format!(
            "material {} requests {:?}; the verified spec.sps template is currently restricted to opaque profile inputs",
            material.source_material_index, material.surface_mode
        ))
        );
    }
    if material.parameters.double_sided
        || material
            .parameters
            .transmission_factor
            .is_some_and(|value| value > 0.0)
        || material.parameters.ior.is_some()
    {
        return Err(invalid_input(format!(
            "material {} requests double-sided, transmission or IOR behavior not represented by spec.sps",
            material.source_material_index
        ))
        );
    }
    if material.parameters.base_color_factor != [1.0; 4]
        || material.parameters.metallic != 0.0
        || material.parameters.roughness != 1.0
        || material
            .parameters
            .emissive_factor
            .iter()
            .any(|value| *value != 0.0)
        || material.parameters.emissive_strength != 1.0
    {
        return Err(invalid_input(format!(
            "material {} has non-default baseColorFactor, metallic, roughness or emissive factors that spec.sps does not map; bake intended tint into albedo and keep baseColorFactor white, metallic=0, roughness=1 and emissive neutral",
            material.source_material_index
        ))
        );
    }
    if material.texture_roles.normal.is_some()
        || material.texture_roles.metallic_roughness.is_some()
        || material.texture_roles.occlusion.is_some()
        || material.texture_roles.emissive.is_some()
        || material.texture_roles.specular_color.is_some()
        || material.texture_roles.tint_palette.is_some()
    {
        return Err(invalid_input(format!(
            "material {} requests normal, metallic-roughness, occlusion, emissive, specularColor or tintPalette textures; spec.sps only exposes DiffuseSampler and SpecSampler in this verified signature",
            material.source_material_index
        ))
        );
    }

    let resolve = |binding: Option<&TextureBindingInfo>| -> Result<Option<String>, io::Error> {
        let Some(binding) = binding else {
            return Ok(None);
        };
        let texture = inspection
            .textures
            .get(binding.texture_index)
            .ok_or_else(|| invalid_data("material texture index is missing"))?;
        let image_index = texture
            .source_image_index
            .ok_or_else(|| invalid_input("spec.sps cannot bind an unresolved texture source"))?;
        let name = image_names.get(&image_index).cloned().ok_or_else(|| {
            invalid_input(format!(
                "texture {} image {} was not decoded as an embedded PNG",
                binding.texture_index, image_index
            ))
        })?;
        Ok(Some(name))
    };

    let albedo_name = resolve(material.texture_roles.albedo.as_ref())?;
    let specular_name = resolve(material.texture_roles.specular.as_ref())?;

    let mut shader = template.clone();
    set_shader_sampler(&mut shader, 0xF1FE_2B71, albedo_name)?;
    set_shader_sampler(&mut shader, 0x6087_99C6, specular_name)?;
    shader.texture_references = shader
        .parameters
        .iter()
        .filter(|parameter| parameter.data_type == 0)
        .map(|parameter| ShaderTextureReference {
            parameter_hash: parameter.parameter_hash,
            texture_name: parameter.texture_name.clone(),
        })
        .collect();
    Ok(shader)
}

fn bind_normal_spec_tint_shader_to_material(
    template: &ShaderInfo,
    material: &MaterialAuthoringSettings,
    inspection: &ragelab_gltf::GlbInspectionReport,
    image_names: &BTreeMap<usize, String>,
) -> Result<ShaderInfo, io::Error> {
    if material.shader_intent != ShaderIntent::Lit {
        return Err(invalid_input(format!(
            "material {} requests {:?}; normal_spec_tnt.sps is restricted to lit surfaces",
            material.source_material_index, material.shader_intent
        )));
    }
    if material.surface_mode != SurfaceMode::Opaque {
        return Err(invalid_input(format!(
            "material {} requests {:?}; normal_spec_tnt.sps is restricted to opaque surfaces",
            material.source_material_index, material.surface_mode
        )));
    }
    if material.parameters.double_sided
        || material
            .parameters
            .transmission_factor
            .is_some_and(|value| value > 0.0)
        || material.parameters.ior.is_some()
    {
        return Err(invalid_input(format!(
            "material {} requests double-sided, transmission or IOR behavior not represented by normal_spec_tnt.sps",
            material.source_material_index
        ))
        );
    }
    if material.parameters.base_color_factor != [1.0; 4]
        || material.parameters.metallic != 0.0
        || material.parameters.roughness != 1.0
        || material
            .parameters
            .emissive_factor
            .iter()
            .any(|value| *value != 0.0)
        || material.parameters.emissive_strength != 1.0
    {
        return Err(invalid_input(format!(
            "material {} has baseColorFactor, metallic, roughness or emissive factors not represented by normal_spec_tnt.sps; bake tint into albedo and use white baseColorFactor, metallic=0, roughness=1 and neutral emissive values",
            material.source_material_index
        ))
        );
    }
    if material.texture_roles.metallic_roughness.is_some()
        || material.texture_roles.occlusion.is_some()
        || material.texture_roles.emissive.is_some()
        || material.texture_roles.specular_color.is_some()
    {
        return Err(invalid_input(format!(
            "material {} requests metallic-roughness, occlusion, emissive or specularColor textures; normal_spec_tnt.sps supports albedo, tintPalette, normal and specular only",
            material.source_material_index
        ))
        );
    }

    let resolve = |binding: Option<&TextureBindingInfo>| -> Result<Option<String>, io::Error> {
        let Some(binding) = binding else {
            return Ok(None);
        };
        let texture = inspection
            .textures
            .get(binding.texture_index)
            .ok_or_else(|| invalid_data("material texture index is missing"))?;
        let image_index = texture.source_image_index.ok_or_else(|| {
            invalid_input("normal_spec_tnt.sps cannot bind an unresolved texture source")
        })?;
        let name = image_names.get(&image_index).cloned().ok_or_else(|| {
            invalid_input(format!(
                "texture {} image {} was not decoded as an embedded PNG",
                binding.texture_index, image_index
            ))
        })?;
        Ok(Some(name))
    };

    let albedo_name = resolve(material.texture_roles.albedo.as_ref())?;
    let tint_palette_name = resolve(material.texture_roles.tint_palette.as_ref())?;
    let normal_name = resolve(material.texture_roles.normal.as_ref())?;
    let specular_name = resolve(material.texture_roles.specular.as_ref())?;

    let mut shader = template.clone();
    set_shader_sampler(&mut shader, 0xF1FE_2B71, albedo_name)?;
    set_shader_sampler(&mut shader, 0xF648_A067, tint_palette_name)?;
    set_shader_sampler(&mut shader, 0x46B7_C64F, normal_name)?;
    set_shader_sampler(&mut shader, 0x6087_99C6, specular_name)?;

    if let Some(scale) = material
        .texture_roles
        .normal
        .as_ref()
        .and_then(|binding| binding.scale)
    {
        if !scale.is_finite() || scale < 0.0 || !(scale as f32).is_finite() {
            return Err(invalid_input(format!(
                "material {} normal scale must be finite, non-negative and representable as f32",
                material.source_material_index
            )));
        }
        let bumpiness = shader
            .parameters
            .iter_mut()
            .find(|parameter| parameter.parameter_hash == 0xF671_2B81)
            .ok_or_else(|| {
                invalid_data("normal_spec_tnt.sps template has no bumpiness parameter")
            })?;
        if bumpiness.data_type != 1 || bumpiness.vector_data.len() != 1 {
            return Err(invalid_data(
                "normal_spec_tnt.sps bumpiness parameter has an unsupported layout",
            ));
        }
        bumpiness.vector_data[0][0] = (scale as f32).to_bits();
    }

    shader.texture_references = shader
        .parameters
        .iter()
        .filter(|parameter| parameter.data_type == 0)
        .map(|parameter| ShaderTextureReference {
            parameter_hash: parameter.parameter_hash,
            texture_name: parameter.texture_name.clone(),
        })
        .collect();
    Ok(shader)
}

fn bind_emissive_shader_to_material(
    template: &ShaderInfo,
    preset_name: &str,
    material: &MaterialAuthoringSettings,
    inspection: &ragelab_gltf::GlbInspectionReport,
    image_names: &BTreeMap<usize, String>,
) -> Result<ShaderInfo, io::Error> {
    if material.shader_intent != ShaderIntent::Emissive {
        return Err(invalid_input(format!(
            "material {} requests {:?}; {preset_name} requires shaderIntent=emissive",
            material.source_material_index, material.shader_intent
        )));
    }
    if material.surface_mode != SurfaceMode::Opaque {
        return Err(invalid_input(format!(
            "material {} requests {:?}; {preset_name} is restricted to opaque surfaces",
            material.source_material_index, material.surface_mode
        )));
    }
    if material.parameters.double_sided
        || material
            .parameters
            .transmission_factor
            .is_some_and(|value| value > 0.0)
        || material.parameters.ior.is_some()
    {
        return Err(invalid_input(format!(
            "material {} requests double-sided, transmission or IOR behavior not represented by {preset_name}",
            material.source_material_index
        ))
        );
    }
    if material.parameters.base_color_factor != [1.0; 4]
        || material.parameters.metallic != 0.0
        || material.parameters.roughness != 1.0
    {
        return Err(invalid_input(format!(
            "material {} has baseColorFactor, metallic or roughness inputs not represented by {preset_name}; use white baseColorFactor, metallic=0 and roughness=1",
            material.source_material_index
        ))
        );
    }
    if !material
        .parameters
        .emissive_factor
        .iter()
        .any(|value| *value > 0.0)
        || material.parameters.emissive_strength <= 0.0
        || !(material.parameters.emissive_strength as f32).is_finite()
        || material
            .parameters
            .emissive_factor
            .iter()
            .any(|value| !(*value as f32).is_finite())
    {
        return Err(invalid_input(format!(
            "material {} requires a non-zero emissiveFactor and a positive emissiveStrength representable as f32",
            material.source_material_index
        ))
        );
    }
    if material.texture_roles.normal.is_some()
        || material.texture_roles.metallic_roughness.is_some()
        || material.texture_roles.specular.is_some()
        || material.texture_roles.specular_color.is_some()
        || material.texture_roles.occlusion.is_some()
        || material.texture_roles.tint_palette.is_some()
    {
        return Err(invalid_input(format!(
            "material {} requests normal, metallic-roughness, specular, occlusion or tintPalette textures; {preset_name} exposes only one DiffuseSampler",
            material.source_material_index
        ))
        );
    }

    let albedo = material.texture_roles.albedo.as_ref();
    let emissive = material.texture_roles.emissive.as_ref();
    if let (Some(albedo), Some(emissive)) = (albedo, emissive) {
        if albedo.texture_index != emissive.texture_index || albedo.tex_coord != emissive.tex_coord
        {
            return Err(invalid_input(format!(
                "material {} supplies separate albedo and emissive textures, but {preset_name} has only one DiffuseSampler; use one combined source texture or select another workflow",
                material.source_material_index
            ))
            );
        }
    }
    let source_binding = emissive.or(albedo).ok_or_else(|| {
        invalid_input(format!(
            "material {} requires either an albedo or emissive texture for {preset_name} DiffuseSampler",
            material.source_material_index
        ))
    })?;

    let texture = inspection
        .textures
        .get(source_binding.texture_index)
        .ok_or_else(|| invalid_data("material texture index is missing"))?;
    let image_index = texture.source_image_index.ok_or_else(|| {
        invalid_input(format!(
            "{preset_name} cannot bind an unresolved texture source"
        ))
    })?;
    let texture_name = image_names.get(&image_index).cloned().ok_or_else(|| {
        invalid_input(format!(
            "texture {} image {} was not decoded as an embedded PNG",
            source_binding.texture_index, image_index
        ))
    })?;

    let mut shader = template.clone();
    set_shader_sampler(&mut shader, 0xF1FE_2B71, Some(texture_name))?;

    // The conversion registry identifies matMaterialColorScale as a 16-byte
    // color vector and emissiveMultiplier as a scalar. Map the glTF emission
    // factor/strength to those named fields, preserving the untouched lanes.
    let color_scale = shader
        .parameters
        .iter_mut()
        .find(|parameter| parameter.parameter_hash == 0x948A_54F2)
        .ok_or_else(|| {
            invalid_data(format!(
                "{preset_name} template has no matMaterialColorScale"
            ))
        })?;
    if color_scale.data_type != 1 || color_scale.vector_data.len() != 1 {
        return Err(invalid_data(format!(
            "{preset_name} matMaterialColorScale has an unsupported layout"
        )));
    }
    for (index, value) in material.parameters.emissive_factor.iter().enumerate() {
        color_scale.vector_data[0][index] = (*value as f32).to_bits();
    }

    let multiplier = shader
        .parameters
        .iter_mut()
        .find(|parameter| parameter.parameter_hash == 0x5EEB_ED48)
        .ok_or_else(|| invalid_data(format!("{preset_name} template has no emissiveMultiplier")))?;
    if multiplier.data_type != 1 || multiplier.vector_data.len() != 1 {
        return Err(invalid_data(format!(
            "{preset_name} emissiveMultiplier has an unsupported layout"
        )));
    }
    multiplier.vector_data[0][0] = (material.parameters.emissive_strength as f32).to_bits();

    shader.texture_references = shader
        .parameters
        .iter()
        .filter(|parameter| parameter.data_type == 0)
        .map(|parameter| ShaderTextureReference {
            parameter_hash: parameter.parameter_hash,
            texture_name: parameter.texture_name.clone(),
        })
        .collect();
    Ok(shader)
}

fn set_shader_sampler(
    shader: &mut ShaderInfo,
    parameter_hash: u32,
    texture_name: Option<String>,
) -> Result<(), io::Error> {
    let parameter = shader
        .parameters
        .iter_mut()
        .find(|parameter| parameter.parameter_hash == parameter_hash)
        .ok_or_else(|| {
            invalid_data(format!(
                "ShaderFX template is missing sampler hash 0x{parameter_hash:08X}"
            ))
        })?;
    if parameter.data_type != 0 {
        return Err(invalid_data(format!(
            "ShaderFX parameter 0x{parameter_hash:08X} is not a texture sampler"
        )));
    }
    match texture_name {
        Some(name) => {
            let base = parameter.texture_base.as_mut().ok_or_else(|| {
                invalid_data(format!(
                    "ShaderFX sampler 0x{parameter_hash:08X} has no TextureBase template"
                ))
            })?;
            base.name = Some(name.clone());
            parameter.texture_name = Some(name);
        }
        None => {
            parameter.texture_name = None;
            parameter.texture_base = None;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_first_inspection_and_geometry_share_the_glb_parser() {
        let bytes = triangle_glb();
        let inspection = inspect_glb_bytes(&bytes, "simple.glb").unwrap();
        let geometry = decode_glb_geometry_bytes(&bytes, "simple.glb").unwrap();
        assert_eq!(inspection.source.sha256, geometry.source_sha256);
        assert_eq!(inspection.source.byte_length, geometry.source_byte_length);
        assert_eq!(geometry.primitives.len(), 1);
        assert_eq!(geometry.primitives[0].vertex_count, 3);
    }

    #[test]
    fn glb_inspection_and_geometry_handle_deterministic_mutation_corpus() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let seed = triangle_glb();
        let mut state = 0xA076_1D64_78BD_642F_u64;
        for case in 0..160_usize {
            let mut bytes = seed.clone();
            let mutation_count = 1 + case % 4;
            for _ in 0..mutation_count {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let index = (state as usize) % bytes.len();
                bytes[index] ^= ((state >> 40) as u8) | 1;
            }
            if case % 11 == 0 {
                bytes.truncate(case % (bytes.len() + 1));
            }
            if case % 13 == 0 {
                bytes.extend_from_slice(&state.to_le_bytes());
            }

            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let inspection = inspect_glb_bytes(&bytes, "mutated.glb");
                let geometry = decode_glb_geometry_bytes(&bytes, "mutated.glb");
                if let (Ok(inspection), Ok(geometry)) = (inspection, geometry) {
                    assert_eq!(inspection.source.sha256, geometry.source_sha256);
                    assert_eq!(inspection.source.byte_length, geometry.source_byte_length);
                    for primitive in &geometry.primitives {
                        assert_eq!(primitive.vertex_count, primitive.positions.len());
                        assert_eq!(primitive.index_count, primitive.indices.len());
                    }
                }
            }));
            assert!(outcome.is_ok(), "GLB mutation case {case} panicked");
        }

        for case in 0..160_usize {
            let length = case % 257;
            let mut bytes = vec![0_u8; length];
            for byte in &mut bytes {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = (state >> 24) as u8;
            }
            if case % 2 == 0 && bytes.len() >= 4 {
                bytes[..4].copy_from_slice(b"glTF");
            }
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let _ = inspect_glb_bytes(&bytes, "random.glb");
                let _ = decode_glb_geometry_bytes(&bytes, "random.glb");
            }));
            assert!(outcome.is_ok(), "GLB random input case {case} panicked");
        }
    }

    #[test]
    fn default_profile_and_validation_are_byte_first() {
        let bytes = triangle_glb();
        let profile = default_glb_authoring_profile(&bytes, "simple.glb").unwrap();
        let profile_bytes = serde_json::to_vec(&profile).unwrap();
        let report =
            validate_glb_authoring_profile_bytes(&bytes, "simple.glb", &profile_bytes).unwrap();
        assert!(report.valid);
        assert_eq!(report.source_sha256, profile.source.sha256);
    }

    #[test]
    fn export_returns_semantically_reopened_bytes_and_non_runtime_claim() {
        let source_bytes = triangle_glb();
        let result = export_glb_to_ydr(GlbYdrExportRequest {
            source_name: "simple.glb".into(),
            source_bytes,
            profile_bytes: None,
            shader_template: None,
            shader_index: 0,
        })
        .unwrap();
        assert!(result.report.semantic_reopen);
        assert!(!result.report.runtime_renderable);
        assert!(result
            .report
            .limitations
            .iter()
            .any(|item| item
                .contains("CodeWalker Model Viewer is not safe for shaderless drawables")));
        assert!(!result.output_bytes.is_empty());
        assert_eq!(result.report.output_byte_length, result.output_bytes.len());
        assert_eq!(
            result.report.output_sha256,
            sha256_hex(&result.output_bytes)
        );
        assert_eq!(result.report.ydr_version, 165);
        assert_eq!(result.report.geometry_count, 1);
        assert_eq!(result.report.vertex_count, 3);
        assert_eq!(result.report.triangle_count, 1);
        YdrDocument::from_bytes(&result.output_bytes).unwrap();
    }

    #[test]
    fn node_selection_includes_descendants_preserves_world_transforms_and_rejects_overlap() {
        let bytes = triangle_glb();
        let geometry = decode_glb_geometry_bytes(&bytes, "group.glb").unwrap();
        assert_eq!(geometry.primitives.len(), 1);
        assert_eq!(geometry.primitives[0].node_index, 1);
        assert_eq!(geometry.primitives[0].positions[0], [5.0, 0.0, 3.0]);

        let selected_group = export_glb_to_ydr_with_nodes(
            GlbYdrExportRequest {
                source_name: "group.glb".into(),
                source_bytes: bytes.clone(),
                profile_bytes: None,
                shader_template: None,
                shader_index: 0,
            },
            &[0],
        )
        .unwrap();
        assert!(selected_group.report.semantic_reopen);
        assert_eq!(selected_group.report.schema_version, 2);
        assert_eq!(selected_group.report.geometry_count, 1);
        let selection = selected_group.report.selection.unwrap();
        assert_eq!(selection.schema, "ragelab.glb.node-selection");
        assert_eq!(selection.selected_nodes.len(), 1);
        assert_eq!(selection.selected_nodes[0].node_index, 0);
        assert_eq!(
            selection.selected_nodes[0].included_node_indices,
            vec![0, 1]
        );
        assert_eq!(selection.included_mesh_indices, vec![0]);
        assert_eq!(selection.primitive_count, 1);

        let selected_leaf = export_glb_to_ydr_with_nodes(
            GlbYdrExportRequest {
                source_name: "group.glb".into(),
                source_bytes: bytes.clone(),
                profile_bytes: None,
                shader_template: None,
                shader_index: 0,
            },
            &[1],
        )
        .unwrap();
        assert_eq!(selected_leaf.report.geometry_count, 1);
        assert_eq!(
            selected_leaf
                .report
                .selection
                .unwrap()
                .included_node_indices,
            vec![1]
        );
        YdrDocument::from_bytes(&selected_leaf.output_bytes).unwrap();

        let overlap = export_glb_to_ydr_with_nodes(
            GlbYdrExportRequest {
                source_name: "group.glb".into(),
                source_bytes: bytes.clone(),
                profile_bytes: None,
                shader_template: None,
                shader_index: 0,
            },
            &[0, 1],
        )
        .unwrap_err();
        assert!(overlap
            .to_string()
            .contains("already included by selected ancestor 0"));

        let duplicate = export_glb_to_ydr_with_nodes(
            GlbYdrExportRequest {
                source_name: "group.glb".into(),
                source_bytes: bytes.clone(),
                profile_bytes: None,
                shader_template: None,
                shader_index: 0,
            },
            &[1, 1],
        )
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate node indices"));

        let empty = export_glb_to_ydr_with_nodes(
            GlbYdrExportRequest {
                source_name: "group.glb".into(),
                source_bytes: bytes.clone(),
                profile_bytes: None,
                shader_template: None,
                shader_index: 0,
            },
            &[],
        )
        .unwrap_err();
        assert!(empty.to_string().contains("at least one node index"));

        let invalid = export_glb_to_ydr_with_nodes(
            GlbYdrExportRequest {
                source_name: "group.glb".into(),
                source_bytes: bytes,
                profile_bytes: None,
                shader_template: None,
                shader_index: 0,
            },
            &[99],
        )
        .unwrap_err();
        assert!(invalid.to_string().contains("out of range"));
    }

    fn triangle_glb() -> Vec<u8> {
        let mut binary = Vec::new();
        for vertex in [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for component in vertex {
                binary.extend_from_slice(&component.to_le_bytes());
            }
        }
        let indices_offset = binary.len();
        for index in [0_u16, 1, 2] {
            binary.extend_from_slice(&index.to_le_bytes());
        }
        while binary.len() % 4 != 0 {
            binary.push(0);
        }

        let document = serde_json::json!({
            "asset": {"version": "2.0", "generator": "RageLab engine unit fixture"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [
                {"name": "RootGroup", "translation": [5.0, 0.0, 0.0], "children": [1]},
                {"name": "Triangle", "mesh": 0, "translation": [0.0, 3.0, 0.0]}
            ],
            "meshes": [{
                "name": "TriangleMesh",
                "primitives": [{
                    "attributes": {"POSITION": 0},
                    "indices": 1
                }]
            }],
            "accessors": [
                {
                    "bufferView": 0,
                    "componentType": 5126,
                    "count": 3,
                    "type": "VEC3",
                    "min": [0.0, 0.0, 0.0],
                    "max": [1.0, 1.0, 0.0]
                },
                {
                    "bufferView": 1,
                    "componentType": 5123,
                    "count": 3,
                    "type": "SCALAR"
                }
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": indices_offset, "byteLength": 6}
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
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&(total_length as u32).to_le_bytes());
        bytes.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0x4E4F534A_u32.to_le_bytes());
        bytes.extend_from_slice(&json_chunk);
        bytes.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0x004E4942_u32.to_le_bytes());
        bytes.extend_from_slice(&binary);
        bytes
    }
}
