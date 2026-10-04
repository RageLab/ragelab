# Shared Render Data Contract

RageLab render consumers share a Core-owned, renderer-neutral packet instead of reparsing Legacy assets or consuming preview JSON as a GPU transport.

The public Rust contract lives in `ragelab-engine::render` and is versioned independently through `RENDER_PACKAGE_SCHEMA_VERSION`.

## Responsibilities

Core owns:

- YMAP scene assembly and local-over-game precedence;
- YDR/YDD/YFT parsing and selector semantics;
- UV0, normals, indices, shader/material binding and drawable bounds;
- proven diffuse/albedo resolution from embedded textures, archetype YTD and GTXD parent YTDs;
- texture provenance and fail-closed unresolved diagnostics;
- geometry/texture budgets;
- deterministic packet metadata and binary serialization.

Consumers own presentation only:

- **native/wgpu** maps typed buffer views directly into GPU buffers/textures and applies instance transforms;
- **Three.js** creates typed array views over the blob, constructs `BufferGeometry`, materials and `DataTexture` objects, and owns interaction/highlight state;
- **WASM/web** receives the same descriptor/blob contract and must not duplicate RAGE parsing or infer missing material/spatial semantics.

Tauri, CLI and browser transports are adapters. They do not define a second render schema.

## Packet layout

`RenderPackage` contains:

1. a serializable `RenderPackageDescriptor`;
2. one contiguous little-endian binary blob.

The descriptor contains:

- scene root metadata;
- instance records with asset references and proven transforms;
- unique asset descriptors;
- mesh records;
- material records;
- deduplicated texture records;
- diagnostics, summary and explicit limits.

Geometry and image bytes are referenced through `RenderBufferView`:

- `offset`;
- `byteLength`;
- `elementType` (`f32`, `u32`, `u8`);
- component count;
- logical element count.

The current schema emits:

- positions: `f32 x 3`;
- normals: `f32 x 3` when proven and count-compatible;
- UV0: `f32 x 2` when proven and count-compatible;
- indices: `u32 x 1`;
- diffuse textures: `u8 x 4` RGBA8 sRGB.

Buffer ranges and material/texture/asset references are validated before encoding and after decoding.

## Binary envelope

`RenderPackage::encode_binary()` emits:

| Bytes | Meaning |
| --- | --- |
| 0..8 | ASCII magic `RLRPKT01` |
| 8..12 | little-endian render schema version |
| 12..16 | little-endian JSON metadata byte length |
| 16..24 | little-endian blob byte length |
| next N | UTF-8 JSON descriptor |
| remainder | raw typed blob |

The decoder rejects:

- invalid magic;
- unsupported schema versions;
- metadata/blob lengths above hard limits;
- length mismatches;
- invalid buffer ranges;
- invalid asset/material/texture references.

The descriptor JSON is metadata, not the legacy preview transport. Geometry and RGBA data remain binary and are not base64 encoded.

## Scene batching

`workspace_scene_render_package_with_game_index` assembles the YMAP scene once, then resolves every unique `assetRef` from that manifest.

Repeated scene instances reference the same asset descriptor. Asset bytes and external texture dictionaries are prepared once per unique asset during package construction.

This replaces the old consumer pattern of invoking scene assembly and JSON preview independently for each node/asset.

## Limits

Default render-package limits:

- assets: 96, hard maximum 512;
- binary blob: 128 MiB, hard maximum 512 MiB;
- per-asset primitive/vertex/index/shader limits reuse `PreviewOptions`;
- diffuse textures: 32 per asset;
- decoded diffuse texture dimension: 256 pixels maximum on the longest axis.

Budget exhaustion is explicit in diagnostics. Unsupported or unresolved semantics are not guessed.

## Diagnostics and compatibility JSON

`AssetPreviewReport` and `SceneManifestReport` remain supported diagnostic/automation contracts.

They are intentionally separate from the render transport:

- preview JSON may contain arrays and base64 RGBA for human/tool inspection;
- render packages carry the same proven semantics in typed metadata plus raw bytes;
- existing JSON fields are preserved while consumers migrate.

## CLI

A scene package can be exported with:

```bash
ragelab workspace render-package ./stream map.ymap \
  --output scene.rlrender \
  --max-nodes 10000 \
  --max-assets 96 \
  --max-blob-bytes 134217728 \
  --json
```

The command accepts the same fallback, RPF mount and GTA game-index source options as `workspace scene`. Existing output is not overwritten unless `--overwrite` is explicit.

The JSON response reports package counts, limits, blob size, encoded size and diagnostics. The binary file is the renderer transport.
