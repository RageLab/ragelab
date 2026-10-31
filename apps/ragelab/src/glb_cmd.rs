use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use ragelab_gltf::{
    decode_glb_geometry, decode_glb_images, default_authoring_profile, inspect_glb,
    inspect_glb_file, validate_authoring_profile, GlbInspectionReport, GtaAuthoringProfile,
    MaterialAuthoringSettings, ShaderIntent, SurfaceMode, TextureBindingInfo, TextureEncoding,
};
use ragelab_ydr::{
    build_ydr_with_embedded_textures, build_ydr_with_embedded_textures_and_shaders, ShaderInfo,
    ShaderTextureReference, YdrDocument, YdrGeometryInput, YdrTextureInput,
};
use ragelab_yft::YftDocument;
use ragelab_ytd::TextureFormat;
use serde_json::json;

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

pub fn inspect(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let (source, json_output) = parse_inspect_args(args)?;
    let report = inspect_glb_file(&source).map_err(glb_validation_error)?;
    if json_output {
        super::agent::print_success("glb.inspect", serde_json::to_value(&report)?)?;
    } else {
        print_inspection(&source, &report);
    }
    Ok(())
}

pub fn geometry(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let (source, json_output) = parse_inspect_args(args)?;
    let bytes = fs::read(&source)?;
    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb");
    let report =
        ragelab_gltf::decode_glb_geometry(&bytes, file_name).map_err(glb_validation_error)?;

    if json_output {
        super::agent::print_success("glb.geometry", serde_json::to_value(&report)?)?;
    } else {
        println!("file: {}", source.display());
        println!("sha256: {}", report.source_sha256);
        println!(
            "scene: {}",
            report
                .selected_scene
                .map_or_else(|| "none".into(), |value| value.to_string())
        );
        println!("decoded primitives: {}", report.primitives.len());
        for primitive in &report.primitives {
            println!(
                "  node {} / mesh {} / primitive {}: {} vertices, {} triangle indices, material {}",
                primitive.node_name.as_deref().unwrap_or("<unnamed>"),
                primitive.mesh_name.as_deref().unwrap_or("<unnamed>"),
                primitive.primitive_index,
                primitive.vertex_count,
                primitive.index_count,
                primitive
                    .material_index
                    .map_or_else(|| "<default>".into(), |value| value.to_string())
            );
            println!(
                "    normals={} tangents={} uv0={} uv1={} colors0={}",
                primitive.normals.is_some(),
                primitive.tangents.is_some(),
                primitive.texcoord_0.is_some(),
                primitive.texcoord_1.is_some(),
                primitive.colors_0.is_some()
            );
        }
        for warning in &report.warnings {
            println!("warning: {warning}");
        }
    }
    Ok(())
}

