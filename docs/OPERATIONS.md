# Declarative Operations

RageLab supports versioned operation documents for agent-driven, reproducible mutations.

The current mutation scope targets GTA V Legacy/Gen8 assets. Enhanced/Gen9 support is intentionally deferred to a separate future milestone.

The operation engine lives in `ragelab-engine`. CLI, MCP, and desktop adapters must use the same planner and writer policy.

## Schema

The current document schema is `ragelab.operation` version `1`.

```json
{
  "schema": "ragelab.operation",
  "schemaVersion": 1,
  "source": "model.ydr",
  "output": "model-translated.ydr",
  "operations": [
    {
      "type": "ydr.translate",
      "delta": [1.0, 0.0, 0.0]
    }
  ]
}
```

Unknown top-level fields are rejected. Unknown operation types are preserved by parsing but fail closed during planning.

Relative `source` and `output` paths are resolved relative to the operation document, not the caller's current working directory.

## Plan

```bash
ragelab plan operation.json --json
```

Planning:

- reads and parses the source asset;
- evaluates writer eligibility from Rust core capability logic;
- validates operation parameters;
- reports every intended output;
- does not create or modify asset files.

A valid plan can return `allowed: false`. That is a successful planning result and therefore exits with status `0`. Callers must inspect `data.allowed` before applying.

Example plan data:

```json
{
  "schema": "ragelab.operation.plan",
  "schemaVersion": 1,
  "assetType": "YDR",
  "allowed": true,
  "nonDestructive": true,
  "outputExists": false,
  "operations": [
    {
      "index": 0,
      "type": "ydr.translate",
      "allowed": true,
      "reason": null,
      "details": {
        "delta": [1.0, 0.0, 0.0]
      }
    }
  ]
}
```

## Apply

```bash
ragelab apply operation.json --json
```

Apply always plans again immediately before mutation.

The current writer contract is non-destructive:

- `source` is never opened for writing;
- `output` must differ from an existing source by being a new file;
- an existing output is rejected;
- output creation uses create-new semantics and never overwrites;
- the rewritten asset is semantically reopened before it is written;
- source bytes are checked again after output creation.

An apply rejection is reported as the CLI `unsupported` error class with exit status `3`.

## Supported operations

### `ydr.translate`

Rigidly translates a supported legacy YDR.

```json
{
  "type": "ydr.translate",
  "delta": [1.0, 2.0, 3.0]
}
```

Requirements:

- source type must be YDR;
- `delta` must contain exactly three finite f32 values;
- zero translation is rejected;
- the YDR must pass the existing `rigid_translation_capability` gate;
- topology, shader bindings, non-position vertex attributes, embedded texture metadata, and bounds invariants are checked after rewrite.

Multiple `ydr.translate` operations may be listed in one document. They are applied in order and the final semantic verification uses the effective combined translation.

### `ydr.rebind-texture`

Rebinds an existing texture parameter to the TextureBase already referenced by another compatible binding in the same Legacy YDR.

```json
{
  "type": "ydr.rebind-texture",
  "sourceShader": 0,
  "sourceParameter": 0,
  "targetShader": 0,
  "targetParameter": 1
}
```

Requirements:

- source and target bindings must already exist;
- source and target parameter hashes must match;
- the source TextureBase must be uniquely referenced so aliases are not modified implicitly;
- the target must already have a valid texture name;
- no new TextureBase, string, shader parameter, or resource layout is created.

### `ydr.rebind-shader`

Rebinds an existing geometry to another shader already present in the same Legacy YDR.

```json
{
  "type": "ydr.rebind-shader",
  "modelIndex": 0,
  "geometryIndex": 0,
  "targetShaderIndex": 1
}
```

Requirements:

- the selected geometry binding must exist;
- `targetShaderIndex` must reference an existing shader;
- rebinding to the shader already in use is rejected;
- no shader definitions, parameter blocks, geometry, or topology are created.

YDR operations are simulated in document order during `plan`. A multi-operation document must serialize and semantically reopen successfully in memory before the plan is marked allowed. `apply` repeats the same sequence against a fresh edit session and preserves create-new output semantics.

### `ydd.translate`

Rigidly translates one selected drawable inside a Legacy YDD.

```json
{
  "type": "ydd.translate",
  "drawableIndex": 0,
  "delta": [1.0, 0.0, 0.0]
}
```

### `ydd.rebind-texture`

Rebinds an existing texture parameter inside one selected YDD drawable to another compatible existing TextureBase.

```json
{
  "type": "ydd.rebind-texture",
  "drawableIndex": 0,
  "sourceShader": 0,
  "sourceParameter": 0,
  "targetShader": 0,
  "targetParameter": 1
}
```

### `ydd.rebind-shader`

Rebinds an existing geometry inside one selected YDD drawable to another shader already present in that drawable.

```json
{
  "type": "ydd.rebind-shader",
  "drawableIndex": 0,
  "modelIndex": 0,
  "geometryIndex": 0,
  "targetShaderIndex": 1
}
```

YDD requirements:

- `drawableIndex` must reference an existing dictionary entry;
- selected-drawable writer eligibility is evaluated by `YddEditSession`;
- texture and shader rebind rules are identical to the corresponding YDR safety rules;
- dictionary entry count, hash, name, and drawable identity metadata must survive semantic re-open;
- operations are applied in document order to intermediate in-memory YDD bytes, so one document may safely target more than one drawable;
- no output file is created during planning.

## Fail-closed behavior

Unsupported operations do not fall back to guessed binary layouts.

For example:

```json
{
  "type": "ydr.rotate",
  "degrees": 90
}
```

is parsed as an operation request, but planning returns that operation as blocked with an explicit unsupported-operation reason. No output is written.

## Adapter rule

Operation schemas are transport-neutral application contracts.

Adapters may:

- load an operation document;
- display plan results;
- request apply;
- display validation and rejection reasons.

Adapters must not duplicate or override operation eligibility, binary rewrite rules, or semantic verification.
