//! Camera-driven streaming over the persistent GTA Legacy world index.
//!
//! The runtime never scans the game installation or assembles the full world.
//! It loads the persistent index once, queries nearby winning YMAP providers,
//! decodes only desired chunks, keeps a bounded CPU chunk cache, and merges the
//! active chunks into the renderer-neutral `RenderPackage` contract.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use ragelab_assets::WorkspaceIndex;
use ragelab_ymap::Ymap;
use serde::Serialize;

use crate::{
    assemble_ymap_scene, build_scene_render_package, merge_render_packages,
    mount_loaded_game_index_for_archetypes, GtaRpfArchetypeBrowserResolution, GtaRpfAssetIndex,
    GtaRpfIndexLocator, GtaRpfWorldBounds, GtaRpfWorldEntityRecord, GtaRpfWorldFrustum,
    GtaRpfWorldMapHit, GtaRpfWorldMapRecord, GtaRpfWorldPoint, RenderPackage, RenderPackageOptions,
    RenderPackageSummary, RenderSceneRoot, SceneAssemblyOptions, SceneGameIndexSource,
};

pub const WORLD_STREAM_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldStreamConfig {
    pub load_radius: f32,
    pub retain_radius: f32,
    pub max_active_maps: usize,
    pub max_cpu_chunks: usize,
    pub max_cpu_bytes: u64,
    pub scene_options: SceneAssemblyOptions,
    pub render_options: RenderPackageOptions,
}