pub fn export(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str = "usage: ragelab glb export <source.glb> --output <output.ydr> [--profile <profile.json>] [--shader-template <source.yft>] [--shader-index <n>] [--overwrite] [--json]";
    let mut source = None;
    let mut output = None;
    let mut profile_path = None;
    let mut shader_template_path = None;
    let mut shader_index = 0usize;
    let mut overwrite = false;
    let mut json_output = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().ok_or_else(|| invalid_input(USAGE))?,
                ));
            }
            "--profile" => {
                profile_path = Some(PathBuf::from(
                    args.next().ok_or_else(|| invalid_input(USAGE))?,
                ));
            }
            "--shader-template" => {
                shader_template_path = Some(PathBuf::from(
                    args.next().ok_or_else(|| invalid_input(USAGE))?,
                ));
            }
            "--shader-index" => {
                shader_index = args
                    .next()
                    .ok_or_else(|| invalid_input(USAGE))?
                    .parse::<usize>()
                    .map_err(|_| invalid_input("--shader-index must be a non-negative integer"))?;
            }
            "--overwrite" => overwrite = true,
            "--json" => json_output = true,
            value if value.starts_with('-') => {
                return Err(invalid_input(format!("unknown option {value}\n{USAGE}")).into())
            }
            value => {
                if source.replace(PathBuf::from(value)).is_some() {
                    return Err(invalid_input(USAGE).into());
                }
            }
        }
    }
    let source = source.ok_or_else(|| invalid_input(USAGE))?;
    let output = output.ok_or_else(|| invalid_input(USAGE))?;
    ensure_distinct_paths(&source, &output)?;
    if let Some(profile_path) = &profile_path {
        ensure_distinct_paths(profile_path, &output)?;
    }
    if let Some(template_path) = &shader_template_path {
        ensure_distinct_paths(template_path, &output)?;
    } else if shader_index != 0 {
        return Err(invalid_input("--shader-index requires --shader-template").into());
    }

    let source_bytes = fs::read(&source)?;
    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb");
    let inspection = inspect_glb(&source_bytes, file_name).map_err(glb_validation_error)?;
    if !inspection.unsupported_required_extensions.is_empty() {
        return Err(glb_validation_error(format!(
            "required glTF extensions block structural export: {}",
            inspection.unsupported_required_extensions.join(", ")
        ))
        .into());
    }

    let profile = if let Some(path) = &profile_path {
        let bytes = fs::read(path)?;
        serde_json::from_slice::<GtaAuthoringProfile>(&bytes)
            .map_err(|error| invalid_data(format!("cannot parse profile JSON: {error}")))?
    } else {
        default_authoring_profile(&inspection)
    };
    let profile_validation =
        validate_authoring_profile(&inspection, &profile).map_err(glb_validation_error)?;
    let shader_template = if let Some(template_path) = &shader_template_path {
        let template_bytes = fs::read(template_path)?;
        let extension = template_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let shaders = match extension.as_str() {
            "yft" => {
                let template_yft = YftDocument::from_bytes(&template_bytes).map_err(|error| {
                    invalid_data(format!("cannot parse Legacy YFT shader template: {error}"))
                })?;
                let drawable = template_yft
                    .main_drawable
                    .ok_or_else(|| invalid_data("shader template YFT has no main Drawable"))?;
                drawable.model.shaders.clone()
            }
            "ydr" => {
                let template_ydr = YdrDocument::from_bytes(&template_bytes).map_err(|error| {
                    invalid_data(format!("cannot parse Legacy YDR shader template: {error}"))
                })?;
                template_ydr.model.shaders
            }
            _ => {
                return Err(invalid_input(
                    "--shader-template must reference a Legacy .yft or .ydr resource",
                )
                .into());
            }
        };
        let shader = shaders.get(shader_index).cloned().ok_or_else(|| {
            invalid_input(format!(
                "shader index {shader_index} does not exist in template resource ({} shaders)",
                shaders.len()
            ))
        })?;
        let template_kind = validate_supported_shader_template(&shader)?;
        Some((shader, template_kind))
    } else {
        None
    };
    if !profile.settings.embed_textures {
        return Err(invalid_input(
            "structural YDR export requires profile setting embedTextures=true",
        )
        .into());
    }
    if !profile.settings.generate_mipmaps {
        return Err(
            invalid_input("structural YDR export currently requires generateMipmaps=true").into(),
        );
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
                ))
                .into());
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
                ))
                .into());
            }
        }
    }

    let decoded_geometry =
        decode_glb_geometry(&source_bytes, file_name).map_err(glb_validation_error)?;
    if decoded_geometry.primitives.is_empty() {
        return Err(invalid_input("selected GLB scene contains no exportable geometry").into());
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
                    ))
                    .into());
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
            ))
            .into());
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

    let decoded_images = decode_glb_images(&source_bytes).map_err(glb_validation_error)?;
    if decoded_images.len() != inspection.images.len() {
        return Err(
            invalid_data("embedded image decoder count does not match GLB inspection").into(),
        );
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
                ))
                .into());
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

    let drawable_name = drawable_name(&source);
    let ydr_bytes = if shaders.is_empty() {
        build_ydr_with_embedded_textures(&drawable_name, &geometries, &textures)?
    } else {
        build_ydr_with_embedded_textures_and_shaders(
            &drawable_name,
            &geometries,
            &textures,
            &shaders,
        )?
    };
    write_output(&output, &ydr_bytes, overwrite)?;

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
        .fold(BTreeMap::<&str, usize>::new(), |mut counts, format| {
            *counts.entry(format).or_default() += 1;
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
    let shader_template_report = shader_template_path.as_ref().map(|path| {
        json!({
            "source": path,
            "index": shader_index,
            "preset": shader_template.as_ref().map(|(_, kind)| kind.preset_name()),
            "nameHash": shader_template.as_ref().map(|(shader, _)| format!("0x{:08X}", shader.name_hash)),
            "fileHash": shader_template.as_ref().map(|(shader, _)| format!("0x{:08X}", shader.file_hash))
        })
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
            format!("ShaderFX preset {preset} was cloned from a Legacy YFT template"),
            sampler_note.to_owned(),
            "baseColorFactor, metallic, roughness, emissive, alpha and other PBR/surface semantics are not generalized by this template path".to_owned(),
            "output has not been accepted by GTA V runtime or CodeWalker".to_owned(),
        ]
    };

    if json_output {
        super::agent::print_success(
            "glb.export",
            json!({
                "schema": "ragelab.glb.ydr-export",
                "schemaVersion": 1,
                "source": source,
                "output": output,
                "sourceSha256": inspection.source.sha256,
                "ydrVersion": 165,
                "geometryCount": geometries.len(),
                "vertexCount": vertex_count,
                "triangleCount": triangle_count,
                "embeddedTextureCount": textures.len(),
                "textureFormats": texture_formats,
                "profileValidated": profile_validation.valid,
                "materialCount": profile.materials.len(),
                "shaderGroup": if shaders.is_empty() { if textures.is_empty() { "not_emitted" } else { "empty" } } else { "populated" },
                "shaderCount": shaders.len(),
                "shaderTemplate": shader_template_report,
                "materialShaderBindings": material_shader_binding_count,
                "geometryShaderMappings": geometry_shader_mapping_count,
                "shaderTextureBindings": shader_texture_binding_count,
                "runtimeRenderable": false,
                "limitations": limitations
            }),
        )?;
    } else {
        println!("source: {}", source.display());
        println!("output: {}", output.display());
        println!("YDR version: 165");
        println!("geometries: {}", geometries.len());
        println!("vertices: {vertex_count}");
        println!("triangles: {triangle_count}");
        println!("embedded textures: {}", textures.len());
        println!("texture formats: {texture_formats:?}");
        println!("material profile: validated");
        println!("ShaderFX templates: {}", shaders.len());
        println!("material shader bindings: {material_shader_binding_count}");
        println!("geometry shader mappings: {geometry_shader_mapping_count}");
        println!("sampler texture bindings: {shader_texture_binding_count}");
        println!("status: Legacy ShaderFX cloned/bound structurally; GTA runtime rendering is not yet established");
    }
    Ok(())
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
) -> Result<ShaderInfo, Box<dyn Error>> {
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
) -> Result<ShaderInfo, Box<dyn Error>> {
    if material.shader_intent != ShaderIntent::Lit {
        return Err(invalid_input(format!(
            "material {} requests {:?}; the verified normal.sps template supports lit surfaces only",
            material.source_material_index, material.shader_intent
        ))
        .into());
    }
    if material.surface_mode != SurfaceMode::Opaque {
        return Err(invalid_input(format!(
            "material {} requests {:?}; the verified normal.sps template currently supports opaque surfaces only",
            material.source_material_index, material.surface_mode
        ))
        .into());
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
        .into());
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
        .into());
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
        .into());
    }
    if material.texture_roles.specular.is_some() && material.texture_roles.specular_color.is_some()
    {
        return Err(invalid_input(format!(
            "material {} assigns both specular and specularColor to the same SpecSampler",
            material.source_material_index
        ))
        .into());
    }

    let resolve = |binding: Option<&TextureBindingInfo>| -> Result<Option<String>, Box<dyn Error>> {
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
            ))
            .into());
        }
        let bumpiness = shader
            .parameters
            .iter_mut()
            .find(|parameter| parameter.parameter_hash == 0xF671_2B81)
            .ok_or_else(|| invalid_data("normal.sps template has no bumpiness parameter"))?;
        if bumpiness.data_type != 1 || bumpiness.vector_data.len() != 1 {
            return Err(
                invalid_data("normal.sps bumpiness parameter has an unsupported layout").into(),
            );
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
) -> Result<ShaderInfo, Box<dyn Error>> {
    if material.shader_intent != ShaderIntent::Lit {
        return Err(invalid_input(format!(
            "material {} requests {:?}; the verified spec.sps template supports lit surfaces only",
            material.source_material_index, material.shader_intent
        ))
        .into());
    }
    if material.surface_mode != SurfaceMode::Opaque {
        return Err(invalid_input(format!(
            "material {} requests {:?}; the verified spec.sps template is currently restricted to opaque profile inputs",
            material.source_material_index, material.surface_mode
        ))
        .into());
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
        .into());
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
        .into());
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
        .into());
    }

    let resolve = |binding: Option<&TextureBindingInfo>| -> Result<Option<String>, Box<dyn Error>> {
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
) -> Result<ShaderInfo, Box<dyn Error>> {
    if material.shader_intent != ShaderIntent::Lit {
        return Err(invalid_input(format!(
            "material {} requests {:?}; normal_spec_tnt.sps is restricted to lit surfaces",
            material.source_material_index, material.shader_intent
        ))
        .into());
    }
    if material.surface_mode != SurfaceMode::Opaque {
        return Err(invalid_input(format!(
            "material {} requests {:?}; normal_spec_tnt.sps is restricted to opaque surfaces",
            material.source_material_index, material.surface_mode
        ))
        .into());
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
        .into());
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
        .into());
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
        .into());
    }

    let resolve = |binding: Option<&TextureBindingInfo>| -> Result<Option<String>, Box<dyn Error>> {
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
            ))
            .into());
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
            )
            .into());
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
) -> Result<ShaderInfo, Box<dyn Error>> {
    if material.shader_intent != ShaderIntent::Emissive {
        return Err(invalid_input(format!(
            "material {} requests {:?}; {preset_name} requires shaderIntent=emissive",
            material.source_material_index, material.shader_intent
        ))
        .into());
    }
    if material.surface_mode != SurfaceMode::Opaque {
        return Err(invalid_input(format!(
            "material {} requests {:?}; {preset_name} is restricted to opaque surfaces",
            material.source_material_index, material.surface_mode
        ))
        .into());
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
        .into());
    }
    if material.parameters.base_color_factor != [1.0; 4]
        || material.parameters.metallic != 0.0
        || material.parameters.roughness != 1.0
    {
        return Err(invalid_input(format!(
            "material {} has baseColorFactor, metallic or roughness inputs not represented by {preset_name}; use white baseColorFactor, metallic=0 and roughness=1",
            material.source_material_index
        ))
        .into());
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
        .into());
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
        .into());
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
            .into());
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
        ))
        .into());
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
        ))
        .into());
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

