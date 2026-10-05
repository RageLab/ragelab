# CLI Contract

RageLab treats the command-line interface as a public automation API.

## Contract version

Structured asset commands use a common response envelope:

```json
{
  "schema": "ragelab.cli.response",
  "schemaVersion": 1,
  "ok": true,
  "command": "inspect",
  "data": {}
}
```

Errors use the same schema:

```json
{
  "schema": "ragelab.cli.response",
  "schemaVersion": 1,
  "ok": false,
  "command": "inspect",
  "error": {
    "code": "invalid_input",
    "message": "..."
  }
}
```

Consumers must key compatibility decisions on `schema` and `schemaVersion`, not on human-readable output.

The discovery commands `version --json` and global `capabilities --json` retain their dedicated version-1 schemas for compatibility. Per-file `capabilities <file> --json`, `inspect <file> --json`, `validate <file> --json`, `spatial <file> --json`, `preview <file> --json`, `plan <operation.json> --json`, `apply <operation.json> --json`, `workspace preflight ... --json`, `workspace export ... --json`, `workspace scene ... --json`, `workspace render-package ... --json`, `render asset ... --json`, `render scene ... --json`, `render compare ... --json`, `gta discover --json`, `gta catalog ... --json`, and `fivem discover --json` use the common response envelope.

## Agent-first surface

The primary structured workflow is:

```text
inspect -> capabilities -> plan -> apply -> validate
```

The current contract implements the complete control loop:

```bash
ragelab inspect <file> --json
ragelab capabilities <file> --json
ragelab plan <operation.json> --json
ragelab apply <operation.json> --json
ragelab validate <output-file> --json
```

Declarative operation documents are specified in [OPERATIONS.md](OPERATIONS.md).

## Command taxonomy

Canonical commands use namespaces for format- and workspace-specific operations:

```text
ragelab ydr ...
ragelab ydd ...
ragelab ytd ...
ragelab ybn ...
ragelab ymap ...
ragelab ytyp ...
ragelab ymf ...
ragelab workspace ...
ragelab gta ...
ragelab fivem ...
```

Examples:

```bash
ragelab ydr info prop.ydr
ragelab ydr translate source.ydr 1 0 0 output.ydr
ragelab ytd extract-dds textures.ytd 0 texture.dds
ragelab workspace deps ./stream map.ymap
ragelab workspace scene ./stream map.ymap --json
```

Legacy flat command names from the initial RageLab extraction remain compatibility aliases during the 0.x series. New integrations should use the canonical namespace form.

## Discovery

Global command discovery:

```bash
ragelab version --json
ragelab capabilities --json
```

Native GTA V Legacy installation discovery:

```bash
ragelab gta discover --json
```

The GTA discovery command returns the versioned `ragelab.gta.discovery` payload inside the common response envelope. It reports every candidate root, merged provenance, detected edition, filesystem validation checks, Steam metadata when available, and the number of valid Legacy installations.

FiveM discovery is available through:

```bash
ragelab fivem discover --json
```

It returns the versioned `ragelab.fivem.discovery` payload, including installation/storage evidence and the GTA relationship derived from `FiveM.app/CitizenFX.ini` `IVPath`. The relationship is accepted as Legacy only when the referenced GTA path passes the same Legacy validator used by `gta discover`. See [DISCOVERY.md](DISCOVERY.md).

Native filesystem catalog construction is available through:

```bash
ragelab gta catalog <directory> --output <paths.txt> --json
```

The `ragelab.gta.catalog` report distinguishes complete loose-filesystem enumeration from the explicit `partialRpfBoundary` state. Raw/encrypted RPF contents are not silently treated as enumerated. See [CATALOG.md](CATALOG.md).

Global `capabilities --json` exposes:

- `commands`: every currently accepted command ID, including compatibility aliases;
- `canonicalCommands`: preferred command IDs for new integrations;
- `structuredOutput`: commands with versioned machine-readable output;
- `legacyAliases`: flat command IDs retained for compatibility;
- `responseEnvelope`: the current common response schema.

Per-asset discovery:

```bash
ragelab capabilities model.ydr --json
```

Each operation reports:

- `id`;
- `writesAsset`;
- `structuredOutput`;
- `requiresWorkspace`;
- `availability`;
- `reason`;
- `requiresParameters`.

`availability` is one of `available`, `parameterized`, `contextRequired`, or `unavailable`. The value is derived from the current asset when RageLab has enough evidence to decide. Parameterized and workspace operations defer final eligibility until the required selection or context is supplied.

Capability discovery never overrides format-specific write eligibility. Write operations still fail closed when the selected layout, binding, polygon, replacement, or provenance is unsupported.

## Plan and apply

`plan` and `apply` consume a versioned `ragelab.operation` document.

`plan` is read-only and may return `allowed: false` while still exiting successfully because planning itself completed. `apply` plans again, requires an allowed plan, and uses non-overwriting output creation.

See [OPERATIONS.md](OPERATIONS.md) for schema and writer guarantees.

## Inspect

`inspect` identifies the file type, container information, and a bounded format summary.

Example:

