use std::{
    io::{self, Cursor},
    path::Path,
};

use png::{BitDepth, ColorType, Decoder, Encoder, Transformations};
use ragelab_ydd::{YddDictionary, YddEditSession};
use ragelab_ydr::{EditableShaderBinding, EditableTextureBinding, YdrDocument, YdrEditSession};
use ragelab_ytd::{DecodedTexture, TextureInfo, Ytd};
use serde::Serialize;

pub const TEXTURE_AUTHORING_SCHEMA: &str = "ragelab.texture-authoring";
pub const TEXTURE_AUTHORING_SCHEMA_VERSION: u64 = 1;
pub const MATERIAL_AUTHORING_SCHEMA: &str = "ragelab.material-authoring";
pub const MATERIAL_AUTHORING_SCHEMA_VERSION: u64 = 1;
const MAX_PORTABLE_RGBA_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureExportCapabilities {
    pub png_top_mip: bool,
    pub classic_dds_full_mips: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureReplacementCapabilities {
    pub layout_preserving_dds: bool,
    pub relocated_dds: bool,
    pub png: bool,
    pub target_format_preserved: bool,
    pub dimension_changes: bool,
    pub mip_chain_regenerated: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureAuthoringEntry {
    pub index: usize,
    pub name: String,
    pub dictionary_hash: String,
    pub name_hash: String,
    pub dictionary_hash_matches_name: bool,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub stride: u16,
    pub format: String,
    pub format_raw: String,
    pub mip_levels: u8,
    pub usage: u8,
    pub usage_flags: String,
    pub extra_flags: String,
    pub encoded_bytes: usize,
    pub export: TextureExportCapabilities,
    pub replacement: TextureReplacementCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureAuthoringReport {
    pub schema: &'static str,
    pub schema_version: u64,
    pub texture_count: usize,
    pub textures: Vec<TextureAuthoringEntry>,
    pub rules: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialTextureReferenceReport {
    pub parameter_hash: String,
    pub texture_name: Option<String>,
    pub mode: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialShaderReport {
    pub index: usize,
    pub name_hash: String,
    pub file_hash: String,
    pub texture_references: Vec<MaterialTextureReferenceReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialTextureBindingReport {
    pub shader_index: usize,
    pub parameter_index: usize,
    pub parameter_hash: String,
    pub texture_name: Option<String>,
    pub mode: &'static str,
    pub target_scope: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialShaderBindingReport {
    pub model_index: usize,
    pub geometry_index: usize,
    pub shader_index: u16,
    pub mode: &'static str,
    pub target_scope: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialDrawableReport {
    pub drawable_index: Option<usize>,
    pub drawable_name_hash: Option<String>,
    pub drawable_name: Option<String>,
    pub shaders: Vec<MaterialShaderReport>,
    pub texture_bindings: Vec<MaterialTextureBindingReport>,
    pub shader_bindings: Vec<MaterialShaderBindingReport>,
    pub edit_session_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialFieldPolicy {
    pub field: &'static str,
    pub mode: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialAuthoringReport {
    pub schema: &'static str,
    pub schema_version: u64,
    pub asset_type: &'static str,
    pub drawables: Vec<MaterialDrawableReport>,
    pub field_policy: Vec<MaterialFieldPolicy>,
}

pub fn ytd_texture_authoring_report(bytes: &[u8]) -> io::Result<TextureAuthoringReport> {
    let ytd = Ytd::from_bytes(bytes).map_err(core_error)?;
    let textures = ytd
        .textures
        .iter()
        .enumerate()
        .map(|(index, texture)| texture_entry(index, texture))
        .collect();
    Ok(TextureAuthoringReport {
        schema: TEXTURE_AUTHORING_SCHEMA,
        schema_version: TEXTURE_AUTHORING_SCHEMA_VERSION,
        texture_count: ytd.textures.len(),
        textures,
        rules: vec![
            "installed GTA/RPF providers remain read-only",
            "all writes create an explicit caller-selected output",
            "PNG replacement decodes only finite 2D pixels and preserves the target Legacy format",
            "PNG replacement is limited to RGBA8, BC1 and BC3 targets; a complete mip chain is regenerated",
            "layout-preserving DDS replacement requires exact dimensions, format, mip count and encoded byte length",
            "relocated DDS replacement may change dimensions/mips but must preserve the target Legacy format",
            "every supported write must semantic-reopen as a YTD before success",
        ],
    })
}

pub fn export_ytd_texture_dds(bytes: &[u8], index: usize) -> io::Result<Vec<u8>> {
    Ytd::texture_dds(bytes, index).map_err(core_error)
}

pub fn export_ytd_texture_png(bytes: &[u8], index: usize) -> io::Result<Vec<u8>> {
    let decoded = Ytd::decode_top_mip_rgba(bytes, index).map_err(core_error)?;
    encode_rgba_png(&decoded)
}

pub fn replace_ytd_texture_from_png(
    bytes: &[u8],
    index: usize,
    png_bytes: &[u8],
) -> io::Result<Vec<u8>> {
    let before = Ytd::from_bytes(bytes).map_err(core_error)?;
    let target = before.textures.get(index).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "texture index {index} is out of bounds for YTD with {} textures",
                before.textures.len()
            ),
        )
    })?;
    if target.depth != 1 || !target.format.supports_rgba_repack() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "PNG replacement supports only 2D RGBA8/BC1/BC3 targets; texture {index} is depth {} format {}",
                target.depth,
                target.format.normalized_name()
            ),
        ));
    }

    let decoded = decode_png_rgba8(png_bytes)?;
    let output = Ytd::replace_texture_from_rgba_relocated(
        bytes,
        index,
        decoded.width,
        decoded.height,
        &decoded.rgba,
    )
    .map_err(core_error)?;
    let after = Ytd::from_bytes(&output).map_err(core_error)?;
    verify_png_replacement(&before, &after, index)?;
    Ok(output)
}

pub fn ydr_material_authoring_report(bytes: &[u8]) -> io::Result<MaterialAuthoringReport> {
    let document = YdrDocument::from_bytes(bytes).map_err(core_error)?;
    let (texture_bindings, shader_bindings, edit_session_error) =
        match YdrEditSession::from_bytes(bytes) {
            Ok(session) => (
                session.texture_bindings().to_vec(),
                session.shader_bindings().to_vec(),
                None,
            ),
            Err(error) => (Vec::new(), Vec::new(), Some(error.to_string())),
        };
    Ok(MaterialAuthoringReport {
        schema: MATERIAL_AUTHORING_SCHEMA,
        schema_version: MATERIAL_AUTHORING_SCHEMA_VERSION,
        asset_type: "YDR",
        drawables: vec![material_drawable(
            None,
            None,
            document.model.name.clone(),
            &document,
            &texture_bindings,
            &shader_bindings,
            edit_session_error,
        )],
        field_policy: material_field_policy(),
    })
}

pub fn ydd_material_authoring_report(bytes: &[u8]) -> io::Result<MaterialAuthoringReport> {
    let dictionary = YddDictionary::from_bytes(bytes).map_err(core_error)?;
    let mut drawables = Vec::with_capacity(dictionary.entries().len());
    for entry in dictionary.entries() {
        let document = dictionary.document(entry.index).map_err(core_error)?;
        let (texture_bindings, shader_bindings, edit_session_error) =
            match YddEditSession::from_bytes(bytes, entry.index) {
                Ok(session) => (
                    session.texture_bindings().to_vec(),
                    session.shader_bindings().to_vec(),
                    None,
                ),
                Err(error) => (Vec::new(), Vec::new(), Some(error.to_string())),
            };
        drawables.push(material_drawable(
            Some(entry.index),
            Some(format!("0x{:08X}", entry.name_hash)),
            entry.name.clone(),
            &document,
            &texture_bindings,
            &shader_bindings,
            edit_session_error,
        ));
    }

    Ok(MaterialAuthoringReport {
        schema: MATERIAL_AUTHORING_SCHEMA,
        schema_version: MATERIAL_AUTHORING_SCHEMA_VERSION,
        asset_type: "YDD",
        drawables,
        field_policy: material_field_policy(),
    })
}

pub fn write_png_replacement_to_explicit_output(
    source: &Path,
    output: &Path,
    texture_index: usize,
    png_bytes: &[u8],
) -> io::Result<usize> {
    if !source.is_absolute() || !output.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "YTD source and output must be absolute paths",
        ));
    }
    let source = source.canonicalize()?;
    if output == source {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "YTD authoring output must not overwrite the source",
        ));
    }
    if output.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("YTD authoring output already exists: {}", output.display()),
        ));
    }
    let source_bytes = std::fs::read(&source)?;
    let output_bytes = replace_ytd_texture_from_png(&source_bytes, texture_index, png_bytes)?;
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    use std::io::Write;
    let mut file = options.open(output)?;
    file.write_all(&output_bytes)?;
    file.flush()?;
    let written = std::fs::read(output)?;
    Ytd::from_bytes(&written).map_err(core_error)?;
    if std::fs::read(&source)? != source_bytes {
        return Err(io::Error::other(format!(
            "source changed while writing YTD output: {}",
            source.display()
        )));
    }
    Ok(written.len())
}

