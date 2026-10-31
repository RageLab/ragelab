use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::Path,
};

pub const INSPECTION_SCHEMA: &str = "ragelab.glb.inspection";
pub const PROFILE_SCHEMA: &str = "ragelab.glb.authoring-profile";
pub const PROFILE_VALIDATION_SCHEMA: &str = "ragelab.glb.profile-validation";
pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_GLB_BYTES: usize = 512 * 1024 * 1024;
const MAGIC: &[u8; 4] = b"glTF";
const JSON_CHUNK: u32 = 0x4E4F534A;
const BIN_CHUNK: u32 = 0x004E4942;
pub const GLB_MAGIC: &[u8; 4] = MAGIC;
pub const JSON_CHUNK_TYPE: u32 = JSON_CHUNK;
pub const BIN_CHUNK_TYPE: u32 = BIN_CHUNK;

#[derive(Debug)]
pub enum GlbError {
    InvalidContainer(String),
    InvalidDocument(String),
    InvalidProfile(String),
    Unsupported(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}
impl fmt::Display for GlbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContainer(s) => write!(f, "invalid GLB container: {s}"),
            Self::InvalidDocument(s) => write!(f, "invalid glTF document: {s}"),
            Self::InvalidProfile(s) => write!(f, "invalid authoring profile: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported GLB geometry feature: {s}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "invalid GLB JSON chunk: {e}"),
        }
    }
}
impl Error for GlbError {}
impl From<std::io::Error> for GlbError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for GlbError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlbInspectionReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub source: SourceInfo,
    pub container: ContainerInfo,
    pub asset: AssetInfo,
    pub default_scene: Option<usize>,
    pub scenes: Vec<SceneInfo>,
    pub nodes: Vec<NodeInfo>,
    pub meshes: Vec<MeshInfo>,
    pub materials: Vec<MaterialInfo>,
    pub textures: Vec<TextureInfo>,
    pub images: Vec<ImageInfo>,
    pub samplers: Vec<SamplerInfo>,
    pub counts: GlbCounts,
    pub extensions_used: Vec<String>,
    pub extensions_required: Vec<String>,
    pub unsupported_required_extensions: Vec<String>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub file_name: String,
    pub byte_length: usize,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInfo {
    pub version: u32,
    pub declared_length: usize,
    pub json_chunk_bytes: usize,
    pub binary_chunk_bytes: usize,
    pub unknown_chunks: Vec<UnknownChunkInfo>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownChunkInfo {
    pub chunk_type: String,
    pub byte_length: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInfo {
    pub version: String,
    pub min_version: Option<String>,
    pub generator: Option<String>,
    pub copyright: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneInfo {
    pub index: usize,
    pub name: Option<String>,
    pub nodes: Vec<usize>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub index: usize,
    pub name: Option<String>,
    pub mesh_index: Option<usize>,
    pub skin_index: Option<usize>,
    pub camera_index: Option<usize>,
    pub children: Vec<usize>,
    pub translation: Option<Vec<f64>>,
    pub rotation: Option<Vec<f64>>,
    pub scale: Option<Vec<f64>>,
    pub matrix: Option<Vec<f64>>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshInfo {
    pub index: usize,
    pub name: Option<String>,
    pub primitives: Vec<PrimitiveInfo>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimitiveInfo {
    pub index: usize,
    pub mode: u32,
    pub mode_name: &'static str,
    pub material_index: Option<usize>,
    pub indices: Option<AccessorInfo>,
    pub attributes: BTreeMap<String, AccessorInfo>,
    pub morph_target_count: usize,
    pub extensions: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessorInfo {
    pub index: usize,
    pub count: usize,
    pub component_type: u32,
    pub element_type: String,
    pub normalized: bool,
    pub min: Option<Vec<f64>>,
    pub max: Option<Vec<f64>>,
    pub sparse: bool,
    pub buffer_view_index: Option<usize>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialInfo {
    pub index: usize,
    pub name: Option<String>,
    pub base_color_factor: [f64; 4],
    pub base_color_texture: Option<TextureBindingInfo>,
    pub metallic_factor: f64,
    pub roughness_factor: f64,
    pub metallic_roughness_texture: Option<TextureBindingInfo>,
    pub normal_texture: Option<TextureBindingInfo>,
    pub occlusion_texture: Option<TextureBindingInfo>,
    pub emissive_factor: [f64; 3],
    pub emissive_texture: Option<TextureBindingInfo>,
    pub emissive_strength: f64,
    pub specular_texture: Option<TextureBindingInfo>,
    pub specular_color_texture: Option<TextureBindingInfo>,
    pub transmission_factor: Option<f64>,
    pub ior: Option<f64>,
    pub alpha_mode: String,
    pub alpha_cutoff: f64,
    pub double_sided: bool,
    pub unlit: bool,
    pub extensions: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureBindingInfo {
    pub texture_index: usize,
    pub tex_coord: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strength: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureInfo {
    pub index: usize,
    pub name: Option<String>,
    pub source_image_index: Option<usize>,
    pub sampler_index: Option<usize>,
    pub extensions: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub index: usize,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub storage: String,
    pub uri: Option<String>,
    pub buffer_view_index: Option<usize>,
    pub byte_length: Option<usize>,
    pub dimensions: Option<ImageDimensions>,
    pub embedded: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplerInfo {
    pub index: usize,
    pub name: Option<String>,
    pub mag_filter: Option<u32>,
    pub min_filter: Option<u32>,
    pub wrap_s: u32,
    pub wrap_t: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlbCounts {
    pub scenes: usize,
    pub nodes: usize,
    pub meshes: usize,
    pub primitives: usize,
    pub materials: usize,
    pub textures: usize,
    pub images: usize,
    pub samplers: usize,
    pub accessors: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GtaAuthoringProfile {
    pub schema: String,
    pub schema_version: u32,
    pub source: ProfileSource,
    pub settings: ExportSettings,
    pub materials: Vec<MaterialAuthoringSettings>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileSource {
    pub file_name: String,
    pub byte_length: usize,
    pub sha256: String,
    pub material_count: usize,
    pub texture_count: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportSettings {
    pub target: String,
    pub embed_textures: bool,
    pub generate_mipmaps: bool,
    pub texture_encoding: TextureEncoding,
    pub normal_map_convention: NormalMapConvention,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureEncoding {
    Auto,
    Bc1,
    Bc3,
    Rgba8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalMapConvention {
    OpenGl,
    DirectX,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialAuthoringSettings {
    pub source_material_index: usize,
    pub source_name: Option<String>,
    pub shader_intent: ShaderIntent,
    pub surface_mode: SurfaceMode,
    pub texture_roles: TextureRoleMap,
    pub parameters: MaterialParameters,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShaderIntent {
    Lit,
    Unlit,
    Emissive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceMode {
    Opaque,
    Masked,
    Blended,
    Glass,
}
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextureRoleMap {
    pub albedo: Option<TextureBindingInfo>,
    pub normal: Option<TextureBindingInfo>,
    pub metallic_roughness: Option<TextureBindingInfo>,
    pub specular: Option<TextureBindingInfo>,
    pub specular_color: Option<TextureBindingInfo>,
    pub tint_palette: Option<TextureBindingInfo>,
    pub occlusion: Option<TextureBindingInfo>,
    pub emissive: Option<TextureBindingInfo>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialParameters {
    pub base_color_factor: [f64; 4],
    pub metallic: f64,
    pub roughness: f64,
    pub emissive_factor: [f64; 3],
    pub emissive_strength: f64,
    pub alpha_cutoff: f64,
    pub double_sided: bool,
    pub transmission_factor: Option<f64>,
    pub ior: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileValidationReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub valid: bool,
    pub source_sha256: String,
    pub material_count: usize,
    pub mapped_texture_slots: usize,
    pub embed_textures: bool,
    pub notes: Vec<String>,
}

pub(crate) struct Container<'a> {
    pub(crate) json: Value,
    pub(crate) json_len: usize,
    pub(crate) bin: Option<&'a [u8]>,
    pub(crate) unknown: Vec<UnknownChunkInfo>,
}

pub fn inspect_glb_file(path: &Path) -> Result<GlbInspectionReport, GlbError> {
    let bytes = fs::read(path)?;
    let file_name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("model.glb")
        .to_string();
    inspect_glb(&bytes, file_name)
}

pub fn inspect_glb(
    bytes: &[u8],
    file_name: impl Into<String>,
) -> Result<GlbInspectionReport, GlbError> {
    if bytes.len() > MAX_GLB_BYTES {
        return Err(GlbError::InvalidContainer(format!(
            "file exceeds the {MAX_GLB_BYTES} byte safety limit"
        )));
    }
    let container = parse_container(bytes)?;
    let root = &container.json;
    let asset = root
        .get("asset")
        .and_then(Value::as_object)
        .ok_or_else(|| GlbError::InvalidDocument("missing asset object".into()))?;
    let version = str_field(asset, "version")
        .ok_or_else(|| GlbError::InvalidDocument("asset.version is required".into()))?;
    if version != "2.0" {
        return Err(GlbError::InvalidDocument(format!(
            "only glTF 2.0 is supported; asset.version is {version}"
        )));
    }

    let buffers = arr(root, "buffers");
    validate_buffers(buffers, container.bin)?;
    let views = arr(root, "bufferViews");
    validate_views(views, buffers, container.bin)?;
    let accessors = arr(root, "accessors");
    validate_accessors(accessors, views)?;
    let mut warnings = Vec::new();

    let samplers = parse_samplers(arr(root, "samplers"))?;
    let images = parse_images(
        arr(root, "images"),
        views,
        buffers,
        container.bin,
        &mut warnings,
    )?;
    let textures = parse_textures(arr(root, "textures"), images.len(), samplers.len())?;
    let materials = parse_materials(arr(root, "materials"), textures.len())?;
    let meshes = parse_meshes(arr(root, "meshes"), accessors, materials.len())?;
    let nodes = parse_nodes(arr(root, "nodes"), meshes.len())?;
    let scenes = parse_scenes(arr(root, "scenes"), nodes.len())?;

    let default_scene = opt_usize(root.get("scene"), "scene")?;
    if let Some(index) = default_scene {
        idx(index, scenes.len(), "default scene")?;
    }

    let extensions_used = strings(root.get("extensionsUsed"), "extensionsUsed")?;
    let extensions_required = strings(root.get("extensionsRequired"), "extensionsRequired")?;
    let understood = [
        "KHR_materials_emissive_strength",
        "KHR_materials_specular",
        "KHR_materials_transmission",
        "KHR_materials_ior",
        "KHR_materials_unlit",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let unsupported_required_extensions = extensions_required
        .iter()
        .filter(|name| !understood.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for ext in &unsupported_required_extensions {
        warnings.push(format!("required extension {ext} is not fully interpreted by the initial inspector; future conversion must remain blocked"));
    }

    let primitive_count = meshes.iter().map(|m| m.primitives.len()).sum();
    let digest = Sha256::digest(bytes);
    let byte_length = bytes.len();
    Ok(GlbInspectionReport {
        schema: INSPECTION_SCHEMA,
        schema_version: SCHEMA_VERSION,
        source: SourceInfo {
            file_name: file_name.into(),
            byte_length,
            sha256: format!("{digest:x}"),
        },
        container: ContainerInfo {
            version: 2,
            declared_length: byte_length,
            json_chunk_bytes: container.json_len,
            binary_chunk_bytes: container.bin.map_or(0, |v| v.len()),
            unknown_chunks: container.unknown,
        },
        asset: AssetInfo {
            version: version.into(),
            min_version: str_field(asset, "minVersion").map(str::to_string),
            generator: str_field(asset, "generator").map(str::to_string),
            copyright: str_field(asset, "copyright").map(str::to_string),
        },
        default_scene,
        scenes,
        nodes,
        meshes,
        materials,
        textures,
        images,
        samplers,
        counts: GlbCounts {
            scenes: arr(root, "scenes").len(),
            nodes: arr(root, "nodes").len(),
            meshes: arr(root, "meshes").len(),
            primitives: primitive_count,
            materials: arr(root, "materials").len(),
            textures: arr(root, "textures").len(),
            images: arr(root, "images").len(),
            samplers: arr(root, "samplers").len(),
            accessors: accessors.len(),
        },
        extensions_used,
        extensions_required,
        unsupported_required_extensions,
        warnings,
    })
}

pub(crate) fn parse_container(bytes: &[u8]) -> Result<Container<'_>, GlbError> {
    if bytes.len() < 12 {
        return Err(GlbError::InvalidContainer(
            "header is shorter than 12 bytes".into(),
        ));
    }
    if &bytes[..4] != MAGIC {
        return Err(GlbError::InvalidContainer("magic must be glTF".into()));
    }
    if u32_at(bytes, 4)? != 2 {
        return Err(GlbError::InvalidContainer(
            "container version must be 2".into(),
        ));
    }
    let declared = u32_at(bytes, 8)? as usize;
    if declared != bytes.len() {
        return Err(GlbError::InvalidContainer(format!(
            "header declares {declared} bytes but file has {}",
            bytes.len()
        )));
    }
    let mut cursor = 12usize;
    let mut json: Option<Value> = None;
    let mut json_len = 0;
    let mut bin = None;
    let mut unknown = Vec::new();
    let mut chunk = 0usize;
    while cursor < bytes.len() {
        if bytes.len() - cursor < 8 {
            return Err(GlbError::InvalidContainer("truncated chunk header".into()));
        }
        let length = u32_at(bytes, cursor)? as usize;
        let kind = u32_at(bytes, cursor + 4)?;
        let start = cursor + 8;
        let end = start
            .checked_add(length)
            .filter(|v| *v <= bytes.len())
            .ok_or_else(|| GlbError::InvalidContainer("chunk exceeds file bounds".into()))?;
        if length % 4 != 0 {
            return Err(GlbError::InvalidContainer(format!(
                "chunk {chunk} is not 4-byte aligned"
            )));
        }
        let payload = &bytes[start..end];
        match kind {
            JSON_CHUNK => {
                if chunk != 0 || json.is_some() {
                    return Err(GlbError::InvalidContainer(
                        "JSON must be the first and only JSON chunk".into(),
                    ));
                }
                if length > 64 * 1024 * 1024 {
                    return Err(GlbError::InvalidContainer(
                        "JSON chunk exceeds 64 MiB safety limit".into(),
                    ));
                }
                let end_json = payload
                    .iter()
                    .rposition(|v| !matches!(*v, b' ' | b'\t' | b'\r' | b'\n' | 0))
                    .map_or(0, |v| v + 1);
                json = Some(serde_json::from_slice(&payload[..end_json])?);
                json_len = length;
            }
            BIN_CHUNK => {
                if bin.is_some() {
                    return Err(GlbError::InvalidContainer(
                        "multiple BIN chunks are not supported".into(),
                    ));
                }
                bin = Some(payload);
            }
            other => unknown.push(UnknownChunkInfo {
                chunk_type: format!("0x{other:08X}"),
                byte_length: length,
            }),
        }
        cursor = end;
        chunk += 1;
    }
    let json = json.ok_or_else(|| GlbError::InvalidContainer("missing JSON chunk".into()))?;
    if !json.is_object() {
        return Err(GlbError::InvalidDocument(
            "JSON root must be an object".into(),
        ));
    }
    Ok(Container {
        json,
        json_len,
        bin,
        unknown,
    })
}

fn validate_buffers(values: &[Value], bin: Option<&[u8]>) -> Result<(), GlbError> {
    for (i, value) in values.iter().enumerate() {
        let length = req_usize(value, "byteLength", &format!("buffers[{i}]"))?;
        let uri = value.get("uri").and_then(Value::as_str);
        if uri.is_none() && i == 0 {
            let data = bin.ok_or_else(|| {
                GlbError::InvalidDocument("buffer 0 has no URI but BIN chunk is missing".into())
            })?;
            if length > data.len() || data.len() - length > 3 {
                return Err(GlbError::InvalidDocument(format!(
                    "buffers[0] byteLength {length} does not match BIN chunk {}",
                    data.len()
                )));
            }
        } else if uri.is_none() {
            return Err(GlbError::InvalidDocument(format!(
                "buffer {i} has no URI and cannot use the single GLB BIN chunk"
            )));
        }
    }
    Ok(())
}

fn validate_views(views: &[Value], buffers: &[Value], bin: Option<&[u8]>) -> Result<(), GlbError> {
    for (i, view) in views.iter().enumerate() {
        let buffer = req_usize(view, "buffer", &format!("bufferViews[{i}]"))?;
        idx(buffer, buffers.len(), &format!("bufferViews[{i}].buffer"))?;
        let offset = opt_usize(view.get("byteOffset"), "byteOffset")?.unwrap_or(0);
        let length = req_usize(view, "byteLength", &format!("bufferViews[{i}]"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| GlbError::InvalidDocument(format!("bufferViews[{i}] range overflow")))?;
        let declared = req_usize(
            &buffers[buffer],
            "byteLength",
            &format!("buffers[{buffer}]"),
        )?;
        if end > declared {
            return Err(GlbError::InvalidDocument(format!(
                "bufferViews[{i}] exceeds buffers[{buffer}]"
            )));
        }
        if buffer == 0 && buffers[buffer].get("uri").is_none() && end > bin.map_or(0, |v| v.len()) {
            return Err(GlbError::InvalidDocument(format!(
                "bufferViews[{i}] exceeds BIN chunk"
            )));
        }
        if let Some(stride) = opt_usize(view.get("byteStride"), "byteStride")? {
            if !(4..=252).contains(&stride) || stride % 4 != 0 {
                return Err(GlbError::InvalidDocument(format!(
                    "bufferViews[{i}].byteStride must be 4..252 and divisible by 4"
                )));
            }
        }
    }
    Ok(())
}

fn validate_accessors(accessors: &[Value], views: &[Value]) -> Result<(), GlbError> {
    for (i, value) in accessors.iter().enumerate() {
        if req_usize(value, "count", &format!("accessors[{i}]"))? == 0 {
            return Err(GlbError::InvalidDocument(format!(
                "accessors[{i}].count must be positive"
            )));
        }
        let component = req_u32(value, "componentType", &format!("accessors[{i}]"))?;
        if !matches!(component, 5120 | 5121 | 5122 | 5123 | 5125 | 5126) {
            return Err(GlbError::InvalidDocument(format!(
                "accessors[{i}] has invalid componentType {component}"
            )));
        }
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| GlbError::InvalidDocument(format!("accessors[{i}].type is required")))?;
        if !matches!(
            kind,
            "SCALAR" | "VEC2" | "VEC3" | "VEC4" | "MAT2" | "MAT3" | "MAT4"
        ) {
            return Err(GlbError::InvalidDocument(format!(
                "accessors[{i}] has invalid type {kind}"
            )));
        }
        if let Some(view) = opt_usize(value.get("bufferView"), "bufferView")? {
            idx(view, views.len(), &format!("accessors[{i}].bufferView"))?;
        } else if value.get("sparse").is_none() {
            return Err(GlbError::InvalidDocument(format!(
                "accessors[{i}] needs bufferView or sparse data"
            )));
        }
    }
    Ok(())
}

fn parse_scenes(values: &[Value], node_count: usize) -> Result<Vec<SceneInfo>, GlbError> {
    values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let nodes = idx_array(value.get("nodes"), &format!("scenes[{i}].nodes"))?;
            for node in &nodes {
                idx(*node, node_count, &format!("scenes[{i}].nodes"))?;
            }
            Ok(SceneInfo {
                index: i,
                name: name(value),
                nodes,
            })
        })
        .collect()
}
fn parse_nodes(values: &[Value], mesh_count: usize) -> Result<Vec<NodeInfo>, GlbError> {
    values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let mesh_index = opt_usize(value.get("mesh"), "mesh")?;
            if let Some(index) = mesh_index {
                idx(index, mesh_count, &format!("nodes[{i}].mesh"))?;
            }
            let children = idx_array(value.get("children"), &format!("nodes[{i}].children"))?;
            for child in &children {
                idx(*child, values.len(), &format!("nodes[{i}].children"))?;
            }
            let translation = num_vec(value.get("translation"), 3, "translation")?;
            let rotation = num_vec(value.get("rotation"), 4, "rotation")?;
            let scale = num_vec(value.get("scale"), 3, "scale")?;
            let matrix = num_vec(value.get("matrix"), 16, "matrix")?;
            if matrix.is_some() && (translation.is_some() || rotation.is_some() || scale.is_some())
            {
                return Err(GlbError::InvalidDocument(format!(
                    "nodes[{i}] cannot combine matrix and TRS"
                )));
            }
            Ok(NodeInfo {
                index: i,
                name: name(value),
                mesh_index,
                skin_index: opt_usize(value.get("skin"), "skin")?,
                camera_index: opt_usize(value.get("camera"), "camera")?,
                children,
                translation,
                rotation,
                scale,
                matrix,
            })
        })
        .collect()
}

fn parse_meshes(
    values: &[Value],
    accessors: &[Value],
    material_count: usize,
) -> Result<Vec<MeshInfo>, GlbError> {
    values
        .iter()
        .enumerate()
        .map(|(mi, mesh)| {
            let primitives = mesh
                .get("primitives")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    GlbError::InvalidDocument(format!("meshes[{mi}].primitives must be an array"))
                })?;
            let parsed = primitives
                .iter()
                .enumerate()
                .map(|(pi, primitive)| {
                    let mode = primitive.get("mode").and_then(Value::as_u64).unwrap_or(4) as u32;
                    let mode_name = match mode {
                        0 => "POINTS",
                        1 => "LINES",
                        2 => "LINE_LOOP",
                        3 => "LINE_STRIP",
                        4 => "TRIANGLES",
                        5 => "TRIANGLE_STRIP",
                        6 => "TRIANGLE_FAN",
                        _ => {
                            return Err(GlbError::InvalidDocument(format!(
                                "meshes[{mi}].primitives[{pi}] invalid mode {mode}"
                            )))
                        }
                    };
                    let material_index = opt_usize(primitive.get("material"), "material")?;
                    if let Some(index) = material_index {
                        idx(
                            index,
                            material_count,
                            &format!("meshes[{mi}].primitives[{pi}].material"),
                        )?;
                    }
                    let attrs = primitive
                        .get("attributes")
                        .and_then(Value::as_object)
                        .ok_or_else(|| {
                            GlbError::InvalidDocument(format!(
                                "meshes[{mi}].primitives[{pi}].attributes must be object"
                            ))
                        })?;
                    let mut attributes = BTreeMap::new();
                    for (semantic, accessor) in attrs {
                        let index = accessor.as_u64().ok_or_else(|| {
                            GlbError::InvalidDocument(format!(
                                "attribute {semantic} must be an accessor index"
                            ))
                        })? as usize;
                        attributes.insert(semantic.clone(), accessor_info(index, accessors)?);
                    }
                    let indices = opt_usize(primitive.get("indices"), "indices")?
                        .map(|index| accessor_info(index, accessors))
                        .transpose()?;
                    Ok(PrimitiveInfo {
                        index: pi,
                        mode,
                        mode_name,
                        material_index,
                        indices,
                        attributes,
                        morph_target_count: primitive
                            .get("targets")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len),
                        extensions: primitive.get("extensions").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect::<Result<Vec<_>, GlbError>>()?;
            Ok(MeshInfo {
                index: mi,
                name: name(mesh),
                primitives: parsed,
            })
        })
        .collect()
}
fn accessor_info(index: usize, values: &[Value]) -> Result<AccessorInfo, GlbError> {
    let value = values.get(index).ok_or_else(|| {
        GlbError::InvalidDocument(format!("accessor index {index} is out of range"))
    })?;
    Ok(AccessorInfo {
        index,
        count: req_usize(value, "count", "accessor")?,
        component_type: req_u32(value, "componentType", "accessor")?,
        element_type: value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
            .into(),
        normalized: value
            .get("normalized")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        min: number_array(value.get("min"), "accessor.min")?,
        max: number_array(value.get("max"), "accessor.max")?,
        sparse: value.get("sparse").is_some(),
        buffer_view_index: opt_usize(value.get("bufferView"), "bufferView")?,
    })
}

fn parse_materials(values: &[Value], texture_count: usize) -> Result<Vec<MaterialInfo>, GlbError> {
    values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let pbr = value.get("pbrMetallicRoughness");
            let ext = value.get("extensions").cloned().unwrap_or(Value::Null);
            let ext_obj = ext.as_object();
            let spec = ext_obj.and_then(|v| v.get("KHR_materials_specular"));
            let emissive_strength = ext_obj
                .and_then(|v| v.get("KHR_materials_emissive_strength"))
                .and_then(|v| v.get("emissiveStrength"))
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let alpha_mode = value
                .get("alphaMode")
                .and_then(Value::as_str)
                .unwrap_or("OPAQUE");
            if !matches!(alpha_mode, "OPAQUE" | "MASK" | "BLEND") {
                return Err(GlbError::InvalidDocument(format!(
                    "materials[{i}].alphaMode invalid: {alpha_mode}"
                )));
            }
            Ok(MaterialInfo {
                index: i,
                name: name(value),
                base_color_factor: num_vec(
                    pbr.and_then(|v| v.get("baseColorFactor")),
                    4,
                    "baseColorFactor",
                )?
                .map(|v| v.try_into().unwrap())
                .unwrap_or([1.0; 4]),
                base_color_texture: binding(
                    pbr.and_then(|v| v.get("baseColorTexture")),
                    texture_count,
                    None,
                    None,
                )?,
                metallic_factor: pbr
                    .and_then(|v| v.get("metallicFactor"))
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0),
                roughness_factor: pbr
                    .and_then(|v| v.get("roughnessFactor"))
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0),
                metallic_roughness_texture: binding(
                    pbr.and_then(|v| v.get("metallicRoughnessTexture")),
                    texture_count,
                    None,
                    None,
                )?,
                normal_texture: binding(
                    value.get("normalTexture"),
                    texture_count,
                    Some("scale"),
                    None,
                )?,
                occlusion_texture: binding(
                    value.get("occlusionTexture"),
                    texture_count,
                    None,
                    Some("strength"),
                )?,
                emissive_factor: num_vec(value.get("emissiveFactor"), 3, "emissiveFactor")?
                    .map(|v| v.try_into().unwrap())
                    .unwrap_or([0.0; 3]),
                emissive_texture: binding(value.get("emissiveTexture"), texture_count, None, None)?,
                emissive_strength,
                specular_texture: binding(
                    spec.and_then(|v| v.get("specularTexture")),
                    texture_count,
                    None,
                    None,
                )?,
                specular_color_texture: binding(
                    spec.and_then(|v| v.get("specularColorTexture")),
                    texture_count,
                    None,
                    None,
                )?,
                transmission_factor: ext_obj
                    .and_then(|v| v.get("KHR_materials_transmission"))
                    .and_then(|v| v.get("transmissionFactor"))
                    .and_then(Value::as_f64),
                ior: ext_obj
                    .and_then(|v| v.get("KHR_materials_ior"))
                    .and_then(|v| v.get("ior"))
                    .and_then(Value::as_f64),
                alpha_mode: alpha_mode.into(),
                alpha_cutoff: value
                    .get("alphaCutoff")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5),
                double_sided: value
                    .get("doubleSided")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                unlit: ext_obj.is_some_and(|v| v.contains_key("KHR_materials_unlit")),
                extensions: ext,
            })
        })
        .collect()
}
fn binding(
    value: Option<&Value>,
    texture_count: usize,
    scale_key: Option<&str>,
    strength_key: Option<&str>,
) -> Result<Option<TextureBindingInfo>, GlbError> {
    let Some(value) = value else { return Ok(None) };
    let index = req_usize(value, "index", "textureInfo")?;
    idx(index, texture_count, "material texture")?;
    Ok(Some(TextureBindingInfo {
        texture_index: index,
        tex_coord: opt_usize(value.get("texCoord"), "texCoord")?.unwrap_or(0),
        scale: scale_key.and_then(|k| value.get(k)).and_then(Value::as_f64),
        strength: strength_key
            .and_then(|k| value.get(k))
            .and_then(Value::as_f64),
    }))
}
fn parse_textures(
    values: &[Value],
    image_count: usize,
    sampler_count: usize,
) -> Result<Vec<TextureInfo>, GlbError> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let source_image_index = opt_usize(v.get("source"), "texture.source")?;
            let sampler_index = opt_usize(v.get("sampler"), "texture.sampler")?;
            if let Some(index) = source_image_index {
                idx(index, image_count, &format!("textures[{i}].source"))?;
            }
            if let Some(index) = sampler_index {
                idx(index, sampler_count, &format!("textures[{i}].sampler"))?;
            }
            Ok(TextureInfo {
                index: i,
                name: name(v),
                source_image_index,
                sampler_index,
                extensions: v.get("extensions").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}
fn parse_images(
    values: &[Value],
    views: &[Value],
    buffers: &[Value],
    bin: Option<&[u8]>,
    warnings: &mut Vec<String>,
) -> Result<Vec<ImageInfo>, GlbError> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let uri = v.get("uri").and_then(Value::as_str).map(str::to_string);
            let view = opt_usize(v.get("bufferView"), "image.bufferView")?;
            if uri.is_some() && view.is_some() {
                return Err(GlbError::InvalidDocument(format!(
                    "images[{i}] has both uri and bufferView"
                )));
            }
            let mime_type = v
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_string);
            let (storage, len, dims, embedded) = if let Some(view_index) = view {
                idx(view_index, views.len(), "image.bufferView")?;
                if mime_type.is_none() {
                    return Err(GlbError::InvalidDocument(format!(
                        "images[{i}] bufferView needs mimeType"
                    )));
                }
                let view_value = &views[view_index];
                let buffer = req_usize(view_value, "buffer", "image bufferView")?;
                idx(buffer, buffers.len(), "image buffer")?;
                let offset = opt_usize(view_value.get("byteOffset"), "byteOffset")?.unwrap_or(0);
                let length = req_usize(view_value, "byteLength", "image bufferView")?;
                let embedded = buffer == 0 && buffers[0].get("uri").is_none();
                let image = if embedded {
                    bin.and_then(|b| b.get(offset..offset.saturating_add(length)))
                        .and_then(image_dims)
                } else {
                    warnings.push(format!(
                        "image {i} is stored in an external buffer and is not embedded in this GLB"
                    ));
                    None
                };
                ("bufferView".into(), Some(length), image, embedded)
            } else if let Some(uri) = &uri {
                if uri.starts_with("data:") {
                    let length = uri
                        .split_once(',')
                        .map(|(_, body)| body.len().saturating_mul(3) / 4);
                    ("dataUri".into(), length, None, true)
                } else {
                    warnings.push(format!(
                        "image {i} references external URI {uri}; it is not embedded"
                    ));
                    ("externalUri".into(), None, None, false)
                }
            } else {
                return Err(GlbError::InvalidDocument(format!(
                    "images[{i}] needs uri or bufferView"
                )));
            };
            Ok(ImageInfo {
                index: i,
                name: name(v),
                mime_type,
                storage,
                uri,
                buffer_view_index: view,
                byte_length: len,
                dimensions: dims,
                embedded,
            })
        })
        .collect()
}
fn parse_samplers(values: &[Value]) -> Result<Vec<SamplerInfo>, GlbError> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            Ok(SamplerInfo {
                index: i,
                name: name(v),
                mag_filter: opt_u32(v.get("magFilter"), "magFilter")?,
                min_filter: opt_u32(v.get("minFilter"), "minFilter")?,
                wrap_s: opt_u32(v.get("wrapS"), "wrapS")?.unwrap_or(10497),
                wrap_t: opt_u32(v.get("wrapT"), "wrapT")?.unwrap_or(10497),
            })
        })
        .collect()
}

fn image_dims(b: &[u8]) -> Option<ImageDimensions> {
    if b.len() >= 24 && &b[..8] == b"\x89PNG\r\n\x1a\n" && &b[12..16] == b"IHDR" {
        let w = u32::from_be_bytes(b[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(b[20..24].try_into().ok()?);
        return (w > 0 && h > 0).then_some(ImageDimensions {
            width: w,
            height: h,
        });
    }
    None
}
fn num_vec(
    value: Option<&Value>,
    expected: usize,
    label: &str,
) -> Result<Option<Vec<f64>>, GlbError> {
    let Some(value) = value else { return Ok(None) };
    let arr = value
        .as_array()
        .ok_or_else(|| GlbError::InvalidDocument(format!("{label} must be an array")))?;
    if arr.len() != expected {
        return Err(GlbError::InvalidDocument(format!(
            "{label} must contain {expected} numbers"
        )));
    }
    let mut result = Vec::with_capacity(expected);
    for item in arr {
        let n = item.as_f64().filter(|n| n.is_finite()).ok_or_else(|| {
            GlbError::InvalidDocument(format!(
                "{label} contains a non-finite or non-numeric value"
            ))
        })?;
        result.push(n);
    }
    Ok(Some(result))
}
fn number_array(value: Option<&Value>, label: &str) -> Result<Option<Vec<f64>>, GlbError> {
    num_vec(
        value,
        value.and_then(Value::as_array).map_or(0, Vec::len),
        label,
    )
}
fn idx_array(value: Option<&Value>, label: &str) -> Result<Vec<usize>, GlbError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .ok_or_else(|| GlbError::InvalidDocument(format!("{label} must be an array")))?
        .iter()
        .map(|v| {
            v.as_u64()
                .map(|n| n as usize)
                .ok_or_else(|| GlbError::InvalidDocument(format!("{label} must contain indices")))
        })
        .collect()
}
fn strings(value: Option<&Value>, label: &str) -> Result<Vec<String>, GlbError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .ok_or_else(|| GlbError::InvalidDocument(format!("{label} must be an array")))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| GlbError::InvalidDocument(format!("{label} must contain strings")))
        })
        .collect()
}
fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
fn name(v: &Value) -> Option<String> {
    v.get("name").and_then(Value::as_str).map(str::to_string)
}
fn str_field<'a>(v: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}
fn req_usize(v: &Value, key: &str, scope: &str) -> Result<usize, GlbError> {
    v.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| {
            GlbError::InvalidDocument(format!("{scope}.{key} must be a non-negative integer"))
        })
}
fn opt_usize(v: Option<&Value>, label: &str) -> Result<Option<usize>, GlbError> {
    v.map(|v| {
        v.as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| {
                GlbError::InvalidDocument(format!("{label} must be a non-negative integer"))
            })
    })
    .transpose()
}
fn req_u32(v: &Value, key: &str, scope: &str) -> Result<u32, GlbError> {
    v.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| GlbError::InvalidDocument(format!("{scope}.{key} must be a u32")))
}
fn opt_u32(v: Option<&Value>, label: &str) -> Result<Option<u32>, GlbError> {
    v.map(|v| {
        v.as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| GlbError::InvalidDocument(format!("{label} must be a u32")))
    })
    .transpose()
}
pub(crate) fn idx(index: usize, count: usize, label: &str) -> Result<(), GlbError> {
    if index >= count {
        return Err(GlbError::InvalidDocument(format!(
            "{label} index {index} out of range (count {count})"
        )));
    }
    Ok(())
}
fn u32_at(b: &[u8], o: usize) -> Result<u32, GlbError> {
    let s = b
        .get(o..o + 4)
        .ok_or_else(|| GlbError::InvalidContainer("truncated u32".into()))?;
    Ok(u32::from_le_bytes(s.try_into().expect("4 bytes")))
}
