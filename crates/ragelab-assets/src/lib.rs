//! Filesystem asset indexing and the first map dependency resolver.
//!
//! This crate intentionally sits above the binary format crates. It is allowed
//! to use filesystem paths; `ragelab-ymap`/`ragelab-ytyp` remain byte-oriented and
//! suitable for future WASM adapters.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use ragelab_hash::joaat;
use ragelab_meta::MetaHash;
use ragelab_rbf::{RbfFile, RbfStructure, RbfValue};
use ragelab_rpf::{RpfEntryLocator, RpfMount};
use ragelab_ydd::YddDictionary;
use ragelab_ymap::Ymap;
use ragelab_ymf::{ManifestFlags, Ymf, YmfInteriorBounds, YmfMapDependency, YmfYtypDependency};
use ragelab_ytyp::{Archetype, ArchetypeKind, AssetType, Ytyp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AssetKind {
    Ymap,
    Ytyp,
    Ydr,
    Ydd,
    Ytd,
    Ybn,
    Yft,
    Ycd,
}

impl AssetKind {
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "ymap" => Some(Self::Ymap),
            "ytyp" => Some(Self::Ytyp),
            "ydr" => Some(Self::Ydr),
            "ydd" => Some(Self::Ydd),
            "ytd" => Some(Self::Ytd),
            "ybn" => Some(Self::Ybn),
            "yft" => Some(Self::Yft),
            "ycd" => Some(Self::Ycd),
            _ => None,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Ymap => "ymap",
            Self::Ytyp => "ytyp",
            Self::Ydr => "ydr",
            Self::Ydd => "ydd",
            Self::Ytd => "ytd",
            Self::Ybn => "ybn",
            Self::Yft => "yft",
            Self::Ycd => "ycd",
        }
    }
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.extension().to_ascii_uppercase())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DependencyReason {
    SelectedYmap,
    ParentYmap { child_hash: u32 },
    ChildYmap { parent_hash: u32 },
    YmapPhysicsDictionary { ymap_hash: Option<u32> },
    ArchetypeProvider { archetype: u32 },
    PrimaryAsset { archetype: u32 },
    TextureDictionary { archetype: u32 },
    ParentTextureDictionary { child_hash: u32 },
    PhysicsDictionary { archetype: u32 },
    ClipDictionary { archetype: u32 },
    DrawableDictionary { archetype: u32 },
    YtypDependency { ytyp_hash: Option<u32> },
    MloInteriorBounds { archetype: u32 },
}