fn texture_entry(index: usize, texture: &TextureInfo) -> TextureAuthoringEntry {
    let two_d = texture.depth == 1;
    let dds = two_d && texture.format.supports_classic_dds();
    let png_repack = two_d && texture.format.supports_rgba_repack();
    let png_export = two_d && texture.format.supports_rgba_preview();
    let reason = if !two_d {
        Some(format!(
            "texture depth {} is not supported by 2D authoring paths",
            texture.depth
        ))
    } else if !texture.format.supports_rgba_repack() {
        Some(format!(
            "format {} is inspect/export-only for replacement",
            texture.format.normalized_name()
        ))
    } else {
        None
    };
    TextureAuthoringEntry {
        index,
        name: texture.name.clone(),
        dictionary_hash: format!("0x{:08X}", texture.dictionary_hash),
        name_hash: format!("0x{:08X}", texture.name_hash),
        dictionary_hash_matches_name: texture.dictionary_hash_matches_name(),
        width: texture.width,
        height: texture.height,
        depth: texture.depth,
        stride: texture.stride,
        format: texture.format.normalized_name().into(),
        format_raw: format!("0x{:08X}", texture.format.raw()),
        mip_levels: texture.levels,
        usage: texture.usage,
        usage_flags: format!("0x{:08X}", texture.usage_flags),
        extra_flags: format!("0x{:08X}", texture.extra_flags),
        encoded_bytes: texture.data_length,
        export: TextureExportCapabilities {
            png_top_mip: png_export,
            classic_dds_full_mips: dds,
        },
        replacement: TextureReplacementCapabilities {
            layout_preserving_dds: dds,
            relocated_dds: dds,
            png: png_repack,
            target_format_preserved: png_repack || dds,
            dimension_changes: png_repack || dds,
            mip_chain_regenerated: png_repack,
            reason,
        },
    }
}

