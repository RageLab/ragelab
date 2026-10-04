//! Renderer-neutral render packets shared by native, Three.js and WASM consumers.
//!
//! RAGE parsing stays in Core. Consumers receive deterministic metadata plus a
//! little-endian binary blob with typed geometry/index/texture buffers.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use ragelab_assets::AssetKind;
use ragelab_hash::joaat;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    parse_model_preview_source, prepare_scene_asset_preview_input, resolve_diffuse_textures,
    workspace_scene_manifest_with_sources, PreviewOptions, PreviewTextureDictionarySource,
    ResolvedDiffuseTexture, SceneAssemblyOptions, SceneAssetPreviewSources, SceneAssetReference,
    SceneAssetSelector, SceneManifest, SceneNode, SpatialTransform, MODEL_DIFFUSE_TEXTURE_LIMIT,
    MODEL_DIFFUSE_TEXTURE_MAX_DIMENSION,
};

pub const RENDER_PACKAGE_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_RENDER_MAX_ASSETS: usize = 96;
pub const HARD_RENDER_MAX_ASSETS: usize = 512;
pub const DEFAULT_RENDER_MAX_BLOB_BYTES: usize = 128 * 1024 * 1024;
pub const HARD_RENDER_MAX_BLOB_BYTES: usize = 512 * 1024 * 1024;

const MAGIC: &[u8; 8] = b"RLRPKT01";
const HEADER_BYTES: usize = 24;
const HARD_METADATA_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderPackageOptions {
    pub preview: PreviewOptions,
    pub max_assets: usize,
    pub max_blob_bytes: usize,
}

impl Default for RenderPackageOptions {
    fn default() -> Self {
        Self {
            preview: PreviewOptions::default(),
            max_assets: DEFAULT_RENDER_MAX_ASSETS,
            max_blob_bytes: DEFAULT_RENDER_MAX_BLOB_BYTES,
        }
    }
}

impl RenderPackageOptions {
    pub fn validate(self) -> Result<Self, io::Error> {
        self.preview.validate()?;
        if !(1..=HARD_RENDER_MAX_ASSETS).contains(&self.max_assets) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("render max_assets must be within 1..={HARD_RENDER_MAX_ASSETS}"),
            ));
        }
        if !(1..=HARD_RENDER_MAX_BLOB_BYTES).contains(&self.max_blob_bytes) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("render max_blob_bytes must be within 1..={HARD_RENDER_MAX_BLOB_BYTES}"),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderPackage {
    pub descriptor: RenderPackageDescriptor,
    pub blob: Vec<u8>,
}

