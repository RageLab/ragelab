use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::Path,
    time::UNIX_EPOCH,
};

use ragelab_assets::{parse_gtxd_rbf, TextureParentRelationship};
use ragelab_hash::joaat;
use ragelab_rpf::{GtaKeyStore, GtaKeys, Rpf7Archive, RpfEntryLocator};
use ragelab_ytyp::{AssetType, Ytyp};
use serde::{Deserialize, Serialize};

use crate::gta_rpf_archive_order;

const GTA_RPF_INDEX_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GtaRpfAssetKind {
    Ytyp,
    Ydr,
    Ydd,
    Ytd,
    Ybn,
    Yft,
}

impl GtaRpfAssetKind {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Ytyp => "ytyp",
            Self::Ydr => "ydr",
            Self::Ydd => "ydd",
            Self::Ytd => "ytd",
            Self::Ybn => "ybn",
            Self::Yft => "yft",
        }
    }

    fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "ytyp" => Some(Self::Ytyp),
            "ydr" => Some(Self::Ydr),
            "ydd" => Some(Self::Ydd),
            "ytd" => Some(Self::Ytd),
            "ybn" => Some(Self::Ybn),
            "yft" => Some(Self::Yft),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfIndexLocator {
    pub archive_relative: String,
    pub nested: Vec<String>,
    pub entry: String,
    pub load_rank: u32,
}

impl GtaRpfIndexLocator {
    pub fn materialize(&self, game_root: &Path, keys_path: &Path) -> RpfEntryLocator {
        RpfEntryLocator::new(
            game_root.join(&self.archive_relative),
            self.nested.clone(),
            self.entry.clone(),
            keys_path.to_path_buf(),
        )
    }

