# CLI Contract

RageLab treats the command-line interface as a public automation API.

## Contract version

Machine-readable outputs include both a schema identifier and a numeric `schemaVersion`.

The initial contract version is `1`.

Example:

```json
{
  "schema": "ragelab.cli.version",
  "schemaVersion": 1,
  "product": "RageLab",
  "version": "0.1.0"
}
```

Consumers must key compatibility decisions on the schema identifier and schema version rather than human-readable text.

## Discovery

Two commands provide the initial structured discovery surface:

```bash
ragelab version --json
ragelab capabilities --json
```

`capabilities --json` returns the command IDs available in the current binary and identifies which commands currently provide versioned JSON output.

## Stability rules

- Human-readable output may improve between releases.
- Versioned JSON fields are treated as compatibility contracts.
- Existing fields are not silently repurposed.
- Breaking structured-output changes require a schema version increment.
- New optional fields may be added without changing the schema version when existing consumers remain valid.
- Exit status remains authoritative: successful operations return zero; invalid input or failed operations return non-zero.
- Binary writer safety is decided by Rust core logic, never by an external caller.

## Expansion

Structured output will be extended command by command. The target contract is that every inspection, planning, validation, and transformation operation required by software agents can be executed without parsing human-oriented text.

Future structured operations should use explicit operation identifiers and return enough information for an agent to:

1. inspect current state;
2. determine supported capabilities;
3. plan a change;
4. apply the change to a new output;
5. validate the result.