impl RenderPackage {
    pub fn encode_binary(&self) -> Result<Vec<u8>, io::Error> {
        self.validate()?;
        let metadata = serde_json::to_vec(&self.descriptor)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        if metadata.len() > HARD_METADATA_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render package metadata exceeds hard limit",
            ));
        }
        let metadata_len = u32::try_from(metadata.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "metadata length does not fit u32",
            )
        })?;
        let blob_len = u64::try_from(self.blob.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "blob length does not fit u64")
        })?;
        let mut output = Vec::with_capacity(
            HEADER_BYTES
                .checked_add(metadata.len())
                .and_then(|size| size.checked_add(self.blob.len()))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "package size overflow")
                })?,
        );
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&RENDER_PACKAGE_SCHEMA_VERSION.to_le_bytes());
        output.extend_from_slice(&metadata_len.to_le_bytes());
        output.extend_from_slice(&blob_len.to_le_bytes());
        output.extend_from_slice(&metadata);
        output.extend_from_slice(&self.blob);
        Ok(output)
    }

    pub fn decode_binary(bytes: &[u8]) -> Result<Self, io::Error> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid render package header",
            ));
        }
        let schema = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if schema != RENDER_PACKAGE_SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported render package schema {schema}; expected {RENDER_PACKAGE_SCHEMA_VERSION}"
                ),
            ));
        }
        let metadata_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let blob_len = usize::try_from(u64::from_le_bytes(bytes[16..24].try_into().unwrap()))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "blob length overflow"))?;
        if metadata_len > HARD_METADATA_BYTES || blob_len > HARD_RENDER_MAX_BLOB_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render package exceeds hard transport limits",
            ));
        }
        let metadata_end = HEADER_BYTES
            .checked_add(metadata_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "metadata range overflow"))?;
        let expected = metadata_end
            .checked_add(blob_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "blob range overflow"))?;
        if expected != bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render package length does not match header",
            ));
        }
        let descriptor = serde_json::from_slice(&bytes[HEADER_BYTES..metadata_end])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let package = Self {
            descriptor,
            blob: bytes[metadata_end..].to_vec(),
        };
        package.validate()?;
        Ok(package)
    }

    pub fn validate(&self) -> Result<(), io::Error> {
        if self.descriptor.schema_version != RENDER_PACKAGE_SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render descriptor schema mismatch",
            ));
        }
        if self.blob.len() > self.descriptor.limits.max_blob_bytes as usize
            || self.blob.len() > HARD_RENDER_MAX_BLOB_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render blob exceeds declared or hard limit",
            ));
        }
        if self.descriptor.assets.len() != self.descriptor.summary.assets as usize
            || self.descriptor.scene.instances.len() != self.descriptor.summary.instances as usize
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render summary counts do not match descriptor",
            ));
        }
        for (asset_index, asset) in self.descriptor.assets.iter().enumerate() {
            if asset.asset_ref != asset_index as u32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "asset_ref ordering is not canonical",
                ));
            }
            for mesh in &asset.meshes {
                mesh.positions.validate(self.blob.len())?;
                mesh.indices.validate(self.blob.len())?;
                if let Some(view) = mesh.normals {
                    view.validate(self.blob.len())?;
                }
                if let Some(view) = mesh.uv0 {
                    view.validate(self.blob.len())?;
                }
                if mesh
                    .material_ref
                    .is_some_and(|value| value as usize >= asset.materials.len())
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "mesh material_ref is out of range",
                    ));
                }
            }
            for material in &asset.materials {
                if material
                    .diffuse_texture_ref
                    .is_some_and(|value| value as usize >= self.descriptor.textures.len())
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "material texture_ref is out of range",
                    ));
                }
            }
        }
        for instance in &self.descriptor.scene.instances {
            if instance
                .asset_ref
                .is_some_and(|value| value as usize >= self.descriptor.assets.len())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "instance asset_ref is out of range",
                ));
            }
        }
        for (texture_index, texture) in self.descriptor.textures.iter().enumerate() {
            if texture.id != texture_index as u32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "texture IDs are not canonical",
                ));
            }
            texture.data.validate(self.blob.len())?;
            let pixels = u32::from(texture.width) * u32::from(texture.height);
            if texture.data.element_type != RenderElementType::U8
                || texture.data.components != 4
                || texture.data.count != pixels
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "texture data is not canonical RGBA8",
                ));
            }
        }
        Ok(())
    }

    pub fn report(&self) -> RenderPackageReport {
        let metadata_bytes = serde_json::to_vec(&self.descriptor)
            .map(|value| value.len() as u64)
            .unwrap_or(0);
        RenderPackageReport {
            schema: "ragelab.render-package",
            schema_version: self.descriptor.schema_version,
            summary: self.descriptor.summary,
            limits: self.descriptor.limits,
            metadata_bytes,
            blob_bytes: self.blob.len() as u64,
            encoded_bytes: metadata_bytes
                .saturating_add(self.blob.len() as u64)
                .saturating_add(HEADER_BYTES as u64),
            diagnostics: self.descriptor.diagnostics.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderPackageDescriptor {
    pub schema_version: u32,
    pub scene: RenderSceneDescriptor,
    pub assets: Vec<RenderAssetDescriptor>,
    pub textures: Vec<RenderTextureDescriptor>,
    pub diagnostics: Vec<RenderDiagnostic>,
    pub summary: RenderPackageSummary,
    pub limits: RenderPackageLimits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderSceneDescriptor {
    pub root: RenderSceneRoot,
    pub instances: Vec<RenderInstanceDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderSceneRoot {
    pub path: String,
    pub name_hash: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderInstanceDescriptor {
    pub node_index: u32,
    pub source_ymap: String,
    pub entity_index: u32,
    pub archetype_hash: u32,
    pub provider_path: Option<String>,
    pub asset_ref: Option<u32>,
    pub asset_kind: Option<String>,
    pub transform: Option<RenderTransform>,
    pub resolution: String,
    pub reason: Option<RenderResolutionReason>,
    pub collision: Option<RenderCollisionReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderTransform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: Option<[f32; 3]>,
}

impl From<SpatialTransform> for RenderTransform {
    fn from(value: SpatialTransform) -> Self {
        Self {
            translation: value.translation,
            rotation: value.rotation,
            scale: value.scale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderResolutionReason {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderCollisionReference {
    pub hash: u32,
    pub asset_ref: Option<u32>,
    pub state: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderAssetDescriptor {
    pub asset_ref: u32,
    pub kind: String,
    pub hash: Option<u32>,
    pub source: RenderSourceDescriptor,
    pub selector: Option<RenderAssetSelectorDescriptor>,
    pub state: RenderAssetState,
    pub coordinate_convention: Option<String>,
    pub name: Option<String>,
    pub lod: Option<String>,
    pub bounds: Option<RenderBounds>,
    pub meshes: Vec<RenderMeshDescriptor>,
    pub materials: Vec<RenderMaterialDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderSourceDescriptor {
    pub source_type: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderAssetSelectorDescriptor {
    #[serde(rename = "type")]
    pub selector_type: String,
    pub index: u32,
    pub name_hash: Option<u32>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderAssetState {
    Ready,
    Unsupported,
    Error,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderBounds {
    pub center: [f32; 3],
    pub radius: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderMeshDescriptor {
    pub id: u32,
    pub model_index: u32,
    pub geometry_index: u32,
    pub topology: String,
    pub source_shader_index: Option<u32>,
    pub material_ref: Option<u32>,
    pub positions: RenderBufferView,
    pub normals: Option<RenderBufferView>,
    pub uv0: Option<RenderBufferView>,
    pub indices: RenderBufferView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderMaterialDescriptor {
    pub id: u32,
    pub source_shader_index: u32,
    pub shader_name_hash: u32,
    pub shader_file_hash: u32,
    pub diffuse_texture_name: Option<String>,
    pub diffuse_texture_ref: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderTextureDescriptor {
    pub id: u32,
    pub name: String,
    pub name_hash: u32,
    pub source: String,
    pub source_path: String,
    pub original_width: u16,
    pub original_height: u16,
    pub width: u16,
    pub height: u16,
    pub downscaled: bool,
    pub format: RenderTextureFormat,
    pub data: RenderBufferView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderTextureFormat {
    Rgba8Srgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderBufferView {
    pub offset: u64,
    pub byte_length: u64,
    pub element_type: RenderElementType,
    pub components: u8,
    pub count: u32,
}

impl RenderBufferView {
    fn validate(self, blob_len: usize) -> Result<(), io::Error> {
        if self.components == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render buffer view has zero components",
            ));
        }
        let expected = u64::from(self.count)
            .checked_mul(u64::from(self.components))
            .and_then(|value| value.checked_mul(self.element_type.byte_width()))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "buffer size overflow"))?;
        if expected != self.byte_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render buffer byte_length does not match typed element count",
            ));
        }
        if self.offset % self.element_type.alignment() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render buffer offset is not aligned",
            ));
        }
        let end = self
            .offset
            .checked_add(self.byte_length)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "buffer range overflow"))?;
        if end > blob_len as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "render buffer view exceeds blob",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderElementType {
    #[serde(rename = "f32")]
    F32,
    #[serde(rename = "u32")]
    U32,
    #[serde(rename = "u8")]
    U8,
}

impl RenderElementType {
    const fn byte_width(self) -> u64 {
        match self {
            Self::F32 | Self::U32 => 4,
            Self::U8 => 1,
        }
    }

    const fn alignment(self) -> u64 {
        match self {
            Self::F32 | Self::U32 => 4,
            Self::U8 => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderDiagnostic {
    pub severity: RenderDiagnosticSeverity,
    pub code: String,
    pub message: String,
    pub asset_ref: Option<u32>,
    pub node_index: Option<u32>,
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderPackageSummary {
    pub instances: u32,
    pub assets: u32,
    pub ready_assets: u32,
    pub meshes: u32,
    pub materials: u32,
    pub textures: u32,
    pub blob_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderPackageLimits {
    pub max_assets: u32,
    pub max_blob_bytes: u64,
    pub omitted_assets: u32,
    pub max_primitives_per_asset: u32,
    pub max_vertices_per_asset: u32,
    pub max_indices_per_asset: u32,
    pub max_shaders_per_asset: u32,
    pub max_diffuse_textures_per_asset: u32,
    pub max_texture_dimension: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderPackageReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub summary: RenderPackageSummary,
    pub limits: RenderPackageLimits,
    pub metadata_bytes: u64,
    pub blob_bytes: u64,
    pub encoded_bytes: u64,
    pub diagnostics: Vec<RenderDiagnostic>,
}

pub fn workspace_scene_render_package(
    workspace: &Path,
    ymap_path: &Path,
    scene_options: SceneAssemblyOptions,
    render_options: RenderPackageOptions,
) -> Result<RenderPackage, io::Error> {
    workspace_scene_render_package_with_sources(
        workspace,
        ymap_path,
        &[],
        &[],
        scene_options,
        render_options,
    )
}

pub fn workspace_scene_render_package_with_sources(
    workspace: &Path,
    ymap_path: &Path,
    fallback_roots: &[PathBuf],
    rpf_mounts: &[crate::SceneRpfMount],
    scene_options: SceneAssemblyOptions,
    render_options: RenderPackageOptions,
) -> Result<RenderPackage, io::Error> {
    workspace_scene_render_package_with_game_index(
        workspace,
        ymap_path,
        SceneAssetPreviewSources {
            fallback_roots,
            rpf_mounts,
            game_index: None,
        },
        scene_options,
        render_options,
    )
}

pub fn workspace_scene_render_package_with_game_index(
    workspace: &Path,
    ymap_path: &Path,
    sources: SceneAssetPreviewSources<'_>,
    scene_options: SceneAssemblyOptions,
    render_options: RenderPackageOptions,
) -> Result<RenderPackage, io::Error> {
    let options = render_options.validate()?;
    let manifest = workspace_scene_manifest_with_sources(
        workspace,
        ymap_path,
        sources.fallback_roots,
        sources.rpf_mounts,
        sources.game_index,
        scene_options,
    )?;
    build_scene_render_package(&manifest, options)
}

pub fn render_asset_package_bytes_as(
    path_label: &str,
    asset_type: &str,
    bytes: &[u8],
    preview_options: PreviewOptions,
    external_texture_dictionaries: &[PreviewTextureDictionarySource<'_>],
    max_blob_bytes: usize,
) -> Result<RenderPackage, io::Error> {
    let options = RenderPackageOptions {
        preview: preview_options,
        max_assets: 1,
        max_blob_bytes,
    }
    .validate()?;
    let mut builder = PackageBuilder::new(options.max_blob_bytes);
    let selector = if let Some(index) = preview_options.drawable_index {
        Some(RenderAssetSelectorDescriptor {
            selector_type: "yddDrawable".into(),
            index: to_u32(index, "drawable selector index")?,
            name_hash: None,
            name: None,
        })
    } else {
        None
    };
    let asset = builder.build_model_asset(
        0,
        asset_type,
        None,
        RenderSourceDescriptor {
            source_type: "directBytes".into(),
            path: path_label.to_string(),
        },
        selector,
        path_label,
        asset_type,
        bytes,
        options.preview,
        external_texture_dictionaries,
        &[],
    )?;
    let assets = vec![asset];
    let summary = summarize(&[], &assets, &builder.textures, builder.blob.data.len());
    let descriptor = RenderPackageDescriptor {
        schema_version: RENDER_PACKAGE_SCHEMA_VERSION,
        scene: RenderSceneDescriptor {
            root: RenderSceneRoot {
                path: path_label.to_string(),
                name_hash: None,
            },
            instances: Vec::new(),
        },
        assets,
        textures: builder.textures,
        diagnostics: builder.diagnostics,
        summary,
        limits: render_limits(options, 0),
    };
    let package = RenderPackage {
        descriptor,
        blob: builder.blob.data,
    };
    package.validate()?;
    Ok(package)
}

fn build_scene_render_package(
    manifest: &SceneManifest,
    options: RenderPackageOptions,
) -> Result<RenderPackage, io::Error> {
    let mut builder = PackageBuilder::new(options.max_blob_bytes);
    let mut assets = Vec::with_capacity(manifest.assets.len());
    let mut omitted_assets = 0_u32;

    for asset in &manifest.assets {
        if asset.id >= options.max_assets {
            omitted_assets = omitted_assets.saturating_add(1);
            builder.diagnostics.push(RenderDiagnostic {
                severity: RenderDiagnosticSeverity::Warning,
                code: "assetBudgetExceeded".into(),
                message: format!(
                    "assetRef {} omitted because render max_assets is {}",
                    asset.id, options.max_assets
                ),
                asset_ref: Some(to_u32(asset.id, "assetRef")?),
                node_index: None,
                details: None,
            });
            assets.push(render_asset_shell(asset, RenderAssetState::Omitted)?);
            continue;
        }

        match asset.kind {
            AssetKind::Ydr | AssetKind::Ydd | AssetKind::Yft => {
                let input = match prepare_scene_asset_preview_input(asset, options.preview) {
                    Ok(input) => input,
                    Err(error) => {
                        builder.diagnostics.push(RenderDiagnostic {
                            severity: RenderDiagnosticSeverity::Error,
                            code: "assetReadFailed".into(),
                            message: error.to_string(),
                            asset_ref: Some(to_u32(asset.id, "assetRef")?),
                            node_index: None,
                            details: None,
                        });
                        assets.push(render_asset_shell(asset, RenderAssetState::Error)?);
                        continue;
                    }
                };
                let external = input.external_sources();
                let built = builder.build_model_asset(
                    to_u32(asset.id, "assetRef")?,
                    &asset.kind.to_string(),
                    Some(asset.hash),
                    RenderSourceDescriptor {
                        source_type: asset.path.source_type().to_string(),
                        path: asset.path.provenance(),
                    },
                    render_selector(asset)?,
                    &input.path_label,
                    input.asset_type,
                    &input.bytes,
                    input.options,
                    &external,
                    &input.external_texture_errors,
                )?;
                assets.push(built);
            }
            AssetKind::Ybn => {
                builder.diagnostics.push(RenderDiagnostic {
                    severity: RenderDiagnosticSeverity::Info,
                    code: "collisionAssetDeferred".into(),
                    message: "YBN remains dependency-only in render package schema v1".into(),
                    asset_ref: Some(to_u32(asset.id, "assetRef")?),
                    node_index: None,
                    details: None,
                });
                assets.push(render_asset_shell(asset, RenderAssetState::Unsupported)?);
            }
            _ => {
                builder.diagnostics.push(RenderDiagnostic {
                    severity: RenderDiagnosticSeverity::Warning,
                    code: "unsupportedAssetKind".into(),
                    message: format!("{} is not renderable by schema v1", asset.kind),
                    asset_ref: Some(to_u32(asset.id, "assetRef")?),
                    node_index: None,
                    details: None,
                });
                assets.push(render_asset_shell(asset, RenderAssetState::Unsupported)?);
            }
        }
    }

    let instances = manifest
        .nodes
        .iter()
        .map(render_instance)
        .collect::<Result<Vec<_>, _>>()?;
    let summary = summarize(
        &instances,
        &assets,
        &builder.textures,
        builder.blob.data.len(),
    );
    let descriptor = RenderPackageDescriptor {
        schema_version: RENDER_PACKAGE_SCHEMA_VERSION,
        scene: RenderSceneDescriptor {
            root: RenderSceneRoot {
                path: manifest.root.path.display().to_string(),
                name_hash: manifest.root.name_hash,
            },
            instances,
        },
        assets,
        textures: builder.textures,
        diagnostics: builder.diagnostics,
        summary,
        limits: render_limits(options, omitted_assets),
    };
    let package = RenderPackage {
        descriptor,
        blob: builder.blob.data,
    };
    package.validate()?;
    Ok(package)
}

fn render_asset_shell(
    asset: &SceneAssetReference,
    state: RenderAssetState,
) -> Result<RenderAssetDescriptor, io::Error> {
    Ok(RenderAssetDescriptor {
        asset_ref: to_u32(asset.id, "assetRef")?,
        kind: asset.kind.to_string(),
        hash: Some(asset.hash),
        source: RenderSourceDescriptor {
            source_type: asset.path.source_type().to_string(),
            path: asset.path.provenance(),
        },
        selector: render_selector(asset)?,
        state,
        coordinate_convention: None,
        name: None,
        lod: None,
        bounds: None,
        meshes: Vec::new(),
        materials: Vec::new(),
    })
}

fn render_selector(
    asset: &SceneAssetReference,
) -> Result<Option<RenderAssetSelectorDescriptor>, io::Error> {
    match asset.selector.as_ref() {
        None => Ok(None),
        Some(SceneAssetSelector::YddDrawable {
            index,
            name_hash,
            name,
        }) => Ok(Some(RenderAssetSelectorDescriptor {
            selector_type: "yddDrawable".into(),
            index: to_u32(*index, "drawable selector index")?,
            name_hash: Some(*name_hash),
            name: name.clone(),
        })),
    }
}

fn render_instance(node: &SceneNode) -> Result<RenderInstanceDescriptor, io::Error> {
    let collision = if let Some(collision) = node.collision.as_ref() {
        Some(RenderCollisionReference {
            hash: collision.hash,
            asset_ref: collision
                .asset_ref
                .map(|value| to_u32(value, "collision assetRef"))
                .transpose()?,
            state: collision.state.as_str().to_string(),
            reason: collision.reason.clone(),
        })
    } else {
        None
    };

    Ok(RenderInstanceDescriptor {
        node_index: to_u32(node.index, "node index")?,
        source_ymap: node.source_ymap.display().to_string(),
        entity_index: to_u32(node.entity_index, "entity index")?,
        archetype_hash: node.archetype_hash,
        provider_path: node.provider_path.clone(),
        asset_ref: node
            .asset_ref
            .map(|value| to_u32(value, "instance assetRef"))
            .transpose()?,
        asset_kind: node.asset_kind.map(|kind| kind.to_string()),
        transform: node.transform.map(RenderTransform::from),
        resolution: node.resolution.as_str().to_string(),
        reason: node.reason.as_ref().map(|reason| RenderResolutionReason {
            code: reason.code.as_str().to_string(),
            message: reason.message.clone(),
        }),
        collision,
    })
}

fn render_limits(options: RenderPackageOptions, omitted_assets: u32) -> RenderPackageLimits {
    RenderPackageLimits {
        max_assets: options.max_assets as u32,
        max_blob_bytes: options.max_blob_bytes as u64,
        omitted_assets,
        max_primitives_per_asset: options.preview.max_primitives as u32,
        max_vertices_per_asset: options.preview.max_vertices as u32,
        max_indices_per_asset: options.preview.max_indices as u32,
        max_shaders_per_asset: options.preview.max_shaders as u32,
        max_diffuse_textures_per_asset: MODEL_DIFFUSE_TEXTURE_LIMIT as u32,
        max_texture_dimension: MODEL_DIFFUSE_TEXTURE_MAX_DIMENSION,
    }
}

fn summarize(
    instances: &[RenderInstanceDescriptor],
    assets: &[RenderAssetDescriptor],
    textures: &[RenderTextureDescriptor],
    blob_bytes: usize,
) -> RenderPackageSummary {
    RenderPackageSummary {
        instances: instances.len().try_into().unwrap_or(u32::MAX),
        assets: assets.len().try_into().unwrap_or(u32::MAX),
        ready_assets: assets
            .iter()
            .filter(|asset| asset.state == RenderAssetState::Ready)
            .count()
            .try_into()
            .unwrap_or(u32::MAX),
        meshes: assets
            .iter()
            .map(|asset| asset.meshes.len())
            .sum::<usize>()
            .try_into()
            .unwrap_or(u32::MAX),
        materials: assets
            .iter()
            .map(|asset| asset.materials.len())
            .sum::<usize>()
            .try_into()
            .unwrap_or(u32::MAX),
        textures: textures.len().try_into().unwrap_or(u32::MAX),
        blob_bytes: blob_bytes as u64,
    }
}

fn to_u32(value: usize, label: &str) -> Result<u32, io::Error> {
    u32::try_from(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label} {value} does not fit u32"),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TextureIdentityKey {
    source: String,
    source_path: String,
    name_hash: u32,
    width: u16,
    height: u16,
}

struct PackageBuilder {
    blob: BlobBuilder,
    textures: Vec<RenderTextureDescriptor>,
    texture_ids: BTreeMap<TextureIdentityKey, u32>,
    diagnostics: Vec<RenderDiagnostic>,
}

impl PackageBuilder {
    fn new(max_blob_bytes: usize) -> Self {
        Self {
            blob: BlobBuilder::new(max_blob_bytes),
            textures: Vec::new(),
            texture_ids: BTreeMap::new(),
            diagnostics: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_model_asset(
        &mut self,
        asset_ref: u32,
        kind: &str,
        hash: Option<u32>,
        source: RenderSourceDescriptor,
        selector: Option<RenderAssetSelectorDescriptor>,
        path_label: &str,
        asset_type: &str,
        bytes: &[u8],
        options: PreviewOptions,
        external_texture_dictionaries: &[PreviewTextureDictionarySource<'_>],
        external_texture_errors: &[String],
    ) -> Result<RenderAssetDescriptor, io::Error> {
        let parsed = match parse_model_preview_source(asset_type, bytes, options) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.diagnostics.push(RenderDiagnostic {
                    severity: RenderDiagnosticSeverity::Error,
                    code: "modelParseFailed".into(),
                    message: error.to_string(),
                    asset_ref: Some(asset_ref),
                    node_index: None,
                    details: None,
                });
                return Ok(RenderAssetDescriptor {
                    asset_ref,
                    kind: kind.to_string(),
                    hash,
                    source,
                    selector,
                    state: RenderAssetState::Error,
                    coordinate_convention: None,
                    name: None,
                    lod: None,
                    bounds: None,
                    meshes: Vec::new(),
                    materials: Vec::new(),
                });
            }
        };

        for message in external_texture_errors {
            self.diagnostics.push(RenderDiagnostic {
                severity: RenderDiagnosticSeverity::Warning,
                code: "externalTextureDictionaryReadFailed".into(),
                message: message.clone(),
                asset_ref: Some(asset_ref),
                node_index: None,
                details: None,
            });
        }

        let resolution = resolve_diffuse_textures(
            &parsed.model,
            parsed.embedded_textures.as_ref(),
            path_label,
            external_texture_dictionaries,
            options,
        );
        for details in &resolution.external_dictionary_errors {
            self.diagnostics.push(RenderDiagnostic {
                severity: RenderDiagnosticSeverity::Warning,
                code: "externalTextureDictionaryParseFailed".into(),
                message: "external texture dictionary could not be parsed".into(),
                asset_ref: Some(asset_ref),
                node_index: None,
                details: Some(details.clone()),
            });
        }
        for details in &resolution.unresolved {
            self.diagnostics.push(RenderDiagnostic {
                severity: RenderDiagnosticSeverity::Warning,
                code: details
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("diffuseTextureUnresolved")
                    .to_string(),
                message: "proven diffuse texture binding could not be resolved".into(),
                asset_ref: Some(asset_ref),
                node_index: None,
                details: Some(details.clone()),
            });
        }
        if resolution.truncated {
            self.diagnostics.push(RenderDiagnostic {
                severity: RenderDiagnosticSeverity::Warning,
                code: "diffuseTextureBudgetExceeded".into(),
                message: format!(
                    "{} diffuse texture identities exceed per-asset limit {}",
                    resolution.requested, MODEL_DIFFUSE_TEXTURE_LIMIT
                ),
                asset_ref: Some(asset_ref),
                node_index: None,
                details: None,
            });
        }

        let mut texture_ids_by_hash = BTreeMap::new();
        for texture in resolution.resolved {
            let name_hash = texture.name_hash;
            let texture_id = self.add_texture(texture)?;
            texture_ids_by_hash.entry(name_hash).or_insert(texture_id);
        }

        let mut materials = Vec::new();
        for (shader_index, shader) in parsed
            .model
            .shaders
            .iter()
            .take(options.max_shaders)
            .enumerate()
        {
            let diffuse_texture_name = shader.diffuse_texture_name().map(str::to_string);
            let diffuse_texture_ref = diffuse_texture_name
                .as_deref()
                .map(joaat)
                .and_then(|name_hash| texture_ids_by_hash.get(&name_hash).copied());
            materials.push(RenderMaterialDescriptor {
                id: to_u32(shader_index, "material id")?,
                source_shader_index: to_u32(shader_index, "shader index")?,
                shader_name_hash: shader.name_hash,
                shader_file_hash: shader.file_hash,
                diffuse_texture_name,
                diffuse_texture_ref,
            });
        }
        if parsed.model.shaders.len() > options.max_shaders {
            self.diagnostics.push(RenderDiagnostic {
                severity: RenderDiagnosticSeverity::Warning,
                code: "shaderBudgetExceeded".into(),
                message: format!(
                    "{} shaders exceed max_shaders {}",
                    parsed.model.shaders.len(),
                    options.max_shaders
                ),
                asset_ref: Some(asset_ref),
                node_index: None,
                details: None,
            });
        }

        let mut remaining_vertices = options.max_vertices;
        let mut remaining_indices = options.max_indices;
        let mut meshes = Vec::new();

        for primitive in parsed.model.primitives.iter().take(options.max_primitives) {
            if primitive.positions.len() > remaining_vertices
                || primitive.indices.len() > remaining_indices
            {
                self.diagnostics.push(RenderDiagnostic {
                    severity: RenderDiagnosticSeverity::Warning,
                    code: "geometryBudgetExceeded".into(),
                    message: format!(
                        "model {} geometry {} omitted by geometry budget",
                        primitive.model_index, primitive.geometry_index
                    ),
                    asset_ref: Some(asset_ref),
                    node_index: None,
                    details: None,
                });
                continue;
            }
            if primitive
                .indices
                .iter()
                .any(|index| *index as usize >= primitive.positions.len())
            {
                self.diagnostics.push(RenderDiagnostic {
                    severity: RenderDiagnosticSeverity::Error,
                    code: "invalidGeometryIndex".into(),
                    message: format!(
                        "model {} geometry {} has an index outside positions",
                        primitive.model_index, primitive.geometry_index
                    ),
                    asset_ref: Some(asset_ref),
                    node_index: None,
                    details: None,
                });
                continue;
            }

            remaining_vertices -= primitive.positions.len();
            remaining_indices -= primitive.indices.len();

            let positions = self.blob.append_f32x3(&primitive.positions)?;
            let normals = match primitive.normals.as_ref() {
                Some(normals) if normals.len() == primitive.positions.len() => {
                    Some(self.blob.append_f32x3(normals)?)
                }
                Some(_) => {
                    self.diagnostics.push(RenderDiagnostic {
                        severity: RenderDiagnosticSeverity::Warning,
                        code: "normalCountMismatch".into(),
                        message: format!(
                            "model {} geometry {} normals do not match positions",
                            primitive.model_index, primitive.geometry_index
                        ),
                        asset_ref: Some(asset_ref),
                        node_index: None,
                        details: None,
                    });
                    None
                }
                None => None,
            };
            let uv0 = match primitive.uv0.as_ref() {
                Some(uv0) if uv0.len() == primitive.positions.len() => {
                    Some(self.blob.append_f32x2(uv0)?)
                }
                Some(_) => {
                    self.diagnostics.push(RenderDiagnostic {
                        severity: RenderDiagnosticSeverity::Warning,
                        code: "uvCountMismatch".into(),
                        message: format!(
                            "model {} geometry {} UV0 does not match positions",
                            primitive.model_index, primitive.geometry_index
                        ),
                        asset_ref: Some(asset_ref),
                        node_index: None,
                        details: None,
                    });
                    None
                }
                None => None,
            };
            let indices = self.blob.append_u32(&primitive.indices)?;
            let source_shader_index = primitive.shader_index.map(u32::from);
            let material_ref = source_shader_index.filter(|index| *index < materials.len() as u32);
            if source_shader_index.is_some() && material_ref.is_none() {
                self.diagnostics.push(RenderDiagnostic {
                    severity: RenderDiagnosticSeverity::Warning,
                    code: "materialBindingOmitted".into(),
                    message: format!(
                        "model {} geometry {} shader is outside material budget",
                        primitive.model_index, primitive.geometry_index
                    ),
                    asset_ref: Some(asset_ref),
                    node_index: None,
                    details: None,
                });
            }

            meshes.push(RenderMeshDescriptor {
                id: to_u32(meshes.len(), "mesh id")?,
                model_index: to_u32(primitive.model_index, "model index")?,
                geometry_index: to_u32(primitive.geometry_index, "geometry index")?,
                topology: primitive.topology.as_str().to_string(),
                source_shader_index,
                material_ref,
                positions,
                normals,
                uv0,
                indices,
            });
        }

        if parsed.model.primitives.len() > options.max_primitives {
            self.diagnostics.push(RenderDiagnostic {
                severity: RenderDiagnosticSeverity::Warning,
                code: "primitiveBudgetExceeded".into(),
                message: format!(
                    "{} primitives exceed max_primitives {}",
                    parsed.model.primitives.len(),
                    options.max_primitives
                ),
                asset_ref: Some(asset_ref),
                node_index: None,
                details: None,
            });
        }

        Ok(RenderAssetDescriptor {
            asset_ref,
            kind: kind.to_string(),
            hash,
            source,
            selector,
            state: RenderAssetState::Ready,
            coordinate_convention: Some(parsed.model.coordinate_convention.as_str().to_string()),
            name: parsed.model.name.clone(),
            lod: Some(parsed.model.lod.as_str().to_string()),
            bounds: Some(RenderBounds {
                center: parsed.model.bounds.center,
                radius: parsed.model.bounds.radius,
                min: parsed.model.bounds.min,
                max: parsed.model.bounds.max,
            }),
            meshes,
            materials,
        })
    }

    fn add_texture(&mut self, texture: ResolvedDiffuseTexture) -> Result<u32, io::Error> {
        let expected = usize::from(texture.width)
            .checked_mul(usize::from(texture.height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "texture size overflow"))?;
        if texture.rgba.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "decoded texture {} has {} RGBA bytes; expected {expected}",
                    texture.name,
                    texture.rgba.len()
                ),
            ));
        }

        let key = TextureIdentityKey {
            source: texture.source.clone(),
            source_path: texture.source_path.clone(),
            name_hash: texture.name_hash,
            width: texture.width,
            height: texture.height,
        };
        if let Some(id) = self.texture_ids.get(&key) {
            return Ok(*id);
        }

        let pixels = u32::from(texture.width) * u32::from(texture.height);
        let data = self.blob.append_u8(4, pixels, &texture.rgba)?;
        let id = to_u32(self.textures.len(), "texture id")?;
        self.textures.push(RenderTextureDescriptor {
            id,
            name: texture.name,
            name_hash: texture.name_hash,
            source: texture.source,
            source_path: texture.source_path,
            original_width: texture.original_width,
            original_height: texture.original_height,
            width: texture.width,
            height: texture.height,
            downscaled: texture.downscaled,
            format: RenderTextureFormat::Rgba8Srgb,
            data,
        });
        self.texture_ids.insert(key, id);
        Ok(id)
    }
}

struct BlobBuilder {
    data: Vec<u8>,
    max_bytes: usize,
}

impl BlobBuilder {
    fn new(max_bytes: usize) -> Self {
        Self {
            data: Vec::new(),
            max_bytes,
        }
    }

    fn append_f32x3(&mut self, values: &[[f32; 3]]) -> Result<RenderBufferView, io::Error> {
        self.append_f32(
            values.len(),
            3,
            values.iter().flat_map(|value| value.iter().copied()),
        )
    }

    fn append_f32x2(&mut self, values: &[[f32; 2]]) -> Result<RenderBufferView, io::Error> {
        self.append_f32(
            values.len(),
            2,
            values.iter().flat_map(|value| value.iter().copied()),
        )
    }

    fn append_f32(
        &mut self,
        count: usize,
        components: u8,
        values: impl Iterator<Item = f32>,
    ) -> Result<RenderBufferView, io::Error> {
        let byte_length = count
            .checked_mul(usize::from(components))
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "f32 buffer size overflow")
            })?;
        let offset = self.prepare_append(byte_length, 4)?;
        for value in values {
            self.data.extend_from_slice(&value.to_le_bytes());
        }
        self.finish_view(
            offset,
            byte_length,
            RenderElementType::F32,
            components,
            count,
        )
    }

    fn append_u32(&mut self, values: &[u32]) -> Result<RenderBufferView, io::Error> {
        let byte_length = values.len().checked_mul(4).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "u32 buffer size overflow")
        })?;
        let offset = self.prepare_append(byte_length, 4)?;
        for value in values {
            self.data.extend_from_slice(&value.to_le_bytes());
        }
        self.finish_view(offset, byte_length, RenderElementType::U32, 1, values.len())
    }

    fn append_u8(
        &mut self,
        components: u8,
        count: u32,
        values: &[u8],
    ) -> Result<RenderBufferView, io::Error> {
        let expected = usize::try_from(count)
            .ok()
            .and_then(|value| value.checked_mul(usize::from(components)))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "u8 buffer size overflow"))?;
        if values.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "u8 buffer length does not match typed element count",
            ));
        }
        let offset = self.prepare_append(values.len(), 1)?;
        self.data.extend_from_slice(values);
        self.finish_view(
            offset,
            values.len(),
            RenderElementType::U8,
            components,
            usize::try_from(count).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "u8 element count overflow")
            })?,
        )
    }

    fn prepare_append(&mut self, byte_length: usize, alignment: usize) -> Result<usize, io::Error> {
        let padding = (alignment - (self.data.len() % alignment)) % alignment;
        let end = self
            .data
            .len()
            .checked_add(padding)
            .and_then(|value| value.checked_add(byte_length))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "render blob size overflow")
            })?;
        if end > self.max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("render blob would exceed max_blob_bytes {}", self.max_bytes),
            ));
        }
        self.data.resize(self.data.len() + padding, 0);
        Ok(self.data.len())
    }

    fn finish_view(
        &self,
        offset: usize,
        byte_length: usize,
        element_type: RenderElementType,
        components: u8,
        count: usize,
    ) -> Result<RenderBufferView, io::Error> {
        Ok(RenderBufferView {
            offset: u64::try_from(offset).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "buffer offset overflow")
            })?,
            byte_length: u64::try_from(byte_length).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "buffer length overflow")
            })?,
            element_type,
            components,
            count: to_u32(count, "buffer element count")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    fn fixture(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/synthetic")
            .join(relative)
    }

    fn render_workspace() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let source = fixture("stream");
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let temporary = std::env::temp_dir().join(format!(
            "ragelab-render-package-{}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temporary);
        fs::create_dir_all(&temporary).expect("create render workspace");
        fs::copy(source.join("simple.ymap"), temporary.join("simple.ymap")).expect("copy YMAP");
        fs::copy(source.join("simple.ytyp"), temporary.join("simple.ytyp")).expect("copy YTYP");
        fs::copy(
            fixture("ydr/simple.ydr"),
            temporary.join("test_drawable.ydr"),
        )
        .expect("copy YDR");
        fs::copy(
            fixture("ybn/simple.ybn"),
            temporary.join("test_collision.ybn"),
        )
        .expect("copy YBN");
        temporary
    }

    #[test]
    fn scene_package_is_deterministic_roundtrippable_and_typed() {
        let workspace = render_workspace();
        let package = workspace_scene_render_package(
            &workspace,
            Path::new("simple.ymap"),
            SceneAssemblyOptions::default(),
            RenderPackageOptions::default(),
        )
        .expect("build render package");

        assert_eq!(
            package.descriptor.schema_version,
            RENDER_PACKAGE_SCHEMA_VERSION
        );
        assert_eq!(package.descriptor.summary.instances, 1);
        assert_eq!(package.descriptor.summary.assets, 2);
        assert_eq!(package.descriptor.summary.ready_assets, 1);
        assert_eq!(package.descriptor.assets[0].state, RenderAssetState::Ready);
        assert_eq!(
            package.descriptor.assets[1].state,
            RenderAssetState::Unsupported
        );
        let mesh = &package.descriptor.assets[0].meshes[0];
        assert_eq!(mesh.positions.element_type, RenderElementType::F32);
        assert_eq!(mesh.positions.components, 3);
        assert_eq!(mesh.indices.element_type, RenderElementType::U32);
        assert_eq!(mesh.indices.components, 1);

        let first = package.encode_binary().expect("encode package");
        let second = package.encode_binary().expect("encode package again");
        assert_eq!(first, second, "binary transport must be deterministic");

        let decoded = RenderPackage::decode_binary(&first).expect("decode package");
        assert_eq!(decoded, package);
        decoded.validate().expect("validate package");

        fs::remove_dir_all(workspace).expect("remove render workspace");
    }

    #[test]
    fn package_preserves_primary_workspace_provenance() {
        let source = fixture("stream");
        let temporary =
            std::env::temp_dir().join(format!("ragelab-render-precedence-{}", std::process::id()));
        let primary = temporary.join("primary");
        let fallback = temporary.join("fallback");
        let _ = fs::remove_dir_all(&temporary);
        fs::create_dir_all(&primary).expect("create primary");
        fs::create_dir_all(&fallback).expect("create fallback");
        fs::copy(source.join("simple.ymap"), primary.join("simple.ymap")).expect("copy YMAP");
        fs::copy(source.join("simple.ytyp"), fallback.join("simple.ytyp"))
            .expect("copy fallback YTYP");
        fs::copy(source.join("simple.ytyp"), primary.join("simple.ytyp"))
            .expect("copy primary YTYP");
        fs::copy(
            fixture("ydr/simple.ydr"),
            fallback.join("test_drawable.ydr"),
        )
        .expect("copy fallback YDR");
        fs::copy(
            fixture("ybn/simple.ybn"),
            fallback.join("test_collision.ybn"),
        )
        .expect("copy fallback YBN");
        fs::copy(fixture("ydr/simple.ydr"), primary.join("test_drawable.ydr"))
            .expect("copy primary YDR");
        fs::copy(
            fixture("ybn/simple.ybn"),
            primary.join("test_collision.ybn"),
        )
        .expect("copy primary YBN");

        let fallback_roots = vec![fallback];
        let package = workspace_scene_render_package_with_sources(
            &primary,
            Path::new("simple.ymap"),
            &fallback_roots,
            &[],
            SceneAssemblyOptions::default(),
            RenderPackageOptions::default(),
        )
        .expect("build package with fallback");
        assert!(Path::new(&package.descriptor.assets[0].source.path).starts_with(&primary));
        assert_eq!(package.descriptor.assets[0].source.source_type, "loose");

        fs::remove_dir_all(temporary).expect("remove precedence workspace");
    }

    #[test]
    fn package_rejects_corrupt_buffer_ranges() {
        let workspace = render_workspace();
        let mut package = workspace_scene_render_package(
            &workspace,
            Path::new("simple.ymap"),
            SceneAssemblyOptions::default(),
            RenderPackageOptions::default(),
        )
        .expect("build render package");
        package.descriptor.assets[0].meshes[0].positions.offset = package.blob.len() as u64 + 4;
        assert_eq!(
            package.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(workspace).expect("remove render workspace");
    }

    #[test]
    fn isolated_asset_packet_uses_raw_typed_buffers_without_base64() {
        let path = fixture("ydr/editable.ydr");
        let bytes = fs::read(&path).expect("read YDR fixture");
        let package = render_asset_package_bytes_as(
            &path.display().to_string(),
            "YDR",
            &bytes,
            PreviewOptions::default(),
            &[],
            DEFAULT_RENDER_MAX_BLOB_BYTES,
        )
        .expect("build isolated render packet");
        assert_eq!(package.descriptor.summary.assets, 1);
        assert!(!package.descriptor.assets[0].meshes.is_empty());
        let metadata = serde_json::to_string(&package.descriptor).expect("serialize descriptor");
        assert!(!metadata.contains("rgbaBase64"));
        package.validate().expect("validate isolated packet");
    }
}
