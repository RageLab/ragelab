//! Byte-first GLB authoring use cases shared by the CLI and RageLab Studio.
//!
//! This module coordinates the format crates. GLB parsing/decoding remains in
//! ragelab-gltf, YFT template parsing in ragelab-yft, and YDR serialization and
//! semantic reopen in ragelab-ydr. No filesystem or process discovery occurs here.

use std::{collections::BTreeMap, io, path::Path};

use ragelab_gltf::{
    decode_glb_geometry as decode_geometry, decode_glb_images, default_authoring_profile,
    inspect_glb as inspect_document, validate_authoring_profile, MaterialAuthoringSettings,
    ShaderIntent, SurfaceMode, TextureBindingInfo, TextureEncoding,
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
}

#[derive(Debug)]
pub struct GlbYdrExportResult {
    pub output_bytes: Vec<u8>,
    pub report: GlbYdrExportReport,
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
            if binding.tex_coord > 1 {
                return Err(invalid_input(format!(
                    "material {} texture {} uses TEXCOORD_{}, but the structural YDR writer supports UV sets 0 and 1 only",
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

    let decoded_geometry =
        decode_geometry(source_bytes, source_name).map_err(glb_validation_error)?;
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
        schema_version: 1,
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
            "nodes": [{"name": "Triangle", "mesh": 0}],
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
