# Native Offscreen Renderer

RageLab's native renderer lives in `crates/ragelab-render` and consumes only the versioned `ragelab-engine::RenderPackage` contract.

It does not parse YDR/YDD/YFT/YMAP/RPF data directly. RAGE format parsing, dependency resolution, local-over-game precedence, GTXD texture ancestry, material semantics and fail-closed decisions remain Core responsibilities.

## Backend

The first renderer milestone is offscreen wgpu.

Current implementation:

- wgpu 22.1, selected to remain compatible with the workspace Rust/Cargo 1.80 baseline;
- RGBA8 sRGB color target;
- Depth32Float depth target;
- position/normal/UV/index uploads from typed render-package views;
- decoded RGBA8 texture uploads;
- stable asset and texture GPU caches;
- per-instance model uniforms;
- deterministic directional lighting;
- automatic scene bounds and camera fit;
- PNG readback without a window or Tauri dependency.

The renderer supports only semantics present in the render packet. Missing material bindings use a neutral white fallback. Missing or unproven scene transforms are rejected rather than inferred.

## Camera and views

Supported views:

- `auto`
- `front`
- `back`
- `left`
- `right`
- `top`
- `isometric`

Supported projections:

- `perspective`
- `orthographic`

Camera placement derives only from emitted drawable bounds and proven instance transforms.

Optional overlays:

- `--grid`
- `--wireframe`
- `--bounds`

The background can be made transparent with `--transparent`.

## CLI

Render an isolated model:

```bash
ragelab render asset prop.ydr --output prop.png --view isometric --json
ragelab render asset props.ydd --drawable-index 0 --output prop.png --json
ragelab render asset fragment.yft --output fragment.png --json
```

Isolated asset rendering has only file-local context. External YTD resolution is therefore unavailable unless the source resource embeds its texture dictionary. Use scene rendering when workspace/game-index context is required.

Render a scene:

```bash
ragelab render scene ./stream map.ymap \
  --output map.png \
  --game-root "C:/Program Files/Rockstar Games/Grand Theft Auto V Legacy" \
  --game-index "<cache>/legacy-v3.bin" \
  --rpf-keys "<key-cache>" \
  --view isometric \
  --json
```

`render scene` accepts the same workspace/fallback/RPF/game-index source options used by `workspace scene`.

Every successful render writes:

1. the requested PNG;
2. a metadata sidecar, by default `<output>.json`.

Metadata includes:

- render schema version;
- image dimensions;
- view/projection/overlay settings;
- render-package schema version;
- instance/asset/mesh/material/texture counts;
- SHA-256 of the raw RGBA image.

PNG and metadata outputs are non-destructive unless `--overwrite` is explicit.

## Visual regression

Compare two RGBA8 PNG files:

```bash
ragelab render compare expected.png actual.png --tolerance 2 --json
```

The report includes:

- pixel count;
- changed-pixel count;
- maximum channel delta;
- mean absolute channel error;
- configured per-channel tolerance;
- whether every pixel is within tolerance.

Use tolerance 0 for exact same-backend determinism checks. Cross-adapter or cross-driver CI may use a small explicit tolerance because texture sampling and floating-point rasterization can differ slightly between GPU backends.

Synthetic render cases are declared under `fixtures/render/cases.json`. Baseline PNGs are intentionally not treated as universal across all GPU drivers; CI should record backend/adapter metadata with any baseline artifact.

## Interactive native surface

`SurfaceRenderer` reuses the same device-side asset/material/texture cache used by offscreen rendering and accepts any owned target implementing `HasWindowHandle + HasDisplayHandle`. It has no Tauri dependency.

The interactive API provides:

- surface configuration using an adapter explicitly compatible with the target window;
- resize handling and depth-target recreation;
- orbit, pan, zoom and fly camera controls;
- perspective/orthographic projection switching;
- CPU ray/AABB entity picking from normalized viewport coordinates;
- per-instance selection highlight via the model uniform, without duplicating source materials;
- read-only game instances first and loose workspace instances last, with a subtle workspace tint;
- frame/load/cache statistics for Studio parity benchmarking.

The current Studio integration uses a native Tauri window without a WebView as the wgpu presentation target. Pointer/keyboard input stays in the main WebView and is forwarded as normalized renderer input, avoiding Win32 subclassing and WebView2 HWND contention.

## Cache identity

GPU assets are keyed by Core-provided stable source identity:

- source type;
- provenance path;
- asset hash;
- selector identity for dictionary drawables.

Textures are keyed by:

- texture source class;
- source provenance path;
- name hash;
- decoded dimensions.

Repeated YMAP instances reuse the same uploaded asset and texture data.

## Safety and limits

- Render dimensions are bounded to 8192×8192.
- Render-package limits remain authoritative for geometry/assets/textures.
- Buffer views are validated by Core and checked again before GPU upload.
- Unsupported asset kinds are not rendered.
- The renderer never writes back to source assets.
- GTA/RPF sources remain read-only.
