# Camera-driven world streaming

RageLab world streaming is transport-neutral Core policy layered on the persistent GTA Legacy world index.

## Invariants

- The installed GTA world is never assembled or rendered globally.
- `GtaRpfAssetIndex` is loaded once and remains the source of load-order/provider truth.
- Camera queries use persisted YMAP bounds only; no RPF reads occur during broadphase.
- A map with ambiguous winning providers is skipped rather than guessed.
- Maps without proven bounds are not auto-streamed merely because the index keeps them in the global fail-closed bucket.
- Workspace/FiveM overlays are separate packages. A local overlay with the same YMAP hash suppresses the read-only game map and is merged last.
- RAGE parsing, archetype/provider resolution and render packaging remain in Core.

## Runtime contract

`WorldStreamingRuntime::open` validates the game root, key store and persistent index fingerprint once.

Each `update(WorldStreamView)`:

1. queries the v4 world index at `retain_radius`;
2. applies `load_radius` for new maps and `retain_radius` hysteresis for already-active maps;
3. optionally uses a proven frustum to prioritize visible maps;
4. suppresses ambiguous/unbounded/local-overridden maps;
5. follows unique parent relationships while the active-map budget permits;
6. batches archetype dependency planning for newly requested chunks;
7. decodes only missing YMAP chunks and caches their `RenderPackage` on CPU;
8. merges active packages with canonical asset/texture/node remapping;
9. drops the farthest active game chunks deterministically if the merged package would exceed hard renderer asset/blob limits;
10. evicts least-recently-used inactive CPU chunks until configured count/byte budgets are met.

The runtime exposes per-update and cumulative cache diagnostics and returns an optional renderer-neutral active `RenderPackage`.

## LOD policy

Card 10 world-index schema v4 persists map/entity extents, transforms, flags and hierarchy, but it does **not** persist proven GTA entity `lodDist` / `childLodDist` semantics.

Therefore card 11 does not invent distance thresholds. Spatial map streaming is active, while entity LOD selection is reported as deferred for active entities. A later schema revision may enable metadata-backed entity LOD once those fields are normalized and validated.

## Renderer residency

`SurfaceRenderer::update_streaming_package` uploads only cache misses and preserves the existing camera instead of fitting to every package update.

GPU caches use the same `RenderAssetDescriptor::stable_key` and `RenderTextureDescriptor::stable_key` identities as Core package merging.

`GpuCacheBudget` bounds inactive residency by:

- asset count;
- texture count;
- estimated asset-buffer bytes;
- uploaded RGBA texture bytes.

Active package resources are always protected. LRU eviction removes inactive assets first, then textures no longer protected by either the active package or remaining cached assets. If active resources alone exceed the configured budget, rendering continues and `gpuBudgetOverflow` is reported rather than dropping active resources implicitly.

`ViewportStats` exposes cache counts, estimated cache bytes, cumulative hit/miss counts, evictions and budget-overflow state. These byte counts cover cached mesh/index/wire/fallback buffers and RGBA textures; they are residency estimates, not driver-total VRAM accounting.