fn material_drawable(
    drawable_index: Option<usize>,
    drawable_name_hash: Option<String>,
    drawable_name: Option<String>,
    document: &YdrDocument,
    texture_bindings: &[EditableTextureBinding],
    shader_bindings: &[EditableShaderBinding],
    edit_session_error: Option<String>,
) -> MaterialDrawableReport {
    let shaders = document
        .model
        .shaders
        .iter()
        .enumerate()
        .map(|(index, shader)| MaterialShaderReport {
            index,
            name_hash: format!("0x{:08X}", shader.name_hash),
            file_hash: format!("0x{:08X}", shader.file_hash),
            texture_references: shader
                .texture_references
                .iter()
                .map(|reference| MaterialTextureReferenceReport {
                    parameter_hash: format!("0x{:08X}", reference.parameter_hash),
                    texture_name: reference.texture_name.clone(),
                    mode: "inspectOnly",
                })
                .collect(),
        })
        .collect();
    MaterialDrawableReport {
        drawable_index,
        drawable_name_hash,
        drawable_name,
        shaders,
        texture_bindings: texture_bindings
            .iter()
            .map(|binding| MaterialTextureBindingReport {
                shader_index: binding.key.shader_index,
                parameter_index: binding.key.parameter_index,
                parameter_hash: format!("0x{:08X}", binding.parameter_hash),
                texture_name: binding.texture_name.clone(),
                mode: "rebindExisting",
                target_scope: "sameDrawableExistingTextureBinding",
            })
            .collect(),
        shader_bindings: shader_bindings
            .iter()
            .map(|binding| MaterialShaderBindingReport {
                model_index: binding.key.model_index,
                geometry_index: binding.key.geometry_index,
                shader_index: binding.shader_index,
                mode: "rebindExisting",
                target_scope: "sameDrawableExistingShader",
            })
            .collect(),
        edit_session_error,
    }
}