impl fmt::Display for DependencyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelectedYmap => f.write_str("selected YMAP"),
            Self::ParentYmap { child_hash } => {
                write!(f, "parent YMAP of 0x{child_hash:08X}")
            }
            Self::ChildYmap { parent_hash } => {
                write!(f, "child YMAP of 0x{parent_hash:08X}")
            }
            Self::YmapPhysicsDictionary { ymap_hash } => match ymap_hash {
                Some(hash) => write!(f, "YMAP 0x{hash:08X} physics dictionary"),
                None => f.write_str("YMAP physics dictionary"),
            },
            Self::ArchetypeProvider { archetype } => {
                write!(f, "defines archetype 0x{archetype:08X}")
            }
            Self::PrimaryAsset { archetype } => {
                write!(f, "primary asset for archetype 0x{archetype:08X}")
            }
            Self::TextureDictionary { archetype } => {
                write!(f, "texture dictionary for archetype 0x{archetype:08X}")
            }
            Self::ParentTextureDictionary { child_hash } => {
                write!(f, "parent texture dictionary of 0x{child_hash:08X}")
            }
            Self::PhysicsDictionary { archetype } => {
                write!(f, "physics dictionary for archetype 0x{archetype:08X}")
            }
            Self::ClipDictionary { archetype } => {
                write!(f, "clip dictionary for archetype 0x{archetype:08X}")
            }
            Self::DrawableDictionary { archetype } => {
                write!(f, "drawable dictionary for archetype 0x{archetype:08X}")
            }
            Self::YtypDependency { ytyp_hash } => match ytyp_hash {
                Some(hash) => write!(f, "declared by YTYP 0x{hash:08X}"),
                None => f.write_str("declared by YTYP"),
            },
            Self::MloInteriorBounds { archetype } => {
                write!(f, "MLO interior bounds for archetype 0x{archetype:08X}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFile {
    pub path: PathBuf,
    pub kind: AssetKind,
    pub hashes: BTreeSet<u32>,
    pub reasons: BTreeSet<DependencyReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnresolvedKind {
    ArchetypeProvider,
    File(AssetKind),
    PrimaryAssetUnknownType,
}

impl fmt::Display for UnresolvedKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArchetypeProvider => f.write_str("YTYP provider"),
            Self::File(kind) => write!(f, "{kind}"),
            Self::PrimaryAssetUnknownType => f.write_str("primary asset (unknown type)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedDependency {
    pub kind: UnresolvedKind,
    pub hash: u32,
    pub reason: DependencyReason,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextureParentRelationship {
    pub parent: String,
    pub child: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyReport {
    pub files: BTreeMap<PathBuf, ResolvedFile>,
    pub unresolved: Vec<UnresolvedDependency>,
    pub warnings: Vec<String>,
    pub texture_parents: BTreeSet<TextureParentRelationship>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Permit exporting even when local dependencies remain unresolved.
    /// External/vanilla archetypes commonly appear unresolved, so callers may
    /// opt in after reviewing the report.
    pub allow_unresolved: bool,
    /// Remove an existing output directory before writing the new resource.
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct ExportResult {
    pub output_dir: PathBuf,
    pub stream_dir: PathBuf,
    pub copied_files: Vec<PathBuf>,
    pub manifest_path: PathBuf,
    pub gtxd_path: Option<PathBuf>,
    pub report: DependencyReport,
    pub manifest: Ymf,
}

#[derive(Debug, Clone)]
pub struct MloArchetypeAudit {
    pub archetype_hash: u32,
    pub ytyp_path: PathBuf,
    pub entity_count: usize,
    pub unique_entity_archetypes: usize,
    pub room_count: usize,
    pub portal_count: usize,
    pub report: DependencyReport,
}

/// Scene-facing asset identity. Dependency/export APIs intentionally remain
/// filesystem-only; this locator exists so preview/scene assembly can read a
/// proven asset directly from an RPF without materializing it to disk.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SceneAssetLocator {
    Loose(PathBuf),
    Rpf(RpfEntryLocator),
}

impl SceneAssetLocator {
    pub fn provenance(&self) -> String {
        match self {
            Self::Loose(path) => path.display().to_string(),
            Self::Rpf(locator) => locator.provenance(),
        }
    }

    pub const fn source_type(&self) -> &'static str {
        match self {
            Self::Loose(_) => "loose",
            Self::Rpf(_) => "rpf",
        }
    }

    pub fn read_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::Loose(path) => {
                fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
            }
            Self::Rpf(locator) => locator.read().map_err(|error| error.to_string()),
        }
    }
}

impl From<PathBuf> for SceneAssetLocator {
    fn from(value: PathBuf) -> Self {
        Self::Loose(value)
    }
}

/// Strict scene-facing lookup result for one YMAP archetype.
///
/// Dependency closure can retain multiple candidates for diagnostics, but a
/// scene node must never silently pick one. This contract fails closed unless
/// provider and renderable identity are uniquely proven by the workspace or a
/// lower-priority read-only game source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneArchetypeResolution {
    pub archetype_hash: u32,
    pub provider: SceneAssetLocator,
    pub provider_ytyp_hash: Option<u32>,
    pub asset: SceneResolvedAsset,
    pub texture_dictionary: SceneTextureDictionaryLookup,
    pub collision: SceneCollisionLookup,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneTextureDictionarySource {
    pub hash: u32,
    pub locator: SceneAssetLocator,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneTextureDictionaryAmbiguity {
    pub hash: u32,
    pub candidates: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SceneTextureDictionaryLookup {
    None,
    Chain {
        hash: u32,
        sources: Vec<SceneTextureDictionarySource>,
        missing: Vec<u32>,
        ambiguous: Vec<SceneTextureDictionaryAmbiguity>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneResolvedAsset {
    Drawable {
        hash: u32,
        locator: SceneAssetLocator,
    },
    Fragment {
        hash: u32,
        locator: SceneAssetLocator,
    },
    DrawableDictionary {
        dictionary_hash: u32,
        locator: SceneAssetLocator,
        entry: SceneDrawableDictionaryEntry,
    },
}

impl SceneResolvedAsset {
    pub const fn kind(&self) -> AssetKind {
        match self {
            Self::Drawable { .. } => AssetKind::Ydr,
            Self::Fragment { .. } => AssetKind::Yft,
            Self::DrawableDictionary { .. } => AssetKind::Ydd,
        }
    }

    pub fn locator(&self) -> &SceneAssetLocator {
        match self {
            Self::Drawable { locator, .. }
            | Self::Fragment { locator, .. }
            | Self::DrawableDictionary { locator, .. } => locator,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneDrawableDictionaryEntry {
    pub index: usize,
    pub name_hash: u32,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneCollisionLookup {
    None,
    LocalOnly {
        hash: u32,
        locator: SceneAssetLocator,
    },
    Unresolved {
        hash: u32,
        code: SceneCollisionLookupErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneCollisionLookupErrorCode {
    MissingAsset,
    AmbiguousAsset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneAssetLookupError {
    pub code: SceneAssetLookupErrorCode,
    pub archetype_hash: u32,
    pub provider: Option<String>,
    pub expected_kind: Option<AssetKind>,
    pub hash: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneAssetLookupErrorCode {
    ProviderMissing,
    ProviderAmbiguous,
    AssetNameMissing,
    DrawableDictionaryMissing,
    AssetMissing,
    AssetAmbiguous,
    DictionaryUnreadable,
    DictionaryEntryMissing,
    UnsupportedAssetType,
}

impl DependencyReport {
    fn add_file(
        &mut self,
        path: PathBuf,
        kind: AssetKind,
        hash: Option<u32>,
        reason: DependencyReason,
    ) {
        let entry = self
            .files
            .entry(path.clone())
            .or_insert_with(|| ResolvedFile {
                path,
                kind,
                hashes: BTreeSet::new(),
                reasons: BTreeSet::new(),
            });
        if let Some(hash) = hash {
            entry.hashes.insert(hash);
        }
        entry.reasons.insert(reason);
    }

    fn add_unresolved(&mut self, kind: UnresolvedKind, hash: u32, reason: DependencyReason) {
        let item = UnresolvedDependency { kind, hash, reason };
        if !self.unresolved.contains(&item) {
            self.unresolved.push(item);
        }
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.keys().map(PathBuf::as_path)
    }
}

#[derive(Debug, Clone)]
struct ArchetypeProvider {
    path: PathBuf,
    ytyp_name: Option<MetaHash>,
    ytyp_dependencies: Vec<MetaHash>,
    archetype: Archetype,
}

#[derive(Debug, Clone)]
struct RpfArchetypeProvider {
    locator: RpfEntryLocator,
    ytyp_name: Option<MetaHash>,
    archetype: Archetype,
}

#[derive(Debug, Clone)]
struct YtypRecord {
    name: MetaHash,
    dependencies: Vec<MetaHash>,
    contains_mlo: bool,
}

#[derive(Debug, Clone, Default)]
struct ManifestDependencyRecord {
    dependencies: BTreeSet<u32>,
    flags: ManifestFlags,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextureParentRecord {
    parent: String,
    parent_hash: u32,
    child: String,
    source: PathBuf,
}

#[derive(Debug, Clone)]
pub struct WorkspaceIndex {
    root: PathBuf,
    files: BTreeMap<(AssetKind, u32), Vec<PathBuf>>,
    archetype_providers: BTreeMap<u32, Vec<ArchetypeProvider>>,
    scene_rpf_files: BTreeMap<(AssetKind, u32), Vec<RpfEntryLocator>>,
    scene_rpf_archetype_providers: BTreeMap<u32, Vec<RpfArchetypeProvider>>,
    ytyp_records: BTreeMap<u32, Vec<YtypRecord>>,
    source_map_dependencies: BTreeMap<u32, ManifestDependencyRecord>,
    source_ytyp_dependencies: BTreeMap<u32, ManifestDependencyRecord>,
    source_interior_bounds: BTreeMap<u32, BTreeSet<u32>>,
    ymap_children: BTreeMap<u32, BTreeMap<u32, Vec<PathBuf>>>,
    texture_parents: BTreeMap<u32, Vec<TextureParentRecord>>,
    scene_rpf_texture_parents: BTreeMap<u32, Vec<TextureParentRelationship>>,
    pub warnings: Vec<String>,
}

impl WorkspaceIndex {
    pub fn scan(root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_dir() {
            return Err(WorkspaceError::InvalidRoot(root));
        }

        let mut paths = Vec::new();
        walk(&root, &mut paths).map_err(|source| WorkspaceError::Io {
            path: root.clone(),
            source,
        })?;
        paths.sort();

        let mut index = Self {
            root,
            files: BTreeMap::new(),
            archetype_providers: BTreeMap::new(),
            scene_rpf_files: BTreeMap::new(),
            scene_rpf_archetype_providers: BTreeMap::new(),
            ytyp_records: BTreeMap::new(),
            source_map_dependencies: BTreeMap::new(),
            source_ytyp_dependencies: BTreeMap::new(),
            source_interior_bounds: BTreeMap::new(),
            ymap_children: BTreeMap::new(),
            texture_parents: BTreeMap::new(),
            scene_rpf_texture_parents: BTreeMap::new(),
            warnings: Vec::new(),
        };

        for path in &paths {
            let Some(kind) = file_kind(path) else {
                continue;
            };
            if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                index.insert_file(kind, joaat(stem), path.clone());
            }
        }

        // GTXD parenting data is metadata, not part of the YTD binary itself.
        // FiveM commonly exposes it as XML `gtxd.meta`; the base game also uses
        // a direct RBF0 `gtxd.ymt`. Both normalize into the same relationship
        // domain before resolver closure runs.
        for path in &paths {
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if file_name.eq_ignore_ascii_case("gtxd.ymt") {
                let bytes = match fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        index.warnings.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                };
                match parse_gtxd_rbf(&bytes) {
                    Ok(relationships) => {
                        for relationship in relationships {
                            index.insert_texture_parent(relationship, path.clone());
                        }
                    }
                    Err(error) => index.warnings.push(format!(
                        "{}: GTXD RBF parse failed: {error}",
                        path.display()
                    )),
                }
                continue;
            }
            if !file_name.eq_ignore_ascii_case("gtxd.meta") {
                continue;
            }

            let xml = match fs::read_to_string(path) {
                Ok(xml) => xml,
                Err(error) => {
                    index.warnings.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            match parse_gtxd_xml(&xml) {
                Ok(relationships) => {
                    for relationship in relationships {
                        index.insert_texture_parent(relationship, path.clone());
                    }
                }
                Err(error) => index
                    .warnings
                    .push(format!("{}: GTXD parse failed: {error}", path.display())),
            }
        }

        // Preserve dependency relationships from source pack manifests when
        // available. MLO YTYP binaries commonly have an empty `dependencies`
        // array even though the pack manifest declares the vanilla/custom
        // ITYPs needed by embedded interior entities.
        for path in paths.iter().filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("ymf"))
        }) {
            let bytes = match fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    index.warnings.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            match Ymf::from_bytes(&bytes) {
                Ok(manifest) => {
                    for map in manifest.maps {
                        let record = index.source_map_dependencies.entry(map.ymap.0).or_default();
                        record.flags.0 |= map.flags.0;
                        record
                            .dependencies
                            .extend(map.ytyps.into_iter().map(|hash| hash.0));
                    }
                    for ytyp in manifest.ytyps {
                        let record = index
                            .source_ytyp_dependencies
                            .entry(ytyp.ytyp.0)
                            .or_default();
                        record.flags.0 |= ytyp.flags.0;
                        record
                            .dependencies
                            .extend(ytyp.ytyps.into_iter().map(|hash| hash.0));
                    }
                    for interior in manifest.interiors {
                        index
                            .source_interior_bounds
                            .entry(interior.name.0)
                            .or_default()
                            .extend(interior.bounds.into_iter().map(|hash| hash.0));
                    }
                }
                Err(error) => index
                    .warnings
                    .push(format!("{}: YMF parse failed: {error}", path.display())),
            }
        }

        // Parse YTYPs to index internal names and archetype providers.
        for path in paths
            .iter()
            .filter(|path| file_kind(path) == Some(AssetKind::Ytyp))
        {
            let bytes = match fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    index.warnings.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };

            match Ytyp::from_bytes(&bytes) {
                Ok(ytyp) => {
                    let internal_name = ytyp.name.or_else(|| stem_hash(path).map(MetaHash));
                    if let Some(name) = internal_name {
                        index.insert_file(AssetKind::Ytyp, name.0, path.clone());
                        index
                            .ytyp_records
                            .entry(name.0)
                            .or_default()
                            .push(YtypRecord {
                                name,
                                dependencies: ytyp.dependencies.clone(),
                                contains_mlo: ytyp
                                    .archetypes
                                    .iter()
                                    .any(|item| item.kind == ArchetypeKind::Mlo),
                            });
                    }
                    for archetype in ytyp.archetypes {
                        index
                            .archetype_providers
                            .entry(archetype.name.0)
                            .or_default()
                            .push(ArchetypeProvider {
                                path: path.clone(),
                                ytyp_name: internal_name,
                                ytyp_dependencies: ytyp.dependencies.clone(),
                                archetype,
                            });
                    }
                }
                Err(error) => index
                    .warnings
                    .push(format!("{}: YTYP parse failed: {error}", path.display())),
            }
        }

        // Parse YMAP names so parent hashes can resolve even when a filename
        // does not happen to be the canonical name hash.
        for path in paths
            .iter()
            .filter(|path| file_kind(path) == Some(AssetKind::Ymap))
        {
            let bytes = match fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    index.warnings.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            match Ymap::from_bytes(&bytes) {
                Ok(ymap) => {
                    let child_hash = ymap.name.map(|name| name.0).or_else(|| stem_hash(path));
                    if let Some(name) = ymap.name {
                        index.insert_file(AssetKind::Ymap, name.0, path.clone());
                    }
                    if let (Some(parent), Some(child_hash)) = (ymap.parent, child_hash) {
                        let paths = index
                            .ymap_children
                            .entry(parent.0)
                            .or_default()
                            .entry(child_hash)
                            .or_default();
                        if !paths.contains(path) {
                            paths.push(path.clone());
                        }
                    }
                }
                Err(error) => index
                    .warnings
                    .push(format!("{}: YMAP parse failed: {error}", path.display())),
            }
        }

        Ok(index)
    }

    /// Scan a primary workspace plus ordered read-only fallback roots.
    ///
    /// The primary workspace always wins for an indexed identity. A fallback
    /// contributes a file, archetype provider, or metadata record only when
    /// no higher-priority root already defines that identity. Ambiguity inside
    /// one root is preserved so the existing fail-closed resolver contract
    /// still applies.
    pub fn scan_with_fallbacks(
        root: impl AsRef<Path>,
        fallback_roots: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Result<Self, WorkspaceError> {
        let mut index = Self::scan(root)?;
        for fallback_root in fallback_roots {
            let fallback = Self::scan(fallback_root)?;
            index.merge_fallback(fallback);
        }
        Ok(index)
    }

    fn merge_fallback(&mut self, fallback: Self) {
        let fallback_root = fallback.root.clone();

        for (key, paths) in fallback.files {
            self.files.entry(key).or_insert(paths);
        }
        for (hash, providers) in fallback.archetype_providers {
            self.archetype_providers.entry(hash).or_insert(providers);
        }
        for (key, locators) in fallback.scene_rpf_files {
            self.scene_rpf_files.entry(key).or_insert(locators);
        }
        for (hash, providers) in fallback.scene_rpf_archetype_providers {
            self.scene_rpf_archetype_providers
                .entry(hash)
                .or_insert(providers);
        }
        for (hash, records) in fallback.ytyp_records {
            self.ytyp_records.entry(hash).or_insert(records);
        }
        for (hash, record) in fallback.source_map_dependencies {
            self.source_map_dependencies.entry(hash).or_insert(record);
        }
        for (hash, record) in fallback.source_ytyp_dependencies {
            self.source_ytyp_dependencies.entry(hash).or_insert(record);
        }
        for (hash, bounds) in fallback.source_interior_bounds {
            self.source_interior_bounds.entry(hash).or_insert(bounds);
        }
        for (parent_hash, children) in fallback.ymap_children {
            let target = self.ymap_children.entry(parent_hash).or_default();
            for (child_hash, paths) in children {
                target.entry(child_hash).or_insert(paths);
            }
        }
        for (child_hash, records) in fallback.texture_parents {
            self.texture_parents.entry(child_hash).or_insert(records);
        }
        for (child_hash, relationships) in fallback.scene_rpf_texture_parents {
            self.scene_rpf_texture_parents
                .entry(child_hash)
                .or_insert(relationships);
        }
        self.warnings.extend(
            fallback
                .warnings
                .into_iter()
                .map(|warning| format!("fallback {}: {warning}", fallback_root.display())),
        );
    }

    pub fn mount_scene_texture_parent(&mut self, relationship: TextureParentRelationship) {
        let child_hash = joaat(&relationship.child);
        let relationships = self
            .scene_rpf_texture_parents
            .entry(child_hash)
            .or_default();
        if !relationships.contains(&relationship) {
            relationships.push(relationship);
            relationships.sort();
        }
    }

    pub fn mount_scene_rpf(
        &mut self,
        archive_path: impl AsRef<Path>,
        nested: Vec<String>,
        keys_path: impl AsRef<Path>,
    ) -> Result<usize, WorkspaceError> {
        let archive_path = archive_path.as_ref().to_path_buf();
        let keys_path = keys_path.as_ref().to_path_buf();
        let mount = RpfMount::open(&archive_path, nested, &keys_path).map_err(|error| {
            WorkspaceError::Build(format!(
                "failed to mount RPF {}: {error}",
                archive_path.display()
            ))
        })?;

        let locators = mount
            .files()
            .filter(|file| {
                let path = Path::new(&file.path);
                file_kind(path).is_some()
                    && path.file_stem().and_then(|value| value.to_str()).is_some()
            })
            .map(|file| mount.locator(file.path))
            .collect::<Vec<_>>();
        let mounted = locators.len();
        self.mount_scene_rpf_entries(&locators)?;
        Ok(mounted)
    }

    /// Mount only explicitly selected RPF entries into scene resolution.
    ///
    /// This is the preferred path for a load-order-aware game index: losers
    /// are never added to the workspace overlay, so precedence is resolved
    /// before scene assembly and same-rank ambiguity remains explicit.
    pub fn mount_scene_rpf_entries(
        &mut self,
        locators: &[RpfEntryLocator],
    ) -> Result<usize, WorkspaceError> {
        for locator in locators {
            self.index_scene_rpf_locator(locator)?;
        }

        for candidates in self.scene_rpf_files.values_mut() {
            candidates.sort();
            candidates.dedup();
        }
        for providers in self.scene_rpf_archetype_providers.values_mut() {
            providers.sort_by(|left, right| left.locator.cmp(&right.locator));
            providers.dedup_by(|left, right| left.locator == right.locator);
        }

        Ok(locators.len())
    }

    fn index_scene_rpf_locator(&mut self, locator: &RpfEntryLocator) -> Result<(), WorkspaceError> {
        let virtual_path = Path::new(&locator.entry);
        let Some(kind) = file_kind(virtual_path) else {
            return Ok(());
        };
        let Some(stem) = virtual_path.file_stem().and_then(|value| value.to_str()) else {
            return Ok(());
        };

        self.insert_scene_rpf_file(kind, joaat(stem), locator.clone());
        if kind != AssetKind::Ytyp {
            return Ok(());
        }

        let bytes = locator.read().map_err(|error| {
            WorkspaceError::Build(format!(
                "{}: RPF YTYP read failed: {error}",
                locator.provenance()
            ))
        })?;
        let ytyp = Ytyp::from_bytes(&bytes).map_err(|error| {
            WorkspaceError::Build(format!(
                "{}: RPF YTYP parse failed: {error}",
                locator.provenance()
            ))
        })?;
        let internal_name = ytyp.name.or_else(|| Some(MetaHash(joaat(stem))));
        if let Some(name) = internal_name {
            self.insert_scene_rpf_file(AssetKind::Ytyp, name.0, locator.clone());
        }
        for archetype in ytyp.archetypes {
            self.scene_rpf_archetype_providers
                .entry(archetype.name.0)
                .or_default()
                .push(RpfArchetypeProvider {
                    locator: locator.clone(),
                    ytyp_name: internal_name,
                    archetype,
                });
        }

        Ok(())
    }

    pub fn mount_scene_rpf_provider_archetypes(
        &mut self,
        locator: &RpfEntryLocator,
        archetype_hashes: &[u32],
    ) -> Result<usize, WorkspaceError> {
        let virtual_path = Path::new(&locator.entry);
        if file_kind(virtual_path) != Some(AssetKind::Ytyp) {
            return Err(WorkspaceError::Build(format!(
                "{}: indexed provider entry is not a YTYP",
                locator.provenance()
            )));
        }
        let Some(stem) = virtual_path.file_stem().and_then(|value| value.to_str()) else {
            return Err(WorkspaceError::Build(format!(
                "{}: indexed provider YTYP has no file stem",
                locator.provenance()
            )));
        };

        let requested = archetype_hashes.iter().copied().collect::<BTreeSet<_>>();
        if requested.is_empty() {
            return Ok(0);
        }

        let bytes = locator.read().map_err(|error| {
            WorkspaceError::Build(format!(
                "{}: indexed provider YTYP read failed: {error}",
                locator.provenance()
            ))
        })?;
        let ytyp = Ytyp::from_bytes(&bytes).map_err(|error| {
            WorkspaceError::Build(format!(
                "{}: indexed provider YTYP parse failed: {error}",
                locator.provenance()
            ))
        })?;
        let internal_name = ytyp.name.or_else(|| Some(MetaHash(joaat(stem))));
        self.insert_scene_rpf_file(AssetKind::Ytyp, joaat(stem), locator.clone());
        if let Some(name) = internal_name {
            self.insert_scene_rpf_file(AssetKind::Ytyp, name.0, locator.clone());
        }

        let mut found = BTreeSet::new();
        for archetype in ytyp.archetypes {
            if !requested.contains(&archetype.name.0) {
                continue;
            }
            found.insert(archetype.name.0);
            self.scene_rpf_archetype_providers
                .entry(archetype.name.0)
                .or_default()
                .push(RpfArchetypeProvider {
                    locator: locator.clone(),
                    ytyp_name: internal_name,
                    archetype,
                });
        }

        let missing = requested.difference(&found).copied().collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(WorkspaceError::Build(format!(
                "{}: indexed provider is stale; requested archetype(s) not present: {}",
                locator.provenance(),
                missing
                    .iter()
                    .map(|hash| format!("0x{hash:08X}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        for providers in self.scene_rpf_archetype_providers.values_mut() {
            providers.sort_by(|left, right| left.locator.cmp(&right.locator));
            providers.dedup_by(|left, right| left.locator == right.locator);
        }

        Ok(found.len())
    }

    fn insert_scene_rpf_file(&mut self, kind: AssetKind, hash: u32, locator: RpfEntryLocator) {
        let locators = self.scene_rpf_files.entry((kind, hash)).or_default();
        if !locators.contains(&locator) {
            locators.push(locator);
        }
    }

    fn scene_candidates(&self, kind: AssetKind, hash: u32) -> Vec<SceneAssetLocator> {
        let local = self.candidates(kind, hash);
        if !local.is_empty() {
            return local
                .iter()
                .cloned()
                .map(SceneAssetLocator::Loose)
                .collect();
        }

        self.scene_rpf_files
            .get(&(kind, hash))
            .map(|locators| {
                locators
                    .iter()
                    .cloned()
                    .map(SceneAssetLocator::Rpf)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn candidates(&self, kind: AssetKind, hash: u32) -> &[PathBuf] {
        self.files
            .get(&(kind, hash))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Returns each supported workspace file once, even when it is indexed by
    /// both filename and an internal resource name hash.
    pub fn asset_files(&self) -> Vec<(PathBuf, AssetKind)> {
        let mut files = BTreeMap::<PathBuf, AssetKind>::new();
        for ((kind, _hash), paths) in &self.files {
            for path in paths {
                files.entry(path.clone()).or_insert(*kind);
            }
        }
        files.into_iter().collect()
    }

    /// Resolve one YMAP archetype to the renderable identity needed by scene
    /// assembly. Local workspace providers/assets always take precedence over
    /// mounted RPF sources. Ambiguous candidates within the selected tier fail
    /// closed instead of selecting by ordering or proximity.
    pub fn resolve_scene_archetype(
        &self,
        archetype_hash: u32,
    ) -> Result<SceneArchetypeResolution, SceneAssetLookupError> {
        let (provider, provider_ytyp_name, archetype) = if let Some(providers) =
            self.archetype_providers.get(&archetype_hash)
        {
            if providers.len() != 1 {
                return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::ProviderAmbiguous,
                        archetype_hash,
                        provider: None,
                        expected_kind: None,
                        hash: None,
                        message: format!(
                            "archetype 0x{archetype_hash:08X} has {} local YTYP providers; scene assembly requires exactly one",
                            providers.len()
                        ),
                    });
            }
            let provider = &providers[0];
            (
                SceneAssetLocator::Loose(provider.path.clone()),
                provider.ytyp_name,
                provider.archetype.clone(),
            )
        } else if let Some(providers) = self.scene_rpf_archetype_providers.get(&archetype_hash) {
            if providers.len() != 1 {
                return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::ProviderAmbiguous,
                        archetype_hash,
                        provider: None,
                        expected_kind: None,
                        hash: None,
                        message: format!(
                            "archetype 0x{archetype_hash:08X} has {} mounted RPF YTYP providers; scene assembly requires exactly one",
                            providers.len()
                        ),
                    });
            }
            let provider = &providers[0];
            (
                SceneAssetLocator::Rpf(provider.locator.clone()),
                provider.ytyp_name,
                provider.archetype.clone(),
            )
        } else {
            return Err(SceneAssetLookupError {
                    code: SceneAssetLookupErrorCode::ProviderMissing,
                    archetype_hash,
                    provider: None,
                    expected_kind: None,
                    hash: None,
                    message: format!(
                        "no workspace or mounted RPF YTYP provider defines archetype 0x{archetype_hash:08X}"
                    ),
                });
        };

        let asset = match archetype.asset_type {
            AssetType::Drawable => {
                let Some(asset_name) = archetype.asset_name else {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetNameMissing,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydr),
                        hash: None,
                        message: format!(
                            "YTYP provider for archetype 0x{archetype_hash:08X} declares Drawable without assetName"
                        ),
                    });
                };
                let candidates = self.scene_candidates(AssetKind::Ydr, asset_name.0);
                if candidates.is_empty() {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetMissing,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydr),
                        hash: Some(asset_name.0),
                        message: format!(
                            "drawable 0x{:08X} declared by archetype 0x{archetype_hash:08X} has no workspace or mounted RPF YDR candidate",
                            asset_name.0
                        ),
                    });
                }
                if candidates.len() != 1 {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetAmbiguous,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydr),
                        hash: Some(asset_name.0),
                        message: format!(
                            "drawable 0x{:08X} declared by archetype 0x{archetype_hash:08X} has {} candidates in the selected source tier",
                            asset_name.0,
                            candidates.len()
                        ),
                    });
                }
                SceneResolvedAsset::Drawable {
                    hash: asset_name.0,
                    locator: candidates[0].clone(),
                }
            }
            AssetType::DrawableDictionary => {
                let Some(entry_hash) = archetype.asset_name else {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetNameMissing,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydd),
                        hash: None,
                        message: format!(
                            "YTYP provider for archetype 0x{archetype_hash:08X} declares DrawableDictionary without assetName entry identity"
                        ),
                    });
                };
                let Some(dictionary_hash) = archetype.drawable_dictionary else {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::DrawableDictionaryMissing,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydd),
                        hash: Some(entry_hash.0),
                        message: format!(
                            "archetype 0x{archetype_hash:08X} declares DrawableDictionary but no drawableDictionary hash"
                        ),
                    });
                };
                let candidates = self.scene_candidates(AssetKind::Ydd, dictionary_hash.0);
                if candidates.is_empty() {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetMissing,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydd),
                        hash: Some(dictionary_hash.0),
                        message: format!(
                            "drawable dictionary 0x{:08X} declared by archetype 0x{archetype_hash:08X} has no workspace or mounted RPF YDD candidate",
                            dictionary_hash.0
                        ),
                    });
                }
                if candidates.len() != 1 {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetAmbiguous,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydd),
                        hash: Some(dictionary_hash.0),
                        message: format!(
                            "drawable dictionary 0x{:08X} declared by archetype 0x{archetype_hash:08X} has {} candidates in the selected source tier",
                            dictionary_hash.0,
                            candidates.len()
                        ),
                    });
                }
                let locator = candidates[0].clone();
                let bytes = locator
                    .read_bytes()
                    .map_err(|error| SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::DictionaryUnreadable,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydd),
                        hash: Some(dictionary_hash.0),
                        message: format!(
                            "drawable dictionary 0x{:08X} could not be read: {error}",
                            dictionary_hash.0
                        ),
                    })?;
                let dictionary =
                    YddDictionary::from_bytes(&bytes).map_err(|error| SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::DictionaryUnreadable,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydd),
                        hash: Some(dictionary_hash.0),
                        message: format!(
                            "drawable dictionary 0x{:08X} could not be parsed: {error}",
                            dictionary_hash.0
                        ),
                    })?;
                let Some(entry) = dictionary.entry_by_hash(entry_hash.0) else {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::DictionaryEntryMissing,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Ydd),
                        hash: Some(entry_hash.0),
                        message: format!(
                            "YDD 0x{:08X} does not contain drawable entry 0x{:08X} required by archetype 0x{archetype_hash:08X}",
                            dictionary_hash.0, entry_hash.0
                        ),
                    });
                };
                SceneResolvedAsset::DrawableDictionary {
                    dictionary_hash: dictionary_hash.0,
                    locator,
                    entry: SceneDrawableDictionaryEntry {
                        index: entry.index,
                        name_hash: entry.name_hash,
                        name: entry.name.clone(),
                    },
                }
            }
            AssetType::Fragment => {
                let Some(asset_name) = archetype.asset_name else {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetNameMissing,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Yft),
                        hash: None,
                        message: format!(
                            "YTYP provider for archetype 0x{archetype_hash:08X} declares Fragment without assetName"
                        ),
                    });
                };
                let candidates = self.scene_candidates(AssetKind::Yft, asset_name.0);
                if candidates.is_empty() {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetMissing,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Yft),
                        hash: Some(asset_name.0),
                        message: format!(
                            "fragment 0x{:08X} declared by archetype 0x{archetype_hash:08X} has no workspace or mounted RPF YFT candidate",
                            asset_name.0
                        ),
                    });
                }
                if candidates.len() != 1 {
                    return Err(SceneAssetLookupError {
                        code: SceneAssetLookupErrorCode::AssetAmbiguous,
                        archetype_hash,
                        provider: Some(provider.provenance()),
                        expected_kind: Some(AssetKind::Yft),
                        hash: Some(asset_name.0),
                        message: format!(
                            "fragment 0x{:08X} declared by archetype 0x{archetype_hash:08X} has {} candidates in the selected source tier",
                            asset_name.0,
                            candidates.len()
                        ),
                    });
                }
                SceneResolvedAsset::Fragment {
                    hash: asset_name.0,
                    locator: candidates[0].clone(),
                }
            }
            AssetType::Assetless => {
                return Err(SceneAssetLookupError {
                    code: SceneAssetLookupErrorCode::UnsupportedAssetType,
                    archetype_hash,
                    provider: Some(provider.provenance()),
                    expected_kind: None,
                    hash: None,
                    message: format!(
                        "archetype 0x{archetype_hash:08X} is assetless and has no drawable to instantiate"
                    ),
                });
            }
            AssetType::Uninitialized | AssetType::Unknown(_) => {
                return Err(SceneAssetLookupError {
                    code: SceneAssetLookupErrorCode::UnsupportedAssetType,
                    archetype_hash,
                    provider: Some(provider.provenance()),
                    expected_kind: None,
                    hash: archetype.asset_name.map(|hash| hash.0),
                    message: format!(
                        "archetype 0x{archetype_hash:08X} has an unsupported primary asset type for scene assembly"
                    ),
                });
            }
        };

        let texture_dictionary = match archetype.texture_dictionary {
            None => SceneTextureDictionaryLookup::None,
            Some(hash) => self.resolve_scene_texture_dictionary_chain(hash.0),
        };

        let collision = match archetype.physics_dictionary {
            None => SceneCollisionLookup::None,
            Some(hash) => {
                let candidates = self.scene_candidates(AssetKind::Ybn, hash.0);
                match candidates.as_slice() {
                    [] => SceneCollisionLookup::Unresolved {
                        hash: hash.0,
                        code: SceneCollisionLookupErrorCode::MissingAsset,
                        message: format!(
                            "YTYP declares physics dictionary 0x{:08X}, but no unique standalone YBN is available; no world placement is inferred",
                            hash.0
                        ),
                    },
                    [locator] => SceneCollisionLookup::LocalOnly {
                        hash: hash.0,
                        locator: locator.clone(),
                    },
                    _ => SceneCollisionLookup::Unresolved {
                        hash: hash.0,
                        code: SceneCollisionLookupErrorCode::AmbiguousAsset,
                        message: format!(
                            "YTYP declares physics dictionary 0x{:08X}, but {} candidates exist in the selected source tier; no world placement is inferred",
                            hash.0,
                            candidates.len()
                        ),
                    },
                }
            }
        };

        Ok(SceneArchetypeResolution {
            archetype_hash,
            provider,
            provider_ytyp_hash: provider_ytyp_name.map(|hash| hash.0),
            asset,
            texture_dictionary,
            collision,
        })
    }

    fn resolve_scene_texture_dictionary_chain(
        &self,
        requested_hash: u32,
    ) -> SceneTextureDictionaryLookup {
        let mut sources = Vec::new();
        let mut missing = Vec::new();
        let mut ambiguous = Vec::new();
        let mut visited = BTreeSet::new();
        let mut current = requested_hash;

        loop {
            if !visited.insert(current) {
                break;
            }

            let candidates = self.scene_candidates(AssetKind::Ytd, current);
            match candidates.as_slice() {
                [] => missing.push(current),
                [locator] => sources.push(SceneTextureDictionarySource {
                    hash: current,
                    locator: locator.clone(),
                }),
                _ => {
                    ambiguous.push(SceneTextureDictionaryAmbiguity {
                        hash: current,
                        candidates: candidates.len(),
                    });
                    break;
                }
            }

            let local_parent = self
                .texture_parents
                .get(&current)
                .and_then(|records| records.first())
                .map(|record| record.parent_hash);
            let game_parent = self
                .scene_rpf_texture_parents
                .get(&current)
                .and_then(|relationships| relationships.first())
                .map(|relationship| joaat(&relationship.parent));
            let Some(parent_hash) = local_parent.or(game_parent) else {
                break;
            };
            current = parent_hash;
        }

        SceneTextureDictionaryLookup::Chain {
            hash: requested_hash,
            sources,
            missing,
            ambiguous,
        }
    }

    /// Audits MLO archetypes directly referenced by a YMAP and returns a
    /// dependency report scoped to each interior.
    pub fn audit_map_mlos(
        &self,
        selected_ymap: impl AsRef<Path>,
    ) -> Result<Vec<MloArchetypeAudit>, WorkspaceError> {
        let selected_ymap = self.resolve_input_path(selected_ymap.as_ref());
        let bytes = fs::read(&selected_ymap).map_err(|source| WorkspaceError::Io {
            path: selected_ymap.clone(),
            source,
        })?;
        let ymap = Ymap::from_bytes(&bytes).map_err(|error| WorkspaceError::Parse {
            path: selected_ymap.clone(),
            message: error.to_string(),
        })?;

        let mut audits = Vec::new();
        let mut seen = BTreeSet::<(u32, PathBuf)>::new();
        for archetype_hash in ymap.entities.iter().map(|entity| entity.archetype_name.0) {
            let Some(providers) = self.archetype_providers.get(&archetype_hash) else {
                continue;
            };
            for provider in providers {
                if provider.archetype.kind != ArchetypeKind::Mlo
                    || !seen.insert((archetype_hash, provider.path.clone()))
                {
                    continue;
                }
                let mut report = DependencyReport {
                    warnings: self.warnings.clone(),
                    ..DependencyReport::default()
                };
                let mut visited = BTreeSet::new();
                self.resolve_archetype_inner(&mut report, archetype_hash, &mut visited);
                audits.push(MloArchetypeAudit {
                    archetype_hash,
                    ytyp_path: provider.path.clone(),
                    entity_count: provider.archetype.mlo_entity_count,
                    unique_entity_archetypes: provider.archetype.mlo_entity_archetypes.len(),
                    room_count: provider.archetype.mlo_room_count,
                    portal_count: provider.archetype.mlo_portal_count,
                    report,
                });
            }
        }
        audits.sort_by(|left, right| {
            left.archetype_hash
                .cmp(&right.archetype_hash)
                .then_with(|| left.ytyp_path.cmp(&right.ytyp_path))
        });
        Ok(audits)
    }

    pub fn resolve_map(
        &self,
        selected_ymap: impl AsRef<Path>,
    ) -> Result<DependencyReport, WorkspaceError> {
        let selected_input = self.resolve_input_path(selected_ymap.as_ref());
        let selected_bytes = fs::read(&selected_input).map_err(|source| WorkspaceError::Io {
            path: selected_input.clone(),
            source,
        })?;
        let selected =
            Ymap::from_bytes(&selected_bytes).map_err(|error| WorkspaceError::Parse {
                path: selected_input.clone(),
                message: error.to_string(),
            })?;

        let mut report = DependencyReport {
            warnings: self.warnings.clone(),
            ..DependencyReport::default()
        };
        let selected_hash = selected
            .name
            .map(|hash| hash.0)
            .or_else(|| stem_hash(&selected_input));
        let selected_ymap = selected_hash
            .and_then(|hash| self.indexed_equivalent_path(AssetKind::Ymap, hash, &selected_input))
            .unwrap_or(selected_input);
        report.add_file(
            selected_ymap.clone(),
            AssetKind::Ymap,
            selected_hash,
            DependencyReason::SelectedYmap,
        );

        let mut visited_maps = BTreeSet::new();
        self.resolve_ymap_contents(
            &selected_ymap,
            &selected,
            &mut report,
            &mut visited_maps,
            true,
        )?;

        Ok(report)
    }

    /// Resolves multiple selected YMAPs into one dependency closure.
    ///
    /// Files, dependency reasons, unresolved entries and GTXD relationships are
    /// merged deterministically so callers can export a single FiveM resource
    /// without duplicating dependencies shared by multiple maps.
    pub fn resolve_maps(
        &self,
        selected_ymaps: &[PathBuf],
    ) -> Result<DependencyReport, WorkspaceError> {
        if selected_ymaps.is_empty() {
            return Err(WorkspaceError::Build(
                "combined export requires at least one YMAP".into(),
            ));
        }

        let mut combined = DependencyReport::default();
        for selected in selected_ymaps {
            let report = self.resolve_map(selected)?;
            for (path, file) in report.files {
                let entry = combined
                    .files
                    .entry(path.clone())
                    .or_insert_with(|| ResolvedFile {
                        path,
                        kind: file.kind,
                        hashes: BTreeSet::new(),
                        reasons: BTreeSet::new(),
                    });
                if entry.kind != file.kind {
                    return Err(WorkspaceError::Build(format!(
                        "dependency {} resolved as both {} and {}",
                        entry.path.display(),
                        entry.kind,
                        file.kind
                    )));
                }
                entry.hashes.extend(file.hashes);
                entry.reasons.extend(file.reasons);
            }
            for unresolved in report.unresolved {
                if !combined.unresolved.contains(&unresolved) {
                    combined.unresolved.push(unresolved);
                }
            }
            for warning in report.warnings {
                if !combined.warnings.contains(&warning) {
                    combined.warnings.push(warning);
                }
            }
            combined.texture_parents.extend(report.texture_parents);
        }
        combined.unresolved.sort_by(|left, right| {
            left.hash
                .cmp(&right.hash)
                .then_with(|| left.kind.to_string().cmp(&right.kind.to_string()))
                .then_with(|| left.reason.cmp(&right.reason))
        });
        combined.warnings.sort();
        Ok(combined)
    }

    /// Builds the PSO pack manifest corresponding to a resolved map closure.
    ///
    /// This recomputes map->YTYP relationships from parsed YMAP archetypes so
    /// the generated manifest describes the exported files rather than the
    /// source pack's original `_manifest.ymf`.
    pub fn build_ymf_for_report(&self, report: &DependencyReport) -> Result<Ymf, WorkspaceError> {
        let mut manifest = Ymf::default();
        let mut used_ytyps = BTreeSet::<u32>::new();
        let mut used_mlo_archetypes = BTreeSet::<u32>::new();

        for file in report
            .files
            .values()
            .filter(|file| file.kind == AssetKind::Ymap)
        {
            let bytes = fs::read(&file.path).map_err(|source| WorkspaceError::Io {
                path: file.path.clone(),
                source,
            })?;
            let ymap = Ymap::from_bytes(&bytes).map_err(|error| WorkspaceError::Parse {
                path: file.path.clone(),
                message: error.to_string(),
            })?;
            let ymap_name = ymap
                .name
                .or_else(|| stem_hash(&file.path).map(MetaHash))
                .ok_or_else(|| {
                    WorkspaceError::Build(format!(
                        "cannot determine YMAP name for {}",
                        file.path.display()
                    ))
                })?;

            let mut direct = BTreeSet::<u32>::new();
            let mut interior = false;
            for archetype_hash in ymap.entities.iter().map(|entity| entity.archetype_name.0) {
                let Some(providers) = self.archetype_providers.get(&archetype_hash) else {
                    continue;
                };
                for provider in providers {
                    if let Some(name) = provider.ytyp_name {
                        direct.insert(name.0);
                        used_ytyps.insert(name.0);
                    }
                    if provider.archetype.kind == ArchetypeKind::Mlo {
                        interior = true;
                        used_mlo_archetypes.insert(archetype_hash);
                    }
                }
            }

            if let Some(source) = self.source_map_dependencies.get(&ymap_name.0) {
                direct.extend(source.dependencies.iter().copied());
                used_ytyps.extend(source.dependencies.iter().copied());
                if source.flags.contains(ManifestFlags::INTERIOR_DATA) {
                    interior = true;
                }
            }

            manifest.maps.push(YmfMapDependency {
                ymap: ymap_name,
                ytyps: direct.into_iter().map(MetaHash).collect(),
                flags: if interior {
                    ManifestFlags::INTERIOR_DATA
                } else {
                    ManifestFlags::NONE
                },
            });
        }

        // Follow YTYP dependency chains transitively. This is independent from
        // the file-copy report so manifest closure remains explicit.
        let mut queued: Vec<u32> = used_ytyps.iter().copied().collect();
        let mut cursor = 0_usize;
        let mut emitted = BTreeSet::<u32>::new();
        while cursor < queued.len() {
            let ytyp_hash = queued[cursor];
            cursor += 1;
            if !emitted.insert(ytyp_hash) {
                continue;
            }
            let records = self.ytyp_records.get(&ytyp_hash);
            let source = self.source_ytyp_dependencies.get(&ytyp_hash);
            if records.is_none() && source.is_none() {
                continue;
            }

            let mut dependencies = BTreeSet::<u32>::new();
            let mut flags = source
                .map(|record| record.flags)
                .unwrap_or(ManifestFlags::NONE);
            if let Some(records) = records {
                for record in records {
                    dependencies.extend(record.dependencies.iter().map(|hash| hash.0));
                    if record.contains_mlo {
                        flags.0 |= ManifestFlags::INTERIOR_DATA.0;
                    }
                }
            }
            if let Some(source) = source {
                dependencies.extend(source.dependencies.iter().copied());
            }

            for dependency in &dependencies {
                if used_ytyps.insert(*dependency) {
                    queued.push(*dependency);
                }
            }

            if !dependencies.is_empty() || flags != ManifestFlags::NONE {
                manifest.ytyps.push(YmfYtypDependency {
                    ytyp: MetaHash(ytyp_hash),
                    ytyps: dependencies.into_iter().map(MetaHash).collect(),
                    flags,
                });
            }
        }

        // Preserve explicit source-manifest interior bounds when available.
        // Otherwise fall back to the observed legacy convention where the MLO
        // archetype and its streamed YBN share a hash.
        for archetype_hash in used_mlo_archetypes {
            let bounds = self.interior_bounds_for_archetype(archetype_hash);
            if !bounds.is_empty() {
                manifest.interiors.push(YmfInteriorBounds {
                    name: MetaHash(archetype_hash),
                    bounds: bounds.into_iter().map(MetaHash).collect(),
                });
            }
        }

        manifest.normalize();
        Ok(manifest)
    }

    pub fn export_map(
        &self,
        selected_ymap: impl AsRef<Path>,
        output_dir: impl AsRef<Path>,
        options: ExportOptions,
    ) -> Result<ExportResult, WorkspaceError> {
        let mut report = self.resolve_map(selected_ymap)?;
        if !options.allow_unresolved && !report.unresolved.is_empty() {
            return Err(WorkspaceError::Build(format!(
                "{} unresolved dependencies remain; review with `deps` or pass allow_unresolved",
                report.unresolved.len()
            )));
        }

        let manifest = self.build_ymf_for_report(&report)?;
        if manifest.interiors.is_empty() {
            for map in &manifest.maps {
                if map.flags.contains(ManifestFlags::INTERIOR_DATA) {
                    report.warnings.push(format!(
                        "YMAP 0x{:08X} contains an MLO archetype but no same-name \
                         interior YBN was inferred",
                        map.ymap.0
                    ));
                }
            }
        }

        let output_dir = output_dir.as_ref().to_path_buf();
        if output_dir == self.root {
            return Err(WorkspaceError::Build(
                "refusing to export over the workspace root".into(),
            ));
        }
        if output_dir.exists() {
            if !options.overwrite {
                return Err(WorkspaceError::Build(format!(
                    "output already exists: {} (use overwrite)",
                    output_dir.display()
                )));
            }
            fs::remove_dir_all(&output_dir).map_err(|source| WorkspaceError::Io {
                path: output_dir.clone(),
                source,
            })?;
        }
        let stream_dir = output_dir.join("stream");
        fs::create_dir_all(&stream_dir).map_err(|source| WorkspaceError::Io {
            path: stream_dir.clone(),
            source,
        })?;

        let mut copied_files = Vec::new();
        let mut destinations = BTreeMap::<String, PathBuf>::new();
        for file in report.files.values() {
            let filename = file.path.file_name().ok_or_else(|| {
                WorkspaceError::Build(format!(
                    "dependency has no filename: {}",
                    file.path.display()
                ))
            })?;
            let key = filename.to_string_lossy().to_ascii_lowercase();
            if let Some(existing) = destinations.get(&key) {
                if existing != &file.path {
                    return Err(WorkspaceError::Build(format!(
                        "cannot flatten two different dependencies named {}: {} and {}",
                        filename.to_string_lossy(),
                        existing.display(),
                        file.path.display()
                    )));
                }
                continue;
            }
            destinations.insert(key, file.path.clone());
            let destination = stream_dir.join(filename);
            fs::copy(&file.path, &destination).map_err(|source| WorkspaceError::Io {
                path: destination.clone(),
                source,
            })?;
            copied_files.push(destination);
        }

        let manifest_path = stream_dir.join("_manifest.ymf");
        let manifest_bytes = manifest
            .to_bytes()
            .map_err(|error| WorkspaceError::Build(format!("YMF encode failed: {error}")))?;
        fs::write(&manifest_path, manifest_bytes).map_err(|source| WorkspaceError::Io {
            path: manifest_path.clone(),
            source,
        })?;

        let gtxd_path = if report.texture_parents.is_empty() {
            None
        } else {
            let data_dir = output_dir.join("data");
            fs::create_dir_all(&data_dir).map_err(|source| WorkspaceError::Io {
                path: data_dir.clone(),
                source,
            })?;
            let path = data_dir.join("gtxd.meta");
            fs::write(&path, render_gtxd_xml(&report.texture_parents)).map_err(|source| {
                WorkspaceError::Io {
                    path: path.clone(),
                    source,
                }
            })?;
            Some(path)
        };

        let mut fxmanifest_contents =
            String::from("fx_version 'cerulean'\ngame 'gta5'\n\nthis_is_a_map 'yes'\n");
        if gtxd_path.is_some() {
            fxmanifest_contents.push_str(
                "\nfiles {\n    'data/gtxd.meta'\n}\n\ndata_file 'GTXD_PARENTING_DATA' 'data/gtxd.meta'\n",
            );
        }
        let fxmanifest = output_dir.join("fxmanifest.lua");
        fs::write(&fxmanifest, fxmanifest_contents).map_err(|source| WorkspaceError::Io {
            path: fxmanifest,
            source,
        })?;

        Ok(ExportResult {
            output_dir,
            stream_dir,
            copied_files,
            manifest_path,
            gtxd_path,
            report,
            manifest,
        })
    }

    pub fn export_maps(
        &self,
        selected_ymaps: &[PathBuf],
        output_dir: impl AsRef<Path>,
        options: ExportOptions,
    ) -> Result<ExportResult, WorkspaceError> {
        let mut report = self.resolve_maps(selected_ymaps)?;
        if !options.allow_unresolved && !report.unresolved.is_empty() {
            return Err(WorkspaceError::Build(format!(
                "{} unresolved dependencies remain; review with `deps` or pass allow_unresolved",
                report.unresolved.len()
            )));
        }

        let manifest = self.build_ymf_for_report(&report)?;
        if manifest.interiors.is_empty() {
            for map in &manifest.maps {
                if map.flags.contains(ManifestFlags::INTERIOR_DATA) {
                    report.warnings.push(format!(
                        "YMAP 0x{:08X} contains an MLO archetype but no same-name \
                         interior YBN was inferred",
                        map.ymap.0
                    ));
                }
            }
        }

        let output_dir = output_dir.as_ref().to_path_buf();
        if output_dir == self.root {
            return Err(WorkspaceError::Build(
                "refusing to export over the workspace root".into(),
            ));
        }
        if output_dir.exists() {
            if !options.overwrite {
                return Err(WorkspaceError::Build(format!(
                    "output already exists: {} (use overwrite)",
                    output_dir.display()
                )));
            }
            fs::remove_dir_all(&output_dir).map_err(|source| WorkspaceError::Io {
                path: output_dir.clone(),
                source,
            })?;
        }
        let stream_dir = output_dir.join("stream");
        fs::create_dir_all(&stream_dir).map_err(|source| WorkspaceError::Io {
            path: stream_dir.clone(),
            source,
        })?;

        let mut copied_files = Vec::new();
        let mut destinations = BTreeMap::<String, PathBuf>::new();
        for file in report.files.values() {
            let filename = file.path.file_name().ok_or_else(|| {
                WorkspaceError::Build(format!(
                    "dependency has no filename: {}",
                    file.path.display()
                ))
            })?;
            let key = filename.to_string_lossy().to_ascii_lowercase();
            if let Some(existing) = destinations.get(&key) {
                if existing != &file.path {
                    return Err(WorkspaceError::Build(format!(
                        "cannot flatten two different dependencies named {}: {} and {}",
                        filename.to_string_lossy(),
                        existing.display(),
                        file.path.display()
                    )));
                }
                continue;
            }
            destinations.insert(key, file.path.clone());
            let destination = stream_dir.join(filename);
            fs::copy(&file.path, &destination).map_err(|source| WorkspaceError::Io {
                path: destination.clone(),
                source,
            })?;
            copied_files.push(destination);
        }

        let manifest_path = stream_dir.join("_manifest.ymf");
        let manifest_bytes = manifest
            .to_bytes()
            .map_err(|error| WorkspaceError::Build(format!("YMF encode failed: {error}")))?;
        fs::write(&manifest_path, manifest_bytes).map_err(|source| WorkspaceError::Io {
            path: manifest_path.clone(),
            source,
        })?;

        let gtxd_path = if report.texture_parents.is_empty() {
            None
        } else {
            let data_dir = output_dir.join("data");
            fs::create_dir_all(&data_dir).map_err(|source| WorkspaceError::Io {
                path: data_dir.clone(),
                source,
            })?;
            let path = data_dir.join("gtxd.meta");
            fs::write(&path, render_gtxd_xml(&report.texture_parents)).map_err(|source| {
                WorkspaceError::Io {
                    path: path.clone(),
                    source,
                }
            })?;
            Some(path)
        };

        let mut fxmanifest_contents =
            String::from("fx_version 'cerulean'\ngame 'gta5'\n\nthis_is_a_map 'yes'\n");
        if gtxd_path.is_some() {
            fxmanifest_contents.push_str(
                "\nfiles {\n    'data/gtxd.meta'\n}\n\ndata_file 'GTXD_PARENTING_DATA' 'data/gtxd.meta'\n",
            );
        }
        let fxmanifest = output_dir.join("fxmanifest.lua");
        fs::write(&fxmanifest, fxmanifest_contents).map_err(|source| WorkspaceError::Io {
            path: fxmanifest,
            source,
        })?;

        Ok(ExportResult {
            output_dir,
            stream_dir,
            copied_files,
            manifest_path,
            gtxd_path,
            report,
            manifest,
        })
    }

    fn resolve_input_path(&self, input: &Path) -> PathBuf {
        if input.is_absolute() || input.exists() {
            input.to_path_buf()
        } else {
            self.root.join(input)
        }
    }

    fn indexed_equivalent_path(&self, kind: AssetKind, hash: u32, path: &Path) -> Option<PathBuf> {
        let canonical = fs::canonicalize(path).ok()?;
        self.candidates(kind, hash).iter().find_map(|candidate| {
            fs::canonicalize(candidate)
                .ok()
                .filter(|indexed| indexed == &canonical)
                .map(|_| candidate.clone())
        })
    }

    fn resolve_ymap_contents(
        &self,
        path: &Path,
        ymap: &Ymap,
        report: &mut DependencyReport,
        visited_maps: &mut BTreeSet<u32>,
        include_children: bool,
    ) -> Result<(), WorkspaceError> {
        let map_hash = ymap.name.map(|hash| hash.0).or_else(|| stem_hash(path));
        if let Some(hash) = map_hash {
            if !visited_maps.insert(hash) {
                return Ok(());
            }
        }

        if let Some(parent) = ymap.parent {
            let reason = DependencyReason::ParentYmap {
                child_hash: map_hash.unwrap_or(0),
            };
            let candidates = self.candidates(AssetKind::Ymap, parent.0);
            if candidates.is_empty() {
                report.add_unresolved(UnresolvedKind::File(AssetKind::Ymap), parent.0, reason);
            } else {
                if candidates.len() > 1 {
                    report.warnings.push(format!(
                        "YMAP parent hash 0x{:08X} has {} local candidates",
                        parent.0,
                        candidates.len()
                    ));
                }
                for candidate in candidates {
                    report.add_file(
                        candidate.clone(),
                        AssetKind::Ymap,
                        Some(parent.0),
                        reason.clone(),
                    );
                }

                // One parent hash should identify one logical map. Recurse via
                // the first candidate but keep all duplicate candidates visible.
                let parent_path = &candidates[0];
                let bytes = fs::read(parent_path).map_err(|source| WorkspaceError::Io {
                    path: parent_path.clone(),
                    source,
                })?;
                let parent_map =
                    Ymap::from_bytes(&bytes).map_err(|error| WorkspaceError::Parse {
                        path: parent_path.clone(),
                        message: error.to_string(),
                    })?;
                self.resolve_ymap_contents(parent_path, &parent_map, report, visited_maps, false)?;
            }
        }

        if include_children {
            self.resolve_ymap_children(map_hash, report, visited_maps)?;
        }

        for dictionary in &ymap.physics_dictionaries {
            self.resolve_file_hash(
                report,
                AssetKind::Ybn,
                dictionary.0,
                DependencyReason::YmapPhysicsDictionary {
                    ymap_hash: map_hash,
                },
            );
        }

        let archetypes: BTreeSet<u32> = ymap
            .entities
            .iter()
            .map(|entity| entity.archetype_name.0)
            .collect();
        for archetype_hash in archetypes {
            self.resolve_archetype(report, archetype_hash);
        }

        Ok(())
    }

    fn resolve_ymap_children(
        &self,
        parent_hash: Option<u32>,
        report: &mut DependencyReport,
        visited_maps: &mut BTreeSet<u32>,
    ) -> Result<(), WorkspaceError> {
        let Some(parent_hash) = parent_hash else {
            return Ok(());
        };
        let Some(children) = self.ymap_children.get(&parent_hash) else {
            return Ok(());
        };

        for (child_hash, child_paths) in children {
            let reason = DependencyReason::ChildYmap { parent_hash };
            if child_paths.len() > 1 {
                report.warnings.push(format!(
                    "YMAP child hash 0x{child_hash:08X} of 0x{parent_hash:08X} has {} local candidates",
                    child_paths.len()
                ));
            }
            for child_path in child_paths {
                report.add_file(
                    child_path.clone(),
                    AssetKind::Ymap,
                    Some(*child_hash),
                    reason.clone(),
                );
            }

            // One child hash should identify one logical map. Keep all duplicate
            // candidates visible, but recurse deterministically through the first
            // path. Descendants remain eligible for reverse-parent closure while
            // ancestors do not expand their siblings.
            let Some(child_path) = child_paths.first() else {
                continue;
            };
            let bytes = fs::read(child_path).map_err(|source| WorkspaceError::Io {
                path: child_path.clone(),
                source,
            })?;
            let child_map = Ymap::from_bytes(&bytes).map_err(|error| WorkspaceError::Parse {
                path: child_path.clone(),
                message: error.to_string(),
            })?;
            self.resolve_ymap_contents(child_path, &child_map, report, visited_maps, true)?;
        }

        Ok(())
    }

    fn interior_bounds_for_archetype(&self, archetype_hash: u32) -> BTreeSet<u32> {
        if let Some(bounds) = self
            .source_interior_bounds
            .get(&archetype_hash)
            .filter(|bounds| !bounds.is_empty())
        {
            return bounds.clone();
        }

        if self.candidates(AssetKind::Ybn, archetype_hash).is_empty() {
            BTreeSet::new()
        } else {
            BTreeSet::from([archetype_hash])
        }
    }

    fn resolve_archetype(&self, report: &mut DependencyReport, archetype_hash: u32) {
        let mut visited = BTreeSet::new();
        self.resolve_archetype_inner(report, archetype_hash, &mut visited);
    }

    fn resolve_archetype_inner(
        &self,
        report: &mut DependencyReport,
        archetype_hash: u32,
        visited: &mut BTreeSet<u32>,
    ) {
        if !visited.insert(archetype_hash) {
            return;
        }

        let Some(providers) = self.archetype_providers.get(&archetype_hash) else {
            report.add_unresolved(
                UnresolvedKind::ArchetypeProvider,
                archetype_hash,
                DependencyReason::ArchetypeProvider {
                    archetype: archetype_hash,
                },
            );
            return;
        };

        if providers.len() > 1 {
            report.warnings.push(format!(
                "archetype 0x{archetype_hash:08X} has {} local YTYP providers",
                providers.len()
            ));
        }

        for provider in providers {
            report.add_file(
                provider.path.clone(),
                AssetKind::Ytyp,
                provider.ytyp_name.map(|hash| hash.0),
                DependencyReason::ArchetypeProvider {
                    archetype: archetype_hash,
                },
            );

            let mut visited_ytyps = BTreeSet::new();
            for dependency in &provider.ytyp_dependencies {
                self.resolve_ytyp_dependency(
                    report,
                    dependency.0,
                    provider.ytyp_name.map(|hash| hash.0),
                    &mut visited_ytyps,
                );
            }

            if provider.archetype.kind == ArchetypeKind::Mlo {
                let archetype_hash = provider.archetype.name.0;
                let reason = DependencyReason::MloInteriorBounds {
                    archetype: archetype_hash,
                };
                for bound_hash in self.interior_bounds_for_archetype(archetype_hash) {
                    let candidates = self.candidates(AssetKind::Ybn, bound_hash);
                    if candidates.is_empty() {
                        report.add_unresolved(
                            UnresolvedKind::File(AssetKind::Ybn),
                            bound_hash,
                            reason.clone(),
                        );
                        continue;
                    }
                    for candidate in candidates {
                        report.add_file(
                            candidate.clone(),
                            AssetKind::Ybn,
                            Some(bound_hash),
                            reason.clone(),
                        );
                    }
                }
            }

            self.resolve_archetype_assets(report, &provider.archetype);

            if provider.archetype.kind == ArchetypeKind::Mlo {
                for entity_archetype in &provider.archetype.mlo_entity_archetypes {
                    self.resolve_archetype_inner(report, entity_archetype.0, visited);
                }
            }
        }
    }

    fn resolve_ytyp_dependency(
        &self,
        report: &mut DependencyReport,
        hash: u32,
        declared_by: Option<u32>,
        visited: &mut BTreeSet<u32>,
    ) {
        if !visited.insert(hash) {
            return;
        }
        self.resolve_file_hash(
            report,
            AssetKind::Ytyp,
            hash,
            DependencyReason::YtypDependency {
                ytyp_hash: declared_by,
            },
        );

        let Some(records) = self.ytyp_records.get(&hash) else {
            return;
        };
        if records.len() > 1 {
            report.warnings.push(format!(
                "YTYP hash 0x{hash:08X} has {} parsed candidates while closing dependencies",
                records.len()
            ));
        }
        for record in records {
            for dependency in &record.dependencies {
                self.resolve_ytyp_dependency(report, dependency.0, Some(record.name.0), visited);
            }
        }
    }

    fn resolve_texture_dictionary(
        &self,
        report: &mut DependencyReport,
        hash: u32,
        reason: DependencyReason,
    ) {
        let mut visited = BTreeSet::new();
        self.resolve_texture_dictionary_inner(report, hash, reason, &mut visited);
    }

    fn resolve_texture_dictionary_inner(
        &self,
        report: &mut DependencyReport,
        hash: u32,
        reason: DependencyReason,
        visited: &mut BTreeSet<u32>,
    ) {
        if !visited.insert(hash) {
            report.warnings.push(format!(
                "GTXD parent cycle detected at YTD hash 0x{hash:08X}"
            ));
            return;
        }

        self.resolve_file_hash(report, AssetKind::Ytd, hash, reason);

        let Some(records) = self.texture_parents.get(&hash) else {
            return;
        };
        let Some(record) = records.first() else {
            return;
        };

        report.texture_parents.insert(TextureParentRelationship {
            parent: record.parent.clone(),
            child: record.child.clone(),
        });
        self.resolve_texture_dictionary_inner(
            report,
            record.parent_hash,
            DependencyReason::ParentTextureDictionary { child_hash: hash },
            visited,
        );
    }

    fn resolve_archetype_assets(&self, report: &mut DependencyReport, archetype: &Archetype) {
        if let Some(asset_name) = archetype.asset_name {
            match archetype.asset_type {
                AssetType::Fragment => self.resolve_file_hash(
                    report,
                    AssetKind::Yft,
                    asset_name.0,
                    DependencyReason::PrimaryAsset {
                        archetype: archetype.name.0,
                    },
                ),
                AssetType::Drawable => self.resolve_file_hash(
                    report,
                    AssetKind::Ydr,
                    asset_name.0,
                    DependencyReason::PrimaryAsset {
                        archetype: archetype.name.0,
                    },
                ),
                AssetType::DrawableDictionary => self.resolve_file_hash(
                    report,
                    AssetKind::Ydd,
                    asset_name.0,
                    DependencyReason::PrimaryAsset {
                        archetype: archetype.name.0,
                    },
                ),
                AssetType::Assetless => {}
                AssetType::Uninitialized | AssetType::Unknown(_) => {
                    let reason = DependencyReason::PrimaryAsset {
                        archetype: archetype.name.0,
                    };
                    let mut found = false;
                    for kind in [AssetKind::Ydr, AssetKind::Ydd, AssetKind::Yft] {
                        let candidates = self.candidates(kind, asset_name.0);
                        for path in candidates {
                            found = true;
                            report.add_file(path.clone(), kind, Some(asset_name.0), reason.clone());
                        }
                    }
                    if !found {
                        report.add_unresolved(
                            UnresolvedKind::PrimaryAssetUnknownType,
                            asset_name.0,
                            reason,
                        );
                    }
                }
            }
        }

        if let Some(hash) = archetype.texture_dictionary {
            self.resolve_texture_dictionary(
                report,
                hash.0,
                DependencyReason::TextureDictionary {
                    archetype: archetype.name.0,
                },
            );
        }
        if let Some(hash) = archetype.physics_dictionary {
            let reason = DependencyReason::PhysicsDictionary {
                archetype: archetype.name.0,
            };
            let candidates = self.candidates(AssetKind::Ybn, hash.0);
            if !candidates.is_empty() {
                for path in candidates {
                    report.add_file(path.clone(), AssetKind::Ybn, Some(hash.0), reason.clone());
                }
            } else if !(archetype.asset_type == AssetType::Drawable
                && archetype.asset_name == Some(hash))
            {
                // Drawable archetypes commonly use their own asset hash as
                // `physicsDictionary` while carrying collision in the YDR.
                // A same-name standalone YBN, when present, is still copied;
                // its absence alone is not a missing streamed dependency.
                report.add_unresolved(UnresolvedKind::File(AssetKind::Ybn), hash.0, reason);
            }
        }
        if let Some(hash) = archetype.clip_dictionary {
            self.resolve_file_hash(
                report,
                AssetKind::Ycd,
                hash.0,
                DependencyReason::ClipDictionary {
                    archetype: archetype.name.0,
                },
            );
        }
        if let Some(hash) = archetype.drawable_dictionary {
            self.resolve_file_hash(
                report,
                AssetKind::Ydd,
                hash.0,
                DependencyReason::DrawableDictionary {
                    archetype: archetype.name.0,
                },
            );
        }
    }

    fn resolve_file_hash(
        &self,
        report: &mut DependencyReport,
        kind: AssetKind,
        hash: u32,
        reason: DependencyReason,
    ) {
        let candidates = self.candidates(kind, hash);
        if candidates.is_empty() {
            report.add_unresolved(UnresolvedKind::File(kind), hash, reason);
            return;
        }

        if candidates.len() > 1 {
            report.warnings.push(format!(
                "{kind} hash 0x{hash:08X} has {} local candidates",
                candidates.len()
            ));
        }
        for path in candidates {
            report.add_file(path.clone(), kind, Some(hash), reason.clone());
        }
    }

    fn insert_file(&mut self, kind: AssetKind, hash: u32, path: PathBuf) {
        let paths = self.files.entry((kind, hash)).or_default();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    fn insert_texture_parent(&mut self, relationship: TextureParentRelationship, source: PathBuf) {
        let child_hash = joaat(&relationship.child);
        let parent_hash = joaat(&relationship.parent);

        let existing = self.texture_parents.get(&child_hash);
        if existing.is_some_and(|records| {
            records.iter().any(|record| {
                record.parent_hash == parent_hash
                    && record.parent.eq_ignore_ascii_case(&relationship.parent)
                    && record.child.eq_ignore_ascii_case(&relationship.child)
            })
        }) {
            return;
        }

        if let Some(records) = existing.filter(|records| !records.is_empty()) {
            let existing = records
                .iter()
                .map(|record| format!("{} ({})", record.parent, record.source.display()))
                .collect::<Vec<_>>()
                .join(", ");
            self.warnings.push(format!(
                "GTXD child {} (0x{child_hash:08X}) has multiple parents; keeping deterministic source order: {existing}, {} ({})",
                relationship.child,
                relationship.parent,
                source.display()
            ));
        }

        self.texture_parents
            .entry(child_hash)
            .or_default()
            .push(TextureParentRecord {
                parent: relationship.parent,
                parent_hash,
                child: relationship.child,
                source,
            });
    }
}