impl Default for WorldStreamConfig {
    fn default() -> Self {
        Self {
            load_radius: 750.0,
            retain_radius: 950.0,
            max_active_maps: 16,
            max_cpu_chunks: 32,
            max_cpu_bytes: 512 * 1024 * 1024,
            scene_options: SceneAssemblyOptions::default(),
            render_options: RenderPackageOptions::default(),
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
    pub active_chunks: Vec<WorldStreamActiveMap>,
    pub overlay_packages: usize,
    pub overlay_suppressed_maps: usize,
    pub ambiguous_maps: usize,
    pub unknown_bounds_maps: usize,
    pub deferred_parent_maps: usize,
    pub lod_deferred_entities: usize,
    pub loaded_chunks: usize,
    pub unloaded_chunks: usize,
    pub cache_hits: usize,
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
    pub package: Option<RenderPackage>,
    pub report: WorldStreamReport,
}

#[derive(Debug)]
struct CachedChunk {
    package: RenderPackage,
    bytes: u64,
    last_used_tick: u64,
}

#[derive(Debug, Clone)]
struct Candidate {
    key: WorldStreamChunkKey,
    bounds: GtaRpfWorldBounds,
    distance_squared: f32,
    visible: bool,
    entity_count: usize,
    parent_hash: Option<u32>,
}

pub struct WorldStreamingRuntime {
    source: SceneGameIndexSource,
    index: GtaRpfAssetIndex,
    config: WorldStreamConfig,
    tick: u64,
    cache: BTreeMap<WorldStreamChunkKey, CachedChunk>,
    active: BTreeSet<WorldStreamChunkKey>,
    overlay_packages: Vec<RenderPackage>,
    overlay_map_hashes: BTreeSet<u32>,
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

        Ok(Self {
            source,
            index,
            config,
            tick: 0,
            cache: BTreeMap::new(),
            active: BTreeSet::new(),
            overlay_packages: Vec::new(),
            overlay_map_hashes: BTreeSet::new(),
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

    pub fn set_overlay_packages(&mut self, packages: Vec<RenderPackage>) -> Result<(), io::Error> {
        let mut hashes = BTreeSet::new();
        for package in &packages {
            package.validate()?;
            if let Some(hash) = package.descriptor.scene.root.name_hash {
                hashes.insert(hash);
            }
        }
        self.overlay_map_hashes = hashes;
        self.overlay_packages = packages;
        Ok(())
    }

    pub fn clear_cpu_cache(&mut self) {
        self.cache.clear();
        self.active.clear();
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
            if !already_active && distance_squared > load_radius_squared {
                continue;
            }
            candidates.push(Candidate {
                visible: view.frustum.is_none()
                    || visible_keys.contains(&(map_hash, hit.provider.clone())),
                key,
                bounds,
                distance_squared,
                entity_count: hit.entity_count,
                parent_hash: hit.parent_hash,
            });
        }

        candidates.sort_by(|left, right| {
            right
                .visible
                .cmp(&left.visible)
                .then_with(|| {
                    self.active
                        .contains(&right.key)
                        .cmp(&self.active.contains(&left.key))
                })
                .then_with(|| left.distance_squared.total_cmp(&right.distance_squared))
                .then_with(|| left.key.cmp(&right.key))
        });
        candidates.truncate(self.config.max_active_maps);

        let mut selected = candidates
            .iter()
            .map(|candidate| candidate.key.clone())
            .collect::<BTreeSet<_>>();
        let mut deferred_parent_maps = 0;
        let selected_snapshot = candidates.clone();
        for candidate in &selected_snapshot {
            let mut parent_hash = candidate.parent_hash;
            let mut depth = 0_u8;
            while let Some(parent) = parent_hash {
                if depth >= 16 {
                    deferred_parent_maps += 1;
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
                    break;
                }
                let record = &parent_candidates[0];
                let key = WorldStreamChunkKey {
                    map_hash: parent,
                    provider: record.provider.clone(),
                };
                if selected.contains(&key) {
                    parent_hash = record.parent_hash;
                    continue;
                }
                if selected.len() >= self.config.max_active_maps {
                    deferred_parent_maps += 1;
                    break;
                }
                selected.insert(key.clone());
                candidates.push(Candidate {
                    key,
                    bounds: record.effective_bounds().unwrap_or(candidate.bounds),
                    distance_squared: record
                        .effective_bounds()
                        .map(|bounds| bounds.distance_squared_to_point(view.position))
                        .unwrap_or(candidate.distance_squared),
                    visible: candidate.visible,
                    entity_count: record.entities.len(),
                    parent_hash: record.parent_hash,
                });
                parent_hash = record.parent_hash;
            }
        }

        candidates.sort_by(|left, right| {
            right
                .visible
                .cmp(&left.visible)
                .then_with(|| left.distance_squared.total_cmp(&right.distance_squared))
                .then_with(|| left.key.cmp(&right.key))
        });
        candidates.dedup_by(|left, right| left.key == right.key);

        let ordered_keys = candidates
            .iter()
            .filter(|candidate| selected.contains(&candidate.key))
            .map(|candidate| candidate.key.clone())
            .collect::<Vec<_>>();

        let mut cache_hits = 0;
        let mut cache_misses = 0;
        let missing = ordered_keys
            .iter()
            .filter_map(|key| {
                if let Some(cached) = self.cache.get_mut(key) {
                    cached.last_used_tick = self.tick;
                    cache_hits += 1;
                    None
                } else {
                    cache_misses += 1;
                    Some(key.clone())
                }
            })
            .collect::<Vec<_>>();
        self.cumulative_cache_hits = self.cumulative_cache_hits.saturating_add(cache_hits as u64);
        self.cumulative_cache_misses = self
            .cumulative_cache_misses
            .saturating_add(cache_misses as u64);

        let mut batch_warnings = Vec::new();
        let mut load_errors = Vec::new();
        let mut loaded_chunks = 0;
        if !missing.is_empty() {
            let mut resolver = WorkspaceIndex::empty(self.source.game_root.clone());
            let archetypes = missing.iter().flat_map(|key| {
                self.unique_record(key)
                    .into_iter()
                    .flat_map(|record| record.entities.iter().map(|entity| entity.archetype_hash))
            });
            batch_warnings = mount_loaded_game_index_for_archetypes(
                &mut resolver,
                archetypes,
                &self.source,
                &self.index,
            )?;

            for key in &missing {
                match self.load_chunk(&resolver, key) {
                    Ok(chunk) => {
                        self.cache.insert(key.clone(), chunk);
                        loaded_chunks += 1;
                    }
                    Err(error) => {
                        selected.remove(key);
                        load_errors.push(WorldStreamChunkError {
                            chunk: key.clone(),
                            message: error.to_string(),
                        });
                    }
                }
            }
        }

        let mut final_order = ordered_keys
            .into_iter()
            .filter(|key| selected.contains(key) && self.cache.contains_key(key))
            .collect::<Vec<_>>();
        let mut render_budget_drops = 0;
        let merged = loop {
            let mut refs = final_order
                .iter()
                .filter_map(|key| self.cache.get(key).map(|chunk| &chunk.package))
                .collect::<Vec<_>>();
            refs.extend(self.overlay_packages.iter());
            if refs.is_empty() {
                break None;
            }
            let result = merge_render_packages(
                RenderSceneRoot {
                    path: "gta://world-stream".into(),
                    name_hash: None,
                },
                &refs,
            );
            match result {
                Ok(package) => break Some(package),
                Err(error)
                    if !final_order.is_empty()
                        && (error.to_string().contains("hard asset limit")
                            || error.to_string().contains("hard blob limit")) =>
                {
                    final_order.pop();
                    render_budget_drops += 1;
                }
                Err(error) => return Err(error),
            }
        };

        self.active = final_order.iter().cloned().collect();
        let unloaded_chunks = previous_active.difference(&self.active).count();
        let cpu_evictions = self.evict_cpu_cache();
        let cpu_cached_bytes = self.cpu_cached_bytes();
        let cpu_budget_overflow = self.cache.len() > self.config.max_cpu_chunks
            || cpu_cached_bytes > self.config.max_cpu_bytes;

        let active_entities = candidates
            .iter()
            .filter(|candidate| self.active.contains(&candidate.key))
            .map(|candidate| candidate.entity_count)
            .sum::<usize>();
        let visible_maps = candidates
            .iter()
            .filter(|candidate| candidate.visible && self.active.contains(&candidate.key))
            .count();
        let mut active_chunks = Vec::with_capacity(final_order.len());
        let mut merged_node_base = 0_u32;
        for key in &final_order {
            let Some(record) = self.unique_record(key) else {
                continue;
            };
            let Some(chunk) = self.cache.get(key) else {
                continue;
            };
            let node_by_entity = chunk
                .package
                .descriptor
                .scene
                .instances
                .iter()
                .map(|instance| {
                    (
                        (instance.entity_index, instance.archetype_hash),
                        merged_node_base.saturating_add(instance.node_index),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            active_chunks.push(WorldStreamActiveMap {
                map_hash: key.map_hash,
                provider: record.provider.clone(),
                parent_hash: record.parent_hash,
                flags: record.flags,
                content_flags: record.content_flags,
                bounds: record.effective_bounds(),
                entities: record
                    .entities
                    .iter()
                    .cloned()
                    .map(|entity| WorldStreamActiveEntity {
                        render_node_index: node_by_entity
                            .get(&(entity.index, entity.archetype_hash))
                            .copied(),
                        resolution: self
                            .index
                            .archetype_browser_resolution(entity.archetype_hash),
                        entity,
                    })
                    .collect(),
            });
            merged_node_base = merged_node_base.saturating_add(
                u32::try_from(chunk.package.descriptor.scene.instances.len()).unwrap_or(u32::MAX),
            );
        }

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
                active_chunks,
                overlay_packages: self.overlay_packages.len(),
                overlay_suppressed_maps,
                ambiguous_maps,
                unknown_bounds_maps,
                deferred_parent_maps,
                lod_deferred_entities: active_entities,
                loaded_chunks,
                unloaded_chunks,
                cache_hits,
                cache_misses,
                cumulative_cache_hits: self.cumulative_cache_hits,
                cumulative_cache_misses: self.cumulative_cache_misses,
                cpu_cached_chunks: self.cache.len(),
                cpu_cached_bytes,
                cpu_evictions,
                cpu_budget_overflow,
                render_budget_drops,
                merged_summary: merged.as_ref().map(|package| package.descriptor.summary),
                batch_warnings,
                load_errors,
            },
            package: merged,
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
        let bytes = locator
            .read()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let ymap = Ymap::from_bytes(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let label = locator.provenance();
        let manifest = assemble_ymap_scene(
            resolver,
            Path::new(&label),
            &ymap,
            self.config.scene_options,
        );
        let package =
            build_scene_render_package(&manifest, self.config.render_options.validate()?)?;
        let report = package.report();
        Ok(CachedChunk {
            bytes: report.encoded_bytes,
            package,
            last_used_tick: self.tick,
        })
    }

    fn evict_cpu_cache(&mut self) -> usize {
        let mut evictions = 0;
        loop {
            let bytes = self.cpu_cached_bytes();
            if self.cache.len() <= self.config.max_cpu_chunks && bytes <= self.config.max_cpu_bytes
            {
                break;
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
