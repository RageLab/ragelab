# Headless Preview

RageLab exposes renderer-neutral Legacy/Gen8 preview data for software agents and non-graphical consumers. The preview contract serializes normalized Rust domain models; it does not expose Three.js objects or browser rendering state.

Preview construction and limit policy are owned by `ragelab-engine`. CLI, Studio, and future MCP adapters consume the same `PreviewOptions` and `AssetPreviewReport` contracts rather than implementing format-specific preview logic independently.

## Command

```bash
ragelab preview asset.ydr --json
ragelab preview dictionary.ydd --drawable-index 0 --json
ragelab preview collision.ybn --json
```

YDD requires an explicit zero-based `--drawable-index`. YDR rejects that selector.

## Model preview

YDR and selected YDD drawables report:

- selected LOD and source coordinate convention;
- drawable bounds;
- total shader, primitive, vertex, index, and triangle counts;
- bounded shader and texture-reference metadata;
- bounded primitive metadata including topology, vertex declaration, shader binding, and winding summary;
- positions, normals, UV0, and indices when the complete primitive fits the remaining geometry budget.

Isolated drawable preview data is always `localOnly`. RageLab does not infer a world transform from filenames, bounds, dictionary membership, or workspace relationships.

## Collision preview

Legacy YBN preview reports renderer-neutral collision data:

- root and child bounds;
- child type and per-child vertex/triangle/shape counts;
- bounded collision material metadata;
- bounded mesh positions and material-grouped triangle indices;
- sphere, capsule, box, and cylinder shape primitives;
- source polygon index when available.

YBN preview is always `localOnly`. The normalized coordinates already include the transforms encoded inside the collision resource, but RageLab does not infer any placement of an isolated YBN in world space.

When a preview is placed through an engine-owned YMAP `SceneManifest`, world placement comes only from the parsed entity transform. Legacy `CEntityDef.scaleXY` and `scaleZ` are decoded and propagated as `[scaleXY, scaleXY, scaleZ]`. If only one scale component is present, RageLab marks that entity placement unresolved rather than inferring the missing value.

The mesh position array is all-or-nothing. If the complete global position array exceeds `--max-vertices`, positions are omitted and all mesh indices are also omitted. This prevents emitted indices from referring to vertices that are absent from the response.

The `--max-primitives` budget is shared by mesh primitive groups and shape primitives. Mesh groups consume the budget first, followed by shape primitives.

## Geometry integrity

RageLab never emits a partially sliced primitive geometry. A primitive's complete vertex/index payload is included only when both fit within the remaining budgets. Otherwise its metadata is emitted with:

```json
{
  "geometryIncluded": false,
  "geometryOmittedReason": "preview limits",
  "geometry": null
}
```

This prevents indices from referencing vertices omitted by response truncation.

## Limits

Defaults:

| Limit | Default | Hard maximum |
| --- | ---: | ---: |
| primitives | 64 | 512 |
| vertices | 10,000 | 100,000 |
| indices | 30,000 | 300,000 |
| shaders | 128 | 1,024 |
| texture references | 512 | 4,096 |
| children | 256 | 4,096 |
| materials | 512 | 8,192 |

Override them with:

```text
--max-primitives <n>
--max-vertices <n>
--max-indices <n>
--max-shaders <n>
--max-texture-references <n>
--max-children <n>
--max-materials <n>
```

All limits must be positive. Hard maxima are validated by `ragelab-engine`, so every adapter receives the same bounded behavior. Requests above the hard maximum fail with `invalid_input` rather than generating an unbounded JSON response.

## Compatibility

The current preview contract targets GTA V Legacy/Gen8 YDR resource version 165, Legacy YDD dictionaries containing supported YDR drawables, and Legacy PC YBN resource version 43. Enhanced/Gen9 is outside the active compatibility contract.