#[cfg(test)]
mod workspace_index_tests {
    use std::{
        fs, process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("ragelab-assets-{label}-{}-{nonce}", process::id()))
    }

    #[test]
    fn source_manifest_interior_bounds_override_same_hash_fallback() {
        let root = unique_temp_dir("source-interior-bounds");
        fs::create_dir_all(&root).expect("temp workspace should be created");

        let archetype_hash = joaat("test_mlo");
        let first_bound = joaat("custom_bound_a");
        let second_bound = joaat("custom_bound_b");
        let manifest = Ymf {
            interiors: vec![YmfInteriorBounds {
                name: MetaHash(archetype_hash),
                bounds: vec![MetaHash(first_bound), MetaHash(second_bound)],
            }],
            ..Ymf::default()
        };
        fs::write(
            root.join("_manifest.ymf"),
            manifest
                .to_bytes()
                .expect("synthetic manifest should encode"),
        )
        .expect("manifest should be written");
        fs::write(root.join("test_mlo.ybn"), b"").expect("same-hash fallback should exist");
        fs::write(root.join("custom_bound_a.ybn"), b"").expect("first source bound should exist");
        fs::write(root.join("custom_bound_b.ybn"), b"").expect("second source bound should exist");

        let index = WorkspaceIndex::scan(&root).expect("workspace should index");
        assert_eq!(
            index.interior_bounds_for_archetype(archetype_hash),
            BTreeSet::from([first_bound, second_bound])
        );

        fs::remove_dir_all(&root).expect("temp workspace should clean up");
    }

    #[test]
    fn same_hash_interior_bound_remains_the_manifest_free_fallback() {
        let root = unique_temp_dir("fallback-interior-bounds");
        fs::create_dir_all(&root).expect("temp workspace should be created");

        let archetype_hash = joaat("test_mlo");
        fs::write(root.join("test_mlo.ybn"), b"").expect("same-hash fallback should exist");

        let index = WorkspaceIndex::scan(&root).expect("workspace should index");
        assert_eq!(
            index.interior_bounds_for_archetype(archetype_hash),
            BTreeSet::from([archetype_hash])
        );

        fs::remove_dir_all(&root).expect("temp workspace should clean up");
    }
}

