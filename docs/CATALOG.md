# GTA Legacy Catalog

RageLab can construct a deterministic vanilla-style file catalog from asset paths that are directly visible on the filesystem.

The active compatibility scope is GTA V Legacy / Gen8.

## CLI

```bash
ragelab gta catalog <directory> --output <paths.txt> --json
```

Optional flags:

- `--overwrite` — replace an existing output file explicitly;
- `--json` — return the versioned machine-readable report.

Without `--overwrite`, the command uses create-new output semantics and refuses to replace an existing file.

The legacy compatibility command remains available:

```bash
ragelab gta vanilla-index <extracted-directory> <paths.txt>
```

New integrations should use `gta catalog`.

## Report contract

The structured response uses the common `ragelab.cli.response` envelope. `data` uses:

```json
{
  "schema": "ragelab.gta.catalog",
  "schemaVersion": 1
}
```

Important fields:

- `root` — normalized source root;
- `sourceKind` — `filesystemTree` or `gtaLegacyInstallation`;
- `coverage` — `filesystemTreeComplete` or `partialRpfBoundary`;
- `complete` — whether every supported asset visible through the current filesystem enumeration path was cataloged without an RPF boundary;
- `scannedFiles` — regular files visited;
- `supportedLooseFiles` — directly visible supported asset files;
- `pathEntries` — deterministic path lines written to the output;
- `uniqueCatalogEntries` — unique `(AssetKind, JOAAT(filename stem))` identities;
- `ignoredFiles` — visible files outside the supported asset extensions and RPF boundary;
- `archiveBoundary` — RPF count, bounded examples, enumeration support, and the explicit limitation reason;
- `gtaInstallation` — included when the source root passes the GTA V Legacy installation validator;
- `output` and `outputBytes` — CLI output artifact metadata.

`pathEntries` may be larger than `uniqueCatalogEntries`. Multiple paths can share the same asset kind and filename hash. RageLab preserves path-list evidence while the runtime catalog uses the existing `(AssetKind, JOAAT(stem))` identity model.

## Supported loose-file types

The current catalog identity model recognizes:

- YMAP
- YTYP
- YDR
- YDD
- YTD
- YBN
- YFT
- YCD

Extensions are matched case-insensitively. Output paths are normalized to `/`, sorted, and deduplicated.

## Coverage semantics

### `filesystemTreeComplete`

No RPF files were encountered. RageLab enumerated all directly visible supported files beneath the supplied filesystem tree.

This describes coverage of the supplied filesystem tree only. It does not claim that the tree represents every vanilla GTA asset.

### `partialRpfBoundary`

One or more `.rpf` archives were encountered. RageLab catalogs supported loose files that are directly visible, counts the RPF archives, and reports bounded archive-path examples, but it does not enumerate archive contents.

For a normal GTA V Legacy installation this is the expected status today.

Automation must inspect `complete` or `coverage`; it must not treat an output produced from `partialRpfBoundary` as a complete vanilla asset catalog.

## Raw RPF boundary

RageLab does not currently decrypt or enumerate the raw/encrypted GTA RPF archive set for catalog construction.

The report therefore exposes:

```json
{
  "archiveBoundary": {
    "rpfArchivesFound": 3,
    "enumerationSupported": false,
    "reason": "..."
  }
}
```

This is a deliberate fail-closed boundary. Discovering a valid GTA installation does not silently imply complete vanilla catalog coverage.

Complete raw-install catalog construction can be added later behind the same report contract when RPF enumeration is implemented and validated.

## Engine ownership

`ragelab-engine` owns catalog construction and coverage classification. CLI, Studio, and future MCP adapters must call the same builder instead of independently walking directories or interpreting RPF coverage.
