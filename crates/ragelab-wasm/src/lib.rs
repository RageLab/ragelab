use js_sys::{Float32Array, Uint32Array, Uint8Array};
use ragelab_authoring::{
    export_ytd_texture_png, replace_ytd_texture_from_png, ydd_material_authoring_report,
    ydr_material_authoring_report, ytd_texture_authoring_report,
};
use ragelab_hash::joaat;
use ragelab_meta::MetaHash;
use ragelab_ydd::{YddDictionary, YddEditSession};
use ragelab_ydr::{ShaderBindingKey, TextureBindingKey, YdrDocument, YdrEditSession, YdrModel};
use ragelab_yft::YftDocument;
use ragelab_ymap::{
    apply_ymap_edit_command, Quat as YmapQuat, Vec3 as YmapVec3, Ymap, YmapEditCommand,
};
use ragelab_ytd::{Ytd, YtdDictionary};
use ragelab_ytyp::{ArchetypeKind, AssetType, Ytyp};
use serde::Serialize;
use wasm_bindgen::prelude::*;

const API_SCHEMA_VERSION: u32 = 1;
const ERROR_SCHEMA: &str = "ragelab.wasm.error";
const CAPABILITY_SCHEMA: &str = "ragelab.wasm.capabilities";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasmErrorReport {
    schema: &'static str,
    schema_version: u32,
    format: String,
    code: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FormatCapability {
    format: &'static str,
    inspect: bool,
    typed_geometry: bool,
    typed_texture: bool,
    writes: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityReport {
    schema: &'static str,
    schema_version: u32,
    formats: Vec<FormatCapability>,
    desktop_only: Vec<&'static str>,
    rules: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationReport {
    schema: &'static str,
    schema_version: u32,
    format: String,
    valid: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YmapEntityReport {
    index: usize,
    archetype_hash: String,
    position: [f32; 3],
    rotation: [f32; 4],
    scale_xy: Option<f32>,
    scale_z: Option<f32>,
    flags: u32,
    parent_index: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YmapReport {
    schema: &'static str,
    schema_version: u32,
    name_hash: Option<String>,
    parent_hash: Option<String>,
    flags: Option<u32>,
    content_flags: Option<u32>,
    physics_dictionaries: Vec<String>,
    entities_extents_min: Option<[f32; 3]>,
    entities_extents_max: Option<[f32; 3]>,
    streaming_extents_min: Option<[f32; 3]>,
    streaming_extents_max: Option<[f32; 3]>,
    entities: Vec<YmapEntityReport>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YtypArchetypeReport {
    index: usize,
    kind: String,
    name_hash: String,
    asset_type: String,
    asset_name_hash: Option<String>,
    texture_dictionary_hash: Option<String>,
    physics_dictionary_hash: Option<String>,
    drawable_dictionary_hash: Option<String>,
    mlo_entity_count: usize,
    mlo_room_count: usize,
    mlo_portal_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YtypMloReport {
    archetype_hash: String,
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
    entity_count: usize,
    room_count: usize,
    portal_count: usize,
    entity_set_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YtypReport {
    schema: &'static str,
    schema_version: u32,
    name_hash: Option<String>,
    dependencies: Vec<String>,
    archetypes: Vec<YtypArchetypeReport>,
    mlos: Vec<YtypMloReport>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YddEntryReport {
    index: usize,
    name_hash: String,
    name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YddReport {
    schema: &'static str,
    schema_version: u32,
    entries: Vec<YddEntryReport>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YftReport {
    schema: &'static str,
    schema_version: u32,
    name: Option<String>,
    has_main_drawable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoundsReport {
    center: [f32; 3],
    radius: f32,
    min: [f32; 3],
    max: [f32; 3],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShaderTextureReferenceReport {
    parameter_hash: String,
    texture_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShaderReport {
    index: usize,
    name_hash: String,
    file_hash: String,
    diffuse_texture_name: Option<String>,
    texture_references: Vec<ShaderTextureReferenceReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrimitiveSliceReport {
    index: usize,
    model_index: usize,
    geometry_index: usize,
    shader_index: Option<u16>,
    topology: &'static str,
    position_float_offset: usize,
    vertex_count: usize,
    normal_float_offset: Option<usize>,
    uv_float_offset: Option<usize>,
    index_offset: usize,
    index_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelMetadata {
    schema: &'static str,
    schema_version: u32,
    format: String,
    selector_index: Option<usize>,
    selector_hash: Option<String>,
    selector_name: Option<String>,
    name: Option<String>,
    lod: &'static str,
    coordinate_convention: &'static str,
    bounds: BoundsReport,
    primitive_count: usize,
    vertex_count: usize,
    index_count: usize,
    embedded_texture_count: usize,
    shaders: Vec<ShaderReport>,
    primitives: Vec<PrimitiveSliceReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TextureMetadata {
    schema: &'static str,
    schema_version: u32,
    index: usize,
    mip_index: usize,
    name: String,
    name_hash: String,
    width: u16,
    height: u16,
    format: String,
    mip_levels: u8,
}

#[wasm_bindgen]
pub struct WasmModelPacket {
    metadata: ModelMetadata,
    positions: Vec<f32>,
    normals: Vec<f32>,
    uv0: Vec<f32>,
    indices: Vec<u32>,
}

#[wasm_bindgen]
impl WasmModelPacket {
    pub fn metadata(&self) -> Result<JsValue, JsValue> {
        to_js(&self.metadata)
    }

    pub fn positions(&self) -> Float32Array {
        Float32Array::from(self.positions.as_slice())
    }

    pub fn normals(&self) -> Float32Array {
        Float32Array::from(self.normals.as_slice())
    }

    pub fn uv0(&self) -> Float32Array {
        Float32Array::from(self.uv0.as_slice())
    }

    pub fn indices(&self) -> Uint32Array {
        Uint32Array::from(self.indices.as_slice())
    }
}

#[wasm_bindgen]
pub struct WasmTexturePacket {
    metadata: TextureMetadata,
    rgba: Vec<u8>,
}

#[wasm_bindgen]
impl WasmTexturePacket {
    pub fn metadata(&self) -> Result<JsValue, JsValue> {
        to_js(&self.metadata)
    }

    pub fn rgba(&self) -> Uint8Array {
        Uint8Array::from(self.rgba.as_slice())
    }
}

#[wasm_bindgen(js_name = capabilities)]
pub fn capabilities() -> Result<JsValue, JsValue> {
    to_js(&CapabilityReport {
        schema: CAPABILITY_SCHEMA,
        schema_version: API_SCHEMA_VERSION,
        formats: vec![
            FormatCapability {
                format: "ymap",
                inspect: true,
                typed_geometry: false,
                typed_texture: false,
                writes: vec!["setTransform", "setArchetype", "setFlags", "setParent"],
            },
            FormatCapability {
                format: "ytyp",
                inspect: true,
                typed_geometry: false,
                typed_texture: false,
                writes: vec![],
            },
            FormatCapability {
                format: "ydr",
                inspect: true,
                typed_geometry: true,
                typed_texture: true,
                writes: vec!["rebindShader", "rebindTexture"],
            },
            FormatCapability {
                format: "ydd",
                inspect: true,
                typed_geometry: true,
                typed_texture: true,
                writes: vec!["rebindShader", "rebindTexture"],
            },
            FormatCapability {
                format: "yft",
                inspect: true,
                typed_geometry: true,
                typed_texture: true,
                writes: vec![],
            },
            FormatCapability {
                format: "ytd",
                inspect: true,
                typed_geometry: false,
                typed_texture: true,
                writes: vec!["replacePng"],
            },
        ],
        desktop_only: vec![
            "rpf",
            "gameIndex",
            "gtaKeys",
            "installedGameDiscovery",
            "nativeWgpuRenderer",
            "filesystemOperationApply",
        ],
        rules: vec![
            "browser APIs operate only on caller-supplied bytes",
            "RPF/game-index/key handling is not linked into ragelab-wasm",
            "unsupported writes fail closed and never fall back destructively",
            "write results are returned as bytes only; the browser chooses whether to download them",
            "typed geometry and texture payloads are exposed as JavaScript typed arrays",
        ],
    })
}

#[wasm_bindgen(js_name = validateAsset)]
pub fn validate_asset(format: &str, bytes: &[u8]) -> Result<JsValue, JsValue> {
    match normalize_format(format).as_str() {
        "ymap" => Ymap::from_bytes(bytes)
            .map(|_| ())
            .map_err(|e| wasm_error("ymap", "parseFailed", e))?,
        "ytyp" => Ytyp::from_bytes(bytes)
            .map(|_| ())
            .map_err(|e| wasm_error("ytyp", "parseFailed", e))?,
        "ydr" => YdrDocument::from_bytes(bytes)
            .map(|_| ())
            .map_err(|e| wasm_error("ydr", "parseFailed", e))?,
        "ydd" => YddDictionary::from_bytes(bytes)
            .map(|_| ())
            .map_err(|e| wasm_error("ydd", "parseFailed", e))?,
        "yft" => YftDocument::from_bytes(bytes)
            .map(|_| ())
            .map_err(|e| wasm_error("yft", "parseFailed", e))?,
        "ytd" => Ytd::from_bytes(bytes)
            .map(|_| ())
            .map_err(|e| wasm_error("ytd", "parseFailed", e))?,
        other => {
            return Err(wasm_error(
                other,
                "unsupportedFormat",
                "supported formats are YMAP/YTYP/YDR/YDD/YFT/YTD",
            ))
        }
    }
    to_js(&ValidationReport {
        schema: "ragelab.wasm.validation",
        schema_version: API_SCHEMA_VERSION,
        format: normalize_format(format),
        valid: true,
    })
}

#[wasm_bindgen(js_name = inspectYmap)]
pub fn inspect_ymap(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let ymap = Ymap::from_bytes(bytes).map_err(|e| wasm_error("ymap", "parseFailed", e))?;
    to_js(&ymap_report(&ymap))
}

#[wasm_bindgen(js_name = inspectYtyp)]
pub fn inspect_ytyp(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let ytyp = Ytyp::from_bytes(bytes).map_err(|e| wasm_error("ytyp", "parseFailed", e))?;
    to_js(&ytyp_report(&ytyp))
}

#[wasm_bindgen(js_name = inspectYtd)]
pub fn inspect_ytd(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let report =
        ytd_texture_authoring_report(bytes).map_err(|e| wasm_error("ytd", "parseFailed", e))?;
    to_js(&report)
}

#[wasm_bindgen(js_name = inspectYdrMaterials)]
pub fn inspect_ydr_materials(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let report =
        ydr_material_authoring_report(bytes).map_err(|e| wasm_error("ydr", "parseFailed", e))?;
    to_js(&report)
}

#[wasm_bindgen(js_name = inspectYddMaterials)]
pub fn inspect_ydd_materials(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let report =
        ydd_material_authoring_report(bytes).map_err(|e| wasm_error("ydd", "parseFailed", e))?;
    to_js(&report)
}

#[wasm_bindgen(js_name = inspectYdd)]
pub fn inspect_ydd(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let dictionary =
        YddDictionary::from_bytes(bytes).map_err(|e| wasm_error("ydd", "parseFailed", e))?;
    let entries = dictionary
        .entries()
        .iter()
        .map(|entry| YddEntryReport {
            index: entry.index,
            name_hash: hex_hash(entry.name_hash),
            name: entry.name.clone(),
        })
        .collect();
    to_js(&YddReport {
        schema: "ragelab.wasm.ydd",
        schema_version: API_SCHEMA_VERSION,
        entries,
    })
}

#[wasm_bindgen(js_name = inspectYft)]
pub fn inspect_yft(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let document =
        YftDocument::from_bytes(bytes).map_err(|e| wasm_error("yft", "parseFailed", e))?;
    to_js(&YftReport {
        schema: "ragelab.wasm.yft",
        schema_version: API_SCHEMA_VERSION,
        name: document.name,
        has_main_drawable: document.main_drawable.is_some(),
    })
}

#[wasm_bindgen(js_name = ydrModel)]
pub fn ydr_model(bytes: &[u8]) -> Result<WasmModelPacket, JsValue> {
    let document =
        YdrDocument::from_bytes(bytes).map_err(|e| wasm_error("ydr", "parseFailed", e))?;
    let embedded_texture_count = document
        .embedded_textures
        .as_ref()
        .map(|dictionary| dictionary.textures().len())
        .unwrap_or(0);
    Ok(model_packet(
        "ydr",
        None,
        None,
        None,
        embedded_texture_count,
        &document.model,
    ))
}

#[wasm_bindgen(js_name = yddModel)]
pub fn ydd_model(bytes: &[u8], drawable_index: usize) -> Result<WasmModelPacket, JsValue> {
    let dictionary =
        YddDictionary::from_bytes(bytes).map_err(|e| wasm_error("ydd", "parseFailed", e))?;
    let entry = dictionary
        .entries()
        .get(drawable_index)
        .cloned()
        .ok_or_else(|| {
            wasm_error(
                "ydd",
                "indexOutOfBounds",
                format!(
                    "drawable index {drawable_index} exceeds dictionary size {}",
                    dictionary.entries().len()
                ),
            )
        })?;
    let document = dictionary
        .document(drawable_index)
        .map_err(|e| wasm_error("ydd", "parseFailed", e))?;
    let embedded_texture_count = document
        .embedded_textures
        .as_ref()
        .map(|textures| textures.textures().len())
        .unwrap_or(0);
    Ok(model_packet(
        "ydd",
        Some(entry.index),
        Some(hex_hash(entry.name_hash)),
        entry.name,
        embedded_texture_count,
        &document.model,
    ))
}

#[wasm_bindgen(js_name = yftModel)]
pub fn yft_model(bytes: &[u8]) -> Result<WasmModelPacket, JsValue> {
    let document =
        YftDocument::from_bytes(bytes).map_err(|e| wasm_error("yft", "parseFailed", e))?;
    let drawable = document.main_drawable.ok_or_else(|| {
        wasm_error(
            "yft",
            "unsupportedAsset",
            "YFT has no pristine main drawable available for preview",
        )
    })?;
    let embedded_texture_count = drawable
        .embedded_textures
        .as_ref()
        .map(|textures| textures.textures().len())
        .unwrap_or(0);
    Ok(model_packet(
        "yft",
        None,
        None,
        document.name,
        embedded_texture_count,
        &drawable.model,
    ))
}

#[wasm_bindgen(js_name = ytdTexture)]
pub fn ytd_texture(bytes: &[u8], index: usize) -> Result<WasmTexturePacket, JsValue> {
    texture_packet(bytes, index, 0)
}

#[wasm_bindgen(js_name = ytdTextureByName)]
pub fn ytd_texture_by_name(bytes: &[u8], name: &str) -> Result<WasmTexturePacket, JsValue> {
    let dictionary =
        YtdDictionary::from_bytes(bytes).map_err(|e| wasm_error("ytd", "parseFailed", e))?;
    let index = texture_index_by_name("ytd", &dictionary, name)?;
    embedded_texture_packet("ytd", &dictionary, index)
}

#[wasm_bindgen(js_name = ydrEmbeddedTexture)]
pub fn ydr_embedded_texture(
    bytes: &[u8],
    texture_index: usize,
) -> Result<WasmTexturePacket, JsValue> {
    let document =
        YdrDocument::from_bytes(bytes).map_err(|e| wasm_error("ydr", "parseFailed", e))?;
    let dictionary = document.embedded_textures.as_ref().ok_or_else(|| {
        wasm_error(
            "ydr",
            "missingDependency",
            "YDR has no embedded TextureDictionary",
        )
    })?;
    embedded_texture_packet("ydr", dictionary, texture_index)
}

#[wasm_bindgen(js_name = ydrEmbeddedTextureByName)]
pub fn ydr_embedded_texture_by_name(
    bytes: &[u8],
    name: &str,
) -> Result<WasmTexturePacket, JsValue> {
    let document =
        YdrDocument::from_bytes(bytes).map_err(|e| wasm_error("ydr", "parseFailed", e))?;
    let dictionary = document.embedded_textures.as_ref().ok_or_else(|| {
        wasm_error(
            "ydr",
            "missingDependency",
            "YDR has no embedded TextureDictionary",
        )
    })?;
    let index = texture_index_by_name("ydr", dictionary, name)?;
    embedded_texture_packet("ydr", dictionary, index)
}

#[wasm_bindgen(js_name = yddEmbeddedTexture)]
pub fn ydd_embedded_texture(
    bytes: &[u8],
    drawable_index: usize,
    texture_index: usize,
) -> Result<WasmTexturePacket, JsValue> {
    let dictionary =
        YddDictionary::from_bytes(bytes).map_err(|e| wasm_error("ydd", "parseFailed", e))?;
    if drawable_index >= dictionary.entries().len() {
        return Err(wasm_error(
            "ydd",
            "indexOutOfBounds",
            format!(
                "drawable index {drawable_index} exceeds dictionary size {}",
                dictionary.entries().len()
            ),
        ));
    }
    let document = dictionary
        .document(drawable_index)
        .map_err(|e| wasm_error("ydd", "parseFailed", e))?;
    let textures = document.embedded_textures.as_ref().ok_or_else(|| {
        wasm_error(
            "ydd",
            "missingDependency",
            format!("YDD drawable {drawable_index} has no embedded TextureDictionary"),
        )
    })?;
    embedded_texture_packet("ydd", textures, texture_index)
}

#[wasm_bindgen(js_name = yddEmbeddedTextureByName)]
pub fn ydd_embedded_texture_by_name(
    bytes: &[u8],
    drawable_index: usize,
    name: &str,
) -> Result<WasmTexturePacket, JsValue> {
    let dictionary =
        YddDictionary::from_bytes(bytes).map_err(|e| wasm_error("ydd", "parseFailed", e))?;
    if drawable_index >= dictionary.entries().len() {
        return Err(wasm_error(
            "ydd",
            "indexOutOfBounds",
            format!(
                "drawable index {drawable_index} exceeds dictionary size {}",
                dictionary.entries().len()
            ),
        ));
    }
    let document = dictionary
        .document(drawable_index)
        .map_err(|e| wasm_error("ydd", "parseFailed", e))?;
    let textures = document.embedded_textures.as_ref().ok_or_else(|| {
        wasm_error(
            "ydd",
            "missingDependency",
            format!("YDD drawable {drawable_index} has no embedded TextureDictionary"),
        )
    })?;
    let index = texture_index_by_name("ydd", textures, name)?;
    embedded_texture_packet("ydd", textures, index)
}

#[wasm_bindgen(js_name = yftEmbeddedTexture)]
pub fn yft_embedded_texture(
    bytes: &[u8],
    texture_index: usize,
) -> Result<WasmTexturePacket, JsValue> {
    let document =
        YftDocument::from_bytes(bytes).map_err(|e| wasm_error("yft", "parseFailed", e))?;
    let drawable = document.main_drawable.as_ref().ok_or_else(|| {
        wasm_error(
            "yft",
            "unsupportedAsset",
            "YFT has no pristine main drawable available for preview",
        )
    })?;
    let dictionary = drawable.embedded_textures.as_ref().ok_or_else(|| {
        wasm_error(
            "yft",
            "missingDependency",
            "YFT main drawable has no embedded TextureDictionary",
        )
    })?;
    embedded_texture_packet("yft", dictionary, texture_index)
}

#[wasm_bindgen(js_name = yftEmbeddedTextureByName)]
pub fn yft_embedded_texture_by_name(
    bytes: &[u8],
    name: &str,
) -> Result<WasmTexturePacket, JsValue> {
    let document =
        YftDocument::from_bytes(bytes).map_err(|e| wasm_error("yft", "parseFailed", e))?;
    let drawable = document.main_drawable.as_ref().ok_or_else(|| {
        wasm_error(
            "yft",
            "unsupportedAsset",
            "YFT has no pristine main drawable available for preview",
        )
    })?;
    let dictionary = drawable.embedded_textures.as_ref().ok_or_else(|| {
        wasm_error(
            "yft",
            "missingDependency",
            "YFT main drawable has no embedded TextureDictionary",
        )
    })?;
    let index = texture_index_by_name("yft", dictionary, name)?;
    embedded_texture_packet("yft", dictionary, index)
}

#[wasm_bindgen(js_name = ytdTextureMip)]
pub fn ytd_texture_mip(
    bytes: &[u8],
    index: usize,
    mip_index: usize,
) -> Result<WasmTexturePacket, JsValue> {
    texture_packet(bytes, index, mip_index)
}

#[wasm_bindgen(js_name = exportYtdTexturePng)]
pub fn wasm_export_ytd_texture_png(bytes: &[u8], index: usize) -> Result<Uint8Array, JsValue> {
    let output =
        export_ytd_texture_png(bytes, index).map_err(|e| wasm_error("ytd", "exportFailed", e))?;
    Ok(Uint8Array::from(output.as_slice()))
}

#[wasm_bindgen(js_name = replaceYtdTexturePng)]
pub fn wasm_replace_ytd_texture_png(
    bytes: &[u8],
    index: usize,
    png_bytes: &[u8],
) -> Result<Uint8Array, JsValue> {
    let output = replace_ytd_texture_from_png(bytes, index, png_bytes)
        .map_err(|e| wasm_error("ytd", "writeRejected", e))?;
    Ytd::from_bytes(&output).map_err(|e| wasm_error("ytd", "semanticReopenFailed", e))?;
    Ok(Uint8Array::from(output.as_slice()))
}

#[wasm_bindgen(js_name = ymapSetTransform)]
pub fn ymap_set_transform(
    bytes: &[u8],
    index: usize,
    position: &[f32],
    rotation: &[f32],
    scale_xy: Option<f32>,
    scale_z: Option<f32>,
) -> Result<Uint8Array, JsValue> {
    let position = vec3(position, "position")?;
    let rotation = quat(rotation, "rotation")?;
    let output = apply_ymap_edit_command(
        bytes,
        &YmapEditCommand::SetTransform {
            index,
            position,
            rotation,
            scale_xy,
            scale_z,
        },
    )
    .map_err(|e| wasm_error("ymap", "writeRejected", e))?;
    semantic_reopen_ymap(output)
}

#[wasm_bindgen(js_name = ymapSetArchetype)]
pub fn ymap_set_archetype(
    bytes: &[u8],
    index: usize,
    archetype_hash: u32,
) -> Result<Uint8Array, JsValue> {
    if archetype_hash == 0 {
        return Err(wasm_error(
            "ymap",
            "invalidInput",
            "archetype hash must not be zero",
        ));
    }
    let output = apply_ymap_edit_command(
        bytes,
        &YmapEditCommand::SetProperties {
            index,
            archetype_name: Some(MetaHash(archetype_hash)),
            flags: None,
            parent_index: None,
        },
    )
    .map_err(|e| wasm_error("ymap", "writeRejected", e))?;
    semantic_reopen_ymap(output)
}

#[wasm_bindgen(js_name = ymapSetFlags)]
pub fn ymap_set_flags(bytes: &[u8], index: usize, flags: u32) -> Result<Uint8Array, JsValue> {
    let output = apply_ymap_edit_command(
        bytes,
        &YmapEditCommand::SetProperties {
            index,
            archetype_name: None,
            flags: Some(flags),
            parent_index: None,
        },
    )
    .map_err(|e| wasm_error("ymap", "writeRejected", e))?;
    semantic_reopen_ymap(output)
}

#[wasm_bindgen(js_name = ymapSetParent)]
pub fn ymap_set_parent(
    bytes: &[u8],
    index: usize,
    parent_index: Option<i32>,
) -> Result<Uint8Array, JsValue> {
    let output = apply_ymap_edit_command(
        bytes,
        &YmapEditCommand::SetProperties {
            index,
            archetype_name: None,
            flags: None,
            parent_index: Some(parent_index),
        },
    )
    .map_err(|e| wasm_error("ymap", "writeRejected", e))?;
    semantic_reopen_ymap(output)
}

#[wasm_bindgen(js_name = ydrRebindShader)]
pub fn ydr_rebind_shader(
    bytes: &[u8],
    model_index: usize,
    geometry_index: usize,
    target_shader_index: u16,
) -> Result<Uint8Array, JsValue> {
    let mut session =
        YdrEditSession::from_bytes(bytes).map_err(|e| wasm_error("ydr", "writeRejected", e))?;
    session
        .rebind_shader(
            ShaderBindingKey {
                model_index,
                geometry_index,
            },
            target_shader_index,
        )
        .map_err(|e| wasm_error("ydr", "writeRejected", e))?;
    let output = session
        .to_bytes()
        .map_err(|e| wasm_error("ydr", "semanticReopenFailed", e))?;
    Ok(Uint8Array::from(output.as_slice()))
}

#[wasm_bindgen(js_name = ydrRebindTexture)]
pub fn ydr_rebind_texture(
    bytes: &[u8],
    source_shader_index: usize,
    source_parameter_index: usize,
    target_shader_index: usize,
    target_parameter_index: usize,
) -> Result<Uint8Array, JsValue> {
    let mut session =
        YdrEditSession::from_bytes(bytes).map_err(|e| wasm_error("ydr", "writeRejected", e))?;
    session
        .rebind_texture(
            TextureBindingKey {
                shader_index: source_shader_index,
                parameter_index: source_parameter_index,
            },
            TextureBindingKey {
                shader_index: target_shader_index,
                parameter_index: target_parameter_index,
            },
        )
        .map_err(|e| wasm_error("ydr", "writeRejected", e))?;
    let output = session
        .to_bytes()
        .map_err(|e| wasm_error("ydr", "semanticReopenFailed", e))?;
    Ok(Uint8Array::from(output.as_slice()))
}

#[wasm_bindgen(js_name = yddRebindShader)]
pub fn ydd_rebind_shader(
    bytes: &[u8],
    drawable_index: usize,
    model_index: usize,
    geometry_index: usize,
    target_shader_index: u16,
) -> Result<Uint8Array, JsValue> {
    let mut session = YddEditSession::from_bytes(bytes, drawable_index)
        .map_err(|e| wasm_error("ydd", "writeRejected", e))?;
    session
        .rebind_shader(
            ShaderBindingKey {
                model_index,
                geometry_index,
            },
            target_shader_index,
        )
        .map_err(|e| wasm_error("ydd", "writeRejected", e))?;
    let output = session
        .to_bytes()
        .map_err(|e| wasm_error("ydd", "semanticReopenFailed", e))?;
    Ok(Uint8Array::from(output.as_slice()))
}

#[wasm_bindgen(js_name = yddRebindTexture)]
pub fn ydd_rebind_texture(
    bytes: &[u8],
    drawable_index: usize,
    source_shader_index: usize,
    source_parameter_index: usize,
    target_shader_index: usize,
    target_parameter_index: usize,
) -> Result<Uint8Array, JsValue> {
    let mut session = YddEditSession::from_bytes(bytes, drawable_index)
        .map_err(|e| wasm_error("ydd", "writeRejected", e))?;
    session
        .rebind_texture(
            TextureBindingKey {
                shader_index: source_shader_index,
                parameter_index: source_parameter_index,
            },
            TextureBindingKey {
                shader_index: target_shader_index,
                parameter_index: target_parameter_index,
            },
        )
        .map_err(|e| wasm_error("ydd", "writeRejected", e))?;
    let output = session
        .to_bytes()
        .map_err(|e| wasm_error("ydd", "semanticReopenFailed", e))?;
    Ok(Uint8Array::from(output.as_slice()))
}

fn texture_index_by_name(
    format: &'static str,
    dictionary: &YtdDictionary,
    name: &str,
) -> Result<usize, JsValue> {
    let name_hash = joaat(name);
    dictionary
        .texture_by_name(name)
        .or_else(|| dictionary.texture_by_hash(name_hash))
        .map(|texture| texture.index)
        .ok_or_else(|| {
            wasm_error(
                format,
                "textureNotFound",
                format!("texture {name:?} (0x{name_hash:08X}) was not found"),
            )
        })
}

fn embedded_texture_packet(
    format: &'static str,
    dictionary: &YtdDictionary,
    texture_index: usize,
) -> Result<WasmTexturePacket, JsValue> {
    let info = dictionary.textures().get(texture_index).ok_or_else(|| {
        wasm_error(
            format,
            "indexOutOfBounds",
            format!(
                "embedded texture index {texture_index} exceeds dictionary size {}",
                dictionary.textures().len()
            ),
        )
    })?;
    let decoded = dictionary
        .decode_top_mip(texture_index)
        .map_err(|e| wasm_error(format, "decodeFailed", e))?;
    Ok(WasmTexturePacket {
        metadata: TextureMetadata {
            schema: "ragelab.wasm.texture",
            schema_version: API_SCHEMA_VERSION,
            index: texture_index,
            mip_index: 0,
            name: info
                .name
                .clone()
                .unwrap_or_else(|| hex_hash(info.name_hash)),
            name_hash: hex_hash(info.name_hash),
            width: decoded.width,
            height: decoded.height,
            format: info.format.normalized_name().to_string(),
            mip_levels: info.levels,
        },
        rgba: decoded.rgba,
    })
}

fn texture_packet(
    bytes: &[u8],
    index: usize,
    mip_index: usize,
) -> Result<WasmTexturePacket, JsValue> {
    let ytd = Ytd::from_bytes(bytes).map_err(|e| wasm_error("ytd", "parseFailed", e))?;
    let info = ytd.textures.get(index).ok_or_else(|| {
        wasm_error(
            "ytd",
            "indexOutOfBounds",
            format!(
                "texture index {index} exceeds dictionary size {}",
                ytd.textures.len()
            ),
        )
    })?;
    if mip_index >= usize::from(info.levels) {
        return Err(wasm_error(
            "ytd",
            "indexOutOfBounds",
            format!(
                "mip index {mip_index} exceeds texture {index} mip count {}",
                info.levels
            ),
        ));
    }
    let decoded = Ytd::decode_mip_rgba(bytes, index, mip_index)
        .map_err(|e| wasm_error("ytd", "decodeFailed", e))?;
    Ok(WasmTexturePacket {
        metadata: TextureMetadata {
            schema: "ragelab.wasm.texture",
            schema_version: API_SCHEMA_VERSION,
            index,
            mip_index,
            name: info.name.clone(),
            name_hash: hex_hash(info.name_hash),
            width: decoded.width,
            height: decoded.height,
            format: info.format.normalized_name().to_string(),
            mip_levels: info.levels,
        },
        rgba: decoded.rgba,
    })
}

fn model_packet(
    format: &str,
    selector_index: Option<usize>,
    selector_hash: Option<String>,
    selector_name: Option<String>,
    embedded_texture_count: usize,
    model: &YdrModel,
) -> WasmModelPacket {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uv0 = Vec::new();
    let mut indices = Vec::new();
    let mut primitives = Vec::with_capacity(model.primitives.len());

    for (index, primitive) in model.primitives.iter().enumerate() {
        let position_float_offset = positions.len();
        for position in &primitive.positions {
            positions.extend_from_slice(position);
        }

        let normal_float_offset = primitive.normals.as_ref().map(|values| {
            let offset = normals.len();
            for normal in values {
                normals.extend_from_slice(normal);
            }
            offset
        });

        let uv_float_offset = primitive.uv0.as_ref().map(|values| {
            let offset = uv0.len();
            for uv in values {
                uv0.extend_from_slice(uv);
            }
            offset
        });

        let index_offset = indices.len();
        indices.extend_from_slice(&primitive.indices);
        primitives.push(PrimitiveSliceReport {
            index,
            model_index: primitive.model_index,
            geometry_index: primitive.geometry_index,
            shader_index: primitive.shader_index,
            topology: primitive.topology.as_str(),
            position_float_offset,
            vertex_count: primitive.positions.len(),
            normal_float_offset,
            uv_float_offset,
            index_offset,
            index_count: primitive.indices.len(),
        });
    }

    let shaders = model
        .shaders
        .iter()
        .enumerate()
        .map(|(index, shader)| ShaderReport {
            index,
            name_hash: hex_hash(shader.name_hash),
            file_hash: hex_hash(shader.file_hash),
            diffuse_texture_name: shader.diffuse_texture_name().map(str::to_owned),
            texture_references: shader
                .texture_references
                .iter()
                .map(|reference| ShaderTextureReferenceReport {
                    parameter_hash: hex_hash(reference.parameter_hash),
                    texture_name: reference.texture_name.clone(),
                })
                .collect(),
        })
        .collect();

    WasmModelPacket {
        metadata: ModelMetadata {
            schema: "ragelab.wasm.model",
            schema_version: API_SCHEMA_VERSION,
            format: format.to_string(),
            selector_index,
            selector_hash,
            selector_name,
            name: model.name.clone(),
            lod: model.lod.as_str(),
            coordinate_convention: model.coordinate_convention.as_str(),
            bounds: BoundsReport {
                center: model.bounds.center,
                radius: model.bounds.radius,
                min: model.bounds.min,
                max: model.bounds.max,
            },
            primitive_count: model.primitives.len(),
            vertex_count: model.vertex_count(),
            index_count: model.index_count(),
            embedded_texture_count,
            shaders,
            primitives,
        },
        positions,
        normals,
        uv0,
        indices,
    }
}

fn ymap_report(ymap: &Ymap) -> YmapReport {
    YmapReport {
        schema: "ragelab.wasm.ymap",
        schema_version: API_SCHEMA_VERSION,
        name_hash: ymap.name.map(|value| hex_hash(value.0)),
        parent_hash: ymap.parent.map(|value| hex_hash(value.0)),
        flags: ymap.flags,
        content_flags: ymap.content_flags,
        physics_dictionaries: ymap
            .physics_dictionaries
            .iter()
            .map(|value| hex_hash(value.0))
            .collect(),
        entities_extents_min: ymap.entities_extents_min.map(ymap_vec3_array),
        entities_extents_max: ymap.entities_extents_max.map(ymap_vec3_array),
        streaming_extents_min: ymap.streaming_extents_min.map(ymap_vec3_array),
        streaming_extents_max: ymap.streaming_extents_max.map(ymap_vec3_array),
        entities: ymap
            .entities
            .iter()
            .enumerate()
            .map(|(index, entity)| YmapEntityReport {
                index,
                archetype_hash: hex_hash(entity.archetype_name.0),
                position: ymap_vec3_array(entity.position),
                rotation: [
                    entity.rotation.x,
                    entity.rotation.y,
                    entity.rotation.z,
                    entity.rotation.w,
                ],
                scale_xy: entity.scale_xy,
                scale_z: entity.scale_z,
                flags: entity.flags,
                parent_index: entity.parent_index,
            })
            .collect(),
    }
}

fn ytyp_report(ytyp: &Ytyp) -> YtypReport {
    YtypReport {
        schema: "ragelab.wasm.ytyp",
        schema_version: API_SCHEMA_VERSION,
        name_hash: ytyp.name.map(|value| hex_hash(value.0)),
        dependencies: ytyp
            .dependencies
            .iter()
            .map(|value| hex_hash(value.0))
            .collect(),
        archetypes: ytyp
            .archetypes
            .iter()
            .enumerate()
            .map(|(index, archetype)| YtypArchetypeReport {
                index,
                kind: archetype_kind_name(archetype.kind),
                name_hash: hex_hash(archetype.name.0),
                asset_type: asset_type_name(archetype.asset_type),
                asset_name_hash: archetype.asset_name.map(|value| hex_hash(value.0)),
                texture_dictionary_hash: archetype
                    .texture_dictionary
                    .map(|value| hex_hash(value.0)),
                physics_dictionary_hash: archetype
                    .physics_dictionary
                    .map(|value| hex_hash(value.0)),
                drawable_dictionary_hash: archetype
                    .drawable_dictionary
                    .map(|value| hex_hash(value.0)),
                mlo_entity_count: archetype.mlo_entity_count,
                mlo_room_count: archetype.mlo_room_count,
                mlo_portal_count: archetype.mlo_portal_count,
            })
            .collect(),
        mlos: ytyp
            .mlos
            .iter()
            .map(|mlo| YtypMloReport {
                archetype_hash: hex_hash(mlo.archetype_name.0),
                bounds_min: [mlo.bounds_min.x, mlo.bounds_min.y, mlo.bounds_min.z],
                bounds_max: [mlo.bounds_max.x, mlo.bounds_max.y, mlo.bounds_max.z],
                entity_count: mlo.entities.len(),
                room_count: mlo.rooms.len(),
                portal_count: mlo.portals.len(),
                entity_set_count: mlo.entity_sets.len(),
            })
            .collect(),
    }
}

fn semantic_reopen_ymap(output: Vec<u8>) -> Result<Uint8Array, JsValue> {
    Ymap::from_bytes(&output).map_err(|e| wasm_error("ymap", "semanticReopenFailed", e))?;
    Ok(Uint8Array::from(output.as_slice()))
}

fn vec3(values: &[f32], field: &str) -> Result<YmapVec3, JsValue> {
    if values.len() != 3 || values.iter().any(|value| !value.is_finite()) {
        return Err(wasm_error(
            "ymap",
            "invalidInput",
            format!("{field} must contain exactly three finite numbers"),
        ));
    }
    Ok(YmapVec3 {
        x: values[0],
        y: values[1],
        z: values[2],
    })
}

fn quat(values: &[f32], field: &str) -> Result<YmapQuat, JsValue> {
    if values.len() != 4 || values.iter().any(|value| !value.is_finite()) {
        return Err(wasm_error(
            "ymap",
            "invalidInput",
            format!("{field} must contain exactly four finite numbers"),
        ));
    }
    Ok(YmapQuat {
        x: values[0],
        y: values[1],
        z: values[2],
        w: values[3],
    })
}

fn ymap_vec3_array(value: ragelab_ymap::Vec3) -> [f32; 3] {
    [value.x, value.y, value.z]
}

fn archetype_kind_name(kind: ArchetypeKind) -> String {
    match kind {
        ArchetypeKind::Base => "base".into(),
        ArchetypeKind::Time => "time".into(),
        ArchetypeKind::Mlo => "mlo".into(),
        ArchetypeKind::Unknown(value) => format!("unknown:{}", hex_hash(value.0)),
    }
}

fn asset_type_name(asset_type: AssetType) -> String {
    match asset_type {
        AssetType::Uninitialized => "uninitialized".into(),
        AssetType::Fragment => "fragment".into(),
        AssetType::Drawable => "drawable".into(),
        AssetType::DrawableDictionary => "drawableDictionary".into(),
        AssetType::Assetless => "assetless".into(),
        AssetType::Unknown(value) => format!("unknown:{value}"),
    }
}

fn normalize_format(format: &str) -> String {
    format.trim().trim_start_matches('.').to_ascii_lowercase()
}

fn hex_hash(value: u32) -> String {
    format!("0x{value:08X}")
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(value)
        .map_err(|error| wasm_error("wasm", "serializationFailed", error))
}

fn wasm_error(
    format: impl Into<String>,
    code: &'static str,
    error: impl std::fmt::Display,
) -> JsValue {
    let report = WasmErrorReport {
        schema: ERROR_SCHEMA,
        schema_version: API_SCHEMA_VERSION,
        format: format.into(),
        code,
        message: error.to_string(),
    };
    serde_wasm_bindgen::to_value(&report).unwrap_or_else(|_| JsValue::from_str(&report.message))
}

#[cfg(test)]
mod tests {
    use super::*;

    const YMAP: &[u8] = include_bytes!("../../../fixtures/synthetic/stream/simple.ymap");
    const YTYP: &[u8] = include_bytes!("../../../fixtures/synthetic/mlo.ytyp");
    const YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/simple.ydr");
    const YTD: &[u8] = include_bytes!("../../../fixtures/synthetic/ytd/simple.ytd");

    #[test]
    fn portable_reports_parse_supplied_files_without_filesystem_context() {
        let ymap = Ymap::from_bytes(YMAP).expect("ymap");
        let report = ymap_report(&ymap);
        assert_eq!(report.entities.len(), 1);

        let ytyp = Ytyp::from_bytes(YTYP).expect("ytyp");
        let report = ytyp_report(&ytyp);
        assert_eq!(report.mlos.len(), 1);
        assert_eq!(report.mlos[0].room_count, 2);
    }

    #[test]
    fn model_packet_uses_flat_typed_array_payload_contract() {
        let document = YdrDocument::from_bytes(YDR).expect("ydr");
        let embedded_texture_count = document
            .embedded_textures
            .as_ref()
            .map(|textures| textures.textures().len())
            .unwrap_or(0);
        let packet = model_packet(
            "ydr",
            None,
            None,
            None,
            embedded_texture_count,
            &document.model,
        );
        assert_eq!(packet.metadata.primitive_count, 1);
        assert_eq!(packet.positions.len(), packet.metadata.vertex_count * 3);
        assert_eq!(packet.indices.len(), packet.metadata.index_count);
    }

    #[test]
    fn web_ytd_write_reuses_shared_authoring_and_semantic_reopens() {
        let png = export_ytd_texture_png(YTD, 0).expect("png");
        let output = replace_ytd_texture_from_png(YTD, 0, &png).expect("replace");
        let reopened = Ytd::from_bytes(&output).expect("semantic reopen");
        assert_eq!(reopened.textures.len(), 1);
    }
}