pub fn profile(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let mut args = args.into_iter();
    let action = args
        .next()
        .ok_or_else(|| invalid_input("usage: ragelab glb profile <init|validate> ..."))?;
    match action.as_str() {
        "init" => profile_init(args.collect()),
        "validate" => profile_validate(args.collect()),
        other => Err(invalid_input(format!("unknown glb profile action: {other}")).into()),
    }
}

fn profile_init(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str =
        "usage: ragelab glb profile init <source.glb> --output <profile.json> [--overwrite] [--json]";
    let mut source = None;
    let mut output = None;
    let mut overwrite = false;
    let mut json_output = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().ok_or_else(|| invalid_input(USAGE))?,
                ))
            }
            "--overwrite" => overwrite = true,
            "--json" => json_output = true,
            value if value.starts_with('-') => {
                return Err(invalid_input(format!("unknown option {value}\n{USAGE}")).into())
            }
            value => {
                if source.replace(PathBuf::from(value)).is_some() {
                    return Err(invalid_input(USAGE).into());
                }
            }
        }
    }
    let source = source.ok_or_else(|| invalid_input(USAGE))?;
    let output = output.ok_or_else(|| invalid_input(USAGE))?;
    ensure_distinct_paths(&source, &output)?;

    let report = inspect_glb_file(&source).map_err(glb_validation_error)?;
    let profile = default_authoring_profile(&report);
    let body = serde_json::to_vec_pretty(&profile)?;
    write_profile(&output, &body, overwrite)?;

    if json_output {
        super::agent::print_success(
            "glb.profile.init",
            json!({
                "source": source,
                "profile": output,
                "materialCount": profile.materials.len(),
                "textureCount": profile.source.texture_count,
                "embedTextures": profile.settings.embed_textures,
                "sourceSha256": profile.source.sha256,
                "nextStep": "edit the profile JSON, then run glb profile validate"
            }),
        )?;
    } else {
        println!("profile: {}", output.display());
        println!("source: {}", source.display());
        println!("materials: {}", profile.materials.len());
        println!("textures: {}", profile.source.texture_count);
        println!("embedded textures: {}", profile.settings.embed_textures);
        println!("next: edit the profile JSON and run glb profile validate");
    }
    Ok(())
}

