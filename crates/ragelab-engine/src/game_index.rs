use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::Path,
    time::UNIX_EPOCH,
};

use ragelab_assets::{parse_gtxd_rbf, TextureParentRelationship};
use ragelab_hash::joaat;
use ragelab_rpf::{GtaKeyStore, GtaKeys, Rpf7Archive, RpfEntryLocator};
use ragelab_ymap::{Vec3 as YmapVec3, Ymap};
use ragelab_ytyp::{AssetType, Ytyp};
use serde::{Deserialize, Serialize};

use crate::gta_rpf_archive_order;

const GTA_RPF_INDEX_SCHEMA_VERSION: u32 = 4;
const WORLD_SPATIAL_CELL_SIZE: f32 = 512.0;
const WORLD_SPATIAL_MAX_CELLS_PER_MAP: i64 = 4_096;
const WORLD_SPATIAL_MAX_QUERY_CELLS: i64 = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GtaRpfAssetKind {
    Ytyp,
    Ymap,
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
            Self::Ymap => "ymap",
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
            "ymap" => Some(Self::Ymap),
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldPoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<YmapVec3> for GtaRpfWorldPoint {
    fn from(value: YmapVec3) -> Self {
        Self {
            x: value.x,
            y: value.y,
            z: value.z,
        }
    }
}

impl GtaRpfWorldPoint {
    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldBounds {
    pub min: GtaRpfWorldPoint,
    pub max: GtaRpfWorldPoint,
}

impl GtaRpfWorldBounds {
    pub fn is_valid(self) -> bool {
        self.min.is_finite()
            && self.max.is_finite()
            && self.min.x <= self.max.x
            && self.min.y <= self.max.y
            && self.min.z <= self.max.z
    }

    pub fn intersects(self, other: Self) -> bool {
        self.is_valid()
            && other.is_valid()
            && self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    pub fn contains_point(self, point: GtaRpfWorldPoint) -> bool {
        self.is_valid()
            && point.is_finite()
            && point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }

    pub fn distance_squared_to_point(self, point: GtaRpfWorldPoint) -> f32 {
        if !self.is_valid() || !point.is_finite() {
            return f32::INFINITY;
        }
        let dx = if point.x < self.min.x {
            self.min.x - point.x
        } else if point.x > self.max.x {
            point.x - self.max.x
        } else {
            0.0
        };
        let dy = if point.y < self.min.y {
            self.min.y - point.y
        } else if point.y > self.max.y {
            point.y - self.max.y
        } else {
            0.0
        };
        let dz = if point.z < self.min.z {
            self.min.z - point.z
        } else if point.z > self.max.z {
            point.z - self.max.z
        } else {
            0.0
        };
        dx * dx + dy * dy + dz * dz
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldEntityRecord {
    pub index: u32,
    pub archetype_hash: u32,
    pub position: GtaRpfWorldPoint,
    pub rotation: [f32; 4],
    pub scale_xy: Option<f32>,
    pub scale_z: Option<f32>,
    pub flags: u32,
    pub parent_index: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldMapRecord {
    pub provider: GtaRpfIndexLocator,
    pub file_name_hash: u32,
    pub map_name_hash: Option<u32>,
    pub parent_hash: Option<u32>,
    pub flags: Option<u32>,
    pub content_flags: Option<u32>,
    pub entities_bounds: Option<GtaRpfWorldBounds>,
    pub streaming_bounds: Option<GtaRpfWorldBounds>,
    pub physics_dictionary_hashes: Vec<u32>,
    pub archetype_hashes: Vec<u32>,
    pub entities: Vec<GtaRpfWorldEntityRecord>,
}

impl GtaRpfWorldMapRecord {
    pub fn map_hash(&self) -> u32 {
        self.map_name_hash.unwrap_or(self.file_name_hash)
    }

    pub fn effective_bounds(&self) -> Option<GtaRpfWorldBounds> {
        self.streaming_bounds.or(self.entities_bounds)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfYtypRecord {
    pub provider: GtaRpfIndexLocator,
    pub file_name_hash: u32,
    pub name_hash: Option<u32>,
    pub dependencies: Vec<u32>,
    pub archetype_hashes: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorldMapWinnerSet {
    load_rank: u32,
    candidates: Vec<GtaRpfWorldMapRecord>,
}

impl WorldMapWinnerSet {
    fn insert(&mut self, load_rank: u32, candidate: GtaRpfWorldMapRecord) {
        if load_rank > self.load_rank {
            self.load_rank = load_rank;
            self.candidates.clear();
            self.candidates.push(candidate);
            return;
        }
        if load_rank == self.load_rank
            && !self
                .candidates
                .iter()
                .any(|existing| existing.provider == candidate.provider)
        {
            self.candidates.push(candidate);
            self.candidates
                .sort_by(|left, right| left.provider.cmp(&right.provider));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldPlane {
    pub normal: GtaRpfWorldPoint,
    pub distance: f32,
}

impl GtaRpfWorldPlane {
    fn is_valid(self) -> bool {
        self.normal.is_finite() && self.distance.is_finite()
    }

    fn signed_distance(self, point: GtaRpfWorldPoint) -> f32 {
        self.normal.x * point.x + self.normal.y * point.y + self.normal.z * point.z + self.distance
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldFrustum {
    pub broadphase_bounds: GtaRpfWorldBounds,
    pub planes: [GtaRpfWorldPlane; 6],
}

impl GtaRpfWorldFrustum {
    pub fn is_valid(self) -> bool {
        self.broadphase_bounds.is_valid() && self.planes.iter().all(|plane| plane.is_valid())
    }

    pub fn contains_point(self, point: GtaRpfWorldPoint) -> bool {
        point.is_finite()
            && self
                .planes
                .iter()
                .all(|plane| plane.signed_distance(point) >= 0.0)
    }

    pub fn intersects_bounds(self, bounds: GtaRpfWorldBounds) -> bool {
        if !bounds.is_valid() {
            return false;
        }
        self.planes.iter().all(|plane| {
            let positive = GtaRpfWorldPoint {
                x: if plane.normal.x >= 0.0 {
                    bounds.max.x
                } else {
                    bounds.min.x
                },
                y: if plane.normal.y >= 0.0 {
                    bounds.max.y
                } else {
                    bounds.min.y
                },
                z: if plane.normal.z >= 0.0 {
                    bounds.max.z
                } else {
                    bounds.min.z
                },
            };
            plane.signed_distance(positive) >= 0.0
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldMapHit {
    pub map_hash: u32,
    pub provider: GtaRpfIndexLocator,
    pub parent_hash: Option<u32>,
    pub flags: Option<u32>,
    pub content_flags: Option<u32>,
    pub entities_bounds: Option<GtaRpfWorldBounds>,
    pub streaming_bounds: Option<GtaRpfWorldBounds>,
    pub bounds: Option<GtaRpfWorldBounds>,
    pub entity_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldEntityHit {
    pub map_hash: u32,
    pub map_provider: GtaRpfIndexLocator,
    pub entity: GtaRpfWorldEntityRecord,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfWorldQueryReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub bounds: GtaRpfWorldBounds,
    pub maps: Vec<GtaRpfWorldMapHit>,
    pub entities: Vec<GtaRpfWorldEntityHit>,
    pub candidate_map_keys: usize,
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
    ytyps: BTreeMap<u32, WinnerSet<GtaRpfYtypRecord>>,
    world_maps: BTreeMap<u32, WorldMapWinnerSet>,
    world_children: BTreeMap<u32, Vec<u32>>,
    world_cells: BTreeMap<(i32, i32), Vec<u32>>,
    world_global_maps: Vec<u32>,
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
    pub parsed_ymaps: usize,
    pub parsed_gtxd_files: usize,
    pub archetypes: usize,
    pub ytyp_records: usize,
    pub world_maps: usize,
    pub world_entities: usize,
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
            ytyps: BTreeMap::new(),
            world_maps: BTreeMap::new(),
            world_children: BTreeMap::new(),
            world_cells: BTreeMap::new(),
            world_global_maps: Vec::new(),
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

        index.rebuild_world_acceleration();

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
                parsed_ymaps: scan.parsed_ymaps,
                parsed_gtxd_files: scan.parsed_gtxd_files,
                archetypes: index.archetypes.len(),
                ytyp_records: index.ytyps.len(),
                world_maps: index.world_maps.len(),
                world_entities: scan.world_entities,
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

    pub fn ytyp_candidates(&self, hash: u32) -> &[GtaRpfYtypRecord] {
        self.ytyps
            .get(&hash)
            .map(|winner| winner.candidates.as_slice())
            .unwrap_or(&[])
    }

    pub fn world_map_candidates(&self, hash: u32) -> &[GtaRpfWorldMapRecord] {
        self.world_maps
            .get(&hash)
            .map(|winner| winner.candidates.as_slice())
            .unwrap_or(&[])
    }

    pub fn world_children(&self, parent_hash: u32) -> &[u32] {
        self.world_children
            .get(&parent_hash)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn query_world_box(
        &self,
        bounds: GtaRpfWorldBounds,
        include_entities: bool,
    ) -> Result<GtaRpfWorldQueryReport, io::Error> {
        if !bounds.is_valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world query bounds must be finite and ordered",
            ));
        }

        let candidate_keys = self.world_candidate_keys(bounds);
        let mut maps = Vec::new();
        let mut entities = Vec::new();

        for map_hash in &candidate_keys {
            for record in self.world_map_candidates(*map_hash) {
                let record_bounds = record.effective_bounds();
                if record_bounds.is_some_and(|candidate| !candidate.intersects(bounds)) {
                    continue;
                }

                maps.push(GtaRpfWorldMapHit {
                    map_hash: *map_hash,
                    provider: record.provider.clone(),
                    parent_hash: record.parent_hash,
                    flags: record.flags,
                    content_flags: record.content_flags,
                    entities_bounds: record.entities_bounds,
                    streaming_bounds: record.streaming_bounds,
                    bounds: record_bounds,
                    entity_count: record.entities.len(),
                });

                if include_entities {
                    for entity in &record.entities {
                        if bounds.contains_point(entity.position) {
                            entities.push(GtaRpfWorldEntityHit {
                                map_hash: *map_hash,
                                map_provider: record.provider.clone(),
                                entity: entity.clone(),
                            });
                        }
                    }
                }
            }
        }

        maps.sort_by(|left, right| {
            left.map_hash
                .cmp(&right.map_hash)
                .then_with(|| left.provider.cmp(&right.provider))
        });
        entities.sort_by(|left, right| {
            left.map_hash
                .cmp(&right.map_hash)
                .then_with(|| left.map_provider.cmp(&right.map_provider))
                .then_with(|| left.entity.index.cmp(&right.entity.index))
        });

        Ok(GtaRpfWorldQueryReport {
            schema: "ragelab.gta.world-query",
            schema_version: 1,
            bounds,
            maps,
            entities,
            candidate_map_keys: candidate_keys.len(),
        })
    }

    pub fn query_world_radius(
        &self,
        center: GtaRpfWorldPoint,
        radius: f32,
        include_entities: bool,
    ) -> Result<GtaRpfWorldQueryReport, io::Error> {
        if !center.is_finite() || !radius.is_finite() || radius < 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world radius query requires finite center and non-negative finite radius",
            ));
        }
        let bounds = GtaRpfWorldBounds {
            min: GtaRpfWorldPoint {
                x: center.x - radius,
                y: center.y - radius,
                z: center.z - radius,
            },
            max: GtaRpfWorldPoint {
                x: center.x + radius,
                y: center.y + radius,
                z: center.z + radius,
            },
        };
        let mut report = self.query_world_box(bounds, include_entities)?;
        let radius_squared = radius * radius;
        report.maps.retain(|hit| {
            hit.bounds.map_or(true, |candidate| {
                candidate.distance_squared_to_point(center) <= radius_squared
            })
        });
        report.entities.retain(|hit| {
            let dx = hit.entity.position.x - center.x;
            let dy = hit.entity.position.y - center.y;
            let dz = hit.entity.position.z - center.z;
            dx * dx + dy * dy + dz * dz <= radius_squared
        });
        Ok(report)
    }

    pub fn query_world_frustum(
        &self,
        frustum: GtaRpfWorldFrustum,
        include_entities: bool,
    ) -> Result<GtaRpfWorldQueryReport, io::Error> {
        if !frustum.is_valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world frustum query requires finite broadphase bounds and planes",
            ));
        }
        let mut report = self.query_world_box(frustum.broadphase_bounds, include_entities)?;
        report.maps.retain(|hit| {
            hit.bounds
                .map_or(true, |bounds| frustum.intersects_bounds(bounds))
        });
        report
            .entities
            .retain(|hit| frustum.contains_point(hit.entity.position));
        Ok(report)
    }

    fn world_candidate_keys(&self, bounds: GtaRpfWorldBounds) -> BTreeSet<u32> {
        let mut keys = self
            .world_global_maps
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let Some((min_x, max_x, min_y, max_y)) = world_cell_range(bounds) else {
            keys.extend(self.world_maps.keys().copied());
            return keys;
        };
        let count = cell_range_count(min_x, max_x, min_y, max_y);
        if count > WORLD_SPATIAL_MAX_QUERY_CELLS {
            keys.extend(self.world_maps.keys().copied());
            return keys;
        }

        for x in min_x..=max_x {
            for y in min_y..=max_y {
                if let Some(cell) = self.world_cells.get(&(x, y)) {
                    keys.extend(cell.iter().copied());
                }
            }
        }
        keys
    }

    fn rebuild_world_acceleration(&mut self) {
        let mut children = BTreeMap::<u32, Vec<u32>>::new();
        let mut cells = BTreeMap::<(i32, i32), Vec<u32>>::new();
        let mut global = Vec::<u32>::new();

        for (map_hash, winner) in &self.world_maps {
            let mut map_is_global = false;
            for record in &winner.candidates {
                if let Some(parent_hash) = record.parent_hash {
                    children.entry(parent_hash).or_default().push(*map_hash);
                }

                let Some(bounds) = record.effective_bounds() else {
                    map_is_global = true;
                    continue;
                };
                let Some((min_x, max_x, min_y, max_y)) = world_cell_range(bounds) else {
                    map_is_global = true;
                    continue;
                };
                if cell_range_count(min_x, max_x, min_y, max_y) > WORLD_SPATIAL_MAX_CELLS_PER_MAP {
                    map_is_global = true;
                    continue;
                }
                for x in min_x..=max_x {
                    for y in min_y..=max_y {
                        cells.entry((x, y)).or_default().push(*map_hash);
                    }
                }
            }
            if map_is_global {
                global.push(*map_hash);
            }
        }

        for values in children.values_mut() {
            values.sort_unstable();
            values.dedup();
        }
        for values in cells.values_mut() {
            values.sort_unstable();
            values.dedup();
        }
        global.sort_unstable();
        global.dedup();

        self.world_children = children;
        self.world_cells = cells;
        self.world_global_maps = global;
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
    parsed_ymaps: usize,
    parsed_gtxd_files: usize,
    world_entities: usize,
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
        let file_name_hash = joaat(stem);
        insert_file_winner(index, kind, file_name_hash, locator.clone());
        counters.indexed_files += 1;

        if kind == GtaRpfAssetKind::Ymap {
            let bytes = match archive.read_file(&file.path, Some(keys)) {
                Ok(bytes) => bytes,
                Err(error) => {
                    warnings.push(format!(
                        "{}: YMAP read failed: {error}",
                        provenance(&locator)
                    ));
                    continue;
                }
            };
            let ymap = match Ymap::from_bytes(&bytes) {
                Ok(ymap) => ymap,
                Err(error) => {
                    warnings.push(format!(
                        "{}: YMAP parse failed: {error}",
                        provenance(&locator)
                    ));
                    continue;
                }
            };
            counters.parsed_ymaps += 1;
            counters.world_entities += ymap.entities.len();
            let record = world_map_record(locator.clone(), file_name_hash, ymap);
            insert_world_map_winner(index, record.map_hash(), record);
            continue;
        }

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

        let ytyp_key = ytyp.name.map(|hash| hash.0).unwrap_or(file_name_hash);
        let mut dependencies = ytyp
            .dependencies
            .iter()
            .map(|hash| hash.0)
            .collect::<Vec<_>>();
        dependencies.sort_unstable();
        dependencies.dedup();
        let mut archetype_hashes = ytyp
            .archetypes
            .iter()
            .map(|archetype| archetype.name.0)
            .collect::<Vec<_>>();
        archetype_hashes.sort_unstable();
        archetype_hashes.dedup();
        insert_ytyp_winner(
            index,
            ytyp_key,
            GtaRpfYtypRecord {
                provider: locator.clone(),
                file_name_hash,
                name_hash: ytyp.name.map(|hash| hash.0),
                dependencies,
                archetype_hashes,
            },
        );

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

fn world_map_record(
    provider: GtaRpfIndexLocator,
    file_name_hash: u32,
    ymap: Ymap,
) -> GtaRpfWorldMapRecord {
    let mut physics_dictionary_hashes = ymap
        .physics_dictionaries
        .iter()
        .map(|hash| hash.0)
        .collect::<Vec<_>>();
    physics_dictionary_hashes.sort_unstable();
    physics_dictionary_hashes.dedup();

    let mut archetype_hashes = ymap
        .entities
        .iter()
        .map(|entity| entity.archetype_name.0)
        .collect::<Vec<_>>();
    archetype_hashes.sort_unstable();
    archetype_hashes.dedup();

    let entities = ymap
        .entities
        .into_iter()
        .enumerate()
        .map(|(index, entity)| GtaRpfWorldEntityRecord {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            archetype_hash: entity.archetype_name.0,
            position: entity.position.into(),
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
        .collect();

    GtaRpfWorldMapRecord {
        provider,
        file_name_hash,
        map_name_hash: ymap.name.map(|hash| hash.0),
        parent_hash: ymap.parent.map(|hash| hash.0),
        flags: ymap.flags,
        content_flags: ymap.content_flags,
        entities_bounds: ymap_bounds(ymap.entities_extents_min, ymap.entities_extents_max),
        streaming_bounds: ymap_bounds(ymap.streaming_extents_min, ymap.streaming_extents_max),
        physics_dictionary_hashes,
        archetype_hashes,
        entities,
    }
}

fn ymap_bounds(min: Option<YmapVec3>, max: Option<YmapVec3>) -> Option<GtaRpfWorldBounds> {
    let bounds = GtaRpfWorldBounds {
        min: min?.into(),
        max: max?.into(),
    };
    bounds.is_valid().then_some(bounds)
}

fn insert_world_map_winner(index: &mut GtaRpfAssetIndex, hash: u32, record: GtaRpfWorldMapRecord) {
    let load_rank = record.provider.load_rank;
    match index.world_maps.get_mut(&hash) {
        Some(winner) => winner.insert(load_rank, record),
        None => {
            index.world_maps.insert(
                hash,
                WorldMapWinnerSet {
                    load_rank,
                    candidates: vec![record],
                },
            );
        }
    }
}

fn insert_ytyp_winner(index: &mut GtaRpfAssetIndex, hash: u32, record: GtaRpfYtypRecord) {
    let load_rank = record.provider.load_rank;
    match index.ytyps.get_mut(&hash) {
        Some(winner) => winner.insert(load_rank, record),
        None => {
            index.ytyps.insert(
                hash,
                WinnerSet {
                    load_rank,
                    candidates: vec![record],
                },
            );
        }
    }
}

fn world_cell_range(bounds: GtaRpfWorldBounds) -> Option<(i32, i32, i32, i32)> {
    if !bounds.is_valid() {
        return None;
    }
    Some((
        world_cell_coordinate(bounds.min.x)?,
        world_cell_coordinate(bounds.max.x)?,
        world_cell_coordinate(bounds.min.y)?,
        world_cell_coordinate(bounds.max.y)?,
    ))
}

fn world_cell_coordinate(value: f32) -> Option<i32> {
    if !value.is_finite() {
        return None;
    }
    let cell = (f64::from(value) / f64::from(WORLD_SPATIAL_CELL_SIZE)).floor();
    if cell < f64::from(i32::MIN) || cell > f64::from(i32::MAX) {
        None
    } else {
        Some(cell as i32)
    }
}

fn cell_range_count(min_x: i32, max_x: i32, min_y: i32, max_y: i32) -> i64 {
    let width = i64::from(max_x) - i64::from(min_x) + 1;
    let height = i64::from(max_y) - i64::from(min_y) + 1;
    width.saturating_mul(height)
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

    fn world_bounds(min: [f32; 3], max: [f32; 3]) -> GtaRpfWorldBounds {
        GtaRpfWorldBounds {
            min: GtaRpfWorldPoint {
                x: min[0],
                y: min[1],
                z: min[2],
            },
            max: GtaRpfWorldPoint {
                x: max[0],
                y: max[1],
                z: max[2],
            },
        }
    }

    fn world_record(
        rank: u32,
        entry: &str,
        map_hash: u32,
        parent_hash: Option<u32>,
        bounds: Option<GtaRpfWorldBounds>,
        entity_position: Option<GtaRpfWorldPoint>,
    ) -> GtaRpfWorldMapRecord {
        let entities = entity_position
            .into_iter()
            .map(|position| GtaRpfWorldEntityRecord {
                index: 0,
                archetype_hash: 0xAABBCCDD,
                position,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale_xy: Some(1.0),
                scale_z: Some(1.0),
                flags: 7,
                parent_index: None,
            })
            .collect();
        GtaRpfWorldMapRecord {
            provider: locator(rank, entry),
            file_name_hash: map_hash,
            map_name_hash: Some(map_hash),
            parent_hash,
            flags: Some(1),
            content_flags: Some(2),
            entities_bounds: bounds,
            streaming_bounds: bounds,
            physics_dictionary_hashes: vec![0x01020304],
            archetype_hashes: vec![0xAABBCCDD],
            entities,
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
            ytyps: BTreeMap::new(),
            world_maps: BTreeMap::new(),
            world_children: BTreeMap::new(),
            world_cells: BTreeMap::new(),
            world_global_maps: Vec::new(),
        }
    }

    #[test]
    fn load_rejects_previous_world_index_schema() {
        let index = empty_index();
        let mut bytes = bincode::serialize(&index).expect("serialize world index");
        bytes[..4].copy_from_slice(&3_u32.to_le_bytes());

        let path = std::env::temp_dir().join(format!(
            "ragelab-world-index-old-schema-{}.bin",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        fs::write(&path, bytes).expect("write old-schema fixture");

        let error = GtaRpfAssetIndex::load(&path).expect_err("schema v3 must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("unsupported GTA RPF index schema 3; expected 4"));

        fs::remove_file(path).expect("remove old-schema fixture");
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
            ytyps: BTreeMap::new(),
            world_maps: BTreeMap::new(),
            world_children: BTreeMap::new(),
            world_cells: BTreeMap::new(),
            world_global_maps: Vec::new(),
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
    fn world_map_winner_and_grid_follow_load_rank() {
        let mut index = empty_index();
        insert_world_map_winner(
            &mut index,
            0x1000,
            world_record(
                10,
                "old.ymap",
                0x1000,
                None,
                Some(world_bounds([0.0, 0.0, 0.0], [100.0, 100.0, 100.0])),
                Some(GtaRpfWorldPoint {
                    x: 50.0,
                    y: 50.0,
                    z: 10.0,
                }),
            ),
        );
        insert_world_map_winner(
            &mut index,
            0x1000,
            world_record(
                20,
                "new.ymap",
                0x1000,
                None,
                Some(world_bounds([1000.0, 1000.0, 0.0], [1100.0, 1100.0, 100.0])),
                Some(GtaRpfWorldPoint {
                    x: 1050.0,
                    y: 1050.0,
                    z: 10.0,
                }),
            ),
        );
        index.rebuild_world_acceleration();

        let old = index
            .query_world_box(
                world_bounds([-10.0, -10.0, -10.0], [120.0, 120.0, 120.0]),
                true,
            )
            .expect("old area query");
        assert!(old.maps.is_empty());
        assert!(old.entities.is_empty());

        let new = index
            .query_world_box(
                world_bounds([990.0, 990.0, -10.0], [1110.0, 1110.0, 120.0]),
                true,
            )
            .expect("new area query");
        assert_eq!(new.maps.len(), 1);
        assert_eq!(new.maps[0].provider.entry, "new.ymap");
        assert_eq!(new.entities.len(), 1);
    }

    #[test]
    fn world_hierarchy_and_radius_query_are_persisted() {
        let mut index = empty_index();
        insert_world_map_winner(
            &mut index,
            0x2000,
            world_record(
                20,
                "child.ymap",
                0x2000,
                Some(0x1000),
                Some(world_bounds([0.0, 0.0, 0.0], [200.0, 200.0, 100.0])),
                Some(GtaRpfWorldPoint {
                    x: 25.0,
                    y: 30.0,
                    z: 5.0,
                }),
            ),
        );
        index.rebuild_world_acceleration();
        assert_eq!(index.world_children(0x1000), &[0x2000]);

        let radius = index
            .query_world_radius(
                GtaRpfWorldPoint {
                    x: 25.0,
                    y: 30.0,
                    z: 5.0,
                },
                2.0,
                true,
            )
            .expect("radius query");
        assert_eq!(radius.maps.len(), 1);
        assert_eq!(radius.entities.len(), 1);

        let path =
            std::env::temp_dir().join(format!("ragelab-world-index-v4-{}.bin", std::process::id()));
        let _ = fs::remove_file(&path);
        index.save(&path, true).expect("save world index");
        let loaded = GtaRpfAssetIndex::load(&path).expect("load world index");
        assert_eq!(loaded.world_children(0x1000), &[0x2000]);
        assert_eq!(
            loaded
                .query_world_radius(
                    GtaRpfWorldPoint {
                        x: 25.0,
                        y: 30.0,
                        z: 5.0,
                    },
                    2.0,
                    true,
                )
                .expect("loaded radius query")
                .entities
                .len(),
            1
        );
        fs::remove_file(path).expect("remove world index fixture");
    }

    #[test]
    fn world_frustum_filters_box_candidates_without_rpf_io() {
        let mut index = empty_index();
        insert_world_map_winner(
            &mut index,
            0x3000,
            world_record(
                20,
                "inside.ymap",
                0x3000,
                None,
                Some(world_bounds([-5.0, -5.0, -5.0], [5.0, 5.0, 5.0])),
                Some(GtaRpfWorldPoint {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                }),
            ),
        );
        insert_world_map_winner(
            &mut index,
            0x4000,
            world_record(
                20,
                "outside.ymap",
                0x4000,
                None,
                Some(world_bounds([50.0, 50.0, -5.0], [60.0, 60.0, 5.0])),
                Some(GtaRpfWorldPoint {
                    x: 55.0,
                    y: 55.0,
                    z: 0.0,
                }),
            ),
        );
        index.rebuild_world_acceleration();

        let frustum = GtaRpfWorldFrustum {
            broadphase_bounds: world_bounds([-100.0, -100.0, -100.0], [100.0, 100.0, 100.0]),
            planes: [
                GtaRpfWorldPlane {
                    normal: GtaRpfWorldPoint {
                        x: 1.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    distance: 10.0,
                },
                GtaRpfWorldPlane {
                    normal: GtaRpfWorldPoint {
                        x: -1.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    distance: 10.0,
                },
                GtaRpfWorldPlane {
                    normal: GtaRpfWorldPoint {
                        x: 0.0,
                        y: 1.0,
                        z: 0.0,
                    },
                    distance: 10.0,
                },
                GtaRpfWorldPlane {
                    normal: GtaRpfWorldPoint {
                        x: 0.0,
                        y: -1.0,
                        z: 0.0,
                    },
                    distance: 10.0,
                },
                GtaRpfWorldPlane {
                    normal: GtaRpfWorldPoint {
                        x: 0.0,
                        y: 0.0,
                        z: 1.0,
                    },
                    distance: 10.0,
                },
                GtaRpfWorldPlane {
                    normal: GtaRpfWorldPoint {
                        x: 0.0,
                        y: 0.0,
                        z: -1.0,
                    },
                    distance: 10.0,
                },
            ],
        };
        let report = index
            .query_world_frustum(frustum, true)
            .expect("frustum query");
        assert_eq!(report.maps.len(), 1);
        assert_eq!(report.maps[0].map_hash, 0x3000);
        assert_eq!(report.entities.len(), 1);
        assert_eq!(report.entities[0].map_hash, 0x3000);
    }

    #[test]
    fn ytyp_winner_preserves_dependency_relationships() {
        let mut index = empty_index();
        insert_ytyp_winner(
            &mut index,
            0x9000,
            GtaRpfYtypRecord {
                provider: locator(10, "old.ytyp"),
                file_name_hash: 0x9000,
                name_hash: Some(0x9000),
                dependencies: vec![1],
                archetype_hashes: vec![2],
            },
        );
        insert_ytyp_winner(
            &mut index,
            0x9000,
            GtaRpfYtypRecord {
                provider: locator(30, "new.ytyp"),
                file_name_hash: 0x9000,
                name_hash: Some(0x9000),
                dependencies: vec![3, 4],
                archetype_hashes: vec![5, 6],
            },
        );
        let candidates = index.ytyp_candidates(0x9000);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provider.entry, "new.ytyp");
        assert_eq!(candidates[0].dependencies, vec![3, 4]);
        assert_eq!(candidates[0].archetype_hashes, vec![5, 6]);
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