pub fn parse_gtxd_rbf(bytes: &[u8]) -> Result<Vec<TextureParentRelationship>, String> {
    let file = RbfFile::from_bytes(bytes).map_err(|error| error.to_string())?;
    if file.root.name != "CMapParentTxds" {
        return Err(format!(
            "unexpected root {}; expected CMapParentTxds",
            file.root.name
        ));
    }

    let relationship_blocks: Vec<&RbfStructure> =
        file.root.child_structures("txdRelationships").collect();
    if relationship_blocks.is_empty() {
        return Err("missing txdRelationships structure".into());
    }

    let mut relationships = Vec::new();
    for block in relationship_blocks {
        for item in block.child_structures("item") {
            relationships.push(TextureParentRelationship {
                parent: parse_gtxd_rbf_name(item, "parent")?,
                child: parse_gtxd_rbf_name(item, "child")?,
            });
        }
    }
    Ok(relationships)
}

fn parse_gtxd_rbf_name(item: &RbfStructure, field: &str) -> Result<String, String> {
    let mut fields = item.child_structures(field);
    let structure = fields
        .next()
        .ok_or_else(|| format!("GTXD item is missing {field} structure"))?;
    if fields.next().is_some() {
        return Err(format!("GTXD item has multiple {field} structures"));
    }
    if !structure.attributes.is_empty() || structure.children.len() != 1 {
        return Err(format!(
            "GTXD {field} must contain exactly one byte blob and no attributes"
        ));
    }
    let raw = match &structure.children[0] {
        RbfValue::Bytes(bytes) => bytes,
        _ => return Err(format!("GTXD {field} payload is not a byte blob")),
    };

    let mut value = String::with_capacity(raw.len());
    for (index, byte) in raw.iter().copied().enumerate() {
        if byte == 0 {
            continue;
        }
        if !byte.is_ascii() {
            return Err(format!(
                "GTXD {field} contains non-ASCII byte 0x{byte:02X} at index {index}"
            ));
        }
        value.push(char::from(byte));
    }
    if value.is_empty() {
        return Err(format!("GTXD {field} name must not be empty"));
    }
    Ok(value)
}

