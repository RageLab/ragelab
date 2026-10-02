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

The discovery commands `version --json` and global `capabilities --json` retain their dedicated version-1 schemas for compatibility. Per-file `capabilities <file> --json`, `inspect <file> --json`, and `validate <file> --json` use the common response envelope.

## Agent-first surface

The primary structured workflow is:

```text
inspect -> capabilities -> plan -> apply -> validate
```

The current contract implements the first, second, and final stages:

```bash
ragelab inspect <file> --json
ragelab capabilities <file> --json
ragelab validate <file> --json
```

`plan` and `apply` are reserved for the declarative operation contract.

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
- `requiresWorkspace`.

A capability entry reports availability in the current binary. It does not override format-specific write eligibility. Write operations still fail closed when the asset layout is unsupported.

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
