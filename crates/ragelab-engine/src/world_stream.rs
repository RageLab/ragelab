//! Camera-driven streaming over the persistent GTA Legacy world index.
//!
//! The runtime never scans the game installation or assembles the full world.
//! It loads the persistent index once, queries nearby winning YMAP providers,
//! decodes only desired chunks, keeps a bounded CPU chunk cache, and shares
//! active renderer-neutral `RenderPackage` chunks without monolithic merging.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    path::Path,
    sync::Arc,
    thread,
    time::Instant,
};

use ragelab_assets::WorkspaceIndex;
use ragelab_ymap::Ymap;
use serde::Serialize;

use crate::{
    assemble_ymap_scene, build_world_render_package, mount_loaded_game_index_for_archetypes,
    GtaRpfArchetypeBrowserResolution, GtaRpfAssetIndex, GtaRpfIndexLocator, GtaRpfWorldBounds,
    GtaRpfWorldEntityRecord, GtaRpfWorldFrustum, GtaRpfWorldMapHit, GtaRpfWorldMapRecord,
    GtaRpfWorldPoint, RenderPackage, RenderPackageOptions, RenderPackageSummary,
    SceneAssemblyOptions, SceneGameIndexSource, SceneManifest, SceneResolutionState,
};

pub const WORLD_STREAM_SCHEMA_VERSION: u32 = 7;
const PREFETCH_CHUNK_LOADS_PER_UPDATE: usize = 1;
const WORLD_STREAM_MAX_ASSETS_PER_SUBCHUNK: usize = 192;
const WORLD_STREAM_SUBCHUNK_RADIUS_FACTOR: f32 = 0.4;
const WORLD_STREAM_TILE_STEP_FACTOR: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldStreamConfig {
    pub load_radius: f32,
    pub retain_radius: f32,
    pub max_active_maps: usize,
    pub max_chunk_loads_per_update: usize,
    pub max_cpu_chunks: usize,
    pub max_cpu_bytes: u64,
    pub scene_options: SceneAssemblyOptions,
    pub render_options: RenderPackageOptions,
}

impl Default for WorldStreamConfig {
    fn default() -> Self {
        let mut render_options = RenderPackageOptions::default();
        render_options.preview.max_primitives = 128;
        render_options.preview.max_vertices = 50_000;
        render_options.preview.max_indices = 150_000;
        render_options.max_assets = WORLD_STREAM_MAX_ASSETS_PER_SUBCHUNK;

        Self {
            load_radius: 1_150.0,
            retain_radius: 1_500.0,
            max_active_maps: 20,
            max_chunk_loads_per_update: 2,
            max_cpu_chunks: 48,
            max_cpu_bytes: 768 * 1024 * 1024,
            scene_options: SceneAssemblyOptions::default(),
            render_options,
        }
    }
}

impl WorldStreamConfig {
    pub fn validate(self) -> Result<Self, io::Error> {
        if !self.load_radius.is_finite() || self.load_radius <= 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world stream load_radius must be finite and greater than zero",
            ));
        }
        if !self.retain_radius.is_finite() || self.retain_radius < self.load_radius {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world stream retain_radius must be finite and >= load_radius",
            ));
        }
        if self.max_active_maps == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world stream max_active_maps must be greater than zero",
            ));
        }
        if self.max_chunk_loads_per_update == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world stream max_chunk_loads_per_update must be greater than zero",
            ));
        }
        if self.max_cpu_chunks < self.max_active_maps {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world stream max_cpu_chunks must be >= max_active_maps",
            ));
        }
        if self.max_cpu_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world stream max_cpu_bytes must be greater than zero",
            ));
        }
        self.render_options.validate()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldStreamView {
    pub position: GtaRpfWorldPoint,
    pub frustum: Option<GtaRpfWorldFrustum>,
}