fn profile_validate(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str = "usage: ragelab glb profile validate <source.glb> <profile.json> [--json]";
    let mut positional = Vec::new();
    let mut json_output = false;
    for arg in args {
        if arg == "--json" {
            json_output = true;
        } else if arg.starts_with('-') {
            return Err(invalid_input(format!("unknown option {arg}\n{USAGE}")).into());
        } else {
            positional.push(PathBuf::from(arg));
        }
    }
    if positional.len() != 2 {
        return Err(invalid_input(USAGE).into());
    }
    let report = inspect_glb_file(&positional[0]).map_err(glb_validation_error)?;
    let profile_bytes = fs::read(&positional[1])?;
    let profile: GtaAuthoringProfile = serde_json::from_slice(&profile_bytes)
        .map_err(|error| invalid_data(format!("cannot parse profile JSON: {error}")))?;
    let validation = validate_authoring_profile(&report, &profile).map_err(glb_validation_error)?;
    if json_output {
        super::agent::print_success("glb.profile.validate", serde_json::to_value(&validation)?)?;
    } else {
        println!("valid: {}", validation.valid);
        println!("materials: {}", validation.material_count);
        println!("mapped texture slots: {}", validation.mapped_texture_slots);
        println!("embedded textures requested: {}", validation.embed_textures);
        for note in validation.notes {
            println!("note: {note}");
        }
    }
    Ok(())
}