fn parse_gtxd_xml(xml: &str) -> Result<Vec<TextureParentRelationship>, String> {
    let relationships_body = xml_element_body_ci(xml, "txdRelationships")
        .ok_or_else(|| "missing <txdRelationships> element".to_string())?;

    let mut relationships = Vec::new();
    let mut remaining = relationships_body;
    loop {
        if find_xml_open_tag_ci(remaining, "item").is_none() {
            break;
        }
        let (item, rest) = take_xml_element_ci(remaining, "item")
            .ok_or_else(|| "unterminated GTXD <Item> element".to_string())?;
        let parent = xml_element_body_ci(item, "parent")
            .map(decode_xml_text)
            .ok_or_else(|| "GTXD item is missing <parent>".to_string())?;
        let child = xml_element_body_ci(item, "child")
            .map(decode_xml_text)
            .ok_or_else(|| "GTXD item is missing <child>".to_string())?;
        if parent.is_empty() || child.is_empty() {
            return Err("GTXD parent/child names must not be empty".into());
        }
        relationships.push(TextureParentRelationship { parent, child });
        remaining = rest;
    }

    Ok(relationships)
}

fn take_xml_element_ci<'a>(input: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let (open_start, open_end) = find_xml_open_tag_ci(input, tag)?;
    let lower = input.to_ascii_lowercase();
    let close = format!("</{}>", tag.to_ascii_lowercase());
    let close_rel = lower[open_end..].find(&close)?;
    let close_start = open_end + close_rel;
    let close_end = close_start + close.len();
    let _ = open_start;
    Some((&input[open_end..close_start], &input[close_end..]))
}