fn material_field_policy() -> Vec<MaterialFieldPolicy> {
    vec![
        MaterialFieldPolicy {
            field: "shader.nameHash",
            mode: "inspectOnly",
            reason: "shader identity is parsed, but replacing shader definitions is not proven safe",
        },
        MaterialFieldPolicy {
            field: "shader.fileHash",
            mode: "inspectOnly",
            reason: "shader file identity is parsed, but file/parameter semantics are not authored",
        },
        MaterialFieldPolicy {
            field: "shader.textureReference.parameterHash",
            mode: "inspectOnly",
            reason: "parameter hashes are preserved exactly and are never invented or rewritten",
        },
        MaterialFieldPolicy {
            field: "shader.textureBinding",
            mode: "rebindExisting",
            reason: "only texture bindings proven writable by YdrEditSession may point at another existing binding",
        },
        MaterialFieldPolicy {
            field: "geometry.shaderBinding",
            mode: "rebindExisting",
            reason: "geometry may only select another shader already present in the same drawable",
        },
        MaterialFieldPolicy {
            field: "shader.unknownParameters",
            mode: "inspectOnly",
            reason: "unknown shader parameter semantics remain immutable",
        },
    ]
}

fn encode_rgba_png(decoded: &DecodedTexture) -> io::Result<Vec<u8>> {
    let expected = rgba_len(decoded.width, decoded.height)?;
    if decoded.rgba.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "decoded RGBA payload length {} does not match {}x{} length {expected}",
                decoded.rgba.len(),
                decoded.width,
                decoded.height
            ),
        ));
    }
    let mut output = Vec::new();
    {
        let mut encoder = Encoder::new(
            &mut output,
            u32::from(decoded.width),
            u32::from(decoded.height),
        );
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(png_error)?;
        writer.write_image_data(&decoded.rgba).map_err(png_error)?;
    }
    Ok(output)
}

fn decode_png_rgba8(bytes: &[u8]) -> io::Result<DecodedTexture> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(png_error)?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).map_err(png_error)?;
    if info.bit_depth != BitDepth::Eight {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "PNG replacement requires 8-bit channels after palette expansion; got {:?}",
                info.bit_depth
            ),
        ));
    }
    let width = u16::try_from(info.width).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("PNG width {} exceeds Legacy u16 dimensions", info.width),
        )
    })?;
    let height = u16::try_from(info.height).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("PNG height {} exceeds Legacy u16 dimensions", info.height),
        )
    })?;
    if width == 0 || height == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PNG width and height must be greater than zero",
        ));
    }
    let payload = &buffer[..info.buffer_size()];
    let expected = rgba_len(width, height)?;
    let mut rgba = Vec::with_capacity(expected);
    match info.color_type {
        ColorType::Rgba => rgba.extend_from_slice(payload),
        ColorType::Rgb => {
            for pixel in payload.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        ColorType::Grayscale => {
            for &value in payload {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        ColorType::GrayscaleAlpha => {
            for pixel in payload.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        ColorType::Indexed => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "indexed PNG remained indexed after EXPAND transformation",
            ));
        }
    }
    if rgba.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "PNG decoded RGBA payload length {} does not match expected {expected}",
                rgba.len()
            ),
        ));
    }
    Ok(DecodedTexture {
        width,
        height,
        rgba,
    })
}

