# Headless Preview

RageLab exposes renderer-neutral Legacy/Gen8 preview data for software agents and non-graphical consumers. The preview contract serializes normalized Rust domain models; it does not expose Three.js objects or browser rendering state.

## Command

```bash
ragelab preview asset.ydr --json
ragelab preview dictionary.ydd --drawable-index 0 --json
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

Override them with:

```text
--max-primitives <n>
--max-vertices <n>
--max-indices <n>
--max-shaders <n>
--max-texture-references <n>
```

All limits must be positive. Requests above the hard maximum fail with `invalid_input` rather than generating an unbounded JSON response.

## Compatibility

The current model preview contract targets GTA V Legacy/Gen8 YDR resource version 165 and Legacy YDD dictionaries that contain supported YDR drawables. Enhanced/Gen9 is outside the active compatibility contract.