fn xml_element_body_ci<'a>(input: &'a str, tag: &str) -> Option<&'a str> {
    take_xml_element_ci(input, tag).map(|(body, _rest)| body)
}

fn find_xml_open_tag_ci(input: &str, tag: &str) -> Option<(usize, usize)> {
    let lower = input.to_ascii_lowercase();
    let needle = format!("<{}", tag.to_ascii_lowercase());
    let bytes = lower.as_bytes();
    let mut offset = 0_usize;

    while let Some(relative) = lower[offset..].find(&needle) {
        let start = offset + relative;
        let boundary = start + needle.len();
        let valid_boundary = bytes
            .get(boundary)
            .is_some_and(|byte| matches!(*byte, b'>' | b' ' | b'\t' | b'\r' | b'\n'));
        if valid_boundary {
            let end = lower[boundary..].find('>')? + boundary + 1;
            return Some((start, end));
        }
        offset = boundary;
    }

    None
}

fn decode_xml_text(value: &str) -> String {
    value
        .trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn encode_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_gtxd_xml(relationships: &BTreeSet<TextureParentRelationship>) -> String {
    let mut output = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<CMapParentTxds>\n  <txdRelationships>\n",
    );
    for relationship in relationships {
        output.push_str("    <Item>\n      <parent>");
        output.push_str(&encode_xml_text(&relationship.parent));
        output.push_str("</parent>\n      <child>");
        output.push_str(&encode_xml_text(&relationship.child));
        output.push_str("</child>\n    </Item>\n");
    }
    output.push_str("  </txdRelationships>\n</CMapParentTxds>\n");
    output
}

#[derive(Debug)]
pub enum WorkspaceError {
    InvalidRoot(PathBuf),
    Io { path: PathBuf, source: io::Error },
    Parse { path: PathBuf, message: String },
    Build(String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot(path) => write!(f, "not a directory: {}", path.display()),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Build(message) => f.write_str(message),
        }
    }
}

impl Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidRoot(_) | Self::Parse { .. } | Self::Build(_) => None,
        }
    }
}

fn file_kind(path: &Path) -> Option<AssetKind> {
    let extension = path.extension()?.to_str()?;
    AssetKind::from_extension(extension)
}

fn stem_hash(path: &Path) -> Option<u32> {
    path.file_stem().and_then(|value| value.to_str()).map(joaat)
}

fn walk(root: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            walk(&path, output)?;
        } else if file_type.is_file() {
            output.push(path);
        }
    }
    Ok(())
}
