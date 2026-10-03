//! Rust-owned YMAP scene assembly.
//!
//! This module intentionally emits only normalized instance/reference metadata.
//! Geometry remains behind the existing lazy preview endpoints.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_assets::{
    AssetKind, SceneArchetypeResolution, SceneAssetLookupError, SceneAssetLookupErrorCode,
    SceneCollisionLookup, SceneCollisionLookupErrorCode, SceneResolvedAsset, WorkspaceIndex,
};
use ragelab_ymap::Ymap;
use serde::Serialize;

use crate::{ymap_entity_spatial_context, SpatialTransform};

pub const DEFAULT_SCENE_NODE_LIMIT: usize = 10_000;
pub const MAX_SCENE_NODE_LIMIT: usize = 50_000;

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
    pub provider_path: Option<PathBuf>,
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
    pub path: PathBuf,
    pub selector: Option<SceneAssetSelector>,
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
                    provider_path: node
                        .provider_path
                        .as_ref()
                        .map(|path| path.display().to_string()),
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
                    path: asset.path.display().to_string(),
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SceneAssetKey {
    kind: AssetKind,
    hash: u32,
    path: PathBuf,
    selector: Option<SceneAssetSelector>,
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
    if !workspace.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a directory: {}", workspace.display()),
        ));
    }

    let resolved_ymap = if ymap_path.is_absolute() {
        ymap_path.to_path_buf()
    } else {
        workspace.join(ymap_path)
    };
    let bytes = fs::read(&resolved_ymap)?;
    let ymap = Ymap::from_bytes(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let index =
        WorkspaceIndex::scan(workspace).map_err(|error| io::Error::other(error.to_string()))?;
    let manifest = assemble_ymap_scene(&index, &resolved_ymap, &ymap, options);
    Ok(SceneManifestReport::from(&manifest))
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
                let asset_ref =
                    insert_primary_asset(&resolution.asset, &mut assets, &mut asset_ids);
                let collision =
                    collision_relationship(resolution.collision, &mut assets, &mut asset_ids);
                (
                    Some(resolution.provider_path),
                    Some(asset_ref),
                    Some(asset_kind),
                    collision,
                    None,
                )
            }
            Err(error) => {
                let provider_path = error.provider_path.clone();
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
    assets: &mut Vec<SceneAssetReference>,
    ids: &mut BTreeMap<SceneAssetKey, usize>,
) -> usize {
    match asset {
        SceneResolvedAsset::Drawable { hash, path } => {
            insert_asset(AssetKind::Ydr, *hash, path.clone(), None, assets, ids)
        }
        SceneResolvedAsset::DrawableDictionary {
            dictionary_hash,
            path,
            entry,
        } => insert_asset(
            AssetKind::Ydd,
            *dictionary_hash,
            path.clone(),
            Some(SceneAssetSelector::YddDrawable {
                index: entry.index,
                name_hash: entry.name_hash,
                name: entry.name.clone(),
            }),
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
        SceneCollisionLookup::LocalOnly { hash, path } => {
            let asset_ref = insert_asset(AssetKind::Ybn, hash, path, None, assets, ids);
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
    path: PathBuf,
    selector: Option<SceneAssetSelector>,
    assets: &mut Vec<SceneAssetReference>,
    ids: &mut BTreeMap<SceneAssetKey, usize>,
) -> usize {
    let key = SceneAssetKey {
        kind,
        hash,
        path: path.clone(),
        selector: selector.clone(),
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
                    provider_path: Some(PathBuf::from("/workspace/simple.ytyp")),
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