impl WorldStreamView {
    pub const fn at(position: GtaRpfWorldPoint) -> Self {
        Self {
            position,
            frustum: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStreamChunkKey {
    pub map_hash: u32,
    pub provider: GtaRpfIndexLocator,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStreamChunkError {
    pub chunk: WorldStreamChunkKey,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStreamActiveEntity {
    pub entity: GtaRpfWorldEntityRecord,
    pub render_node_index: Option<u32>,
    pub resolution: Option<GtaRpfArchetypeBrowserResolution>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStreamActiveMap {
    pub map_hash: u32,
    pub provider: GtaRpfIndexLocator,
    pub parent_hash: Option<u32>,
    pub flags: Option<u32>,
    pub content_flags: Option<u32>,
    pub bounds: Option<GtaRpfWorldBounds>,
    pub entities: Vec<WorldStreamActiveEntity>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStreamChunkLoadMetric {
    pub chunk: WorldStreamChunkKey,
    pub prefetched: bool,
    pub elapsed_ms: f64,
    pub read_ms: f64,
    pub parse_ms: f64,
    pub assemble_ms: f64,
    pub render_package_ms: f64,
    pub encoded_bytes: u64,
    pub instances: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStreamReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub tick: u64,
    pub position: GtaRpfWorldPoint,
    pub load_radius: f32,
    pub retain_radius: f32,
    pub candidate_map_keys: usize,
    pub candidate_maps: usize,
    pub visible_maps: usize,
    pub active_maps: usize,
    pub active_entities: usize,
    pub hd_lod_instances: usize,
    pub proxy_lod_instances: usize,
    pub unknown_lod_instances: usize,
    pub spatial_omitted_nodes: usize,
    pub spatial_omitted_assets: usize,
    pub render_omitted_assets: usize,
    pub render_omitted_geometry: usize,
    pub active_chunks: Vec<WorldStreamActiveMap>,
    pub overlay_packages: usize,
    pub overlay_suppressed_maps: usize,
    pub ambiguous_maps: usize,
    pub unknown_bounds_maps: usize,
    pub deferred_parent_maps: usize,
    pub lod_deferred_entities: usize,
    pub loaded_chunks: usize,
    pub prefetched_chunks: usize,
    pub chunk_loads: Vec<WorldStreamChunkLoadMetric>,
    pub pending_chunks: usize,
    pub pending_pages: usize,
    pub failed_pages: usize,
    pub compiled_pages: usize,
    pub prefetch_pending: usize,
    pub unloaded_chunks: usize,
    pub cache_hits: usize,
    pub cached_tile_swaps: usize,
    pub cached_retired_tiles: usize,
    pub cache_misses: usize,
    pub cumulative_cache_hits: u64,
    pub cumulative_cache_misses: u64,
    pub cpu_cached_chunks: usize,
    pub cpu_cached_bytes: u64,
    pub cpu_evictions: usize,
    pub cpu_budget_overflow: bool,
    pub render_budget_drops: usize,
    pub merged_summary: Option<RenderPackageSummary>,
    pub batch_warnings: Vec<String>,
    pub load_errors: Vec<WorldStreamChunkError>,
}

#[derive(Debug)]
pub struct WorldStreamUpdate {
    pub packages: Option<Vec<Arc<RenderPackage>>>,
    pub report: WorldStreamReport,
}

#[derive(Debug)]
struct CachedChunk {
    packages: Vec<Arc<RenderPackage>>,
    pending_pages: VecDeque<SceneManifest>,
    page_error: Option<String>,
    bytes: u64,
    read_ms: f64,
    parse_ms: f64,
    assemble_ms: f64,
    render_package_ms: f64,
    build_position: GtaRpfWorldPoint,
    spatialized: bool,
    spatial_omitted_nodes: usize,
    spatial_omitted_assets: usize,
    render_omitted_assets: usize,
    render_omitted_geometry: usize,
    last_used_tick: u64,
}

#[derive(Debug, Clone)]
struct Candidate {
    key: WorldStreamChunkKey,
    bounds: GtaRpfWorldBounds,
    distance_squared: f32,
    visible: bool,
    entity_count: usize,
    mlo_roots: usize,
    parent_hash: Option<u32>,
    flags: Option<u32>,
    content_flags: Option<u32>,
}

fn candidate_locality_class(candidate: &Candidate, retain_radius: f32) -> u8 {
    if candidate.entity_count == 0 {
        return 3;
    }

    // MLO roots can expand a single YMAP entity into a large interior graph.
    // Exterior world streaming must fill nearby ordinary map geometry first;
    // interiors remain eligible only when no higher-priority exterior chunks
    // need the residency slot.
    if candidate.mlo_roots > 0 {
        return 2;
    }

    let span_x = (candidate.bounds.max.x - candidate.bounds.min.x).abs();
    let span_y = (candidate.bounds.max.y - candidate.bounds.min.y).abs();
    let macro_span = (retain_radius * 4.0).max(1_500.0);
    if span_x.max(span_y) > macro_span {
        1
    } else {
        0
    }
}

// GTA map bounds often overlap by kilometers. A local *_long_* YMAP can
// contain the camera but still be a billboard/LOD proxy. Prefer STRM
// detail while preserving parent chains as fallbacks until child pages load.
fn world_map_detail_tier(entry: &str) -> u8 {
    let name = entry.rsplit(['/', '\\']).next().unwrap_or(entry);
    let name = name.to_ascii_lowercase();
    if name.contains("_strm_") {
        0
    } else if name.contains("_critical_") {
        1
    } else if name.contains("_long_")
        || name.contains("_slod")
        || name.ends_with("_lod.ymap")
        || name.contains("_lod_")
    {
        3
    } else {
        2
    }
}

fn candidate_detail_tier(candidate: &Candidate) -> u8 {
    world_map_detail_tier(&candidate.key.provider.entry)
}

fn select_active_chunk_load_batch(
    missing: &[WorldStreamChunkKey],
    parent_keys: &BTreeSet<WorldStreamChunkKey>,
    max_loads: usize,
) -> Vec<WorldStreamChunkKey> {
    let mut result = Vec::with_capacity(max_loads.min(missing.len()));
    if max_loads >= 2 {
        // Give the renderer a cheap LOD fallback AND an HD leaf in the
        // same update, rather than loading all parents before any detail.
        if let Some(parent) = missing.iter().find(|key| parent_keys.contains(*key)) {
            result.push(parent.clone());
        }
        if let Some(detail) = missing.iter().find(|key| {
            !parent_keys.contains(*key) && world_map_detail_tier(&key.provider.entry) <= 1
        }) {
            result.push(detail.clone());
        }
    }
    for key in missing {
        if result.len() == max_loads {
            break;
        }
        if !result.contains(key) {
            result.push(key.clone());
        }
    }
    result
}

fn candidate_variant_key(candidate: &Candidate) -> [u64; 9] {
    [
        u64::from(candidate.bounds.min.x.to_bits()),
        u64::from(candidate.bounds.min.y.to_bits()),
        u64::from(candidate.bounds.min.z.to_bits()),
        u64::from(candidate.bounds.max.x.to_bits()),
        u64::from(candidate.bounds.max.y.to_bits()),
        u64::from(candidate.bounds.max.z.to_bits()),
        candidate.entity_count as u64,
        u64::from(candidate.flags.unwrap_or(u32::MAX)),
        u64::from(candidate.content_flags.unwrap_or(u32::MAX)),
    ]
}

fn dedupe_spatial_variants(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut winners = BTreeMap::<[u64; 9], Candidate>::new();
    for candidate in candidates {
        let key = candidate_variant_key(&candidate);
        match winners.get_mut(&key) {
            Some(current) => {
                let candidate_rank = candidate.key.provider.load_rank;
                let current_rank = current.key.provider.load_rank;
                if candidate_rank > current_rank
                    || (candidate_rank == current_rank && candidate.key < current.key)
                {
                    *current = candidate;
                }
            }
            None => {
                winners.insert(key, candidate);
            }
        }
    }
    winners.into_values().collect()
}

fn aggregate_render_package_summary(
    packages: &[Arc<RenderPackage>],
) -> Option<RenderPackageSummary> {
    let mut summaries = packages.iter().map(|package| package.descriptor.summary);
    let first = summaries.next()?;
    Some(summaries.fold(first, |mut total, summary| {
        total.instances = total.instances.saturating_add(summary.instances);
        total.assets = total.assets.saturating_add(summary.assets);
        total.ready_assets = total.ready_assets.saturating_add(summary.ready_assets);
        total.meshes = total.meshes.saturating_add(summary.meshes);
        total.materials = total.materials.saturating_add(summary.materials);
        total.textures = total.textures.saturating_add(summary.textures);
        total.blob_bytes = total.blob_bytes.saturating_add(summary.blob_bytes);
        total
    }))
}

fn activate_cached_spatial_tile(
    cache: &mut BTreeMap<WorldStreamChunkKey, CachedChunk>,
    retired: &mut BTreeMap<RetiredSpatialChunkKey, CachedChunk>,
    key: &WorldStreamChunkKey,
    position: GtaRpfWorldPoint,
    config: WorldStreamConfig,
    tick: u64,
) -> Option<bool> {
    let cached = cache.get_mut(key)?;
    if !cached_chunk_needs_recenter(cached, position, config) {
        cached.last_used_tick = tick;
        return Some(false);
    }
    let target = spatial_subchunk_tile_center(position, config);
    let retired_key = retired_spatial_chunk_key(key, target);
    let replacement = retired.remove(&retired_key)?;
    let outgoing = std::mem::replace(cached, replacement);
    retired.insert(
        retired_spatial_chunk_key(key, outgoing.build_position),
        outgoing,
    );
    cached.last_used_tick = tick;
    Some(true)
}

fn keep_loaded_parent_fallbacks(
    order: &mut Vec<WorldStreamChunkKey>,
    fallback_parents: &[WorldStreamChunkKey],
    direct_parent_hashes: &BTreeMap<WorldStreamChunkKey, Option<u32>>,
    protected_parents: &BTreeSet<WorldStreamChunkKey>,
    max_active_maps: usize,
) -> usize {
    let mut retained = 0;
    for parent in fallback_parents {
        if order.contains(parent) {
            continue;
        }
        if order.len() < max_active_maps {
            order.push(parent.clone());
            retained += 1;
            continue;
        }
        // Never evict unrelated geography to keep a LOD fallback. If the
        // residency budget is full, the fallback can replace only a child
        // in the same hierarchy branch, which the parent itself covers.
        if let Some(index) = order.iter().rposition(|key| {
            !protected_parents.contains(key)
                && direct_parent_hashes.get(key).copied().flatten() == Some(parent.map_hash)
        }) {
            order.remove(index);
            order.push(parent.clone());
            retained += 1;
        }
    }
    retained
}

fn detail_map_budget(max_active_maps: usize) -> usize {
    max_active_maps
        .saturating_mul(2)
        .saturating_div(3)
        .max(4)
        .min(max_active_maps)
}

fn spatial_subchunk_radius(config: WorldStreamConfig) -> f32 {
    (config.load_radius * WORLD_STREAM_SUBCHUNK_RADIUS_FACTOR).max(120.0)
}

type RetiredSpatialChunkKey = (WorldStreamChunkKey, u32, u32);

fn retired_spatial_chunk_key(
    key: &WorldStreamChunkKey,
    build_position: GtaRpfWorldPoint,
) -> RetiredSpatialChunkKey {
    (
        key.clone(),
        build_position.x.to_bits(),
        build_position.y.to_bits(),
    )
}

fn spatial_subchunk_tile_center(
    position: GtaRpfWorldPoint,
    config: WorldStreamConfig,
) -> GtaRpfWorldPoint {
    let tile_size = spatial_subchunk_radius(config) * WORLD_STREAM_TILE_STEP_FACTOR;
    GtaRpfWorldPoint {
        x: (position.x / tile_size).round() * tile_size,
        y: (position.y / tile_size).round() * tile_size,
        z: position.z,
    }
}

fn cached_chunk_needs_recenter(
    chunk: &CachedChunk,
    position: GtaRpfWorldPoint,
    config: WorldStreamConfig,
) -> bool {
    if !chunk.spatialized {
        return false;
    }
    let tile_center = spatial_subchunk_tile_center(position, config);
    chunk.build_position.x != tile_center.x || chunk.build_position.y != tile_center.y
}

// Every node belongs to exactly one page. Page order is camera-centric, but
// page identity is stable inside the world tile because the center is snapped.
fn paginate_world_manifest(
    manifest: SceneManifest,
    center: GtaRpfWorldPoint,
    max_assets: usize,
) -> Result<VecDeque<SceneManifest>, io::Error> {
    if manifest.nodes.is_empty() || manifest.assets.len() <= max_assets {
        return Ok(VecDeque::from([manifest]));
    }
    // MLO internals hold cross-node room/portal indices; splitting those is
    // unsafe until the interior graph has a dedicated pager.
    if !manifest.interiors.is_empty() {
        if manifest.assets.len() > crate::HARD_RENDER_MAX_ASSETS {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "world MLO exceeds hard asset limit; an interior-aware pager is required",
            ));
        }
        return Ok(VecDeque::from([manifest]));
    }

    let mut ranked_nodes = (0..manifest.nodes.len()).collect::<Vec<_>>();
    ranked_nodes.sort_by(|&left, &right| {
        let distance = |index: usize| {
            manifest.nodes[index]
                .transform
                .map_or(f32::INFINITY, |transform| {
                    let dx = transform.translation[0] - center.x;
                    let dy = transform.translation[1] - center.y;
                    dx * dx + dy * dy
                })
        };
        distance(left)
            .total_cmp(&distance(right))
            .then_with(|| left.cmp(&right))
    });

    let mut page_indices = Vec::<Vec<usize>>::new();
    let mut current_indices = Vec::<usize>::new();
    let mut current_assets = BTreeSet::new();

    for index in ranked_nodes {
        let node = &manifest.nodes[index];
        let mut required_assets = BTreeSet::new();
        if let Some(reference) = node.asset_ref {
            required_assets.insert(reference);
        }
        if let Some(reference) = node.collision.as_ref().and_then(|c| c.asset_ref) {
            required_assets.insert(reference);
        }
        if required_assets.len() > max_assets {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "world entity references more assets than fit in one render page",
            ));
        }
        let additional = required_assets.difference(&current_assets).count();
        if !current_indices.is_empty()
            && current_assets.len().saturating_add(additional) > max_assets
        {
            page_indices.push(std::mem::take(&mut current_indices));
            current_assets.clear();
        }
        current_indices.push(index);
        current_assets.extend(required_assets);
    }
    if !current_indices.is_empty() {
        page_indices.push(current_indices);
    }

    let mut pages = VecDeque::with_capacity(page_indices.len());
    for (page_index, indices) in page_indices.into_iter().enumerate() {
        let selected_assets = indices
            .iter()
            .flat_map(|&index| {
                let node = &manifest.nodes[index];
                [
                    node.asset_ref,
                    node.collision
                        .as_ref()
                        .and_then(|collision| collision.asset_ref),
                ]
                .into_iter()
                .flatten()
            })
            .collect::<BTreeSet<_>>();
        let mut asset_mapping = BTreeMap::new();
        let mut assets = Vec::with_capacity(selected_assets.len());
        for asset in &manifest.assets {
            if selected_assets.contains(&asset.id) {
                let mut asset = asset.clone();
                asset_mapping.insert(asset.id, assets.len());
                asset.id = assets.len();
                assets.push(asset);
            }
        }
        if assets.len() != selected_assets.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "world manifest page references an unknown asset",
            ));
        }

        let mut nodes = Vec::with_capacity(indices.len());
        for index in indices {
            let mut node = manifest.nodes[index].clone();
            node.index = nodes.len();
            node.asset_ref = node.asset_ref.map(|id| asset_mapping[&id]);
            if let Some(collision) = node.collision.as_mut() {
                collision.asset_ref = collision.asset_ref.map(|id| asset_mapping[&id]);
            }
            nodes.push(node);
        }
        let resolved = nodes
            .iter()
            .filter(|node| node.resolution == SceneResolutionState::Resolved)
            .count();
        let mut root = manifest.root.clone();
        root.path =
            std::path::PathBuf::from(format!("{}#world-page-{}", root.path.display(), page_index));
        pages.push_back(SceneManifest {
            schema_version: manifest.schema_version,
            root,
            summary: crate::SceneSummary {
                total_entities: nodes.len(),
                emitted_nodes: nodes.len(),
                resolved_nodes: resolved,
                unresolved_nodes: nodes.len().saturating_sub(resolved),
                asset_references: assets.len(),
            },
            nodes,
            interiors: Vec::new(),
            assets,
            warnings: manifest.warnings.clone(),
            limits: manifest.limits,
        });
    }
    Ok(pages)
}

fn estimated_page_bytes(page: &SceneManifest) -> u64 {
    let node_bytes = page
        .nodes
        .len()
        .saturating_mul(std::mem::size_of::<crate::SceneNode>());
    let asset_bytes = page
        .assets
        .len()
        .saturating_mul(std::mem::size_of::<crate::SceneAssetReference>());
    u64::try_from(node_bytes.saturating_add(asset_bytes).saturating_mul(2)).unwrap_or(u64::MAX)
}

