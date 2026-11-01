use std::io;

use ragelab_hash::joaat;
use ragelab_resource::{Rsc7Probe, Rsc7Resource, SYSTEM_BASE};
use ragelab_ydr::{
    inspect_drawable_lods, ShaderInfo, YdrDocument, YdrEditSession, YdrError,
    YdrGeometryInspection, YdrLodInspection,
};
use ragelab_ytd::{DictionaryTextureInfo, YtdError};
use serde::Serialize;
use sha2::{Digest, Sha256};

const RSC7_HEADER_BYTES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum YdrDiagnosticSeverity {
    Error,
    Unsupported,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrDiagnostic {
    pub severity: YdrDiagnosticSeverity,
    pub code: String,
    pub path: Option<String>,
    pub message: String,
    pub promoted_from: Option<YdrDiagnosticSeverity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrBoundsReport {
    pub center: [f32; 3],
    pub radius: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrContainerReport {
    pub magic: String,
    pub resource_version: u32,
    pub system_flags: String,
    pub graphics_flags: String,
    pub system_segment_bytes: usize,
    pub graphics_segment_bytes: usize,
    pub compressed_payload_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrGeometryReport {
    pub model_index: usize,
    pub geometry_index: usize,
    pub shader_index: Option<u16>,
    pub topology: String,
    pub vertex_count: usize,
    pub index_count: usize,
    pub triangle_count: usize,
    pub declaration_flags: String,
    pub declaration_stride: u16,
    pub declaration_component_count: u8,
    pub declaration_types: String,
    pub model_bounds_min: [f32; 3],
    pub model_bounds_max: [f32; 3],
    pub geometry_bounds_min: [f32; 3],
    pub geometry_bounds_max: [f32; 3],
    pub attributes: Vec<String>,
    pub positions_finite: bool,
    pub normals_finite: Option<bool>,
    pub tangents_finite: Option<bool>,
    pub color0_finite: Option<bool>,
    pub uv0_finite: Option<bool>,
    pub uv1_finite: Option<bool>,
    pub uv2_finite: Option<bool>,
    pub winding_aligned: Option<usize>,
    pub winding_opposed: Option<usize>,
    pub winding_degenerate: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrLodReport {
    pub level: String,
    pub model_count: usize,
    pub geometry_count: usize,
    pub vertex_count: usize,
    pub index_count: usize,
    pub triangle_count: usize,
    pub geometries: Vec<YdrGeometryReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrShaderTextureReferenceReport {
    pub parameter_hash: String,
    pub texture_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrShaderParameterReport {
    pub parameter_hash: String,
    pub data_type: u8,
    pub texture_name: Option<String>,
    pub vector_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrShaderReport {
    pub index: usize,
    pub name_hash: String,
    pub file_hash: String,
    pub render_bucket: u8,
    pub render_bucket_mask: String,
    pub parameter_size: u16,
    pub parameter_data_size: u16,
    pub texture_parameters_count: u8,
    pub parameters: Vec<YdrShaderParameterReport>,
    pub texture_references: Vec<YdrShaderTextureReferenceReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrTextureReport {
    pub index: usize,
    pub name: Option<String>,
    pub name_hash: String,
    pub name_hash_matches_name: Option<bool>,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub stride: u16,
    pub format: String,
    pub raw_format: String,
    pub mip_levels: u8,
    pub decoded_mip_levels: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrTextureDictionaryReport {
    pub texture_count: usize,
    pub textures: Vec<YdrTextureReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrStructuralAuditReport {
    pub valid: bool,
    pub geometry_provenance_count: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrDrawableReport {
    pub name: Option<String>,
    pub selected_lod: String,
    pub coordinate_convention: String,
    pub bounds: YdrBoundsReport,
    pub lod_count: usize,
    pub model_count: usize,
    pub geometry_count: usize,
    pub vertex_count: usize,
    pub index_count: usize,
    pub triangle_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrInspectionReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub source_name: String,
    pub source_sha256: String,
    pub source_byte_length: usize,
    pub container: YdrContainerReport,
    pub drawable: YdrDrawableReport,
    pub lods: Vec<YdrLodReport>,
    pub shaders: Vec<YdrShaderReport>,
    pub embedded_textures: Option<YdrTextureDictionaryReport>,
    pub structural_audit: YdrStructuralAuditReport,
    pub semantic_reopen: bool,
    pub runtime_renderable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct YdrValidationReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub source_name: String,
    pub source_sha256: String,
    pub source_byte_length: usize,
    pub strict: bool,
    pub valid: bool,
    pub container_valid: bool,
    pub format_valid: bool,
    pub semantic_reopen: bool,
    pub runtime_renderable: bool,
    pub codewalker_gui_accepted: bool,
    pub inspection: Option<YdrInspectionReport>,
    pub diagnostics: Vec<YdrDiagnostic>,
}

#[derive(Debug)]
struct YdrInspectionFailure {
    container_valid: bool,
    severity: YdrDiagnosticSeverity,
    code: &'static str,
    error: io::Error,
}

fn ydr_io_error(error: YdrError) -> io::Error {
    match &error {
        YdrError::Unsupported(_) => io::Error::new(io::ErrorKind::Unsupported, error),
        _ => io::Error::new(io::ErrorKind::InvalidData, error),
    }
}

fn ydr_inspection_failure(container_valid: bool, error: YdrError) -> YdrInspectionFailure {
    let unsupported = matches!(&error, YdrError::Unsupported(_));
    YdrInspectionFailure {
        container_valid,
        severity: if unsupported {
            YdrDiagnosticSeverity::Unsupported
        } else {
            YdrDiagnosticSeverity::Error
        },
        code: if unsupported {
            "unsupported_ydr_feature"
        } else {
            "invalid_ydr_structure"
        },
        error: ydr_io_error(error),
    }
}

fn inspection_failure(
    container_valid: bool,
    error: impl std::fmt::Display,
) -> YdrInspectionFailure {
    YdrInspectionFailure {
        container_valid,
        severity: YdrDiagnosticSeverity::Error,
        code: "invalid_rsc7_resource",
        error: io::Error::new(io::ErrorKind::InvalidData, error.to_string()),
    }
}

fn inspect_ydr_inner(
    source_bytes: &[u8],
    source_name: &str,
) -> Result<YdrInspectionReport, YdrInspectionFailure> {
    let probe = Rsc7Probe::parse(source_bytes)
        .map_err(|error| inspection_failure(false, format!("invalid RSC7 container: {error}")))?;
    let resource = Rsc7Resource::parse(source_bytes).map_err(|error| {
        inspection_failure(
            false,
            format!("RSC7 decompression/segment validation failed: {error}"),
        )
    })?;
    let document = YdrDocument::from_bytes(source_bytes)
        .map_err(|error| ydr_inspection_failure(true, error))?;
    let lods = inspect_drawable_lods(&resource, SYSTEM_BASE, document.model.shaders.len())
        .map_err(|error| ydr_inspection_failure(true, error))?;
    let edit_session = YdrEditSession::from_bytes(source_bytes)
        .map_err(|error| ydr_inspection_failure(true, error))?;

    let provenance_error = edit_session.structural_audit_error().map(str::to_owned);
    let geometry_provenance_count = edit_session.geometry_provenance().len();
    let selected_geometry_count = document.model.primitives.len();
    let structural_audit_error = provenance_error.or_else(|| {
        (geometry_provenance_count != selected_geometry_count).then(|| {
            format!(
                "geometry provenance count {} does not match selected LOD geometry count {selected_geometry_count}",
                geometry_provenance_count
            )
        })
    });

    let roundtrip_bytes = resource
        .to_bytes()
        .map_err(|error| inspection_failure(true, format!("RSC7 re-encode failed: {error}")))?;
    let reopened_resource = Rsc7Resource::parse(&roundtrip_bytes).map_err(|error| {
        inspection_failure(true, format!("RSC7 semantic reopen failed: {error}"))
    })?;
    let reopened_document = YdrDocument::from_bytes(&roundtrip_bytes)
        .map_err(|error| ydr_inspection_failure(true, error))?;
    let reopened_lods = inspect_drawable_lods(
        &reopened_resource,
        SYSTEM_BASE,
        reopened_document.model.shaders.len(),
    )
    .map_err(|error| {
        inspection_failure(true, format!("YDR LOD semantic reopen failed: {error}"))
    })?;

    let textures_equal = match (
        document.embedded_textures.as_ref(),
        reopened_document.embedded_textures.as_ref(),
    ) {
        (None, None) => true,
        (Some(left), Some(right)) => left.textures() == right.textures(),
        _ => false,
    };
    let semantic_reopen = document.model == reopened_document.model
        && lods == reopened_lods
        && textures_equal
        && resource.system() == reopened_resource.system()
        && resource.graphics() == reopened_resource.graphics();

    let source_hash = Sha256::digest(source_bytes);
    let model = &document.model;
    let total_geometry_count = lods.iter().map(|lod| lod.geometries.len()).sum::<usize>();
    let total_vertex_count = lods
        .iter()
        .flat_map(|lod| &lod.geometries)
        .map(|geometry| geometry.vertex_count)
        .sum::<usize>();
    let total_index_count = lods
        .iter()
        .flat_map(|lod| &lod.geometries)
        .map(|geometry| geometry.index_count)
        .sum::<usize>();
    let total_triangle_count = lods
        .iter()
        .flat_map(|lod| &lod.geometries)
        .map(|geometry| geometry.triangle_count)
        .sum::<usize>();
    let total_model_count = lods.iter().map(|lod| lod.model_count).sum::<usize>();

    let lod_reports = lods.iter().map(to_lod_report).collect();
    let shader_reports = model
        .shaders
        .iter()
        .enumerate()
        .map(|(index, shader)| to_shader_report(index, shader))
        .collect();
    let embedded_textures =
        document
            .embedded_textures
            .as_ref()
            .map(|dictionary| YdrTextureDictionaryReport {
                texture_count: dictionary.textures().len(),
                textures: dictionary
                    .textures()
                    .iter()
                    .map(|texture| to_texture_report(texture, 0))
                    .collect(),
            });

    let _ = source_name;
    Ok(YdrInspectionReport {
        schema: "ragelab.ydr.inspection",
        schema_version: 1,
        source_name: source_bytes
            .get(0..4)
            .filter(|magic| *magic == b"RSC7")
            .map(|_| source_name.to_owned())
            .unwrap_or_else(|| source_name.to_owned()),
        source_sha256: format!("{source_hash:x}"),
        source_byte_length: source_bytes.len(),
        container: YdrContainerReport {
            magic: "RSC7".into(),
            resource_version: probe.header.version,
            system_flags: format!("0x{:08X}", probe.header.system_flags),
            graphics_flags: format!("0x{:08X}", probe.header.graphics_flags),
            system_segment_bytes: resource.system().len(),
            graphics_segment_bytes: resource.graphics().len(),
            compressed_payload_bytes: source_bytes.len().saturating_sub(RSC7_HEADER_BYTES),
        },
        drawable: YdrDrawableReport {
            name: model.name.clone(),
            selected_lod: model.lod.as_str().to_owned(),
            coordinate_convention: model.coordinate_convention.as_str().to_owned(),
            bounds: YdrBoundsReport {
                center: model.bounds.center,
                radius: model.bounds.radius,
                min: model.bounds.min,
                max: model.bounds.max,
            },
            lod_count: lods.len(),
            model_count: total_model_count,
            geometry_count: total_geometry_count,
            vertex_count: total_vertex_count,
            index_count: total_index_count,
            triangle_count: total_triangle_count,
        },
        lods: lod_reports,
        shaders: shader_reports,
        embedded_textures,
        structural_audit: YdrStructuralAuditReport {
            valid: structural_audit_error.is_none(),
            geometry_provenance_count,
            error: structural_audit_error,
        },
        semantic_reopen,
        runtime_renderable: false,
    })
}

fn to_lod_report(lod: &YdrLodInspection) -> YdrLodReport {
    let geometries = lod
        .geometries
        .iter()
        .map(to_geometry_report)
        .collect::<Vec<_>>();
    YdrLodReport {
        level: lod.level.as_str().to_owned(),
        model_count: lod.model_count,
        geometry_count: geometries.len(),
        vertex_count: geometries
            .iter()
            .map(|geometry| geometry.vertex_count)
            .sum(),
        index_count: geometries.iter().map(|geometry| geometry.index_count).sum(),
        triangle_count: geometries
            .iter()
            .map(|geometry| geometry.triangle_count)
            .sum(),
        geometries,
    }
}

fn to_geometry_report(geometry: &YdrGeometryInspection) -> YdrGeometryReport {
    let mut attributes = Vec::new();
    if geometry.has_positions {
        attributes.push("POSITION".into());
    }
    if geometry.has_normals {
        attributes.push("NORMAL".into());
    }
    if geometry.has_tangents {
        attributes.push("TANGENT".into());
    }
    if geometry.has_color0 {
        attributes.push("COLOR_0".into());
    }
    if geometry.has_uv0 {
        attributes.push("TEXCOORD_0".into());
    }
    if geometry.has_uv1 {
        attributes.push("TEXCOORD_1".into());
    }
    if geometry.has_uv2 {
        attributes.push("TEXCOORD_2".into());
    }
    YdrGeometryReport {
        model_index: geometry.model_index,
        geometry_index: geometry.geometry_index,
        shader_index: geometry.shader_index,
        topology: match geometry.topology {
            ragelab_ydr::PrimitiveTopology::TriangleList => "triangleList".into(),
        },
        vertex_count: geometry.vertex_count,
        index_count: geometry.index_count,
        triangle_count: geometry.triangle_count,
        declaration_flags: format!("0x{:08X}", geometry.declaration.flags),
        declaration_stride: geometry.declaration.stride,
        declaration_component_count: geometry.declaration.component_count,
        declaration_types: format!("0x{:016X}", geometry.declaration.types),
        model_bounds_min: geometry.model_bounds_min,
        model_bounds_max: geometry.model_bounds_max,
        geometry_bounds_min: geometry.geometry_bounds_min,
        geometry_bounds_max: geometry.geometry_bounds_max,
        attributes,
        positions_finite: geometry.positions_finite,
        normals_finite: geometry.normals_finite,
        tangents_finite: geometry.tangents_finite,
        color0_finite: geometry.color0_finite,
        uv0_finite: geometry.uv0_finite,
        uv1_finite: geometry.uv1_finite,
        uv2_finite: geometry.uv2_finite,
        winding_aligned: geometry.winding.map(|value| value.aligned),
        winding_opposed: geometry.winding.map(|value| value.opposed),
        winding_degenerate: geometry.winding.map(|value| value.degenerate),
    }
}

fn to_shader_report(index: usize, shader: &ShaderInfo) -> YdrShaderReport {
    YdrShaderReport {
        index,
        name_hash: format!("0x{:08X}", shader.name_hash),
        file_hash: format!("0x{:08X}", shader.file_hash),
        render_bucket: shader.render_bucket,
        render_bucket_mask: format!("0x{:08X}", shader.render_bucket_mask),
        parameter_size: shader.parameter_size,
        parameter_data_size: shader.parameter_data_size,
        texture_parameters_count: shader.texture_parameters_count,
        parameters: shader
            .parameters
            .iter()
            .map(|parameter| YdrShaderParameterReport {
                parameter_hash: format!("0x{:08X}", parameter.parameter_hash),
                data_type: parameter.data_type,
                texture_name: parameter
                    .texture_base
                    .as_ref()
                    .and_then(|base| base.name.clone())
                    .or_else(|| parameter.texture_name.clone()),
                vector_count: parameter.vector_data.len(),
            })
            .collect(),
        texture_references: shader
            .texture_references
            .iter()
            .map(|reference| YdrShaderTextureReferenceReport {
                parameter_hash: format!("0x{:08X}", reference.parameter_hash),
                texture_name: reference.texture_name.clone(),
            })
            .collect(),
    }
}

fn to_texture_report(
    texture: &DictionaryTextureInfo,
    decoded_mip_levels: usize,
) -> YdrTextureReport {
    let name_hash_matches_name = texture
        .name
        .as_deref()
        .map(|name| joaat(name) == texture.name_hash);
    YdrTextureReport {
        index: texture.index,
        name: texture.name.clone(),
        name_hash: format!("0x{:08X}", texture.name_hash),
        name_hash_matches_name,
        width: texture.width,
        height: texture.height,
        depth: texture.depth,
        stride: texture.stride,
        format: texture.format.normalized_name().to_owned(),
        raw_format: format!("0x{:08X}", texture.format.raw()),
        mip_levels: texture.levels,
        decoded_mip_levels,
    }
}

/// Produces the detailed YDR/RSC7 inspection used by both the CLI and Studio.
pub fn inspect_ydr_bytes(
    source_bytes: &[u8],
    source_name: &str,
) -> Result<YdrInspectionReport, io::Error> {
    inspect_ydr_inner(source_bytes, source_name).map_err(|failure| failure.error)
}

/// Validates a YDR structurally and semantically, decoding every embedded mip.
pub fn validate_ydr_bytes(
    source_bytes: &[u8],
    source_name: &str,
    strict: bool,
) -> YdrValidationReport {
    let digest = Sha256::digest(source_bytes);
    let source_sha256 = format!("{digest:x}");
    let mut diagnostics = Vec::new();
    let inspection = match inspect_ydr_inner(source_bytes, source_name) {
        Ok(report) => Some(report),
        Err(failure) => {
            push_diagnostic(
                &mut diagnostics,
                failure.severity,
                failure.code,
                Some("document".into()),
                failure.error.to_string(),
            );
            return YdrValidationReport {
                schema: "ragelab.ydr.validation",
                schema_version: 1,
                source_name: source_name.to_owned(),
                source_sha256,
                source_byte_length: source_bytes.len(),
                strict,
                valid: false,
                container_valid: failure.container_valid,
                format_valid: false,
                semantic_reopen: false,
                runtime_renderable: false,
                codewalker_gui_accepted: false,
                inspection: None,
                diagnostics,
            };
        }
    };
    let mut inspection = inspection.expect("successful inspection branch");
    if !inspection.structural_audit.valid {
        push_diagnostic(
            &mut diagnostics,
            YdrDiagnosticSeverity::Error,
            "geometry_provenance_mismatch",
            Some("drawable.geometries".into()),
            inspection
                .structural_audit
                .error
                .clone()
                .unwrap_or_else(|| "geometry provenance audit failed".into()),
        );
    }
    if !inspection.semantic_reopen {
        push_diagnostic(
            &mut diagnostics,
            YdrDiagnosticSeverity::Error,
            "semantic_reopen_mismatch",
            Some("container".into()),
            "RSC7 re-encode/reopen did not preserve the decompressed segments and parsed YDR/LOD/texture metadata",
        );
    }
    let bounds = &inspection.drawable.bounds;
    let bounds_finite = bounds
        .center
        .iter()
        .chain(bounds.min.iter())
        .chain(bounds.max.iter())
        .all(|value| value.is_finite())
        && bounds.radius.is_finite();
    let bounds_ordered = (0..3).all(|axis| bounds.min[axis] <= bounds.max[axis]);
    if !bounds_finite || !bounds_ordered || bounds.radius < 0.0 {
        push_diagnostic(
            &mut diagnostics,
            YdrDiagnosticSeverity::Error,
            "invalid_drawable_bounds",
            Some("drawable.bounds".into()),
            "Drawable bounds contain non-finite values, an inverted axis, or a negative radius",
        );
    }

    for (lod_index, lod) in inspection.lods.iter().enumerate() {
        if lod.geometry_count == 0 {
            push_diagnostic(
                &mut diagnostics,
                YdrDiagnosticSeverity::Warning,
                "lod_without_geometry",
                Some(format!("lods[{lod_index}]")),
                format!("LOD {} contains models but no decoded geometry", lod.level),
            );
        }
        for (geometry_index, geometry) in lod.geometries.iter().enumerate() {
            let path = format!("lods[{lod_index}].geometries[{geometry_index}]");
            if !geometry.positions_finite {
                push_diagnostic(
                    &mut diagnostics,
                    YdrDiagnosticSeverity::Error,
                    "non_finite_positions",
                    Some(format!("{path}.POSITION")),
                    "geometry contains non-finite vertex positions",
                );
            }
            for (field, finite) in [
                ("NORMAL", geometry.normals_finite),
                ("TANGENT", geometry.tangents_finite),
                ("COLOR_0", geometry.color0_finite),
                ("TEXCOORD_0", geometry.uv0_finite),
                ("TEXCOORD_1", geometry.uv1_finite),
                ("TEXCOORD_2", geometry.uv2_finite),
            ] {
                if finite == Some(false) {
                    push_diagnostic(
                        &mut diagnostics,
                        YdrDiagnosticSeverity::Error,
                        "non_finite_vertex_attribute",
                        Some(format!("{path}.{field}")),
                        format!("geometry contains non-finite values in {field}"),
                    );
                }
            }
            if geometry.index_count == 0 {
                push_diagnostic(
                    &mut diagnostics,
                    YdrDiagnosticSeverity::Warning,
                    "empty_geometry",
                    Some(path.clone()),
                    "geometry has vertices but no indices",
                );
            } else if geometry.index_count % 3 != 0 {
                push_diagnostic(
                    &mut diagnostics,
                    YdrDiagnosticSeverity::Error,
                    "triangle_list_index_count_mismatch",
                    Some(format!("{path}.indices")),
                    "triangle-list index count must be divisible by three",
                );
            }
            if geometry.shader_index.is_none() {
                push_diagnostic(
                    &mut diagnostics,
                    YdrDiagnosticSeverity::Warning,
                    "geometry_without_shader",
                    Some(format!("{path}.shaderIndex")),
                    "geometry has no ShaderFX mapping",
                );
            }
        }
    }

    for (shader_index, shader) in inspection.shaders.iter().enumerate() {
        for (reference_index, reference) in shader.texture_references.iter().enumerate() {
            let Some(name) = reference
                .texture_name
                .as_deref()
                .filter(|name| !name.is_empty())
            else {
                push_diagnostic(
                    &mut diagnostics,
                    YdrDiagnosticSeverity::Warning,
                    "shader_texture_reference_unresolved",
                    Some(format!(
                        "shaders[{shader_index}].textureReferences[{reference_index}]"
                    )),
                    "ShaderFX texture reference has no resolved texture name",
                );
                continue;
            };
            let embedded = inspection
                .embedded_textures
                .as_ref()
                .is_some_and(|dictionary| {
                    dictionary
                        .textures
                        .iter()
                        .any(|texture| texture.name.as_deref() == Some(name))
                });
            if !embedded {
                push_diagnostic(
                    &mut diagnostics,
                    YdrDiagnosticSeverity::Warning,
                    "shader_texture_not_embedded",
                    Some(format!(
                        "shaders[{shader_index}].textureReferences[{reference_index}]"
                    )),
                    format!(
                        "ShaderFX references texture {name}, which is not present in this drawable's embedded TextureDictionary"
                    ),
                );
            }
        }
    }

    if let Some(dictionary) = inspection.embedded_textures.as_ref() {
        for (texture_index, texture) in dictionary.textures.iter().enumerate() {
            if texture.name_hash_matches_name == Some(false) {
                push_diagnostic(
                    &mut diagnostics,
                    YdrDiagnosticSeverity::Warning,
                    "texture_name_hash_mismatch",
                    Some(format!(
                        "embeddedTextures.textures[{texture_index}].nameHash"
                    )),
                    format!(
                        "texture name {:?} does not hash to its stored JOAAT value {}",
                        texture.name.as_deref().unwrap_or(""),
                        texture.name_hash
                    ),
                );
            }
        }
    }

    let mut texture_reports = Vec::new();
    if inspection.embedded_textures.is_some() {
        // Re-open the source document once here to obtain the decoder-backed texture view.
        let document = match YdrDocument::from_bytes(source_bytes) {
            Ok(document) => document,
            Err(error) => {
                push_diagnostic(
                    &mut diagnostics,
                    YdrDiagnosticSeverity::Error,
                    "texture_dictionary_reopen_failed",
                    Some("embeddedTextures".into()),
                    error.to_string(),
                );
                return YdrValidationReport {
                    schema: "ragelab.ydr.validation",
                    schema_version: 1,
                    source_name: source_name.to_owned(),
                    source_sha256,
                    source_byte_length: source_bytes.len(),
                    strict,
                    valid: false,
                    container_valid: true,
                    format_valid: true,
                    semantic_reopen: inspection.semantic_reopen,
                    runtime_renderable: false,
                    codewalker_gui_accepted: false,
                    inspection: Some(inspection),
                    diagnostics,
                };
            }
        };
        if let Some(textures) = document.embedded_textures.as_ref() {
            for texture in textures.textures() {
                let mut decoded = 0usize;
                if texture.width == 0 || texture.height == 0 || texture.levels == 0 {
                    push_diagnostic(
                        &mut diagnostics,
                        YdrDiagnosticSeverity::Error,
                        "invalid_texture_dimensions",
                        Some(format!("embeddedTextures.textures[{}]", texture.index)),
                        "texture width, height and mip level count must be positive",
                    );
                }
                for mip_index in 0..texture.levels as usize {
                    match textures.decode_mip(texture.index, mip_index) {
                        Ok(_) => decoded += 1,
                        Err(error) => {
                            let severity = if matches!(error, YtdError::UnsupportedTextureFormat(_))
                            {
                                YdrDiagnosticSeverity::Unsupported
                            } else {
                                YdrDiagnosticSeverity::Error
                            };
                            push_diagnostic(
                                &mut diagnostics,
                                severity,
                                if severity == YdrDiagnosticSeverity::Unsupported {
                                    "unsupported_texture_format"
                                } else {
                                    "invalid_texture_mip"
                                },
                                Some(format!(
                                    "embeddedTextures.textures[{}].mips[{mip_index}]",
                                    texture.index
                                )),
                                error.to_string(),
                            );
                            break;
                        }
                    }
                }
                texture_reports.push(to_texture_report(texture, decoded));
            }
        }
    }

    if let Some(dictionary) = inspection.embedded_textures.as_mut() {
        dictionary.textures = texture_reports;
    }

    if inspection.shaders.is_empty() {
        push_diagnostic(
            &mut diagnostics,
            YdrDiagnosticSeverity::Warning,
            "empty_shader_group",
            Some("shaders".into()),
            "drawable has no ShaderFX entries; geometry structure is inspectable but material rendering is not established",
        );
    }

    if strict {
        for diagnostic in &mut diagnostics {
            if diagnostic.severity == YdrDiagnosticSeverity::Warning {
                diagnostic.promoted_from = Some(YdrDiagnosticSeverity::Warning);
                diagnostic.severity = YdrDiagnosticSeverity::Error;
            }
        }
    }

    let has_errors = diagnostics
        .iter()
        .any(|item| item.severity == YdrDiagnosticSeverity::Error);
    let has_unsupported = diagnostics
        .iter()
        .any(|item| item.severity == YdrDiagnosticSeverity::Unsupported);
    let valid = !has_errors && !has_unsupported;
    YdrValidationReport {
        schema: "ragelab.ydr.validation",
        schema_version: 1,
        source_name: source_name.to_owned(),
        source_sha256,
        source_byte_length: source_bytes.len(),
        strict,
        valid,
        container_valid: true,
        format_valid: true,
        semantic_reopen: inspection.semantic_reopen,
        runtime_renderable: false,
        codewalker_gui_accepted: false,
        inspection: Some(inspection),
        diagnostics,
    }
}

fn push_diagnostic(
    diagnostics: &mut Vec<YdrDiagnostic>,
    severity: YdrDiagnosticSeverity,
    code: &str,
    path: Option<String>,
    message: impl Into<String>,
) {
    diagnostics.push(YdrDiagnostic {
        severity,
        code: code.to_owned(),
        path,
        message: message.into(),
        promoted_from: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/simple.ydr");
    const EMBEDDED_YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/embedded.ydr");

    #[test]
    fn inspection_reports_lod_geometry_and_semantic_reopen() {
        let report = inspect_ydr_bytes(SIMPLE_YDR, "simple.ydr")
            .expect("synthetic YDR should be inspectable");

        assert_eq!(report.schema, "ragelab.ydr.inspection");
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.container.resource_version, 165);
        assert_eq!(report.drawable.geometry_count, 1);
        assert_eq!(report.drawable.vertex_count, 3);
        assert_eq!(report.lods.len(), 1);
        assert_eq!(report.lods[0].level, "high");
        assert_eq!(report.lods[0].geometries[0].index_count, 3);
        assert!(report.structural_audit.valid);
        assert!(report.semantic_reopen);
        assert!(!report.runtime_renderable);
    }

    #[test]
    fn validation_decodes_embedded_mips_and_warns_on_texture_hash_mismatch() {
        let report = validate_ydr_bytes(EMBEDDED_YDR, "embedded.ydr", false);

        assert!(report.valid);
        assert!(!report.strict);
        assert!(report.semantic_reopen);
        let dictionary = report
            .inspection
            .as_ref()
            .and_then(|inspection| inspection.embedded_textures.as_ref())
            .expect("embedded dictionary should be reported");
        assert_eq!(dictionary.texture_count, 1);
        assert_eq!(dictionary.textures[0].decoded_mip_levels, 1);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "texture_name_hash_mismatch"
                && diagnostic.severity == YdrDiagnosticSeverity::Warning
        }));
    }

    #[test]
    fn strict_validation_promotes_warnings_without_claiming_runtime_acceptance() {
        let report = validate_ydr_bytes(SIMPLE_YDR, "simple.ydr", true);

        assert!(!report.valid);
        assert!(report.strict);
        assert!(report.semantic_reopen);
        assert!(!report.runtime_renderable);
        assert!(!report.codewalker_gui_accepted);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "geometry_without_shader"
                && diagnostic.severity == YdrDiagnosticSeverity::Error
                && diagnostic.promoted_from == Some(YdrDiagnosticSeverity::Warning)
        }));
    }

    #[test]
    fn malformed_rsc7_returns_structured_validation_diagnostics() {
        let report = validate_ydr_bytes(b"not an RSC7 file", "invalid.ydr", false);

        assert!(!report.valid);
        assert!(!report.container_valid);
        assert!(!report.format_valid);
        assert!(!report.semantic_reopen);
        assert!(report.inspection.is_none());
        assert!(!report.diagnostics.is_empty());
    }
}
