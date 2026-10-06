//! Rust-owned YMAP scene assembly.
//!
//! This module intentionally emits only normalized instance/reference metadata.
//! Geometry remains behind the existing lazy preview endpoints.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_assets::{
    AssetKind, SceneArchetypeResolution, SceneAssetLocator, SceneAssetLookupError,
    SceneAssetLookupErrorCode, SceneCollisionLookup, SceneCollisionLookupErrorCode,
    SceneResolvedAsset, SceneTextureDictionaryLookup, WorkspaceIndex,
};
use ragelab_ymap::Ymap;
use serde::Serialize;
use serde_json::Value;

use crate::{
    preview_asset_bytes_as_with_texture_dictionaries, ymap_entity_spatial_context,
    AssetPreviewReport, GtaRpfAssetIndex, PreviewOptions, PreviewTextureDictionarySource,
    SpatialTransform,
};

pub const DEFAULT_SCENE_NODE_LIMIT: usize = 10_000;
pub const MAX_SCENE_NODE_LIMIT: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneRpfMount {
    pub archive: PathBuf,
    pub nested: Vec<String>,
    pub keys: PathBuf,
}

impl SceneRpfMount {
    pub fn new(archive: impl Into<PathBuf>, nested: Vec<String>, keys: impl Into<PathBuf>) -> Self {
        Self {
            archive: archive.into(),
            nested,
            keys: keys.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneGameIndexSource {
    pub game_root: PathBuf,
    pub index: PathBuf,
    pub keys: PathBuf,
}

impl SceneGameIndexSource {
    pub fn new(
        game_root: impl Into<PathBuf>,
        index: impl Into<PathBuf>,
        keys: impl Into<PathBuf>,
    ) -> Self {
        Self {
            game_root: game_root.into(),
            index: index.into(),
            keys: keys.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneAssemblyOptions {
    pub max_nodes: usize,
}

impl Default for SceneAssemblyOptions {
    fn default() -> Self {
        Self {
            max_nodes: DEFAULT_SCENE_NODE_LIMIT,
        }
    }
}

impl SceneAssemblyOptions {
    pub const fn new(max_nodes: usize) -> Self {
        Self { max_nodes }
    }

    fn effective_max_nodes(self) -> usize {
        self.max_nodes.clamp(1, MAX_SCENE_NODE_LIMIT)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneManifest {
    pub schema_version: u32,
    pub root: SceneRoot,
    pub nodes: Vec<SceneNode>,
    pub assets: Vec<SceneAssetReference>,
    pub summary: SceneSummary,
    pub warnings: Vec<String>,
    pub limits: SceneLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneRoot {
    pub path: PathBuf,
    pub name_hash: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneNode {
    pub index: usize,
    pub source_ymap: PathBuf,
    pub entity_index: usize,
    pub archetype_hash: u32,
    pub provider_path: Option<String>,
    pub asset_ref: Option<usize>,
    pub asset_kind: Option<AssetKind>,
    pub transform: Option<SpatialTransform>,
    pub resolution: SceneResolutionState,
    pub reason: Option<SceneResolutionReason>,
    pub collision: Option<SceneCollisionRelationship>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneResolutionState {
    Resolved,
    Unresolved,
}

impl SceneResolutionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneResolutionReason {
    pub code: SceneResolutionReasonCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneResolutionReasonCode {
    ProviderMissing,
    ProviderAmbiguous,
    AssetNameMissing,
    DrawableDictionaryMissing,
    AssetMissing,
    AssetAmbiguous,
    DictionaryUnreadable,
    DictionaryEntryMissing,
    UnsupportedAssetRelation,
    InvalidWorldTransform,
}

impl SceneResolutionReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderMissing => "providerMissing",
            Self::ProviderAmbiguous => "providerAmbiguous",
            Self::AssetNameMissing => "assetNameMissing",
            Self::DrawableDictionaryMissing => "drawableDictionaryMissing",
            Self::AssetMissing => "assetMissing",
            Self::AssetAmbiguous => "assetAmbiguous",
            Self::DictionaryUnreadable => "dictionaryUnreadable",
            Self::DictionaryEntryMissing => "dictionaryEntryMissing",
            Self::UnsupportedAssetRelation => "unsupportedAssetRelation",
            Self::InvalidWorldTransform => "invalidWorldTransform",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneAssetReference {
    pub id: usize,
    pub kind: AssetKind,
    pub hash: u32,
    pub path: SceneAssetLocator,
    pub selector: Option<SceneAssetSelector>,
    pub texture_dictionary: SceneTextureDictionaryLookup,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SceneAssetSelector {
    YddDrawable {
        index: usize,
        name_hash: u32,
        name: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneCollisionRelationship {
    pub hash: u32,
    pub asset_ref: Option<usize>,
    pub state: SceneCollisionState,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneCollisionState {
    LocalOnly,
    Unresolved,
}

impl SceneCollisionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "localOnly",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneSummary {
    pub total_entities: usize,
    pub emitted_nodes: usize,
    pub resolved_nodes: usize,
    pub unresolved_nodes: usize,
    pub asset_references: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneLimits {
    pub max_nodes: usize,
    pub truncated: bool,
    pub omitted_entities: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneManifestReport {
    pub schema_version: u32,
    pub root: SceneRootReport,
    pub nodes: Vec<SceneNodeReport>,
    pub assets: Vec<SceneAssetReferenceReport>,
    pub summary: SceneSummaryReport,
    pub warnings: Vec<String>,
    pub limits: SceneLimitsReport,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneRootReport {
    pub path: String,
    pub name_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneNodeReport {
    pub index: usize,
    pub source_ymap: String,
    pub entity_index: usize,
    pub archetype_hash: String,
    pub provider_path: Option<String>,
    pub asset_ref: Option<usize>,
    pub asset_kind: Option<String>,
    pub transform: Option<SceneTransformReport>,
    pub resolution: String,
    pub reason: Option<SceneResolutionReasonReport>,
    pub collision: Option<SceneCollisionRelationshipReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneTransformReport {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: Option<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneResolutionReasonReport {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneCollisionRelationshipReport {
    pub hash: String,
    pub asset_ref: Option<usize>,
    pub state: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneAssetReferenceReport {
    pub id: usize,
    pub kind: String,
    pub hash: String,
    pub path: String,
    pub selector: Option<SceneAssetSelectorReport>,
    pub texture_dictionary: Option<SceneTextureDictionaryReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneTextureDictionaryReport {
    pub state: String,
    pub hash: String,
    pub sources: Vec<SceneTextureDictionarySourceReport>,
    pub missing: Vec<String>,
    pub ambiguous: Vec<SceneTextureDictionaryAmbiguityReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneTextureDictionarySourceReport {
    pub hash: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneTextureDictionaryAmbiguityReport {
    pub hash: String,
    pub candidates: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneAssetSelectorReport {
    #[serde(rename = "type")]
    pub selector_type: String,
    pub index: usize,
    pub name_hash: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneSummaryReport {
    pub total_entities: usize,
    pub emitted_nodes: usize,
    pub resolved_nodes: usize,
    pub unresolved_nodes: usize,
    pub asset_references: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneLimitsReport {
    pub max_nodes: usize,
    pub truncated: bool,
    pub omitted_entities: usize,
}

impl From<&SceneManifest> for SceneManifestReport {
    fn from(manifest: &SceneManifest) -> Self {
        Self {
            schema_version: manifest.schema_version,
            root: SceneRootReport {
                path: manifest.root.path.display().to_string(),
                name_hash: manifest.root.name_hash.map(|hash| format!("0x{hash:08X}")),
            },
            nodes: manifest
                .nodes
                .iter()
                .map(|node| SceneNodeReport {
                    index: node.index,
                    source_ymap: node.source_ymap.display().to_string(),
                    entity_index: node.entity_index,
                    archetype_hash: format!("0x{:08X}", node.archetype_hash),
                    provider_path: node.provider_path.clone(),
                    asset_ref: node.asset_ref,
                    asset_kind: node.asset_kind.map(|kind| kind.to_string()),
                    transform: node.transform.map(|transform| SceneTransformReport {
                        translation: transform.translation,
                        rotation: transform.rotation,
                        scale: transform.scale,
                    }),
                    resolution: node.resolution.as_str().to_string(),
                    reason: node
                        .reason
                        .as_ref()
                        .map(|reason| SceneResolutionReasonReport {
                            code: reason.code.as_str().to_string(),
                            message: reason.message.clone(),
                        }),
                    collision: node.collision.as_ref().map(|collision| {
                        SceneCollisionRelationshipReport {
                            hash: format!("0x{:08X}", collision.hash),
                            asset_ref: collision.asset_ref,
                            state: collision.state.as_str().to_string(),
                            reason: collision.reason.clone(),
                        }
                    }),
                })
                .collect(),
            assets: manifest
                .assets
                .iter()
                .map(|asset| SceneAssetReferenceReport {
                    id: asset.id,
                    kind: asset.kind.to_string(),
                    hash: format!("0x{:08X}", asset.hash),
                    path: asset.path.provenance(),
                    selector: asset.selector.as_ref().map(|selector| match selector {
                        SceneAssetSelector::YddDrawable {
                            index,
                            name_hash,
                            name,
                        } => SceneAssetSelectorReport {
                            selector_type: "yddDrawable".into(),
                            index: *index,
                            name_hash: format!("0x{name_hash:08X}"),
                            name: name.clone(),
                        },
                    }),
                    texture_dictionary: texture_dictionary_report(&asset.texture_dictionary),
                })
                .collect(),
            summary: SceneSummaryReport {
                total_entities: manifest.summary.total_entities,
                emitted_nodes: manifest.summary.emitted_nodes,
                resolved_nodes: manifest.summary.resolved_nodes,
                unresolved_nodes: manifest.summary.unresolved_nodes,
                asset_references: manifest.summary.asset_references,
            },
            warnings: manifest.warnings.clone(),
            limits: SceneLimitsReport {
                max_nodes: manifest.limits.max_nodes,
                truncated: manifest.limits.truncated,
                omitted_entities: manifest.limits.omitted_entities,
            },
        }
    }
}

fn texture_dictionary_report(
    lookup: &SceneTextureDictionaryLookup,
) -> Option<SceneTextureDictionaryReport> {
    match lookup {
        SceneTextureDictionaryLookup::None => None,
        SceneTextureDictionaryLookup::Chain {
            hash,
            sources,
            missing,
            ambiguous,
        } => {
            let state = if !ambiguous.is_empty() {
                "ambiguous"
            } else if sources.is_empty() {
                "missing"
            } else {
                "resolved"
            };
            Some(SceneTextureDictionaryReport {
                state: state.into(),
                hash: format!("0x{hash:08X}"),
                sources: sources
                    .iter()
                    .map(|source| SceneTextureDictionarySourceReport {
                        hash: format!("0x{:08X}", source.hash),
                        path: source.locator.provenance(),
                    })
                    .collect(),
                missing: missing.iter().map(|hash| format!("0x{hash:08X}")).collect(),
                ambiguous: ambiguous
                    .iter()
                    .map(|item| SceneTextureDictionaryAmbiguityReport {
                        hash: format!("0x{:08X}", item.hash),
                        candidates: item.candidates,
                    })
                    .collect(),
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SceneAssetKey {
    kind: AssetKind,
    hash: u32,
    path: SceneAssetLocator,
    selector: Option<SceneAssetSelector>,
    texture_dictionary: SceneTextureDictionaryLookup,
}

pub fn assemble_ymap_scene(
    index: &WorkspaceIndex,
    ymap_path: &Path,
    ymap: &Ymap,
    options: SceneAssemblyOptions,
) -> SceneManifest {
    assemble_ymap_scene_with_lookup(ymap_path, ymap, options, |archetype_hash| {
        index.resolve_scene_archetype(archetype_hash)
    })
}

pub fn workspace_scene_report(
    workspace: &Path,
    ymap_path: &Path,
    options: SceneAssemblyOptions,
) -> Result<SceneManifestReport, io::Error> {
    workspace_scene_report_with_sources(workspace, ymap_path, &[], &[], options)
}

pub fn workspace_scene_report_with_fallbacks(
    workspace: &Path,
    ymap_path: &Path,
    fallback_roots: &[PathBuf],
    options: SceneAssemblyOptions,
) -> Result<SceneManifestReport, io::Error> {
    workspace_scene_report_with_sources(workspace, ymap_path, fallback_roots, &[], options)
}

pub fn workspace_scene_report_with_sources(
    workspace: &Path,
    ymap_path: &Path,
    fallback_roots: &[PathBuf],
    rpf_mounts: &[SceneRpfMount],
    options: SceneAssemblyOptions,
) -> Result<SceneManifestReport, io::Error> {
    workspace_scene_report_with_game_index(
        workspace,
        ymap_path,
        fallback_roots,
        rpf_mounts,
        None,
        options,
    )
}

pub fn workspace_scene_report_with_game_index(
    workspace: &Path,
    ymap_path: &Path,
    fallback_roots: &[PathBuf],
    rpf_mounts: &[SceneRpfMount],
    game_index: Option<&SceneGameIndexSource>,
    options: SceneAssemblyOptions,
) -> Result<SceneManifestReport, io::Error> {
    let manifest = workspace_scene_manifest_with_sources(
        workspace,
        ymap_path,
        fallback_roots,
        rpf_mounts,
        game_index,
        options,
    )?;
    Ok(SceneManifestReport::from(&manifest))
}

pub struct SceneAssetPreviewSources<'a> {
    pub fallback_roots: &'a [PathBuf],
    pub rpf_mounts: &'a [SceneRpfMount],
    pub game_index: Option<&'a SceneGameIndexSource>,
}

pub(crate) struct OwnedPreviewTextureDictionarySource {
    pub(crate) path: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) source: &'static str,
}

pub(crate) struct SceneAssetPreviewInput {
    pub(crate) path_label: String,
    pub(crate) asset_type: &'static str,
    pub(crate) bytes: Vec<u8>,
    pub(crate) options: PreviewOptions,
    pub(crate) external_texture_dictionaries: Vec<OwnedPreviewTextureDictionarySource>,
    pub(crate) external_texture_errors: Vec<String>,
}

impl SceneAssetPreviewInput {
    pub(crate) fn external_sources(&self) -> Vec<PreviewTextureDictionarySource<'_>> {
        self.external_texture_dictionaries
            .iter()
            .map(|source| PreviewTextureDictionarySource {
                path: &source.path,
                bytes: &source.bytes,
                source: source.source,
            })
            .collect()
    }
}

pub fn workspace_scene_asset_preview_with_sources(
    workspace: &Path,
    ymap_path: &Path,
    fallback_roots: &[PathBuf],
    rpf_mounts: &[SceneRpfMount],
    scene_options: SceneAssemblyOptions,
    asset_ref: usize,
    options: PreviewOptions,
) -> Result<AssetPreviewReport, io::Error> {
    workspace_scene_asset_preview_with_game_index(
        workspace,
        ymap_path,
        SceneAssetPreviewSources {
            fallback_roots,
            rpf_mounts,
            game_index: None,
        },
        scene_options,
        asset_ref,
        options,
    )
}

pub fn workspace_scene_asset_preview_with_game_index(
    workspace: &Path,
    ymap_path: &Path,
    sources: SceneAssetPreviewSources<'_>,
    scene_options: SceneAssemblyOptions,
    asset_ref: usize,
    options: PreviewOptions,
) -> Result<AssetPreviewReport, io::Error> {
    let manifest = workspace_scene_manifest_with_sources(
        workspace,
        ymap_path,
        sources.fallback_roots,
        sources.rpf_mounts,
        sources.game_index,
        scene_options,
    )?;
    let asset = manifest.assets.get(asset_ref).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "scene assetRef {asset_ref} exceeds asset reference count {}",
                manifest.assets.len()
            ),
        )
    })?;
    let input = prepare_scene_asset_preview_input(asset, options)?;
    let external = input.external_sources();

    let mut report = preview_asset_bytes_as_with_texture_dictionaries(
        &input.path_label,
        input.asset_type,
        &input.bytes,
        input.options,
        &external,
    )?;

    if !input.external_texture_errors.is_empty() {
        if let Some(resolution) = report
            .preview
            .get_mut("diffuseTextureResolution")
            .and_then(Value::as_object_mut)
        {
            resolution.insert(
                "externalTextureDictionaryReadErrors".into(),
                serde_json::to_value(input.external_texture_errors).unwrap_or(Value::Null),
            );
        }
    }

    Ok(report)
}

pub(crate) fn prepare_scene_asset_preview_input(
    asset: &SceneAssetReference,
    mut options: PreviewOptions,
) -> Result<SceneAssetPreviewInput, io::Error> {
    let asset_type = match asset.kind {
        AssetKind::Ydr => "YDR",
        AssetKind::Ydd => {
            let Some(SceneAssetSelector::YddDrawable { index, .. }) = asset.selector.as_ref()
            else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "scene YDD asset reference is missing its drawable selector",
                ));
            };
            options.drawable_index = Some(*index);
            "YDD"
        }
        AssetKind::Yft => "YFT",
        AssetKind::Ybn => "YBN",
        other => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("scene preview is unavailable for {other}"),
            ))
        }
    };

    let bytes = asset.path.read_bytes().map_err(io::Error::other)?;
    let mut external_texture_dictionaries = Vec::new();
    let mut external_texture_errors = Vec::new();

    if let SceneTextureDictionaryLookup::Chain { hash, sources, .. } = &asset.texture_dictionary {
        for source in sources {
            let provenance = source.locator.provenance();
            match source.locator.read_bytes() {
                Ok(texture_bytes) => {
                    external_texture_dictionaries.push(OwnedPreviewTextureDictionarySource {
                        path: provenance,
                        bytes: texture_bytes,
                        source: if source.hash == *hash {
                            "archetypeYtd"
                        } else {
                            "parentYtd"
                        },
                    });
                }
                Err(error) => {
                    external_texture_errors.push(format!("{provenance}: {error}"));
                }
            }
        }
    }

    Ok(SceneAssetPreviewInput {
        path_label: asset.path.provenance(),
        asset_type,
        bytes,
        options,
        external_texture_dictionaries,
        external_texture_errors,
    })
}

pub(crate) fn workspace_scene_manifest_with_sources(
    workspace: &Path,
    ymap_path: &Path,
    fallback_roots: &[PathBuf],
    rpf_mounts: &[SceneRpfMount],
    game_index_source: Option<&SceneGameIndexSource>,
    options: SceneAssemblyOptions,
) -> Result<SceneManifest, io::Error> {
    if !workspace.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a directory: {}", workspace.display()),
        ));
    }
    for fallback_root in fallback_roots {
        if !fallback_root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "fallback root is not a directory: {}",
                    fallback_root.display()
                ),
            ));
        }
    }

    let resolved_ymap = if ymap_path.is_absolute() {
        ymap_path.to_path_buf()
    } else {
        workspace.join(ymap_path)
    };
    let bytes = fs::read(&resolved_ymap)?;
    let ymap = Ymap::from_bytes(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut index = WorkspaceIndex::scan_with_fallbacks(workspace, fallback_roots)
        .map_err(|error| io::Error::other(error.to_string()))?;
    for mount in rpf_mounts {
        index
            .mount_scene_rpf(&mount.archive, mount.nested.clone(), &mount.keys)
            .map_err(|error| io::Error::other(error.to_string()))?;
    }

    let mut source_warnings = Vec::new();
    if let Some(source) = game_index_source {
        source_warnings = mount_game_index_for_ymap(&mut index, &ymap, source, options)?;
    }

    let mut manifest = assemble_ymap_scene(&index, &resolved_ymap, &ymap, options);
    manifest.warnings.extend(source_warnings);
    Ok(manifest)
}

fn mount_game_index_for_ymap(
    workspace_index: &mut WorkspaceIndex,
    ymap: &Ymap,
    source: &SceneGameIndexSource,
    options: SceneAssemblyOptions,
) -> Result<Vec<String>, io::Error> {
    if !source.game_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "scene game root is not a directory: {}",
                source.game_root.display()
            ),
        ));
    }
    if !source.index.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("scene game index not found: {}", source.index.display()),
        ));
    }
    if !source.keys.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "scene game RPF key store is not a directory: {}",
                source.keys.display()
            ),
        ));
    }

    let game_index = GtaRpfAssetIndex::load(&source.index)?;
    if !game_index.matches_installation(&source.game_root)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "scene game index is stale for {}; rebuild {}",
                source.game_root.display(),
                source.index.display()
            ),
        ));
    }

    mount_loaded_game_index_for_ymap(workspace_index, ymap, source, &game_index, options)
}

pub(crate) fn mount_loaded_game_index_for_ymap(
    workspace_index: &mut WorkspaceIndex,
    ymap: &Ymap,
    source: &SceneGameIndexSource,
    game_index: &GtaRpfAssetIndex,
    options: SceneAssemblyOptions,
) -> Result<Vec<String>, io::Error> {
    mount_loaded_game_index_for_archetypes(
        workspace_index,
        ymap.entities
            .iter()
            .take(options.effective_max_nodes())
            .map(|entity| entity.archetype_name.0),
        source,
        game_index,
    )
}

pub(crate) fn mount_loaded_game_index_for_archetypes(
    workspace_index: &mut WorkspaceIndex,
    archetype_hashes: impl IntoIterator<Item = u32>,
    source: &SceneGameIndexSource,
    game_index: &GtaRpfAssetIndex,
) -> Result<Vec<String>, io::Error> {
    let plan = game_index.plan_for_archetypes(archetype_hashes);

    for selection in &plan.provider_selections {
        let locator = selection
            .locator
            .materialize(&source.game_root, &source.keys);
        workspace_index
            .mount_scene_rpf_provider_archetypes(&locator, &selection.archetype_hashes)
            .map_err(|error| io::Error::other(error.to_string()))?;
    }

    for parent in &plan.texture_parents {
        workspace_index.mount_scene_texture_parent(parent.relationship());
    }

    let mut selected_entries = BTreeSet::new();
    for locator in plan
        .asset_entries
        .iter()
        .chain(&plan.texture_entries)
        .chain(&plan.collision_entries)
    {
        selected_entries.insert(locator.materialize(&source.game_root, &source.keys));
    }
    workspace_index
        .mount_scene_rpf_entries(&selected_entries.into_iter().collect::<Vec<_>>())
        .map_err(|error| io::Error::other(error.to_string()))?;

    let mut warnings = Vec::new();
    if !plan.unresolved_archetypes.is_empty() {
        warnings.push(format!(
            "game index has no winning provider for {} requested archetype(s)",
            plan.unresolved_archetypes.len()
        ));
    }
    if !plan.ambiguous_archetypes.is_empty() {
        warnings.push(format!(
            "game index has same-rank ambiguity for {} requested archetype(s)",
            plan.ambiguous_archetypes.len()
        ));
    }
    if !plan.unresolved_assets.is_empty() {
        warnings.push(format!(
            "game index has no winning primary asset for {} requested asset key(s)",
            plan.unresolved_assets.len()
        ));
    }
    if !plan.ambiguous_assets.is_empty() {
        warnings.push(format!(
            "game index has same-rank ambiguity for {} requested asset key(s)",
            plan.ambiguous_assets.len()
        ));
    }
    if !plan.ambiguous_texture_parents.is_empty() {
        warnings.push(format!(
            "game index has same-rank GTXD parent ambiguity for {} texture dictionary key(s)",
            plan.ambiguous_texture_parents.len()
        ));
    }
    if !plan.texture_parent_cycles.is_empty() {
        warnings.push(format!(
            "game index detected {} GTXD parent cycle(s)",
            plan.texture_parent_cycles.len()
        ));
    }

    Ok(warnings)
}

fn assemble_ymap_scene_with_lookup<F>(
    ymap_path: &Path,
    ymap: &Ymap,
    options: SceneAssemblyOptions,
    mut lookup: F,
) -> SceneManifest
where
    F: FnMut(u32) -> Result<SceneArchetypeResolution, SceneAssetLookupError>,
{
    let max_nodes = options.effective_max_nodes();
    let emitted_nodes = ymap.entities.len().min(max_nodes);
    let omitted_entities = ymap.entities.len().saturating_sub(emitted_nodes);
    let mut warnings = Vec::new();
    if omitted_entities > 0 {
        warnings.push(format!(
            "scene node limit {max_nodes} reached; {omitted_entities} YMAP entities were omitted"
        ));
    }
    if !ymap.physics_dictionaries.is_empty() {
        warnings.push(format!(
            "YMAP declares {} map-level physics dictionary reference(s); dependency alone does not prove world placement, so no collision world nodes were emitted",
            ymap.physics_dictionaries.len()
        ));
    }

    let mut nodes = Vec::with_capacity(emitted_nodes);
    let mut assets = Vec::new();
    let mut asset_ids = BTreeMap::<SceneAssetKey, usize>::new();
    let mut lookup_cache =
        BTreeMap::<u32, Result<SceneArchetypeResolution, SceneAssetLookupError>>::new();

    for (entity_index, entity) in ymap.entities.iter().take(max_nodes).enumerate() {
        let archetype_hash = entity.archetype_name.0;
        let scene_lookup = lookup_cache
            .entry(archetype_hash)
            .or_insert_with(|| lookup(archetype_hash))
            .clone();
        let spatial = ymap_entity_spatial_context(ymap_path.to_path_buf(), entity_index, entity);

        let (provider_path, asset_ref, asset_kind, collision, lookup_reason) = match scene_lookup {
            Ok(resolution) => {
                let asset_kind = resolution.asset.kind();
                let asset_ref = insert_primary_asset(
                    &resolution.asset,
                    &resolution.texture_dictionary,
                    &mut assets,
                    &mut asset_ids,
                );
                let collision =
                    collision_relationship(resolution.collision, &mut assets, &mut asset_ids);
                (
                    Some(resolution.provider.provenance()),
                    Some(asset_ref),
                    Some(asset_kind),
                    collision,
                    None,
                )
            }
            Err(error) => {
                let provider_path = error.provider.clone();
                let asset_kind = error.expected_kind;
                (
                    provider_path,
                    None,
                    asset_kind,
                    None,
                    Some(scene_lookup_reason(error)),
                )
            }
        };

        let (resolution, reason) = if let Some(reason) = lookup_reason {
            (SceneResolutionState::Unresolved, Some(reason))
        } else if spatial.world_transform.is_some() {
            (SceneResolutionState::Resolved, None)
        } else {
            (
                SceneResolutionState::Unresolved,
                Some(SceneResolutionReason {
                    code: SceneResolutionReasonCode::InvalidWorldTransform,
                    message: spatial
                        .reason
                        .map(|reason| reason.message)
                        .unwrap_or_else(|| {
                            "YMAP entity has no proven finite world transform".into()
                        }),
                }),
            )
        };

        nodes.push(SceneNode {
            index: entity_index,
            source_ymap: ymap_path.to_path_buf(),
            entity_index,
            archetype_hash,
            provider_path,
            asset_ref,
            asset_kind,
            transform: spatial.world_transform,
            resolution,
            reason,
            collision,
        });
    }

    let resolved_nodes = nodes
        .iter()
        .filter(|node| node.resolution == SceneResolutionState::Resolved)
        .count();
    let unresolved_nodes = nodes.len().saturating_sub(resolved_nodes);

    SceneManifest {
        schema_version: 1,
        root: SceneRoot {
            path: ymap_path.to_path_buf(),
            name_hash: ymap.name.map(|hash| hash.0),
        },
        nodes,
        summary: SceneSummary {
            total_entities: ymap.entities.len(),
            emitted_nodes,
            resolved_nodes,
            unresolved_nodes,
            asset_references: assets.len(),
        },
        assets,
        warnings,
        limits: SceneLimits {
            max_nodes,
            truncated: omitted_entities > 0,
            omitted_entities,
        },
    }
}

fn insert_primary_asset(
    asset: &SceneResolvedAsset,
    texture_dictionary: &SceneTextureDictionaryLookup,
    assets: &mut Vec<SceneAssetReference>,
    ids: &mut BTreeMap<SceneAssetKey, usize>,
) -> usize {
    match asset {
        SceneResolvedAsset::Drawable { hash, locator } => insert_asset(
            AssetKind::Ydr,
            *hash,
            locator.clone(),
            None,
            texture_dictionary.clone(),
            assets,
            ids,
        ),
        SceneResolvedAsset::Fragment { hash, locator } => insert_asset(
            AssetKind::Yft,
            *hash,
            locator.clone(),
            None,
            texture_dictionary.clone(),
            assets,
            ids,
        ),
        SceneResolvedAsset::DrawableDictionary {
            dictionary_hash,
            locator,
            entry,
        } => insert_asset(
            AssetKind::Ydd,
            *dictionary_hash,
            locator.clone(),
            Some(SceneAssetSelector::YddDrawable {
                index: entry.index,
                name_hash: entry.name_hash,
                name: entry.name.clone(),
            }),
            texture_dictionary.clone(),
            assets,
            ids,
        ),
    }
}

fn collision_relationship(
    collision: SceneCollisionLookup,
    assets: &mut Vec<SceneAssetReference>,
    ids: &mut BTreeMap<SceneAssetKey, usize>,
) -> Option<SceneCollisionRelationship> {
    match collision {
        SceneCollisionLookup::None => None,
        SceneCollisionLookup::LocalOnly { hash, locator } => {
            let asset_ref = insert_asset(
                AssetKind::Ybn,
                hash,
                locator,
                None,
                SceneTextureDictionaryLookup::None,
                assets,
                ids,
            );
            Some(SceneCollisionRelationship {
                hash,
                asset_ref: Some(asset_ref),
                state: SceneCollisionState::LocalOnly,
                reason:
                    "YTYP proves a collision dependency, but no collision placement transform; YBN remains local-only"
                        .into(),
            })
        }
        SceneCollisionLookup::Unresolved {
            hash,
            code,
            message,
        } => Some(SceneCollisionRelationship {
            hash,
            asset_ref: None,
            state: SceneCollisionState::Unresolved,
            reason: match code {
                SceneCollisionLookupErrorCode::MissingAsset
                | SceneCollisionLookupErrorCode::AmbiguousAsset => message,
            },
        }),
    }
}

fn insert_asset(
    kind: AssetKind,
    hash: u32,
    path: SceneAssetLocator,
    selector: Option<SceneAssetSelector>,
    texture_dictionary: SceneTextureDictionaryLookup,
    assets: &mut Vec<SceneAssetReference>,
    ids: &mut BTreeMap<SceneAssetKey, usize>,
) -> usize {
    let key = SceneAssetKey {
        kind,
        hash,
        path: path.clone(),
        selector: selector.clone(),
        texture_dictionary: texture_dictionary.clone(),
    };
    if let Some(id) = ids.get(&key) {
        return *id;
    }

    let id = assets.len();
    assets.push(SceneAssetReference {
        id,
        kind,
        hash,
        path,
        selector,
        texture_dictionary,
    });
    ids.insert(key, id);
    id
}

fn scene_lookup_reason(error: SceneAssetLookupError) -> SceneResolutionReason {
    let code = match error.code {
        SceneAssetLookupErrorCode::ProviderMissing => SceneResolutionReasonCode::ProviderMissing,
        SceneAssetLookupErrorCode::ProviderAmbiguous => {
            SceneResolutionReasonCode::ProviderAmbiguous
        }
        SceneAssetLookupErrorCode::AssetNameMissing => SceneResolutionReasonCode::AssetNameMissing,
        SceneAssetLookupErrorCode::DrawableDictionaryMissing => {
            SceneResolutionReasonCode::DrawableDictionaryMissing
        }
        SceneAssetLookupErrorCode::AssetMissing => SceneResolutionReasonCode::AssetMissing,
        SceneAssetLookupErrorCode::AssetAmbiguous => SceneResolutionReasonCode::AssetAmbiguous,
        SceneAssetLookupErrorCode::DictionaryUnreadable => {
            SceneResolutionReasonCode::DictionaryUnreadable
        }
        SceneAssetLookupErrorCode::DictionaryEntryMissing => {
            SceneResolutionReasonCode::DictionaryEntryMissing
        }
        SceneAssetLookupErrorCode::UnsupportedAssetType => {
            SceneResolutionReasonCode::UnsupportedAssetRelation
        }
    };
    SceneResolutionReason {
        code,
        message: error.message,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use ragelab_assets::{
        AssetKind, SceneAssetLookupError, SceneAssetLookupErrorCode, WorkspaceIndex,
    };
    use ragelab_ymap::Ymap;

    use super::*;

    const SIMPLE_YMAP: &[u8] = include_bytes!("../../../fixtures/synthetic/simple.ymap");

    fn synthetic_workspace() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/synthetic/stream")
            .canonicalize()
            .expect("synthetic workspace")
    }

    fn parsed_map() -> Ymap {
        Ymap::from_bytes(SIMPLE_YMAP).expect("synthetic YMAP")
    }

    #[test]
    fn assembles_resolved_drawable_with_proven_transform_and_local_collision_only() {
        let workspace = synthetic_workspace();
        let index = WorkspaceIndex::scan(&workspace).expect("scan synthetic workspace");
        let ymap = parsed_map();
        let manifest = assemble_ymap_scene(
            &index,
            &workspace.join("simple.ymap"),
            &ymap,
            SceneAssemblyOptions::default(),
        );

        assert_eq!(manifest.nodes.len(), 1);
        assert_eq!(manifest.summary.resolved_nodes, 1);
        let node = &manifest.nodes[0];
        assert_eq!(node.resolution, SceneResolutionState::Resolved);
        assert_eq!(node.asset_kind, Some(AssetKind::Ydr));
        let transform = node.transform.expect("entity transform");
        assert_eq!(transform.scale, Some([1.25, 1.25, 0.75]));
        assert_eq!(transform.translation, [1.0, 2.0, 3.0]);

        let collision = node
            .collision
            .as_ref()
            .expect("declared collision relation");
        assert_eq!(collision.state, SceneCollisionState::LocalOnly);
        assert!(collision.asset_ref.is_some());
        assert_eq!(
            manifest.nodes.len(),
            1,
            "collision must not become a world node"
        );
        assert_eq!(manifest.assets.len(), 2, "YDR + dependency-only YBN");
    }

    #[test]
    fn workspace_scene_asset_preview_reads_asset_ref_through_core_locator() {
        let source = synthetic_workspace();
        let temporary =
            std::env::temp_dir().join(format!("ragelab-scene-preview-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temporary);
        fs::create_dir_all(&temporary).expect("create preview workspace");

        fs::copy(source.join("simple.ymap"), temporary.join("simple.ymap")).expect("copy YMAP");
        fs::copy(source.join("simple.ytyp"), temporary.join("simple.ytyp")).expect("copy YTYP");
        fs::copy(
            source.join("../ydr/simple.ydr"),
            temporary.join("test_drawable.ydr"),
        )
        .expect("copy valid YDR");
        fs::copy(
            source.join("../ybn/simple.ybn"),
            temporary.join("test_collision.ybn"),
        )
        .expect("copy valid YBN");

        let report = workspace_scene_asset_preview_with_sources(
            &temporary,
            Path::new("simple.ymap"),
            &[],
            &[],
            SceneAssemblyOptions::default(),
            0,
            PreviewOptions::default(),
        )
        .expect("preview scene assetRef");

        assert_eq!(report.asset_type, "YDR");
        assert!(report.path.ends_with("test_drawable.ydr"));
        assert_eq!(report.spatial.classification, "localOnly");

        fs::remove_dir_all(&temporary).expect("remove preview workspace");
    }

    #[test]
    fn serialized_manifest_report_preserves_cli_scene_contract() {
        let workspace = synthetic_workspace();
        let index = WorkspaceIndex::scan(&workspace).expect("scan synthetic workspace");
        let ymap = parsed_map();
        let manifest = assemble_ymap_scene(
            &index,
            &workspace.join("simple.ymap"),
            &ymap,
            SceneAssemblyOptions::new(100),
        );

        let report = SceneManifestReport::from(&manifest);
        let value = serde_json::to_value(report).expect("serialize scene report");

        assert_eq!(value["schemaVersion"], 1);
        assert!(value["root"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("simple.ymap")));
        assert_eq!(value["summary"]["totalEntities"], 1);
        assert_eq!(value["summary"]["resolvedNodes"], 1);
        assert_eq!(value["nodes"][0]["assetKind"], "YDR");
        assert_eq!(value["nodes"][0]["resolution"], "resolved");
        assert_eq!(
            value["nodes"][0]["transform"]["scale"],
            serde_json::json!([1.25, 1.25, 0.75])
        );
        assert_eq!(value["nodes"][0]["collision"]["state"], "localOnly");
        assert!(value["nodes"][0]["archetypeHash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("0x")));
        assert_eq!(value["limits"]["maxNodes"], 100);
        assert_eq!(value["limits"]["truncated"], false);
    }

    #[test]
    fn workspace_scene_report_owns_relative_path_scan_parse_and_limits() {
        let workspace = synthetic_workspace();
        let report = workspace_scene_report(
            &workspace,
            Path::new("simple.ymap"),
            SceneAssemblyOptions::new(1),
        )
        .expect("assemble workspace scene report");

        assert!(report.root.path.ends_with("simple.ymap"));
        assert_eq!(report.summary.total_entities, 1);
        assert_eq!(report.summary.emitted_nodes, 1);
        assert_eq!(report.summary.resolved_nodes, 1);
        assert_eq!(report.nodes[0].resolution, "resolved");
        assert_eq!(report.limits.max_nodes, 1);
        assert!(!report.limits.truncated);
    }

    #[test]
    fn workspace_scene_report_uses_fallbacks_and_preserves_primary_precedence() {
        let source = synthetic_workspace();
        let temporary =
            std::env::temp_dir().join(format!("ragelab-scene-fallback-{}", std::process::id()));
        let primary = temporary.join("primary");
        let fallback = temporary.join("fallback");
        let _ = fs::remove_dir_all(&temporary);
        fs::create_dir_all(&primary).expect("create primary workspace");
        fs::create_dir_all(&fallback).expect("create fallback workspace");

        fs::copy(source.join("simple.ymap"), primary.join("simple.ymap"))
            .expect("copy YMAP to primary");
        for name in ["simple.ytyp", "test_drawable.ydr", "test_collision.ybn"] {
            fs::copy(source.join(name), fallback.join(name)).expect("copy fallback asset");
        }

        let fallback_roots = vec![fallback.clone()];
        let report = workspace_scene_report_with_fallbacks(
            &primary,
            Path::new("simple.ymap"),
            &fallback_roots,
            SceneAssemblyOptions::default(),
        )
        .expect("assemble scene from fallback");

        assert_eq!(report.summary.resolved_nodes, 1);
        assert!(
            Path::new(&report.assets[0].path).starts_with(&fallback),
            "fallback should supply the drawable while primary is missing it"
        );

        for name in ["simple.ytyp", "test_drawable.ydr", "test_collision.ybn"] {
            fs::copy(source.join(name), primary.join(name)).expect("copy primary asset");
        }
        let report = workspace_scene_report_with_fallbacks(
            &primary,
            Path::new("simple.ymap"),
            &fallback_roots,
            SceneAssemblyOptions::default(),
        )
        .expect("assemble scene with primary override");

        assert_eq!(report.summary.resolved_nodes, 1);
        assert!(
            Path::new(&report.assets[0].path).starts_with(&primary),
            "primary workspace must override the fallback for the same identity"
        );

        fs::remove_dir_all(&temporary).expect("remove fallback fixture");
    }

    #[test]
    fn workspace_scene_report_rejects_missing_workspace() {
        let workspace = std::env::temp_dir().join("ragelab-scene-missing-workspace");
        let error = workspace_scene_report(
            &workspace,
            Path::new("simple.ymap"),
            SceneAssemblyOptions::default(),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn deduplicates_repeated_instances_of_the_same_asset() {
        let workspace = synthetic_workspace();
        let index = WorkspaceIndex::scan(&workspace).expect("scan synthetic workspace");
        let mut ymap = parsed_map();
        ymap.entities.push(ymap.entities[0].clone());

        let manifest = assemble_ymap_scene(
            &index,
            &workspace.join("simple.ymap"),
            &ymap,
            SceneAssemblyOptions::default(),
        );

        assert_eq!(manifest.nodes.len(), 2);
        assert_eq!(manifest.nodes[0].asset_ref, manifest.nodes[1].asset_ref);
        assert_eq!(
            manifest.assets.len(),
            2,
            "one YDR and one shared YBN reference"
        );
    }

    #[test]
    fn missing_provider_is_explicitly_unresolved() {
        let workspace = synthetic_workspace();
        let index = WorkspaceIndex::scan(&workspace).expect("scan synthetic workspace");
        let mut ymap = parsed_map();
        ymap.entities[0].archetype_name.0 = 0xDEAD_BEEF;

        let manifest = assemble_ymap_scene(
            &index,
            &workspace.join("simple.ymap"),
            &ymap,
            SceneAssemblyOptions::default(),
        );

        let node = &manifest.nodes[0];
        assert_eq!(node.resolution, SceneResolutionState::Unresolved);
        assert_eq!(
            node.reason.as_ref().map(|reason| reason.code),
            Some(SceneResolutionReasonCode::ProviderMissing)
        );
        assert!(node.asset_ref.is_none());
        assert!(node.transform.is_some(), "spatial evidence is preserved");
    }

    #[test]
    fn provider_with_missing_drawable_is_distinct_from_missing_provider() {
        let source = synthetic_workspace();
        let temporary =
            std::env::temp_dir().join(format!("ragelab-scene-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temporary);
        fs::create_dir_all(&temporary).expect("create temp workspace");
        fs::copy(source.join("simple.ytyp"), temporary.join("simple.ytyp")).expect("copy YTYP");

        let index = WorkspaceIndex::scan(&temporary).expect("scan temp workspace");
        let ymap = parsed_map();
        let manifest = assemble_ymap_scene(
            &index,
            &temporary.join("simple.ymap"),
            &ymap,
            SceneAssemblyOptions::default(),
        );
        fs::remove_dir_all(&temporary).expect("remove temp workspace");

        assert_eq!(
            manifest.nodes[0].reason.as_ref().map(|reason| reason.code),
            Some(SceneResolutionReasonCode::AssetMissing)
        );
    }

    #[test]
    fn unsupported_asset_relation_is_not_guessed() {
        let ymap = parsed_map();
        let manifest = assemble_ymap_scene_with_lookup(
            Path::new("/workspace/simple.ymap"),
            &ymap,
            SceneAssemblyOptions::default(),
            |archetype_hash| {
                Err(SceneAssetLookupError {
                    code: SceneAssetLookupErrorCode::UnsupportedAssetType,
                    archetype_hash,
                    provider: Some("/workspace/simple.ytyp".into()),
                    expected_kind: Some(AssetKind::Yft),
                    hash: Some(0x1234_5678),
                    message: "fragment assets are outside the scene drawable contract".into(),
                })
            },
        );

        assert_eq!(
            manifest.nodes[0].resolution,
            SceneResolutionState::Unresolved
        );
        assert_eq!(
            manifest.nodes[0].reason.as_ref().map(|reason| reason.code),
            Some(SceneResolutionReasonCode::UnsupportedAssetRelation)
        );
        assert!(manifest.nodes[0].asset_ref.is_none());
    }

    #[test]
    fn scene_ordering_and_asset_ids_are_deterministic() {
        let workspace = synthetic_workspace();
        let index = WorkspaceIndex::scan(&workspace).expect("scan synthetic workspace");
        let mut ymap = parsed_map();
        ymap.entities.push(ymap.entities[0].clone());

        let first = assemble_ymap_scene(
            &index,
            &workspace.join("simple.ymap"),
            &ymap,
            SceneAssemblyOptions::default(),
        );
        let second = assemble_ymap_scene(
            &index,
            &workspace.join("simple.ymap"),
            &ymap,
            SceneAssemblyOptions::default(),
        );

        assert_eq!(first, second);
    }

    #[test]
    fn scene_limit_is_explicit_and_diagnostic() {
        let workspace = synthetic_workspace();
        let index = WorkspaceIndex::scan(&workspace).expect("scan synthetic workspace");
        let mut ymap = parsed_map();
        ymap.entities.push(ymap.entities[0].clone());
        ymap.entities.push(ymap.entities[0].clone());

        let manifest = assemble_ymap_scene(
            &index,
            &workspace.join("simple.ymap"),
            &ymap,
            SceneAssemblyOptions::new(2),
        );

        assert_eq!(manifest.nodes.len(), 2);
        assert!(manifest.limits.truncated);
        assert_eq!(manifest.limits.omitted_entities, 1);
        assert_eq!(manifest.summary.total_entities, 3);
        assert!(!manifest.warnings.is_empty());
    }
}