fn estimated_pending_page_bytes(pages: &VecDeque<SceneManifest>) -> u64 {
    pages
        .iter()
        .map(estimated_page_bytes)
        .fold(0_u64, u64::saturating_add)
}

fn package_render_omissions(package: &RenderPackage) -> (usize, usize) {
    let mut assets = 0;
    let mut geometry = 0;
    for diagnostic in &package.descriptor.diagnostics {
        match diagnostic.code.as_str() {
            "assetBudgetExceeded" => assets += 1,
            "geometryBudgetExceeded" => geometry += 1,
            _ => {}
        }
    }
    (assets, geometry)
}

fn compile_pending_world_page(
    chunk: &mut CachedChunk,
    options: RenderPackageOptions,
) -> Result<bool, io::Error> {
    let Some(page) = chunk.pending_pages.pop_front() else {
        return Ok(false);
    };
    let started = Instant::now();
    let focus = chunk.build_position;
    let result = build_world_render_package(&page, options, [focus.x, focus.y, focus.z]);
    let package = match result {
        Ok(package) => package,
        Err(error) => {
            chunk.pending_pages.push_front(page);
            return Err(error);
        }
    };
    let (omitted_assets, omitted_geometry) = package_render_omissions(&package);
    chunk.bytes = chunk
        .bytes
        .saturating_sub(estimated_page_bytes(&page))
        .saturating_add(package.report().encoded_bytes);
    chunk.spatial_omitted_nodes = chunk.spatial_omitted_nodes.saturating_sub(page.nodes.len());
    chunk.spatial_omitted_assets = chunk
        .spatial_omitted_assets
        .saturating_sub(page.assets.len());
    chunk.render_omitted_assets = chunk.render_omitted_assets.saturating_add(omitted_assets);
    chunk.render_omitted_geometry = chunk
        .render_omitted_geometry
        .saturating_add(omitted_geometry);
    chunk.render_package_ms += started.elapsed().as_secs_f64() * 1000.0;
    chunk.packages.push(Arc::new(package));
    Ok(true)
}

fn candidate_center_distance_squared(candidate: &Candidate, position: GtaRpfWorldPoint) -> f32 {
    let Some(center) = candidate.bounds.center() else {
        return f32::INFINITY;
    };
    let dx = center.x - position.x;
    let dy = center.y - position.y;
    let dz = center.z - position.z;
    dx * dx + dy * dy + dz * dz
}

pub struct WorldStreamingRuntime {
    source: SceneGameIndexSource,
    index: GtaRpfAssetIndex,
    config: WorldStreamConfig,
    tick: u64,
    cache: BTreeMap<WorldStreamChunkKey, CachedChunk>,
    retired_tile_cache: BTreeMap<RetiredSpatialChunkKey, CachedChunk>,
    resolver: WorkspaceIndex,
    mounted_archetypes: BTreeSet<u32>,
    mlo_root_cache: BTreeMap<WorldStreamChunkKey, usize>,
    archetype_resolution_cache: BTreeMap<u32, Option<GtaRpfArchetypeBrowserResolution>>,
    prefetch_failures: BTreeSet<WorldStreamChunkKey>,
    last_view_position: Option<GtaRpfWorldPoint>,
    prefetch_direction_xy: Option<[f32; 2]>,
    active: BTreeSet<WorldStreamChunkKey>,
    overlay_packages: Vec<Arc<RenderPackage>>,
    overlay_map_hashes: BTreeSet<u32>,
    render_package_dirty: bool,
    last_merged_summary: Option<RenderPackageSummary>,
    cumulative_cache_hits: u64,
    cumulative_cache_misses: u64,
}