fn parse_inspect_args(args: Vec<String>) -> Result<(PathBuf, bool), io::Error> {
    const USAGE: &str = "usage: ragelab glb inspect <file.glb> [--json]";
    let mut source = None;
    let mut json_output = false;
    for arg in args {
        if arg == "--json" {
            json_output = true;
        } else if arg.starts_with('-') {
            return Err(invalid_input(format!("unknown option {arg}\n{USAGE}")));
        } else if source.replace(PathBuf::from(arg)).is_some() {
            return Err(invalid_input(USAGE));
        }
    }
    Ok((source.ok_or_else(|| invalid_input(USAGE))?, json_output))
}

fn print_inspection(path: &Path, report: &GlbInspectionReport) {
    println!("file: {}", path.display());
    println!("format: GLB / glTF {}", report.asset.version);
    println!("bytes: {}", report.source.byte_length);
    println!("sha256: {}", report.source.sha256);
    println!(
        "scene: {}",
        report
            .default_scene
            .map_or_else(|| "none".into(), |v| v.to_string())
    );
    println!(
        "counts: {} nodes, {} meshes, {} primitives, {} materials, {} textures, {} images",
        report.counts.nodes,
        report.counts.meshes,
        report.counts.primitives,
        report.counts.materials,
        report.counts.textures,
        report.counts.images
    );

    println!("\nMeshes:");
    for mesh in &report.meshes {
        println!(
            "  [{}] {} ({} primitives)",
            mesh.index,
            mesh.name.as_deref().unwrap_or("<unnamed>"),
            mesh.primitives.len()
        );
        for primitive in &mesh.primitives {
            let positions = primitive
                .attributes
                .get("POSITION")
                .map_or_else(|| "no POSITION".into(), |a| format!("{} vertices", a.count));
            println!(
                "    primitive {}: {}, {}, material {}",
                primitive.index,
                primitive.mode_name,
                positions,
                primitive
                    .material_index
                    .map_or_else(|| "<default>".into(), |v| v.to_string())
            );
            println!(
                "      attributes: {}",
                primitive
                    .attributes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    println!("\nMaterials:");
    for material in &report.materials {
        println!(
            "  [{}] {} | alpha={} | roughness={:.3} | metallic={:.3} | doubleSided={}",
            material.index,
            material.name.as_deref().unwrap_or("<unnamed>"),
            material.alpha_mode,
            material.roughness_factor,
            material.metallic_factor,
            material.double_sided
        );
        print_binding("albedo", material.base_color_texture.as_ref());
        print_binding("normal", material.normal_texture.as_ref());
        print_binding(
            "metallic/roughness",
            material.metallic_roughness_texture.as_ref(),
        );
        print_binding("specular", material.specular_texture.as_ref());
        print_binding("specular color", material.specular_color_texture.as_ref());
        print_binding("occlusion", material.occlusion_texture.as_ref());
        print_binding("emissive", material.emissive_texture.as_ref());
        if material.transmission_factor.unwrap_or(0.0) > 0.0 {
            println!(
                "      transmission: {:.3}",
                material.transmission_factor.unwrap_or(0.0)
            );
        }
        if let Some(ior) = material.ior {
            println!("      IOR: {:.3}", ior);
        }
    }

    println!("\nTextures:");
    for texture in &report.textures {
        let image = texture
            .source_image_index
            .and_then(|index| report.images.get(index));
        println!(
            "  [{}] {} -> image {}",
            texture.index,
            texture.name.as_deref().unwrap_or("<unnamed>"),
            texture
                .source_image_index
                .map_or_else(|| "<extension/unknown>".into(), |v| v.to_string())
        );
        if let Some(image) = image {
            println!(
                "      {} | {} | {}",
                image.mime_type.as_deref().unwrap_or("unknown MIME"),
                image.storage,
                image.dimensions.map_or_else(
                    || "dimensions unknown".into(),
                    |d| format!("{}x{}", d.width, d.height)
                )
            );
        }
    }
    println!("\nImages:");
    for image in &report.images {
        println!(
            "  [{}] {} | {} | {} bytes | embedded={}",
            image.index,
            image.name.as_deref().unwrap_or("<unnamed>"),
            image.mime_type.as_deref().unwrap_or("unknown MIME"),
            image
                .byte_length
                .map_or_else(|| "unknown".into(), |v| v.to_string()),
            image.embedded
        );
    }
    for extension in &report.extensions_required {
        println!("required extension: {extension}");
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
}

fn print_binding(label: &str, binding: Option<&TextureBindingInfo>) {
    if let Some(binding) = binding {
        println!(
            "      {label}: texture {} (UV set {})",
            binding.texture_index, binding.tex_coord
        );
    }
}

fn ensure_distinct_paths(source: &Path, output: &Path) -> Result<(), io::Error> {
    let source = source.canonicalize()?;
    let output_abs = if output.exists() {
        output.canonicalize()?
    } else {
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = output
            .file_name()
            .ok_or_else(|| invalid_input("output must name a file"))?;
        parent.canonicalize()?.join(name)
    };
    if source == output_abs {
        return Err(invalid_input("output must not overwrite the input file"));
    }
    Ok(())
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

fn drawable_name(source: &Path) -> String {
    let stem = source
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

fn write_output(path: &Path, body: &[u8], overwrite: bool) -> Result<(), io::Error> {
    let mut options = OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path)?;
    file.write_all(body)?;
    file.flush()
}

fn write_profile(path: &Path, body: &[u8], overwrite: bool) -> Result<(), io::Error> {
    let mut options = OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path)?;
    file.write_all(body)?;
    file.write_all(b"\n")?;
    file.flush()
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
