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

The discovery commands `version --json` and global `capabilities --json` retain their dedicated version-1 schemas for compatibility. Per-file `capabilities <file> --json`, `inspect <file> --json`, `validate <file> --json`, `spatial <file> --json`, `plan <operation.json> --json`, `apply <operation.json> --json`, and `workspace scene ... --json` use the common response envelope.

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

Global discovery:

```bash
ragelab version --json
ragelab capabilities --json
```

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

## Workspace scene

`workspace scene` assembles normalized YMAP instance/reference metadata from a workspace index.

```bash
ragelab workspace scene ./stream map.ymap --max-nodes 10000 --json
```

The scene contract reports resolved and unresolved nodes, referenced assets, proven entity transforms, local-only collision relationships, warnings, and explicit truncation limits. It does not embed model geometry.

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