impl WorldStreamingRuntime {
    pub fn open(
        source: SceneGameIndexSource,
        config: WorldStreamConfig,
    ) -> Result<Self, io::Error> {
        let config = config.validate()?;
        validate_source_paths(&source)?;
        let index = GtaRpfAssetIndex::load(&source.index)?;
        if !index.matches_installation(&source.game_root)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "world stream game index is stale for {}; rebuild {}",
                    source.game_root.display(),
                    source.index.display()
                ),
            ));
        }

        let resolver = WorkspaceIndex::empty(source.game_root.clone());

        Ok(Self {
            source,
            index,
            config,
            tick: 0,
            cache: BTreeMap::new(),
            retired_tile_cache: BTreeMap::new(),
            resolver,
            mounted_archetypes: BTreeSet::new(),
            mlo_root_cache: BTreeMap::new(),
            archetype_resolution_cache: BTreeMap::new(),
            prefetch_failures: BTreeSet::new(),
            last_view_position: None,
            prefetch_direction_xy: None,
            active: BTreeSet::new(),
            overlay_packages: Vec::new(),
            overlay_map_hashes: BTreeSet::new(),
            render_package_dirty: true,
            last_merged_summary: None,
            cumulative_cache_hits: 0,
            cumulative_cache_misses: 0,
        })
    }

    pub fn index(&self) -> &GtaRpfAssetIndex {
        &self.index
    }

    pub const fn config(&self) -> WorldStreamConfig {
        self.config
    }

    fn record_mlo_roots(&self, record: &GtaRpfWorldMapRecord) -> usize {
        record
            .entities
            .iter()
            .filter(|entity| !self.index.mlo_candidates(entity.archetype_hash).is_empty())
            .count()
    }

    fn cached_mlo_roots(
        &mut self,
        key: &WorldStreamChunkKey,
        bounds: GtaRpfWorldBounds,
        entity_count: usize,
    ) -> usize {
        if entity_count == 0 {
            return 0;
        }
        let span_x = (bounds.max.x - bounds.min.x).abs();
        let span_y = (bounds.max.y - bounds.min.y).abs();
        let macro_span = (self.config.retain_radius * 4.0).max(1_500.0);
        if span_x.max(span_y) > macro_span {
            return 0;
        }
        if let Some(value) = self.mlo_root_cache.get(key) {
            return *value;
        }

        let value = self
            .index
            .world_map_candidates(key.map_hash)
            .iter()
            .find(|record| record.provider == key.provider)
            .map(|record| self.record_mlo_roots(record))
            .unwrap_or(0);
        self.mlo_root_cache.insert(key.clone(), value);
        value
    }

    fn cached_archetype_resolution(
        &mut self,
        archetype_hash: u32,
    ) -> Option<GtaRpfArchetypeBrowserResolution> {
        if let Some(value) = self.archetype_resolution_cache.get(&archetype_hash) {
            return value.clone();
        }
        let value = self.index.archetype_browser_resolution(archetype_hash);
        self.archetype_resolution_cache
            .insert(archetype_hash, value.clone());
        value
    }

    fn planned_keys_at(
        &mut self,
        position: GtaRpfWorldPoint,
    ) -> Result<Vec<WorldStreamChunkKey>, io::Error> {
        let query = self
            .index
            .query_world_radius(position, self.config.retain_radius, false)?;

        let mut grouped = BTreeMap::<u32, Vec<GtaRpfWorldMapHit>>::new();
        for hit in &query.maps {
            grouped.entry(hit.map_hash).or_default().push(hit.clone());
        }

        let load_radius_squared = self.config.load_radius * self.config.load_radius;
        let mut candidates = Vec::new();
        for (map_hash, hits) in grouped {
            if self.overlay_map_hashes.contains(&map_hash) || hits.len() != 1 {
                continue;
            }
            let hit = &hits[0];
            let Some(bounds) = hit.bounds else {
                continue;
            };
            let distance_squared = bounds.distance_squared_to_point(position);
            if distance_squared > load_radius_squared {
                continue;
            }
            let key = WorldStreamChunkKey {
                map_hash,
                provider: hit.provider.clone(),
            };
            let mlo_roots = self.cached_mlo_roots(&key, bounds, hit.entity_count);
            candidates.push(Candidate {
                visible: true,
                key,
                bounds,
                distance_squared,
                entity_count: hit.entity_count,
                mlo_roots,
                parent_hash: hit.parent_hash,
                flags: hit.flags,
                content_flags: hit.content_flags,
            });
        }

        candidates = dedupe_spatial_variants(candidates);
        candidates.sort_by(|left, right| {
            candidate_locality_class(left, self.config.retain_radius)
                .cmp(&candidate_locality_class(right, self.config.retain_radius))
                .then_with(|| candidate_detail_tier(left).cmp(&candidate_detail_tier(right)))
                .then_with(|| {
                    candidate_center_distance_squared(left, position)
                        .total_cmp(&candidate_center_distance_squared(right, position))
                })
                .then_with(|| left.distance_squared.total_cmp(&right.distance_squared))
                .then_with(|| left.key.cmp(&right.key))
        });

        let hierarchy_map_hashes = candidates
            .iter()
            .filter_map(|candidate| candidate.parent_hash)
            .collect::<BTreeSet<_>>();
        let detail_budget = detail_map_budget(self.config.max_active_maps);
        let mut selected_detail_maps = 0_usize;
        let mut selected = BTreeSet::new();
        let mut parent_keys = BTreeSet::new();
        let mut hierarchy_candidates = Vec::new();

        for candidate in &candidates {
            if selected.contains(&candidate.key) {
                continue;
            }

            let is_detail_leaf = candidate_locality_class(candidate, self.config.retain_radius)
                == 0
                && candidate_detail_tier(candidate) <= 1
                && !hierarchy_map_hashes.contains(&candidate.key.map_hash);
            if is_detail_leaf && selected_detail_maps >= detail_budget {
                continue;
            }

            let mut chain = vec![candidate.clone()];
            let mut parent_hash = candidate.parent_hash;
            let mut depth = 0_u8;
            let mut chain_complete = true;
            while let Some(parent) = parent_hash {
                if depth >= 16 {
                    chain_complete = false;
                    break;
                }
                depth += 1;
                if self.overlay_map_hashes.contains(&parent) {
                    break;
                }

                let parent_candidates = self.index.world_map_candidates(parent);
                if parent_candidates.len() != 1 {
                    chain_complete = false;
                    break;
                }

                let record = &parent_candidates[0];
                let key = WorldStreamChunkKey {
                    map_hash: parent,
                    provider: record.provider.clone(),
                };
                if !selected.contains(&key) && !chain.iter().any(|entry| entry.key == key) {
                    let bounds = record.effective_bounds().unwrap_or(candidate.bounds);
                    chain.push(Candidate {
                        key,
                        bounds,
                        distance_squared: bounds.distance_squared_to_point(position),
                        visible: true,
                        entity_count: record.entities.len(),
                        mlo_roots: self.record_mlo_roots(record),
                        parent_hash: record.parent_hash,
                        flags: record.flags,
                        content_flags: record.content_flags,
                    });
                }
                parent_hash = record.parent_hash;
            }

            let additional = chain
                .iter()
                .filter(|entry| !selected.contains(&entry.key))
                .count();
            if !chain_complete
                || selected.len().saturating_add(additional) > self.config.max_active_maps
            {
                continue;
            }

            for (index, entry) in chain.into_iter().enumerate() {
                if index > 0 {
                    parent_keys.insert(entry.key.clone());
                }
                if selected.insert(entry.key.clone()) {
                    hierarchy_candidates.push(entry);
                }
            }
            if is_detail_leaf {
                selected_detail_maps = selected_detail_maps.saturating_add(1);
            }

            if selected.len() >= self.config.max_active_maps {
                break;
            }
        }

        hierarchy_candidates.sort_by(|left, right| {
            parent_keys
                .contains(&right.key)
                .cmp(&parent_keys.contains(&left.key))
                .then_with(|| {
                    candidate_locality_class(left, self.config.retain_radius)
                        .cmp(&candidate_locality_class(right, self.config.retain_radius))
                })
                .then_with(|| candidate_detail_tier(left).cmp(&candidate_detail_tier(right)))
                .then_with(|| {
                    candidate_center_distance_squared(left, position)
                        .total_cmp(&candidate_center_distance_squared(right, position))
                })
                .then_with(|| left.distance_squared.total_cmp(&right.distance_squared))
                .then_with(|| left.key.cmp(&right.key))
        });
        hierarchy_candidates.dedup_by(|left, right| left.key == right.key);

        Ok(hierarchy_candidates
            .into_iter()
            .filter(|candidate| selected.contains(&candidate.key))
            .map(|candidate| candidate.key)
            .collect())
    }

    pub fn set_overlay_packages(&mut self, packages: Vec<RenderPackage>) -> Result<(), io::Error> {
        let mut hashes = BTreeSet::new();
        for package in &packages {
            package.validate()?;
            if let Some(hash) = package.descriptor.scene.root.name_hash {
                hashes.insert(hash);
            }
        }
        self.overlay_map_hashes = hashes;
        self.overlay_packages = packages.into_iter().map(Arc::new).collect();
        self.render_package_dirty = true;
        Ok(())
    }

    pub fn clear_cpu_cache(&mut self) {
        self.cache.clear();
        self.retired_tile_cache.clear();
        self.active.clear();
        self.last_view_position = None;
        self.prefetch_direction_xy = None;
        self.render_package_dirty = true;
        self.last_merged_summary = None;
    }

    pub fn update(&mut self, view: WorldStreamView) -> Result<WorldStreamUpdate, io::Error> {
        if !view.position.x.is_finite()
            || !view.position.y.is_finite()
            || !view.position.z.is_finite()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world stream camera position must be finite",
            ));
        }
        if view.frustum.is_some_and(|frustum| !frustum.is_valid()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "world stream frustum must be valid when provided",
            ));
        }

        self.tick = self.tick.saturating_add(1);
        if let Some(previous) = self.last_view_position {
            let dx = view.position.x - previous.x;
            let dy = view.position.y - previous.y;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance > 1.0 && distance <= self.config.retain_radius * 1.5 {
                self.prefetch_direction_xy = Some([dx / distance, dy / distance]);
            } else if distance > self.config.retain_radius * 1.5 {
                self.prefetch_direction_xy = None;
            }
        }
        let prefetch_target = self.prefetch_direction_xy.map(|direction| {
            let lookahead = (self.config.retain_radius - self.config.load_radius).max(64.0);
            GtaRpfWorldPoint {
                x: view.position.x + direction[0] * lookahead,
                y: view.position.y + direction[1] * lookahead,
                z: view.position.z,
            }
        });
        let previous_active = self.active.clone();
        let query =
            self.index
                .query_world_radius(view.position, self.config.retain_radius, false)?;
        let visible_keys = if let Some(frustum) = view.frustum {
            self.index
                .query_world_frustum(frustum, false)?
                .maps
                .into_iter()
                .map(|hit| (hit.map_hash, hit.provider))
                .collect::<BTreeSet<_>>()
        } else {
            BTreeSet::new()
        };

        let mut grouped = BTreeMap::<u32, Vec<GtaRpfWorldMapHit>>::new();
        for hit in &query.maps {
            grouped.entry(hit.map_hash).or_default().push(hit.clone());
        }

        let mut ambiguous_maps = 0;
        let mut unknown_bounds_maps = 0;
        let mut overlay_suppressed_maps = 0;
        let mut candidates = Vec::new();
        let load_radius_squared = self.config.load_radius * self.config.load_radius;

        for (map_hash, hits) in grouped {
            if self.overlay_map_hashes.contains(&map_hash) {
                overlay_suppressed_maps += 1;
                continue;
            }
            if hits.len() != 1 {
                ambiguous_maps += 1;
                continue;
            }
            let hit = &hits[0];
            let Some(bounds) = hit.bounds else {
                unknown_bounds_maps += 1;
                continue;
            };
            let key = WorldStreamChunkKey {
                map_hash,
                provider: hit.provider.clone(),
            };
            let distance_squared = bounds.distance_squared_to_point(view.position);
            let already_active = self.active.contains(&key);
            let mlo_roots = self.cached_mlo_roots(&key, bounds, hit.entity_count);
            let candidate = Candidate {
                visible: view.frustum.is_none()
                    || visible_keys.contains(&(map_hash, hit.provider.clone())),
                key,
                bounds,
                distance_squared,
                entity_count: hit.entity_count,
                mlo_roots,
                parent_hash: hit.parent_hash,
                flags: hit.flags,
                content_flags: hit.content_flags,
            };

            if !already_active && distance_squared > load_radius_squared {
                continue;
            }
            candidates.push(candidate);
        }

        candidates = dedupe_spatial_variants(candidates);

        candidates.sort_by(|left, right| {
            candidate_locality_class(left, self.config.retain_radius)
                .cmp(&candidate_locality_class(right, self.config.retain_radius))
                .then_with(|| candidate_detail_tier(left).cmp(&candidate_detail_tier(right)))
                .then_with(|| right.visible.cmp(&left.visible))
                .then_with(|| {
                    candidate_center_distance_squared(left, view.position)
                        .total_cmp(&candidate_center_distance_squared(right, view.position))
                })
                .then_with(|| {
                    self.active
                        .contains(&right.key)
                        .cmp(&self.active.contains(&left.key))
                })
                .then_with(|| left.distance_squared.total_cmp(&right.distance_squared))
                .then_with(|| left.key.cmp(&right.key))
        });

        let ranked_candidates = candidates.clone();
        let hierarchy_map_hashes = ranked_candidates
            .iter()
            .filter_map(|candidate| candidate.parent_hash)
            .collect::<BTreeSet<_>>();
        let detail_budget = detail_map_budget(self.config.max_active_maps);
        let mut selected_detail_maps = 0_usize;
        let mut selected = BTreeSet::new();
        let mut parent_keys = BTreeSet::new();
        let mut deferred_parent_maps: usize = 0;
        let mut hierarchy_candidates = Vec::new();

        for candidate in &ranked_candidates {
            if selected.contains(&candidate.key) {
                continue;
            }

            let is_detail_leaf = candidate_locality_class(candidate, self.config.retain_radius)
                == 0
                && candidate_detail_tier(candidate) <= 1
                && !hierarchy_map_hashes.contains(&candidate.key.map_hash);
            if is_detail_leaf && selected_detail_maps >= detail_budget {
                continue;
            }

            let mut chain = vec![candidate.clone()];
            let mut parent_hash = candidate.parent_hash;
            let mut depth = 0_u8;
            let mut chain_complete = true;
            while let Some(parent) = parent_hash {
                if depth >= 16 {
                    deferred_parent_maps += 1;
                    chain_complete = false;
                    break;
                }
                depth += 1;
                if self.overlay_map_hashes.contains(&parent) {
                    break;
                }

                let parent_candidates = self.index.world_map_candidates(parent);
                if parent_candidates.len() != 1 {
                    if !parent_candidates.is_empty() {
                        deferred_parent_maps += 1;
                    }
                    chain_complete = false;
                    break;
                }

                let record = &parent_candidates[0];
                let key = WorldStreamChunkKey {
                    map_hash: parent,
                    provider: record.provider.clone(),
                };
                if !selected.contains(&key) && !chain.iter().any(|entry| entry.key == key) {
                    let bounds = record.effective_bounds().unwrap_or(candidate.bounds);
                    chain.push(Candidate {
                        key,
                        bounds,
                        distance_squared: bounds.distance_squared_to_point(view.position),
                        visible: candidate.visible,
                        entity_count: record.entities.len(),
                        mlo_roots: self.record_mlo_roots(record),
                        parent_hash: record.parent_hash,
                        flags: record.flags,
                        content_flags: record.content_flags,
                    });
                }
                parent_hash = record.parent_hash;
            }

            let additional = chain
                .iter()
                .filter(|entry| !selected.contains(&entry.key))
                .count();
            if !chain_complete
                || selected.len().saturating_add(additional) > self.config.max_active_maps
            {
                deferred_parent_maps = deferred_parent_maps.saturating_add(
                    chain
                        .iter()
                        .skip(1)
                        .filter(|entry| !selected.contains(&entry.key))
                        .count(),
                );
                continue;
            }

            for (index, entry) in chain.into_iter().enumerate() {
                if index > 0 {
                    parent_keys.insert(entry.key.clone());
                }
                if selected.insert(entry.key.clone()) {
                    hierarchy_candidates.push(entry);
                }
            }
            if is_detail_leaf {
                selected_detail_maps = selected_detail_maps.saturating_add(1);
            }

            if selected.len() >= self.config.max_active_maps {
                break;
            }
        }

        candidates = hierarchy_candidates;

        candidates.sort_by(|left, right| {
            parent_keys
                .contains(&right.key)
                .cmp(&parent_keys.contains(&left.key))
                .then_with(|| {
                    candidate_locality_class(left, self.config.retain_radius)
                        .cmp(&candidate_locality_class(right, self.config.retain_radius))
                })
                .then_with(|| candidate_detail_tier(left).cmp(&candidate_detail_tier(right)))
                .then_with(|| right.visible.cmp(&left.visible))
                .then_with(|| {
                    candidate_center_distance_squared(left, view.position)
                        .total_cmp(&candidate_center_distance_squared(right, view.position))
                })
                .then_with(|| left.distance_squared.total_cmp(&right.distance_squared))
                .then_with(|| left.key.cmp(&right.key))
        });
        candidates.dedup_by(|left, right| left.key == right.key);

        let ordered_keys = candidates
            .iter()
            .filter(|candidate| selected.contains(&candidate.key))
            .map(|candidate| candidate.key.clone())
            .collect::<Vec<_>>();

        let prefetch_budget = self
            .config
            .max_cpu_chunks
            .saturating_sub(self.config.max_active_maps)
            .min(8);
        let prefetch_window = if let Some(target) = prefetch_target {
            self.planned_keys_at(target)?
                .into_iter()
                .filter(|key| !selected.contains(key))
                .filter(|key| !self.prefetch_failures.contains(key))
                .take(prefetch_budget)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut cache_hits: usize = 0;
        let mut cache_misses: usize = 0;
        let mut cached_tile_swaps: usize = 0;
        let missing_all = ordered_keys
            .iter()
            .filter_map(|key| {
                match activate_cached_spatial_tile(
                    &mut self.cache,
                    &mut self.retired_tile_cache,
                    key,
                    view.position,
                    self.config,
                    self.tick,
                ) {
                    Some(swapped) => {
                        cache_hits += 1;
                        cached_tile_swaps += usize::from(swapped);
                        None
                    }
                    None => {
                        cache_misses += 1;
                        Some(key.clone())
                    }
                }
            })
            .collect::<Vec<_>>();
        let active_load_budget = if self.active.is_empty() {
            // A lightweight parent LOD should reach the screen immediately.
            // Defer the heavy HD leaf to the next background update rather
            // than holding the first visible frame for both chunk builds.
            1
        } else {
            self.config.max_chunk_loads_per_update
        };
        let active_missing =
            select_active_chunk_load_batch(&missing_all, &parent_keys, active_load_budget);
        let prefetch_missing = if cache_misses == 0
            && !ordered_keys.iter().any(|key| {
                self.cache
                    .get(key)
                    .is_some_and(|chunk| !chunk.pending_pages.is_empty())
            }) {
            prefetch_window
                .iter()
                .filter(|key| !self.cache.contains_key(*key))
                .take(PREFETCH_CHUNK_LOADS_PER_UPDATE)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut load_requests = active_missing
            .iter()
            .cloned()
            .map(|key| (key, false, view.position))
            .collect::<Vec<_>>();
        if let Some(prefetch_position) = prefetch_target {
            load_requests.extend(
                prefetch_missing
                    .into_iter()
                    .map(|key| (key, true, prefetch_position)),
            );
        }

        self.cumulative_cache_hits = self.cumulative_cache_hits.saturating_add(cache_hits as u64);
        self.cumulative_cache_misses = self
            .cumulative_cache_misses
            .saturating_add(cache_misses as u64);

        let mut batch_warnings = Vec::new();
        let mut load_errors = Vec::new();
        let mut loaded_chunks = 0;
        let mut prefetched_chunks = 0;
        let mut chunk_loads = Vec::new();
        if !load_requests.is_empty() {
            let requested_archetypes = load_requests
                .iter()
                .flat_map(|(key, _, _)| {
                    self.unique_record(key).into_iter().flat_map(|record| {
                        record.entities.iter().map(|entity| entity.archetype_hash)
                    })
                })
                .collect::<BTreeSet<_>>();
            let new_archetypes = requested_archetypes
                .difference(&self.mounted_archetypes)
                .copied()
                .collect::<Vec<_>>();

            if !new_archetypes.is_empty() {
                batch_warnings = mount_loaded_game_index_for_archetypes(
                    &mut self.resolver,
                    new_archetypes.iter().copied(),
                    &self.source,
                    &self.index,
                )?;
                self.mounted_archetypes.extend(new_archetypes);
            }

            let loaded_batch = if load_requests.len() <= 1 {
                load_requests
                    .iter()
                    .cloned()
                    .map(|(key, prefetched, build_position)| {
                        let started = Instant::now();
                        let result = self.load_chunk(&self.resolver, &key, build_position);
                        (
                            key,
                            prefetched,
                            started.elapsed().as_secs_f64() * 1000.0,
                            result,
                        )
                    })
                    .collect::<Vec<_>>()
            } else {
                let runtime: &WorldStreamingRuntime = self;
                thread::scope(|scope| {
                    let handles = load_requests
                        .iter()
                        .cloned()
                        .map(|(key, prefetched, build_position)| {
                            let join_key = key.clone();
                            let handle = scope.spawn(move || {
                                let started = Instant::now();
                                let result =
                                    runtime.load_chunk(&runtime.resolver, &key, build_position);
                                (started.elapsed().as_secs_f64() * 1000.0, result)
                            });
                            (join_key, prefetched, handle)
                        })
                        .collect::<Vec<_>>();
                    handles
                        .into_iter()
                        .map(|(key, prefetched, handle)| {
                            let (elapsed_ms, result) = handle.join().unwrap_or_else(|_| {
                                (
                                    0.0,
                                    Err(io::Error::other(
                                        "world stream chunk loader thread panicked",
                                    )),
                                )
                            });
                            (key, prefetched, elapsed_ms, result)
                        })
                        .collect::<Vec<_>>()
                })
            };

            for (key, prefetched, elapsed_ms, result) in loaded_batch {
                match result {
                    Ok(chunk) => {
                        chunk_loads.push(WorldStreamChunkLoadMetric {
                            chunk: key.clone(),
                            prefetched,
                            elapsed_ms,
                            read_ms: chunk.read_ms,
                            parse_ms: chunk.parse_ms,
                            assemble_ms: chunk.assemble_ms,
                            render_package_ms: chunk.render_package_ms,
                            encoded_bytes: chunk.bytes,
                            instances: chunk
                                .packages
                                .iter()
                                .map(|package| package.descriptor.scene.instances.len())
                                .sum(),
                        });
                        if !prefetched {
                            self.prefetch_failures.remove(&key);
                        }
                        if let Some(old) = self.cache.insert(key.clone(), chunk) {
                            if old.spatialized {
                                self.retired_tile_cache.insert(
                                    retired_spatial_chunk_key(&key, old.build_position),
                                    old,
                                );
                            }
                        }
                        if prefetched {
                            prefetched_chunks += 1;
                        } else {
                            loaded_chunks += 1;
                        }
                    }
                    Err(error) => {
                        if prefetched {
                            self.prefetch_failures.insert(key.clone());
                        } else {
                            selected.remove(&key);
                        }
                        load_errors.push(WorldStreamChunkError {
                            chunk: key,
                            message: error.to_string(),
                        });
                    }
                }
            }
        }
        let pending_chunks = cache_misses.saturating_sub(loaded_chunks);
        let mut compiled_pages_now = 0;
        if pending_chunks == 0 && load_requests.is_empty() {
            // Compile only one deferred page per update, outside the render
            // thread. Foreground YMAP loads always take precedence.
            for key in &ordered_keys {
                let Some(chunk) = self.cache.get_mut(key) else {
                    continue;
                };
                if chunk.pending_pages.is_empty() || chunk.page_error.is_some() {
                    continue;
                }
                match compile_pending_world_page(chunk, self.config.render_options) {
                    Ok(true) => compiled_pages_now += 1,
                    Ok(false) => {}
                    Err(error) => {
                        chunk.page_error = Some(error.to_string());
                        load_errors.push(WorldStreamChunkError {
                            chunk: key.clone(),
                            message: format!("world page compile failed: {error}"),
                        });
                    }
                }
                break;
            }
        }
        let prefetch_pending = prefetch_window
            .iter()
            .filter(|key| !self.cache.contains_key(*key))
            .count();

        let mut final_order = ordered_keys
            .into_iter()
            .filter(|key| selected.contains(key) && self.cache.contains_key(key))
            .collect::<Vec<_>>();

        if pending_chunks > 0 {
            let missing_parent_hashes = missing_all
                .iter()
                .filter_map(|key| {
                    self.unique_record(key)
                        .and_then(|record| record.parent_hash)
                })
                .collect::<BTreeSet<_>>();
            let fallback_parents = previous_active
                .iter()
                .filter(|key| {
                    missing_parent_hashes.contains(&key.map_hash) && self.cache.contains_key(*key)
                })
                .cloned()
                .collect::<Vec<_>>();
            let direct_parent_hashes = final_order
                .iter()
                .map(|key| {
                    (
                        key.clone(),
                        self.unique_record(key)
                            .and_then(|record| record.parent_hash),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            keep_loaded_parent_fallbacks(
                &mut final_order,
                &fallback_parents,
                &direct_parent_hashes,
                &parent_keys,
                self.config.max_active_maps,
            );
        }

        if pending_chunks > 0 && final_order.len() < self.config.max_active_maps {
            let previous_parent_hashes = previous_active
                .iter()
                .filter_map(|key| {
                    self.unique_record(key)
                        .and_then(|record| record.parent_hash)
                })
                .collect::<BTreeSet<_>>();
            let mut transition_candidates = previous_active
                .iter()
                .filter(|key| !selected.contains(*key) && self.cache.contains_key(*key))
                .filter_map(|key| {
                    let record = self.unique_record(key)?;
                    let bounds = record.effective_bounds()?;
                    Some((
                        !previous_parent_hashes.contains(&key.map_hash),
                        bounds.distance_squared_to_point(view.position),
                        key.clone(),
                    ))
                })
                .collect::<Vec<_>>();
            transition_candidates.sort_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.total_cmp(&right.1))
                    .then_with(|| left.2.cmp(&right.2))
            });

            for (_, _, key) in transition_candidates {
                if final_order.len() >= self.config.max_active_maps {
                    break;
                }
                final_order.push(key);
            }
        }

        let render_budget_drops = 0;
        let next_active = final_order.iter().cloned().collect::<BTreeSet<_>>();
        let active_changed = next_active != previous_active;
        let should_rebuild = active_changed
            || loaded_chunks > 0
            || compiled_pages_now > 0
            || cached_tile_swaps > 0
            || self.render_package_dirty;

        let packages = if should_rebuild {
            let mut packages = final_order
                .iter()
                .filter_map(|key| self.cache.get(key))
                .flat_map(|chunk| chunk.packages.iter().cloned())
                .collect::<Vec<_>>();
            packages.extend(self.overlay_packages.iter().cloned());
            self.last_merged_summary = aggregate_render_package_summary(&packages);
            self.render_package_dirty = false;
            Some(packages)
        } else {
            None
        };

        self.active = next_active;
        let unloaded_chunks = previous_active.difference(&self.active).count();
        let cpu_evictions = self.evict_cpu_cache();
        let cpu_cached_bytes = self.cpu_cached_bytes();
        let cpu_budget_overflow = self.cache.len() + self.retired_tile_cache.len()
            > self.config.max_cpu_chunks
            || cpu_cached_bytes > self.config.max_cpu_bytes;

        let active_entities = self
            .active
            .iter()
            .filter_map(|key| self.unique_record(key))
            .map(|record| record.entities.len())
            .sum::<usize>();
        let mut hd_lod_instances = 0;
        let mut proxy_lod_instances = 0;
        let mut unknown_lod_instances = 0;
        for instance in final_order
            .iter()
            .filter_map(|key| self.cache.get(key))
            .flat_map(|chunk| &chunk.packages)
            .flat_map(|package| &package.descriptor.scene.instances)
            .filter(|instance| instance.asset_ref.is_some())
        {
            match instance.lod_level {
                Some(0 | 5) => hd_lod_instances += 1,    // HD and ORPHANHD
                Some(1..=4) => proxy_lod_instances += 1, // LOD, SLOD1-3
                _ => unknown_lod_instances += 1,
            }
        }
        let spatial_omitted_nodes = final_order
            .iter()
            .filter_map(|key| self.cache.get(key))
            .map(|chunk| chunk.spatial_omitted_nodes)
            .sum::<usize>();
        let spatial_omitted_assets = final_order
            .iter()
            .filter_map(|key| self.cache.get(key))
            .map(|chunk| chunk.spatial_omitted_assets)
            .sum::<usize>();
        let render_omitted_assets = final_order
            .iter()
            .filter_map(|key| self.cache.get(key))
            .map(|chunk| chunk.render_omitted_assets)
            .sum::<usize>();
        let render_omitted_geometry = final_order
            .iter()
            .filter_map(|key| self.cache.get(key))
            .map(|chunk| chunk.render_omitted_geometry)
            .sum::<usize>();
        let pending_pages = final_order
            .iter()
            .filter_map(|key| self.cache.get(key))
            .filter(|chunk| chunk.page_error.is_none())
            .map(|chunk| chunk.pending_pages.len())
            .sum();
        let failed_pages = final_order
            .iter()
            .filter_map(|key| self.cache.get(key))
            .filter(|chunk| chunk.page_error.is_some())
            .map(|chunk| chunk.pending_pages.len())
            .sum();
        let compiled_pages = final_order
            .iter()
            .filter_map(|key| self.cache.get(key))
            .map(|chunk| chunk.packages.len())
            .sum();
        let visible_maps = candidates
            .iter()
            .filter(|candidate| candidate.visible && self.active.contains(&candidate.key))
            .count();
        let mut active_chunks = Vec::with_capacity(final_order.len());
        let mut merged_node_base = 0_u32;
        for key in &final_order {
            let Some(record) = self.unique_record(key).cloned() else {
                continue;
            };
            let Some(chunk) = self.cache.get(key) else {
                continue;
            };
            let mut node_by_entity = BTreeMap::new();
            for package in &chunk.packages {
                for instance in &package.descriptor.scene.instances {
                    node_by_entity.insert(
                        (instance.entity_index, instance.archetype_hash),
                        merged_node_base.saturating_add(instance.node_index),
                    );
                }
                merged_node_base = merged_node_base.saturating_add(
                    u32::try_from(package.descriptor.scene.instances.len()).unwrap_or(u32::MAX),
                );
            }
            let mut entities = Vec::with_capacity(record.entities.len());
            for entity in record.entities.iter().cloned() {
                entities.push(WorldStreamActiveEntity {
                    render_node_index: node_by_entity
                        .get(&(entity.index, entity.archetype_hash))
                        .copied(),
                    resolution: self.cached_archetype_resolution(entity.archetype_hash),
                    entity,
                });
            }
            active_chunks.push(WorldStreamActiveMap {
                map_hash: key.map_hash,
                provider: record.provider.clone(),
                parent_hash: record.parent_hash,
                flags: record.flags,
                content_flags: record.content_flags,
                bounds: record.effective_bounds(),
                entities,
            });
        }

        self.last_view_position = Some(view.position);

        Ok(WorldStreamUpdate {
            report: WorldStreamReport {
                schema: "ragelab.world-stream",
                schema_version: WORLD_STREAM_SCHEMA_VERSION,
                tick: self.tick,
                position: view.position,
                load_radius: self.config.load_radius,
                retain_radius: self.config.retain_radius,
                candidate_map_keys: query.candidate_map_keys,
                candidate_maps: query.maps.len(),
                visible_maps,
                active_maps: self.active.len(),
                active_entities,
                hd_lod_instances,
                proxy_lod_instances,
                unknown_lod_instances,
                spatial_omitted_nodes,
                spatial_omitted_assets,
                render_omitted_assets,
                render_omitted_geometry,
                active_chunks,
                overlay_packages: self.overlay_packages.len(),
                overlay_suppressed_maps,
                ambiguous_maps,
                unknown_bounds_maps,
                deferred_parent_maps,
                lod_deferred_entities: active_entities,
                loaded_chunks,
                prefetched_chunks,
                chunk_loads,
                pending_chunks,
                pending_pages,
                failed_pages,
                compiled_pages,
                prefetch_pending,
                unloaded_chunks,
                cache_hits,
                cached_tile_swaps,
                cached_retired_tiles: self.retired_tile_cache.len(),
                cache_misses,
                cumulative_cache_hits: self.cumulative_cache_hits,
                cumulative_cache_misses: self.cumulative_cache_misses,
                cpu_cached_chunks: self.cache.len() + self.retired_tile_cache.len(),
                cpu_cached_bytes,
                cpu_evictions,
                cpu_budget_overflow,
                render_budget_drops,
                merged_summary: self.last_merged_summary,
                batch_warnings,
                load_errors,
            },
            packages,
        })
    }

    fn unique_record(&self, key: &WorldStreamChunkKey) -> Option<&GtaRpfWorldMapRecord> {
        self.index
            .world_map_candidates(key.map_hash)
            .iter()
            .find(|record| record.provider == key.provider)
    }

    fn load_chunk(
        &self,
        resolver: &WorkspaceIndex,
        key: &WorldStreamChunkKey,
        build_position: GtaRpfWorldPoint,
    ) -> Result<CachedChunk, io::Error> {
        let record = self.unique_record(key).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "stream chunk map 0x{:08X} provider disappeared",
                    key.map_hash
                ),
            )
        })?;
        let locator = record
            .provider
            .materialize(&self.source.game_root, &self.source.keys);

        let read_started = Instant::now();
        let bytes = locator
            .read()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let read_ms = read_started.elapsed().as_secs_f64() * 1000.0;

        let parse_started = Instant::now();
        let ymap = Ymap::from_bytes(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let parse_ms = parse_started.elapsed().as_secs_f64() * 1000.0;

        let label = locator.provenance();
        let assemble_started = Instant::now();
        let manifest = assemble_ymap_scene(
            resolver,
            Path::new(&label),
            &ymap,
            self.config.scene_options,
        );
        let tile_center = spatial_subchunk_tile_center(build_position, self.config);
        let mut pages =
            paginate_world_manifest(manifest, tile_center, self.config.render_options.max_assets)?;
        let spatialized = pages.len() > 1;
        let assemble_ms = assemble_started.elapsed().as_secs_f64() * 1000.0;
        let first = pages.pop_front().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "world manifest produced no pages",
            )
        })?;

        let render_started = Instant::now();
        let mut render_options = self.config.render_options;
        if !first.interiors.is_empty() {
            render_options.max_assets = render_options.max_assets.max(first.assets.len());
        }
        let package = build_world_render_package(
            &first,
            render_options.validate()?,
            [tile_center.x, tile_center.y, tile_center.z],
        )?;
        let render_package_ms = render_started.elapsed().as_secs_f64() * 1000.0;
        let report = package.report();
        let (render_omitted_assets, render_omitted_geometry) = package_render_omissions(&package);
        let spatial_omitted_nodes = pages.iter().map(|page| page.nodes.len()).sum();
        let spatial_omitted_assets = pages.iter().map(|page| page.assets.len()).sum();
        let estimated_pending_bytes = estimated_pending_page_bytes(&pages);
        // Pending scene metadata is counted against the CPU cache budget.
        Ok(CachedChunk {
            bytes: report.encoded_bytes.saturating_add(estimated_pending_bytes),
            packages: vec![Arc::new(package)],
            pending_pages: pages,
            page_error: None,
            read_ms,
            parse_ms,
            assemble_ms,
            render_package_ms,
            build_position: tile_center,
            spatialized,
            spatial_omitted_nodes,
            spatial_omitted_assets,
            render_omitted_assets,
            render_omitted_geometry,
            last_used_tick: self.tick,
        })
    }

    fn evict_cpu_cache(&mut self) -> usize {
        let mut evictions = 0;
        loop {
            let bytes = self.cpu_cached_bytes();
            if self.cache.len() + self.retired_tile_cache.len() <= self.config.max_cpu_chunks
                && bytes <= self.config.max_cpu_bytes
            {
                break;
            }
            // Retired versions are always lower priority than the current
            // version of a YMAP, regardless of which YMAP remains active.
            if let Some(victim) = self
                .retired_tile_cache
                .iter()
                .map(|(key, chunk)| (chunk.last_used_tick, key.clone()))
                .min()
                .map(|(_, key)| key)
            {
                self.retired_tile_cache.remove(&victim);
                evictions += 1;
                continue;
            }
            let victim = select_cpu_eviction_victim(
                self.cache
                    .iter()
                    .map(|(key, chunk)| (key, chunk.last_used_tick)),
                &self.active,
            );
            let Some(key) = victim else {
                break;
            };
            self.cache.remove(&key);
            evictions += 1;
        }
        evictions
    }

    fn cpu_cached_bytes(&self) -> u64 {
        self.cache
            .values()
            .chain(self.retired_tile_cache.values())
            .map(|chunk| chunk.bytes)
            .fold(0_u64, u64::saturating_add)
    }
}