fn verify_png_replacement(before: &Ytd, after: &Ytd, index: usize) -> io::Result<()> {
    if before.resource_version != after.resource_version
        || before.file_vft != after.file_vft
        || before.file_unknown != after.file_unknown
        || before.textures.len() != after.textures.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PNG replacement changed YTD dictionary-level semantics",
        ));
    }
    for (current, (previous, next)) in before.textures.iter().zip(&after.textures).enumerate() {
        if current == index {
            if !same_texture_identity(next, previous) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "PNG replacement changed target texture identity/usage metadata",
                ));
            }
        } else if next != previous {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("PNG replacement changed unrelated texture metadata at index {current}"),
            ));
        }
    }
    Ok(())
}

fn same_texture_identity(left: &TextureInfo, right: &TextureInfo) -> bool {
    left.dictionary_hash == right.dictionary_hash
        && left.name == right.name
        && left.name_hash == right.name_hash
        && left.depth == right.depth
        && left.format == right.format
        && left.usage == right.usage
        && left.usage_flags == right.usage_flags
        && left.extra_flags == right.extra_flags
}

fn rgba_len(width: u16, height: u16) -> io::Result<usize> {
    let bytes = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "RGBA dimensions overflow"))?;
    if bytes > MAX_PORTABLE_RGBA_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "RGBA payload {bytes} bytes exceeds portable authoring limit {MAX_PORTABLE_RGBA_BYTES}"
            ),
        ));
    }
    Ok(bytes)
}

fn core_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn png_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const YTD: &[u8] = include_bytes!("../../../fixtures/synthetic/ytd/simple.ytd");
    const YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/editable.ydr");
    const YDD: &[u8] = include_bytes!("../../../fixtures/synthetic/ydd/editable.ydd");

    #[test]
    fn standalone_ytd_contract_exports_png_and_replaces_from_png() {
        let report = ytd_texture_authoring_report(YTD).expect("texture report");
        assert_eq!(report.texture_count, 1);
        let texture = &report.textures[0];
        assert!(texture.export.png_top_mip);
        assert!(texture.export.classic_dds_full_mips);
        assert!(texture.replacement.png);
        assert_eq!(texture.format, "RGBA8");

        let png = export_ytd_texture_png(YTD, 0).expect("png export");
        let rewritten = replace_ytd_texture_from_png(YTD, 0, &png).expect("png replacement");
        let reopened = Ytd::from_bytes(&rewritten).expect("semantic reopen");
        assert_eq!(reopened.textures.len(), 1);
        assert_eq!(reopened.textures[0].name, report.textures[0].name);
    }

    #[test]
    fn material_reports_separate_inspect_only_from_proven_rebinds() {
        let ydr = ydr_material_authoring_report(YDR).expect("YDR material report");
        assert_eq!(ydr.asset_type, "YDR");
        assert!(!ydr.drawables[0].shaders.is_empty());
        assert!(ydr
            .field_policy
            .iter()
            .any(|field| field.field == "shader.unknownParameters" && field.mode == "inspectOnly"));
        assert!(ydr.drawables[0]
            .texture_bindings
            .iter()
            .all(|binding| binding.mode == "rebindExisting"));

        let ydd = ydd_material_authoring_report(YDD).expect("YDD material report");
        assert_eq!(ydd.asset_type, "YDD");
        assert!(!ydd.drawables.is_empty());
    }

    #[test]
    fn png_codec_expands_rgb_without_inventing_alpha_semantics() {
        let decoded = DecodedTexture {
            width: 2,
            height: 1,
            rgba: vec![10, 20, 30, 255, 40, 50, 60, 128],
        };
        let png = encode_rgba_png(&decoded).expect("encode");
        let reopened = decode_png_rgba8(&png).expect("decode");
        assert_eq!(reopened, decoded);
    }
}