    pub fn mount_key(&self) -> (&str, &[String]) {
        (&self.archive_relative, &self.nested)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WinnerSet<T> {
    load_rank: u32,
    candidates: Vec<T>,
}

impl<T: Clone + Ord> WinnerSet<T> {
    fn insert(&mut self, load_rank: u32, candidate: T) {
        if load_rank > self.load_rank {
            self.load_rank = load_rank;
            self.candidates.clear();
            self.candidates.push(candidate);
            return;
        }
        if load_rank == self.load_rank && !self.candidates.contains(&candidate) {
            self.candidates.push(candidate);
            self.candidates.sort();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfArchetypeRecord {
    pub provider: GtaRpfIndexLocator,
    pub provider_ytyp_hash: Option<u32>,
    pub asset_kind: Option<GtaRpfAssetKind>,
    pub asset_name_hash: Option<u32>,
    pub drawable_dictionary_hash: Option<u32>,
    pub texture_dictionary_hash: Option<u32>,
    pub physics_dictionary_hash: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfTextureParentRecord {
    pub child: String,
    pub child_hash: u32,
    pub parent: String,
    pub parent_hash: u32,
    pub source: GtaRpfIndexLocator,
}

impl GtaRpfTextureParentRecord {
    pub fn relationship(&self) -> TextureParentRelationship {
        TextureParentRelationship {
            parent: self.parent.clone(),
            child: self.child.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfInstallationFingerprint {
    pub gta_exe_size: u64,
    pub gta_exe_modified: u64,
    pub update_rpf_size: u64,
    pub update_rpf_modified: u64,
    pub outer_archive_count: u32,
    pub outer_archive_signature: u64,
}

impl GtaRpfInstallationFingerprint {
    pub fn read(game_root: &Path) -> Result<Self, io::Error> {
        let gta = file_fingerprint(&game_root.join("GTA5.exe"))?;
        let update = file_fingerprint(&game_root.join("update").join("update.rpf"))?;
        let outer_archives = outer_rpf_paths(game_root)?;
        let mut signature = 0xcbf29ce484222325_u64;
        for path in &outer_archives {
            let relative = path.strip_prefix(game_root).unwrap_or(path);
            for byte in relative.to_string_lossy().replace('\\', "/").bytes() {
                signature ^= u64::from(byte);
                signature = signature.wrapping_mul(0x100000001b3);
            }
            let (size, modified) = file_fingerprint(path)?;
            for byte in size.to_le_bytes().into_iter().chain(modified.to_le_bytes()) {
                signature ^= u64::from(byte);
                signature = signature.wrapping_mul(0x100000001b3);
            }
        }

        Ok(Self {
            gta_exe_size: gta.0,
            gta_exe_modified: gta.1,
            update_rpf_size: update.0,
            update_rpf_modified: update.1,
            outer_archive_count: outer_archives.len().try_into().unwrap_or(u32::MAX),
            outer_archive_signature: signature,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GtaRpfAssetIndex {
    schema_version: u32,
    fingerprint: GtaRpfInstallationFingerprint,
    files: BTreeMap<(GtaRpfAssetKind, u32), WinnerSet<GtaRpfIndexLocator>>,
    archetypes: BTreeMap<u32, WinnerSet<GtaRpfArchetypeRecord>>,
    texture_parents: BTreeMap<u32, WinnerSet<GtaRpfTextureParentRecord>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfIndexBuildReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub game_root: String,
    pub ordered_archives: usize,
    pub scanned_archives: usize,
    pub nested_archives: usize,
    pub indexed_files: usize,
    pub parsed_ytyps: usize,
    pub parsed_gtxd_files: usize,
    pub archetypes: usize,
    pub file_keys: usize,
    pub texture_parent_keys: usize,
    pub warnings: Vec<String>,
    pub platform_packs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfIndexCacheReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub index: String,
    pub cache_hit: bool,
    pub fingerprint: GtaRpfInstallationFingerprint,
    pub build: Option<GtaRpfIndexBuildReport>,
}

#[derive(Debug, Clone)]
pub struct GtaRpfIndexBuild {
    pub index: GtaRpfAssetIndex,
    pub report: GtaRpfIndexBuildReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfProviderSelection {
    pub locator: GtaRpfIndexLocator,
    pub archetype_hashes: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfIndexPlan {
    pub requested_archetypes: usize,
    pub provider_entries: Vec<GtaRpfIndexLocator>,
    pub provider_selections: Vec<GtaRpfProviderSelection>,
    pub asset_entries: Vec<GtaRpfIndexLocator>,
    pub texture_entries: Vec<GtaRpfIndexLocator>,
    pub texture_parents: Vec<GtaRpfTextureParentRecord>,
    pub collision_entries: Vec<GtaRpfIndexLocator>,
    pub unresolved_archetypes: Vec<u32>,
    pub ambiguous_archetypes: Vec<u32>,
    pub unresolved_assets: Vec<(GtaRpfAssetKind, u32)>,
    pub ambiguous_assets: Vec<(GtaRpfAssetKind, u32)>,
    pub ambiguous_texture_parents: Vec<u32>,
    pub texture_parent_cycles: Vec<u32>,
}

pub fn prepare_gta_rpf_index(
    game_root: &Path,
    keys_path: &Path,
    cache_root: &Path,
) -> Result<GtaRpfIndexCacheReport, io::Error> {
    let fingerprint = GtaRpfInstallationFingerprint::read(game_root)?;
    fs::create_dir_all(cache_root)?;
    let index_path = cache_root.join(format!(
        "legacy-v{}-{:08x}-{:016x}.bin",
        GTA_RPF_INDEX_SCHEMA_VERSION,
        fingerprint.outer_archive_count,
        fingerprint.outer_archive_signature
    ));

    if index_path.is_file() {
        if let Ok(index) = GtaRpfAssetIndex::load(&index_path) {
            if index.fingerprint == fingerprint {
                return Ok(GtaRpfIndexCacheReport {
                    schema: "ragelab.gta.rpf-index-cache",
                    schema_version: 1,
                    index: index_path.display().to_string(),
                    cache_hit: true,
                    fingerprint,
                    build: None,
                });
            }
        }
    }

    let build = GtaRpfAssetIndex::build(game_root, keys_path)?;
    let temporary = cache_root.join(format!(
        ".legacy-v{}-{:016x}-{}.tmp",
        GTA_RPF_INDEX_SCHEMA_VERSION,
        fingerprint.outer_archive_signature,
        std::process::id()
    ));
    let _ = fs::remove_file(&temporary);
    build.index.save(&temporary, true)?;

    if index_path.exists() {
        fs::remove_file(&index_path)?;
    }
    fs::rename(&temporary, &index_path)?;

    Ok(GtaRpfIndexCacheReport {
        schema: "ragelab.gta.rpf-index-cache",
        schema_version: 1,
        index: index_path.display().to_string(),
        cache_hit: false,
        fingerprint,
        build: Some(build.report),
    })
}

#[derive(Default)]
struct TextureDictionaryPlanState {
    entries: BTreeSet<GtaRpfIndexLocator>,
    parents: BTreeSet<GtaRpfTextureParentRecord>,
    ambiguous_parents: BTreeSet<u32>,
    parent_cycles: BTreeSet<u32>,
}

impl GtaRpfAssetIndex {
    pub fn build(game_root: &Path, keys_path: &Path) -> Result<GtaRpfIndexBuild, io::Error> {
        let fingerprint = GtaRpfInstallationFingerprint::read(game_root)?;
        let order = gta_rpf_archive_order(game_root, keys_path)?;
        let keys =
            GtaKeyStore::load(keys_path).map_err(|error| io::Error::other(error.to_string()))?;

        let mut index = Self {
            schema_version: GTA_RPF_INDEX_SCHEMA_VERSION,
            fingerprint,
            files: BTreeMap::new(),
            archetypes: BTreeMap::new(),
            texture_parents: BTreeMap::new(),
        };
        let mut scan = ScanCounters::default();
        let mut warnings = order.warnings.clone();

        for archive in &order.archives {
            let path = Path::new(&archive.path);
            match Rpf7Archive::open(path, Some(&keys)) {
                Ok(opened) => {
                    scan.scanned_archives += 1;
                    let context = ScanArchiveContext {
                        archive_relative: archive.relative_path.clone(),
                        load_rank: archive.load_rank,
                    };
                    scan_archive(
                        &mut index,
                        &opened,
                        &keys,
                        &context,
                        &[],
                        &mut scan,
                        &mut warnings,
                    );
                }
                Err(error) => warnings.push(format!(
                    "{}: outer RPF open failed: {error}",
                    archive.relative_path
                )),
            }
        }

        Ok(GtaRpfIndexBuild {
            report: GtaRpfIndexBuildReport {
                schema: "ragelab.gta.rpf-index",
                schema_version: GTA_RPF_INDEX_SCHEMA_VERSION,
                game_root: game_root.display().to_string(),
                ordered_archives: order.archives.len(),
                scanned_archives: scan.scanned_archives,
                nested_archives: scan.nested_archives,
                indexed_files: scan.indexed_files,
                parsed_ytyps: scan.parsed_ytyps,
                parsed_gtxd_files: scan.parsed_gtxd_files,
                archetypes: index.archetypes.len(),
                file_keys: index.files.len(),
                texture_parent_keys: index.texture_parents.len(),
                warnings,
                platform_packs: order.platform_packs,
            },
            index,
        })
    }

    pub fn save(&self, path: &Path, overwrite: bool) -> Result<(), io::Error> {
        if path.exists() && !overwrite {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("RPF index already exists: {}", path.display()),
            ));
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let bytes = bincode::serialize(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        fs::write(path, bytes)
    }

    pub fn load(path: &Path) -> Result<Self, io::Error> {
        let bytes = fs::read(path)?;
        let index: Self = bincode::deserialize(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        if index.schema_version != GTA_RPF_INDEX_SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported GTA RPF index schema {}; expected {}",
                    index.schema_version, GTA_RPF_INDEX_SCHEMA_VERSION
                ),
            ));
        }
        Ok(index)
    }

    pub fn matches_installation(&self, game_root: &Path) -> Result<bool, io::Error> {
        Ok(self.fingerprint == GtaRpfInstallationFingerprint::read(game_root)?)
    }

    pub fn file_candidates(&self, kind: GtaRpfAssetKind, hash: u32) -> &[GtaRpfIndexLocator] {
        self.files
            .get(&(kind, hash))
            .map(|winner| winner.candidates.as_slice())
            .unwrap_or(&[])
    }

    pub fn archetype_candidates(&self, hash: u32) -> &[GtaRpfArchetypeRecord] {
        self.archetypes
            .get(&hash)
            .map(|winner| winner.candidates.as_slice())
            .unwrap_or(&[])
    }

    pub fn texture_parent_candidates(&self, child_hash: u32) -> &[GtaRpfTextureParentRecord] {
        self.texture_parents
            .get(&child_hash)
            .map(|winner| winner.candidates.as_slice())
            .unwrap_or(&[])
    }

    pub fn plan_for_archetypes(&self, hashes: impl IntoIterator<Item = u32>) -> GtaRpfIndexPlan {
        let requested = hashes.into_iter().collect::<BTreeSet<_>>();
        let mut provider_entries = BTreeSet::new();
        let mut provider_hashes = BTreeMap::<GtaRpfIndexLocator, BTreeSet<u32>>::new();
        let mut asset_entries = BTreeSet::new();
        let mut texture_plan = TextureDictionaryPlanState::default();
        let mut collision_entries = BTreeSet::new();
        let mut unresolved_archetypes = Vec::new();
        let mut ambiguous_archetypes = Vec::new();
        let mut unresolved_assets = BTreeSet::new();
        let mut ambiguous_assets = BTreeSet::new();

        for hash in &requested {
            let candidates = self.archetype_candidates(*hash);
            let provider = match candidates {
                [] => {
                    unresolved_archetypes.push(*hash);
                    continue;
                }
                [provider] => provider,
                _ => {
                    ambiguous_archetypes.push(*hash);
                    continue;
                }
            };
            provider_entries.insert(provider.provider.clone());
            provider_hashes
                .entry(provider.provider.clone())
                .or_default()
                .insert(*hash);

            if let Some((kind, asset_hash)) = primary_asset_key(provider) {
                match self.file_candidates(kind, asset_hash) {
                    [] => {
                        unresolved_assets.insert((kind, asset_hash));
                    }
                    [locator] => {
                        asset_entries.insert(locator.clone());
                    }
                    _ => {
                        ambiguous_assets.insert((kind, asset_hash));
                    }
                }
            }

            if let Some(hash) = provider.texture_dictionary_hash {
                self.collect_texture_dictionary_chain(
                    hash,
                    &mut texture_plan,
                    &mut unresolved_assets,
                    &mut ambiguous_assets,
                );
            }

            if let Some(hash) = provider.physics_dictionary_hash {
                match self.file_candidates(GtaRpfAssetKind::Ybn, hash) {
                    [] => {}
                    [locator] => {
                        collision_entries.insert(locator.clone());
                    }
                    _ => {
                        ambiguous_assets.insert((GtaRpfAssetKind::Ybn, hash));
                    }
                }
            }
        }

        GtaRpfIndexPlan {
            requested_archetypes: requested.len(),
            provider_entries: provider_entries.into_iter().collect(),
            provider_selections: provider_hashes
                .into_iter()
                .map(|(locator, hashes)| GtaRpfProviderSelection {
                    locator,
                    archetype_hashes: hashes.into_iter().collect(),
                })
                .collect(),
            asset_entries: asset_entries.into_iter().collect(),
            texture_entries: texture_plan.entries.into_iter().collect(),
            texture_parents: texture_plan.parents.into_iter().collect(),
            collision_entries: collision_entries.into_iter().collect(),
            unresolved_archetypes,
            ambiguous_archetypes,
            unresolved_assets: unresolved_assets.into_iter().collect(),
            ambiguous_assets: ambiguous_assets.into_iter().collect(),
            ambiguous_texture_parents: texture_plan.ambiguous_parents.into_iter().collect(),
            texture_parent_cycles: texture_plan.parent_cycles.into_iter().collect(),
        }
    }

    fn collect_texture_dictionary_chain(
        &self,
        requested_hash: u32,
        texture_plan: &mut TextureDictionaryPlanState,
        unresolved_assets: &mut BTreeSet<(GtaRpfAssetKind, u32)>,
        ambiguous_assets: &mut BTreeSet<(GtaRpfAssetKind, u32)>,
    ) {
        let mut visited = BTreeSet::new();
        let mut current = requested_hash;

        loop {
            if !visited.insert(current) {
                texture_plan.parent_cycles.insert(current);
                return;
            }

            let mut file_missing = false;
            match self.file_candidates(GtaRpfAssetKind::Ytd, current) {
                [] => file_missing = true,
                [locator] => {
                    texture_plan.entries.insert(locator.clone());
                }
                _ => {
                    ambiguous_assets.insert((GtaRpfAssetKind::Ytd, current));
                    return;
                }
            }

            match self.texture_parent_candidates(current) {
                [] => {
                    if file_missing {
                        unresolved_assets.insert((GtaRpfAssetKind::Ytd, current));
                    }
                    return;
                }
                [parent] => {
                    texture_plan.parents.insert(parent.clone());
                    current = parent.parent_hash;
                }
                _ => {
                    texture_plan.ambiguous_parents.insert(current);
                    return;
                }
            }
        }
    }
}

#[derive(Default)]
struct ScanCounters {
    scanned_archives: usize,
    nested_archives: usize,
    indexed_files: usize,
    parsed_ytyps: usize,
    parsed_gtxd_files: usize,
}

struct ScanArchiveContext {
    archive_relative: String,
    load_rank: u32,
}

fn scan_archive(
    index: &mut GtaRpfAssetIndex,
    archive: &Rpf7Archive,
    keys: &GtaKeys,
    context: &ScanArchiveContext,
    nested: &[String],
    counters: &mut ScanCounters,
    warnings: &mut Vec<String>,
) {
    let files = archive.files().collect::<Vec<_>>();

    for file in files {
        let path = Path::new(&file.path);
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if extension == "rpf" {
            match archive.open_nested(&file.path, Some(keys)) {
                Ok(child) => {
                    counters.nested_archives += 1;
                    let mut child_chain = nested.to_vec();
                    child_chain.push(file.path.clone());
                    scan_archive(
                        index,
                        &child,
                        keys,
                        context,
                        &child_chain,
                        counters,
                        warnings,
                    );
                }
                Err(error) => warnings.push(format!(
                    "{}{}!/{path}: nested RPF open failed: {error}",
                    context.archive_relative,
                    nested_suffix(nested),
                    path = file.path
                )),
            }
            continue;
        }

        if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("gtxd.ymt"))
        {
            let source = GtaRpfIndexLocator {
                archive_relative: context.archive_relative.clone(),
                nested: nested.to_vec(),
                entry: file.path.clone(),
                load_rank: context.load_rank,
            };
            match archive.read_file(&file.path, Some(keys)) {
                Ok(bytes) => match parse_gtxd_rbf(&bytes) {
                    Ok(relationships) => {
                        counters.parsed_gtxd_files += 1;
                        for relationship in relationships {
                            insert_texture_parent_winner(
                                index,
                                GtaRpfTextureParentRecord {
                                    child_hash: joaat(&relationship.child),
                                    parent_hash: joaat(&relationship.parent),
                                    child: relationship.child,
                                    parent: relationship.parent,
                                    source: source.clone(),
                                },
                            );
                        }
                    }
                    Err(error) => warnings.push(format!(
                        "{}: GTXD parse failed: {error}",
                        provenance(&source)
                    )),
                },
                Err(error) => warnings.push(format!(
                    "{}: GTXD read failed: {error}",
                    provenance(&source)
                )),
            }
            continue;
        }

        let Some(kind) = GtaRpfAssetKind::from_extension(&extension) else {
            continue;
        };
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };

        let locator = GtaRpfIndexLocator {
            archive_relative: context.archive_relative.clone(),
            nested: nested.to_vec(),
            entry: file.path.clone(),
            load_rank: context.load_rank,
        };
        insert_file_winner(index, kind, joaat(stem), locator.clone());
        counters.indexed_files += 1;

        if kind != GtaRpfAssetKind::Ytyp {
            continue;
        }

        let bytes = match archive.read_file(&file.path, Some(keys)) {
            Ok(bytes) => bytes,
            Err(error) => {
                warnings.push(format!(
                    "{}: YTYP read failed: {error}",
                    provenance(&locator)
                ));
                continue;
            }
        };
        let ytyp = match Ytyp::from_bytes(&bytes) {
            Ok(ytyp) => ytyp,
            Err(error) => {
                warnings.push(format!(
                    "{}: YTYP parse failed: {error}",
                    provenance(&locator)
                ));
                continue;
            }
        };
        counters.parsed_ytyps += 1;

        for archetype in ytyp.archetypes {
            let asset_kind = match archetype.asset_type {
                AssetType::Fragment => Some(GtaRpfAssetKind::Yft),
                AssetType::Drawable => Some(GtaRpfAssetKind::Ydr),
                AssetType::DrawableDictionary => Some(GtaRpfAssetKind::Ydd),
                AssetType::Uninitialized | AssetType::Assetless | AssetType::Unknown(_) => None,
            };
            let record = GtaRpfArchetypeRecord {
                provider: locator.clone(),
                provider_ytyp_hash: ytyp.name.map(|hash| hash.0),
                asset_kind,
                asset_name_hash: archetype.asset_name.map(|hash| hash.0),
                drawable_dictionary_hash: archetype.drawable_dictionary.map(|hash| hash.0),
                texture_dictionary_hash: archetype.texture_dictionary.map(|hash| hash.0),
                physics_dictionary_hash: archetype.physics_dictionary.map(|hash| hash.0),
            };
            insert_archetype_winner(index, archetype.name.0, record);
        }
    }
}

fn insert_file_winner(
    index: &mut GtaRpfAssetIndex,
    kind: GtaRpfAssetKind,
    hash: u32,
    locator: GtaRpfIndexLocator,
) {
    match index.files.get_mut(&(kind, hash)) {
        Some(winner) => winner.insert(locator.load_rank, locator),
        None => {
            index.files.insert(
                (kind, hash),
                WinnerSet {
                    load_rank: locator.load_rank,
                    candidates: vec![locator],
                },
            );
        }
    }
}

fn insert_archetype_winner(index: &mut GtaRpfAssetIndex, hash: u32, record: GtaRpfArchetypeRecord) {
    match index.archetypes.get_mut(&hash) {
        Some(winner) => winner.insert(record.provider.load_rank, record),
        None => {
            index.archetypes.insert(
                hash,
                WinnerSet {
                    load_rank: record.provider.load_rank,
                    candidates: vec![record],
                },
            );
        }
    }
}

fn insert_texture_parent_winner(index: &mut GtaRpfAssetIndex, record: GtaRpfTextureParentRecord) {
    let child_hash = record.child_hash;
    let load_rank = record.source.load_rank;
    match index.texture_parents.get_mut(&child_hash) {
        Some(winner) => winner.insert(load_rank, record),
        None => {
            index.texture_parents.insert(
                child_hash,
                WinnerSet {
                    load_rank,
                    candidates: vec![record],
                },
            );
        }
    }
}

fn primary_asset_key(archetype: &GtaRpfArchetypeRecord) -> Option<(GtaRpfAssetKind, u32)> {
    let kind = archetype.asset_kind?;
    let hash = if kind == GtaRpfAssetKind::Ydd {
        archetype.drawable_dictionary_hash?
    } else {
        archetype.asset_name_hash?
    };
    Some((kind, hash))
}

fn nested_suffix(nested: &[String]) -> String {
    let mut suffix = String::new();
    for entry in nested {
        suffix.push_str("!/");
        suffix.push_str(entry);
    }
    suffix
}

fn provenance(locator: &GtaRpfIndexLocator) -> String {
    format!(
        "{}{}!/{}",
        locator.archive_relative,
        nested_suffix(&locator.nested),
        locator.entry
    )
}

fn outer_rpf_paths(game_root: &Path) -> Result<Vec<std::path::PathBuf>, io::Error> {
    let mut paths = Vec::new();

    for entry in fs::read_dir(game_root)?.filter_map(Result::ok) {
        let path = entry.path();
        if is_rpf_file(&path) {
            paths.push(path);
        }
    }

    let update_root = game_root.join("update");
    if update_root.is_dir() {
        for entry in fs::read_dir(&update_root)?.filter_map(Result::ok) {
            let path = entry.path();
            if is_rpf_file(&path) {
                paths.push(path);
            }
        }
    }

    let dlcpacks = update_root.join("x64").join("dlcpacks");
    if dlcpacks.is_dir() {
        for pack in fs::read_dir(&dlcpacks)?.filter_map(Result::ok) {
            if !pack.path().is_dir() {
                continue;
            }
            for entry in fs::read_dir(pack.path())?.filter_map(Result::ok) {
                let path = entry.path();
                if is_rpf_file(&path) {
                    paths.push(path);
                }
            }
        }
    }

    paths.sort_by_key(|path| {
        path.strip_prefix(game_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase()
    });
    Ok(paths)
}

fn is_rpf_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rpf"))
}

fn file_fingerprint(path: &Path) -> Result<(u64, u64), io::Error> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
        .as_secs();
    Ok((metadata.len(), modified))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locator(rank: u32, entry: &str) -> GtaRpfIndexLocator {
        GtaRpfIndexLocator {
            archive_relative: "x64a.rpf".into(),
            nested: vec!["nested.rpf".into()],
            entry: entry.into(),
            load_rank: rank,
        }
    }

    fn empty_index() -> GtaRpfAssetIndex {
        GtaRpfAssetIndex {
            schema_version: GTA_RPF_INDEX_SCHEMA_VERSION,
            fingerprint: GtaRpfInstallationFingerprint {
                gta_exe_size: 1,
                gta_exe_modified: 2,
                update_rpf_size: 3,
                update_rpf_modified: 4,
                outer_archive_count: 0,
                outer_archive_signature: 0,
            },
            files: BTreeMap::new(),
            archetypes: BTreeMap::new(),
            texture_parents: BTreeMap::new(),
        }
    }

    #[test]
    fn prepare_index_reuses_matching_cached_fingerprint() {
        let root =
            std::env::temp_dir().join(format!("ragelab-game-index-cache-{}", std::process::id()));
        let update = root.join("update");
        let cache = root.join("cache");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&update).expect("create synthetic GTA root");
        fs::write(root.join("GTA5.exe"), b"synthetic-gta").expect("write synthetic exe");
        fs::write(update.join("update.rpf"), b"synthetic-update").expect("write update");

        let fingerprint =
            GtaRpfInstallationFingerprint::read(&root).expect("read synthetic fingerprint");
        let index_path = cache.join(format!(
            "legacy-v{}-{:08x}-{:016x}.bin",
            GTA_RPF_INDEX_SCHEMA_VERSION,
            fingerprint.outer_archive_count,
            fingerprint.outer_archive_signature
        ));
        let index = GtaRpfAssetIndex {
            schema_version: GTA_RPF_INDEX_SCHEMA_VERSION,
            fingerprint: fingerprint.clone(),
            files: BTreeMap::new(),
            archetypes: BTreeMap::new(),
            texture_parents: BTreeMap::new(),
        };
        index.save(&index_path, true).expect("seed cached index");

        let report = prepare_gta_rpf_index(&root, Path::new("unused-keys"), &cache)
            .expect("reuse matching cached index");
        assert!(report.cache_hit);
        assert_eq!(std::path::PathBuf::from(report.index), index_path);
        assert_eq!(report.fingerprint, fingerprint);
        assert!(report.build.is_none());

        fs::remove_dir_all(root).expect("remove synthetic fixture");
    }

    #[test]
    fn higher_load_rank_replaces_lower_file_winner() {
        let mut index = empty_index();
        insert_file_winner(&mut index, GtaRpfAssetKind::Ydr, 7, locator(10, "old.ydr"));
        insert_file_winner(&mut index, GtaRpfAssetKind::Ydr, 7, locator(20, "new.ydr"));

        let candidates = index.file_candidates(GtaRpfAssetKind::Ydr, 7);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].entry, "new.ydr");
    }

    #[test]
    fn same_load_rank_preserves_ambiguity() {
        let mut index = empty_index();
        insert_file_winner(&mut index, GtaRpfAssetKind::Ydr, 7, locator(20, "one.ydr"));
        insert_file_winner(&mut index, GtaRpfAssetKind::Ydr, 7, locator(20, "two.ydr"));

        let candidates = index.file_candidates(GtaRpfAssetKind::Ydr, 7);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn plan_includes_unique_texture_dictionary_winner() {
        let mut index = empty_index();
        let provider = GtaRpfArchetypeRecord {
            provider: locator(10, "provider.ytyp"),
            provider_ytyp_hash: None,
            asset_kind: Some(GtaRpfAssetKind::Ydr),
            asset_name_hash: Some(0x1111),
            drawable_dictionary_hash: None,
            texture_dictionary_hash: Some(0x3333),
            physics_dictionary_hash: None,
        };
        insert_archetype_winner(&mut index, 0xAAAA, provider);
        insert_file_winner(
            &mut index,
            GtaRpfAssetKind::Ydr,
            0x1111,
            locator(10, "model.ydr"),
        );
        insert_file_winner(
            &mut index,
            GtaRpfAssetKind::Ytd,
            0x3333,
            locator(10, "textures.ytd"),
        );

        let plan = index.plan_for_archetypes([0xAAAA]);
        assert!(plan.unresolved_assets.is_empty());
        assert!(plan.ambiguous_assets.is_empty());
        assert_eq!(plan.texture_entries.len(), 1);
        assert_eq!(plan.texture_entries[0].entry, "textures.ytd");
    }

    #[test]
    fn plan_follows_gtxd_parent_when_child_ytd_is_absent() {
        let mut index = empty_index();
        let provider = GtaRpfArchetypeRecord {
            provider: locator(10, "provider.ytyp"),
            provider_ytyp_hash: None,
            asset_kind: Some(GtaRpfAssetKind::Ydr),
            asset_name_hash: Some(0x1111),
            drawable_dictionary_hash: None,
            texture_dictionary_hash: Some(0x3333),
            physics_dictionary_hash: None,
        };
        insert_archetype_winner(&mut index, 0xAAAA, provider);
        insert_file_winner(
            &mut index,
            GtaRpfAssetKind::Ydr,
            0x1111,
            locator(10, "model.ydr"),
        );
        insert_file_winner(
            &mut index,
            GtaRpfAssetKind::Ytd,
            0x4444,
            locator(10, "parent.ytd"),
        );
        insert_texture_parent_winner(
            &mut index,
            GtaRpfTextureParentRecord {
                child: "child".into(),
                child_hash: 0x3333,
                parent: "parent".into(),
                parent_hash: 0x4444,
                source: locator(10, "gtxd.ymt"),
            },
        );

        let plan = index.plan_for_archetypes([0xAAAA]);
        assert_eq!(plan.texture_entries.len(), 1);
        assert_eq!(plan.texture_entries[0].entry, "parent.ytd");
        assert_eq!(plan.texture_parents.len(), 1);
        assert_eq!(plan.texture_parents[0].child_hash, 0x3333);
        assert!(!plan
            .unresolved_assets
            .contains(&(GtaRpfAssetKind::Ytd, 0x3333)));
        assert!(plan.ambiguous_texture_parents.is_empty());
        assert!(plan.texture_parent_cycles.is_empty());
    }

    #[test]
    fn plan_uses_dictionary_hash_for_ydd_primary_asset() {
        let mut index = empty_index();
        let provider = GtaRpfArchetypeRecord {
            provider: locator(10, "provider.ytyp"),
            provider_ytyp_hash: None,
            asset_kind: Some(GtaRpfAssetKind::Ydd),
            asset_name_hash: Some(0x1111),
            drawable_dictionary_hash: Some(0x2222),
            texture_dictionary_hash: None,
            physics_dictionary_hash: None,
        };
        insert_archetype_winner(&mut index, 0xAAAA, provider);
        insert_file_winner(
            &mut index,
            GtaRpfAssetKind::Ydd,
            0x2222,
            locator(10, "dictionary.ydd"),
        );

        let plan = index.plan_for_archetypes([0xAAAA]);
        assert!(plan.unresolved_archetypes.is_empty());
        assert!(plan.unresolved_assets.is_empty());
        assert_eq!(plan.provider_entries.len(), 1);
        assert_eq!(plan.provider_selections.len(), 1);
        assert_eq!(plan.provider_selections[0].archetype_hashes, vec![0xAAAA]);
        assert_eq!(plan.asset_entries.len(), 1);
        assert_eq!(plan.asset_entries[0].entry, "dictionary.ydd");
    }
}