fn select_cpu_eviction_victim<'a>(
    entries: impl Iterator<Item = (&'a WorldStreamChunkKey, u64)>,
    active: &BTreeSet<WorldStreamChunkKey>,
) -> Option<WorldStreamChunkKey> {
    entries
        .filter(|(key, _)| !active.contains(*key))
        .map(|(key, last_used_tick)| (last_used_tick, key.clone()))
        .min()
        .map(|(_, key)| key)
}

fn validate_source_paths(source: &SceneGameIndexSource) -> Result<(), io::Error> {
    if !source.game_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "world stream game root not found: {}",
                source.game_root.display()
            ),
        ));
    }
    if !source.index.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "world stream game index not found: {}",
                source.index.display()
            ),
        ));
    }
    if !source.keys.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "world stream key store not found: {}",
                source.keys.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_requires_hysteresis_and_cpu_capacity() {
        let invalid_radius = WorldStreamConfig {
            retain_radius: 100.0,
            load_radius: 200.0,
            ..WorldStreamConfig::default()
        };
        assert!(invalid_radius.validate().is_err());

        let invalid_chunks = WorldStreamConfig {
            max_active_maps: 8,
            max_cpu_chunks: 4,
            ..WorldStreamConfig::default()
        };
        assert!(invalid_chunks.validate().is_err());
    }

    #[test]
    fn gta_lod_level_five_is_orphan_hd_not_proxy() {
        let levels = [Some(0), Some(5), Some(1), Some(2), Some(3), Some(4), None];
        let hd = levels
            .iter()
            .filter(|level| matches!(level, Some(0 | 5)))
            .count();
        let proxy = levels
            .iter()
            .filter(|level| matches!(level, Some(1..=4)))
            .count();
        assert_eq!((hd, proxy), (2, 4));
    }

    #[test]
    fn world_subchunk_grid_is_stable_during_small_movements_and_for_negative_coordinates() {
        let config = WorldStreamConfig {
            load_radius: 450.0,
            retain_radius: 650.0,
            ..WorldStreamConfig::default()
        };
        let point = |x, y| GtaRpfWorldPoint { x, y, z: 10.0 };
        let spawn = spatial_subchunk_tile_center(point(-1625.0, -850.0), config);
        assert_eq!(spawn.x, -1620.0);
        assert_eq!(spawn.y, -900.0);
        assert_eq!(
            spawn,
            spatial_subchunk_tile_center(point(-1580.0, -840.0), config)
        );
        assert_ne!(
            spawn.x,
            spatial_subchunk_tile_center(point(-1520.0, -840.0), config).x
        );
        assert_eq!(
            spatial_subchunk_tile_center(point(-1625.0, -850.0), config),
            spawn
        );
    }

    #[test]
    fn world_subchunk_grid_is_independent_of_camera_movement_history() {
        let config = WorldStreamConfig {
            load_radius: 450.0,
            ..WorldStreamConfig::default()
        };
        let destination = GtaRpfWorldPoint {
            x: -1460.0,
            y: -710.0,
            z: 25.0,
        };
        let direct = spatial_subchunk_tile_center(destination, config);
        for intermediate in [
            GtaRpfWorldPoint {
                x: -1800.0,
                y: -900.0,
                z: 25.0,
            },
            GtaRpfWorldPoint {
                x: -1400.0,
                y: -500.0,
                z: 25.0,
            },
        ] {
            let _ = spatial_subchunk_tile_center(intermediate, config);
            assert_eq!(spatial_subchunk_tile_center(destination, config), direct);
        }
    }

    #[test]
    fn world_pager_preserves_every_entity_and_asset_reference() {
        use ragelab_assets::{AssetKind, SceneAssetLocator, SceneTextureDictionaryLookup};

        let count = 450;
        let assets = (0..count)
            .map(|index| crate::SceneAssetReference {
                id: index,
                kind: AssetKind::Ydr,
                hash: index as u32,
                path: SceneAssetLocator::Loose(format!("asset-{index}.ydr").into()),
                selector: None,
                texture_dictionary: SceneTextureDictionaryLookup::None,
            })
            .collect::<Vec<_>>();
        let nodes = (0..count)
            .map(|index| crate::SceneNode {
                index,
                source_ymap: "large.ymap".into(),
                entity_index: index,
                archetype_hash: index as u32,
                provider_path: None,
                asset_ref: Some(index),
                asset_kind: Some(AssetKind::Ydr),
                transform: (index != 449).then_some(crate::SpatialTransform {
                    translation: [
                        if index == 1 {
                            900.0
                        } else {
                            index as f32 * 5.0
                        },
                        -300.0,
                        0.0,
                    ],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: Some([1.0; 3]),
                }),
                lod_dist: Some(1000.0),
                child_lod_dist: (index == 0).then_some(100.0),
                lod_level: None,
                num_children: (index == 0).then_some(1),
                parent_index: (index == 1).then_some(0),
                parent_map_hash: None,
                resolution: SceneResolutionState::Resolved,
                reason: None,
                collision: None,
                interior: None,
            })
            .collect::<Vec<_>>();
        let manifest = SceneManifest {
            schema_version: 1,
            root: crate::SceneRoot {
                path: "large.ymap".into(),
                name_hash: Some(42),
            },
            nodes,
            interiors: Vec::new(),
            assets,
            summary: crate::SceneSummary {
                total_entities: count,
                emitted_nodes: count,
                resolved_nodes: count,
                unresolved_nodes: 0,
                asset_references: count,
            },
            warnings: Vec::new(),
            limits: crate::SceneLimits {
                max_nodes: 10000,
                truncated: false,
                omitted_entities: 0,
            },
        };
        let pages = paginate_world_manifest(
            manifest,
            GtaRpfWorldPoint {
                x: 900.0,
                y: -300.0,
                z: 0.0,
            },
            192,
        )
        .unwrap();
        assert!(pages.len() >= 3);
        let mut seen_entities = BTreeSet::new();
        let mut seen_archetypes = BTreeSet::new();
        let mut parent_page = None;
        let mut child_page = None;
        for (page_index, page) in pages.iter().enumerate() {
            assert!(page.assets.len() <= 192);
            assert!(page
                .root
                .path
                .display()
                .to_string()
                .ends_with(&format!("#world-page-{page_index}")));
            for node in &page.nodes {
                if node.entity_index == 0 {
                    parent_page = Some(page_index);
                    assert_eq!(node.num_children, Some(1));
                    assert_eq!(node.child_lod_dist, Some(100.0));
                }
                if node.entity_index == 1 {
                    child_page = Some(page_index);
                    assert_eq!(node.parent_index, Some(0));
                }
                assert!(seen_entities.insert(node.entity_index));
                assert!(node.asset_ref.is_some_and(|id| id < page.assets.len()));
                assert_eq!(
                    page.assets[node.asset_ref.unwrap()].hash,
                    node.archetype_hash
                );
                seen_archetypes.insert(node.archetype_hash);
            }
        }
        assert_eq!(seen_entities.len(), count);
        assert_eq!(seen_archetypes.len(), count);
        assert!(seen_entities.contains(&449)); // no-transform nodes are not dropped
        assert!(parent_page.is_some() && child_page.is_some());
        assert_ne!(parent_page, child_page); // original hierarchy survives separate pages
    }

    #[test]
    fn cached_spatial_tile_swaps_reuse_compiled_packages_on_return() {
        use crate::render::{
            RenderPackageDescriptor, RenderPackageLimits, RenderSceneDescriptor, RenderSceneRoot,
            RENDER_PACKAGE_SCHEMA_VERSION,
        };

        let make_package = |name: &str| {
            Arc::new(RenderPackage {
                descriptor: RenderPackageDescriptor {
                    schema_version: RENDER_PACKAGE_SCHEMA_VERSION,
                    scene: RenderSceneDescriptor {
                        root: RenderSceneRoot {
                            path: name.into(),
                            name_hash: None,
                        },
                        instances: Vec::new(),
                    },
                    assets: Vec::new(),
                    textures: Vec::new(),
                    diagnostics: Vec::new(),
                    summary: RenderPackageSummary {
                        instances: 0,
                        assets: 0,
                        ready_assets: 0,
                        meshes: 0,
                        materials: 0,
                        textures: 0,
                        blob_bytes: 0,
                    },
                    limits: RenderPackageLimits {
                        max_assets: 1,
                        max_blob_bytes: 1,
                        omitted_assets: 0,
                        max_primitives_per_asset: 1,
                        max_vertices_per_asset: 1,
                        max_indices_per_asset: 1,
                        max_shaders_per_asset: 1,
                        max_diffuse_textures_per_asset: 1,
                        max_texture_dimension: 1,
                    },
                },
                blob: Vec::new(),
            })
        };
        let config = WorldStreamConfig {
            load_radius: 450.0,
            ..WorldStreamConfig::default()
        };
        let position = |x| GtaRpfWorldPoint {
            x,
            y: -850.0,
            z: 10.0,
        };
        let first_tile = spatial_subchunk_tile_center(position(-1475.0), config);
        let next_tile = spatial_subchunk_tile_center(position(-1340.0), config);
        let first_package = make_package("first");
        let next_package = make_package("next");
        let make_chunk = |package: Arc<RenderPackage>, build_position| CachedChunk {
            packages: vec![package],
            pending_pages: VecDeque::new(),
            page_error: None,
            bytes: 1,
            read_ms: 0.0,
            parse_ms: 0.0,
            assemble_ms: 0.0,
            render_package_ms: 0.0,
            build_position,
            spatialized: true,
            spatial_omitted_nodes: 1,
            spatial_omitted_assets: 1,
            render_omitted_assets: 0,
            render_omitted_geometry: 0,
            last_used_tick: 1,
        };
        let key = WorldStreamChunkKey {
            map_hash: 123,
            provider: GtaRpfIndexLocator {
                archive_relative: "test.rpf".into(),
                nested: Vec::new(),
                entry: "maps/large.ymap".into(),
                load_rank: 10,
            },
        };
        let mut current = BTreeMap::from([(
            key.clone(),
            make_chunk(Arc::clone(&first_package), first_tile),
        )]);
        let mut retired = BTreeMap::from([(
            retired_spatial_chunk_key(&key, next_tile),
            make_chunk(Arc::clone(&next_package), next_tile),
        )]);

        assert_eq!(
            activate_cached_spatial_tile(
                &mut current,
                &mut retired,
                &key,
                position(-1475.0),
                config,
                2,
            ),
            Some(false)
        );
        assert_eq!(
            activate_cached_spatial_tile(
                &mut current,
                &mut retired,
                &key,
                position(-1340.0),
                config,
                3,
            ),
            Some(true)
        );
        assert!(Arc::ptr_eq(&current[&key].packages[0], &next_package));
        assert_eq!(retired.len(), 1);

        assert_eq!(
            activate_cached_spatial_tile(
                &mut current,
                &mut retired,
                &key,
                position(-1475.0),
                config,
                4,
            ),
            Some(true)
        );
        assert!(Arc::ptr_eq(&current[&key].packages[0], &first_package));
        assert_eq!(current[&key].last_used_tick, 4);
        assert_eq!(retired.len(), 1);
    }

    #[test]
    fn spatial_cache_keys_reuse_the_same_world_cell_after_round_trip() {
        let config = WorldStreamConfig {
            load_radius: 450.0,
            ..WorldStreamConfig::default()
        };
        let key = WorldStreamChunkKey {
            map_hash: 123,
            provider: GtaRpfIndexLocator {
                archive_relative: "test.rpf".into(),
                nested: Vec::new(),
                entry: "maps/large.ymap".into(),
                load_rank: 10,
            },
        };
        let point = |x| GtaRpfWorldPoint {
            x,
            y: -850.0,
            z: 10.0,
        };
        let initial =
            retired_spatial_chunk_key(&key, spatial_subchunk_tile_center(point(-1475.0), config));
        let adjacent =
            retired_spatial_chunk_key(&key, spatial_subchunk_tile_center(point(-1340.0), config));
        let revisit =
            retired_spatial_chunk_key(&key, spatial_subchunk_tile_center(point(-1475.0), config));
        assert_ne!(initial, adjacent);
        assert_eq!(initial, revisit);
    }

    #[test]
    fn parent_fallback_only_replaces_a_child_of_the_same_parent() {
        let make = |map_hash| WorldStreamChunkKey {
            map_hash,
            provider: GtaRpfIndexLocator {
                archive_relative: "test.rpf".into(),
                nested: Vec::new(),
                entry: format!("maps/{map_hash}.ymap"),
                load_rank: 1,
            },
        };
        let child = make(1);
        let unrelated = make(2);
        let parent = make(3);
        let mut order = vec![child.clone(), unrelated.clone()];
        let parent_map = BTreeMap::from([
            (child.clone(), Some(parent.map_hash)),
            (unrelated.clone(), Some(100)),
        ]);

        assert_eq!(
            keep_loaded_parent_fallbacks(
                &mut order,
                std::slice::from_ref(&parent),
                &parent_map,
                &BTreeSet::new(),
                2,
            ),
            1
        );
        assert_eq!(order.len(), 2);
        assert!(order.contains(&parent));
        assert!(order.contains(&unrelated));
        assert!(!order.contains(&child));

        let mut unrelated_only = vec![unrelated.clone(), make(4)];
        assert_eq!(
            keep_loaded_parent_fallbacks(
                &mut unrelated_only,
                &[parent.clone()],
                &parent_map,
                &BTreeSet::new(),
                2,
            ),
            0
        );
        assert!(!unrelated_only.contains(&parent));

        let mut protected_child = vec![child.clone(), unrelated];
        assert_eq!(
            keep_loaded_parent_fallbacks(
                &mut protected_child,
                &[parent],
                &parent_map,
                &BTreeSet::from([child]),
                2,
            ),
            0
        );
    }

    #[test]
    fn world_load_batch_interleaves_fallback_and_hd_detail() {
        let make = |hash, entry: &str| WorldStreamChunkKey {
            map_hash: hash,
            provider: GtaRpfIndexLocator {
                archive_relative: "test.rpf".into(),
                nested: Vec::new(),
                entry: entry.into(),
                load_rank: 1,
            },
        };
        let parent_a = make(1, "hei_kt1_09.ymap");
        let parent_b = make(2, "hei_kt1_lod.ymap");
        let leaf = make(3, "hei_kt1_09_strm_0.ymap");
        let missing = vec![parent_a.clone(), parent_b.clone(), leaf.clone()];
        let parents = BTreeSet::from([parent_a.clone(), parent_b.clone()]);
        assert_eq!(
            select_active_chunk_load_batch(&missing, &parents, 2),
            vec![parent_a.clone(), leaf.clone()]
        );
        assert_eq!(
            select_active_chunk_load_batch(&missing, &parents, 1),
            vec![parent_a]
        );
        assert_eq!(
            select_active_chunk_load_batch(&missing[1..], &parents, 2),
            vec![parent_b, leaf]
        );
    }

    #[test]
    fn nearby_strm_details_precede_long_distance_lod_proxies() {
        assert_eq!(world_map_detail_tier("dt1_20_strm_0.ymap"), 0);
        assert_eq!(world_map_detail_tier("hei_kt1_08_strm_0.ymap"), 0);
        assert_eq!(world_map_detail_tier("sm_23_critical_0.ymap"), 1);
        assert_eq!(world_map_detail_tier("dt1_09.ymap"), 2);
        assert_eq!(world_map_detail_tier("dt1_09_long_0.ymap"), 3);
        assert_eq!(world_map_detail_tier("bh1_rd_long_2.ymap"), 3);
        assert_eq!(world_map_detail_tier("dt1_lod.ymap"), 3);
        assert_eq!(world_map_detail_tier("maps\\vb_lod.ymap"), 3);
        assert_eq!(world_map_detail_tier("sm_slod2.ymap"), 3);
    }

    #[test]
    fn detail_budget_prioritizes_local_geometry_without_consuming_all_slots() {
        assert_eq!(detail_map_budget(2), 2);
        assert_eq!(detail_map_budget(6), 4);
        assert_eq!(detail_map_budget(10), 6);
        assert_eq!(detail_map_budget(16), 10);
    }

    #[test]
    fn cpu_eviction_prefers_oldest_inactive_chunk_with_stable_tie_break() {
        let provider = GtaRpfIndexLocator {
            archive_relative: "update/update.rpf".into(),
            nested: vec!["x64/levels.rpf".into()],
            entry: "maps/a.ymap".into(),
            load_rank: 10,
        };
        let key1 = WorldStreamChunkKey {
            map_hash: 1,
            provider: provider.clone(),
        };
        let key2 = WorldStreamChunkKey {
            map_hash: 2,
            provider: provider.clone(),
        };
        let key3 = WorldStreamChunkKey {
            map_hash: 3,
            provider,
        };
        let active = BTreeSet::from([key1.clone()]);
        let entries = vec![(&key1, 1), (&key3, 2), (&key2, 2)];

        assert_eq!(
            select_cpu_eviction_victim(entries.into_iter(), &active),
            Some(key2)
        );
    }

    #[test]
    fn locality_priority_prefers_detail_maps_over_macro_lods_and_empty_maps() {
        let provider = GtaRpfIndexLocator {
            archive_relative: "x64.rpf".into(),
            nested: Vec::new(),
            entry: "maps/test.ymap".into(),
            load_rank: 1,
        };
        let detail = Candidate {
            key: WorldStreamChunkKey {
                map_hash: 1,
                provider: provider.clone(),
            },
            bounds: GtaRpfWorldBounds {
                min: GtaRpfWorldPoint {
                    x: -500.0,
                    y: -500.0,
                    z: -100.0,
                },
                max: GtaRpfWorldPoint {
                    x: 500.0,
                    y: 500.0,
                    z: 300.0,
                },
            },
            distance_squared: 0.0,
            visible: true,
            entity_count: 20,
            mlo_roots: 0,
            parent_hash: None,
            flags: Some(0),
            content_flags: Some(65),
        };
        let macro_lod = Candidate {
            key: WorldStreamChunkKey {
                map_hash: 2,
                provider: provider.clone(),
            },
            bounds: GtaRpfWorldBounds {
                min: GtaRpfWorldPoint {
                    x: -8_000.0,
                    y: -8_000.0,
                    z: -2_000.0,
                },
                max: GtaRpfWorldPoint {
                    x: 8_000.0,
                    y: 8_000.0,
                    z: 2_000.0,
                },
            },
            distance_squared: 0.0,
            visible: true,
            entity_count: 3,
            mlo_roots: 0,
            parent_hash: None,
            flags: Some(0),
            content_flags: Some(4),
        };
        let empty = Candidate {
            key: WorldStreamChunkKey {
                map_hash: 3,
                provider,
            },
            bounds: macro_lod.bounds,
            distance_squared: 0.0,
            visible: true,
            entity_count: 0,
            mlo_roots: 0,
            parent_hash: None,
            flags: Some(2),
            content_flags: Some(256),
        };

        let mut interior = detail.clone();
        interior.key.map_hash = 4;
        interior.mlo_roots = 1;

        assert_eq!(candidate_locality_class(&detail, 350.0), 0);
        assert_eq!(candidate_locality_class(&macro_lod, 350.0), 1);
        assert_eq!(candidate_locality_class(&interior, 350.0), 2);
        assert_eq!(candidate_locality_class(&empty, 350.0), 3);
    }

    #[test]
    fn spatial_variant_dedupe_keeps_highest_rank_provider() {
        let bounds = GtaRpfWorldBounds {
            min: GtaRpfWorldPoint {
                x: -100.0,
                y: -100.0,
                z: -20.0,
            },
            max: GtaRpfWorldPoint {
                x: 100.0,
                y: 100.0,
                z: 80.0,
            },
        };
        let make = |map_hash, load_rank| Candidate {
            key: WorldStreamChunkKey {
                map_hash,
                provider: GtaRpfIndexLocator {
                    archive_relative: format!("rank-{load_rank}.rpf"),
                    nested: Vec::new(),
                    entry: "maps/shared.ymap".into(),
                    load_rank,
                },
            },
            bounds,
            distance_squared: 0.0,
            visible: true,
            entity_count: 12,
            mlo_roots: 0,
            parent_hash: None,
            flags: Some(0),
            content_flags: Some(65),
        };

        let deduped = dedupe_spatial_variants(vec![make(1, 26), make(2, 92)]);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].key.provider.load_rank, 92);
        assert_eq!(deduped[0].key.map_hash, 2);
    }

    #[test]
    fn chunk_keys_order_by_map_and_provider() {
        let provider = GtaRpfIndexLocator {
            archive_relative: "update/update.rpf".into(),
            nested: vec!["x64/levels.rpf".into()],
            entry: "maps/a.ymap".into(),
            load_rank: 10,
        };
        let left = WorldStreamChunkKey {
            map_hash: 1,
            provider: provider.clone(),
        };
        let right = WorldStreamChunkKey {
            map_hash: 2,
            provider,
        };
        assert!(left < right);
    }
}
