# Workspace Export

RageLab exposes Legacy/Gen8 workspace dependency closure and FiveM resource export through `ragelab-engine`. The CLI is an adapter over those engine APIs; it does not reproduce resolver, catalog, manifest, GTXD, MLO, or validation policy.

## Preflight

Use preflight before export:

```bash
ragelab workspace preflight ./stream map.ymap --json
ragelab workspace preflight ./stream map_a.ymap map_b.ymap --json
```

Preflight accepts one or more selected YMAP roots and reports:

- selected roots;
- transitive YMAP closure;
- predicted local files;
- raw, catalog-confirmed vanilla, and unknown unresolved counts;
- grouped unknown dependencies and affected maps;
- MLO audit summaries;
- warnings;
- whether export is allowed without `--allow-unresolved`.

Catalog options are optional:

```text
--durtyfree-object-list <ObjectList.ini>
--vanilla-file-index <paths.txt>
```

Catalogs classify known vanilla references. They do not make missing local files appear present and do not bypass unknown external dependencies.

## Export

The canonical command supports both single-map and combined multi-map exports:

```bash
ragelab workspace export ./stream map.ymap \
  --output ./build/my-resource \
  --json

ragelab workspace export ./stream map_a.ymap map_b.ymap \
  --output ./build/combined-resource \
  --resource-name combined-resource \
  --json
```

With one selected root, RageLab uses the single-map engine path. With multiple roots, it uses the existing combined resolver/export path and produces one merged dependency closure and one FiveM resource.

### Options

- `--output <directory>` — required destination resource directory.
- `--resource-name <name>` — optional metadata name; defaults to the output directory name when available.
- `--allow-unresolved` — explicitly permits unknown external dependencies that would otherwise block export.
- `--overwrite` — explicitly enables the existing engine overwrite behavior. It is never enabled implicitly.
- catalog options — same classification inputs accepted by preflight.
- `--json` — emits the versioned RageLab CLI response envelope.

## Export gate

Unknown external dependencies block export by default. In JSON mode an export-gate rejection uses exit code `3` and error code `unsupported`. The destination is not created by the gate failure.

`--allow-unresolved` is an explicit override. Catalog-confirmed vanilla references are not counted as unknown; unknown references remain visible in the response.

## Output layout

A successful export is validated by the engine and normally contains:

```text
<resource>/
  fxmanifest.lua
  .ragelab-export.json
  stream/
    <resolved assets>
    _manifest.ymf
    _gtxd.meta        # when generated
```

The structured response reports the actual resource, stream, manifest, metadata, and optional GTXD paths. Consumers should use those returned paths instead of reconstructing them.

## Validation

After writing, RageLab validates the exported resource and reports:

- metadata presence;
- manifest parse validity;
- `fxmanifest.lua` presence;
- selected-root and closure counts;
- copied-file count;
- interior map/bounds counts;
- locally missing files;
- post-export unresolved classification;
- warnings and errors.

`validation.valid` is part of the operation result and must be inspected by automation. A completed export may contain warnings, especially when `--allow-unresolved` was explicitly used.

## Compatibility

The active export contract targets GTA V Legacy/Gen8 assets. Enhanced/Gen9 support is intentionally deferred to a separate future milestone.

Legacy `workspace extract` remains available during the 0.x compatibility period for the original single-map human-oriented workflow. New automation should use `workspace export`.