```bash
ragelab inspect map.ymap --json
```

The command parses known formats using the same Rust crates used by other RageLab adapters. It does not infer unsupported layouts.

## Validate

`validate` performs the validation currently available for the detected format.

For RSC7-backed assets this includes container decompression before the typed format parser when applicable.

Example:

```bash
ragelab validate textures.ytd --json
```

A successful response contains `valid: true` and the checks that ran. Validation failures return a non-zero exit status and a structured error when `--json` is requested.

## Spatial context

`spatial` reports only placement that is supported by format evidence.

```bash
ragelab spatial map.ymap --json
ragelab spatial prop.ydr --json
```

YMAP may produce world-space bounds or centers. Isolated YDR, YDD, and YBN assets remain `localOnly`; YTD is `nonSpatial`. RageLab does not promote local bounds or filename relationships into world coordinates.

## Headless preview

`preview` exposes bounded renderer-neutral model data for headless consumers.

```bash
ragelab preview prop.ydr --json
ragelab preview props.ydd --drawable-index 0 --json
```

YDR preview is immediately available for supported Legacy drawables. YDD requires an explicit zero-based drawable selector. Geometry is never partially sliced: complete primitive geometry is emitted only when it fits the configured vertex/index budgets; otherwise only primitive metadata is returned.

Preview coordinates remain local to the asset and include the source coordinate convention. The command does not infer world placement.

See [PREVIEW.md](PREVIEW.md) for limits and schema semantics.

## Workspace preflight and export

`workspace preflight` and `workspace export` expose the engine-owned dependency closure and resource export pipeline to software agents.

```bash
ragelab workspace preflight ./stream map.ymap --json
ragelab workspace export ./stream map.ymap --output ./build/resource --json
ragelab workspace export ./stream map_a.ymap map_b.ymap --output ./build/combined --json
```

Preflight is read-only and reports the combined closure, unresolved classification, MLO audit summaries, warnings, and whether export requires an explicit unresolved override.

Export supports one or multiple selected YMAP roots. Unknown external dependencies fail closed unless `--allow-unresolved` is supplied. Export-gate rejections use exit code `3` / `unsupported` in structured mode and do not create the destination.

A successful export response includes generated paths, copied-file counts, unresolved classification, and post-export validation. Automation must inspect `data.validation.valid`; warnings do not imply failure.

See [EXPORT.md](EXPORT.md) for the full contract and output layout.

## Workspace scene

`workspace scene` assembles normalized YMAP instance/reference metadata from a workspace index.

```bash
ragelab workspace scene ./stream map.ymap --max-nodes 10000 --json
```

The scene contract reports resolved and unresolved nodes, referenced assets, proven entity transforms, local-only collision relationships, warnings, and explicit truncation limits. It does not embed model geometry.

`workspace render-package` consumes the same scene resolution inputs, assembles the scene once, and writes the versioned renderer-neutral binary package used by native, Three.js and WASM consumers:

```bash
ragelab workspace render-package ./stream map.ymap --output scene.rlrender --json
```

Existing output is preserved unless `--overwrite` is explicit. Geometry, indices and RGBA pixels are stored in the binary blob rather than JSON/base64. See [RENDER_CONTRACT.md](RENDER_CONTRACT.md).

## Native rendering

`render asset` renders supported YDR/YDD/YFT model packets offscreen. YDD requires `--drawable-index`. Because isolated files have no workspace dependency context, external YTDs are not guessed.

`render scene` resolves a YMAP through the same workspace/RPF/game-index rules as `workspace scene`, builds one shared `RenderPackage`, and renders its instances through wgpu. Both commands write PNG plus deterministic metadata sidecar and are non-destructive unless `--overwrite` is explicit.

`render compare` reports RGBA8 visual differences with an explicit per-channel tolerance. Use tolerance 0 for same-backend determinism. See [RENDERER.md](RENDERER.md).

## Exit codes

RageLab reserves the following process exit codes for the CLI contract:

| Code | Meaning |
| ---: | --- |
| 0 | Success |
| 1 | Operational failure |
| 2 | Invalid command or input |
| 3 | Unsupported operation or format |
| 4 | Validation or parse failure |

Callers must treat the process exit status as authoritative even when JSON output is enabled.

## Error codes

The common envelope currently emits:

- `operation_failed`
- `invalid_input`
- `unsupported`
- `validation_failed`

Error codes are stable identifiers. Error messages are diagnostic text and may become more specific without a schema version change.

## Stability rules

- Human-readable output may improve between releases.
- Versioned JSON fields are compatibility contracts.
- Existing fields are not silently repurposed.
- Breaking structured-output changes require a schema version increment.
- New optional fields may be added when existing consumers remain valid.
- Existing accepted command IDs are not removed from a versioned discovery contract without a compatibility transition.
- Writer safety is decided by Rust core logic, never by the CLI adapter or an external caller.
- Source assets are not overwritten implicitly by agent-oriented mutation flows.

## Structured-output expansion

Structured output will be extended command by command. The target is that every inspection, planning, validation, and transformation operation required by software agents can be executed without parsing human-oriented text.
