# Capability Matrix

This document tracks the public RageLab capability surface. It distinguishes core support from CLI exposure and agent-facing structured contracts. For format-by-format reader/writer scope, operation availability and known gaps, see [FORMAT_SUPPORT_MATRIX.md](FORMAT_SUPPORT_MATRIX.md).

The current compatibility target is GTA V Legacy/Gen8. Enhanced/Gen9 is not part of the active support contract.

## Availability states

`ragelab capabilities <file> --json` reports an `availability` value for each operation:

- `available` — the operation is supported for the current asset without additional eligibility checks.
- `parameterized` — the operation exists, but final eligibility depends on the requested target, index, binding, replacement, or edit parameters.
- `contextRequired` — the operation requires a workspace root or another external context.
- `unavailable` — the current asset does not satisfy the writer or capability gate. The response includes a reason when available.

Writer eligibility remains fail-closed. A listed operation is not permission to bypass `plan`, writer validation, or format-specific provenance checks.

## Current surface

| Domain | Read / inspection | Mutation / export | Agent-first structured surface | Status |
| --- | --- | --- | --- | --- |
| GLB 2.0 input | GLB container, scenes/nodes, static geometry decoding, UV0/UV1/UV2, normals/tangents/colors, PBR inventory and embedded PNG extraction; `glb.validate` performs structural inspection, active-scene geometry/image decode, and in-memory YDR writer + semantic reopen preflight with separate invalid/unsupported/warning diagnostics | Material profile; YDR v165 geometry + embedded TextureDictionary (RGBA8/BC1/BC3, full mips); optional clone of exact Legacy `normal.sps`, `spec.sps`, `normal_spec_tnt.sps`, `emissivenight.sps` or `emissive.sps` signatures from external YFT/YDR with narrow sampler/material binding; optional `tintPalette` and constrained emissiveFactor/Strength mappings; detail, glass, alpha, full PBR and other surface models remain unsupported | glb.inspect, glb.validate, glb.geometry, glb.profile.init, glb.profile.validate, glb.export | Structural export with five exact supported ShaderFX template signatures; `glb.validate` schema v1; GTA/FiveM runtime renderability not established |
| RSC7 | probe, container validation | resource re-encode primitives in core | `inspect`, `validate` | Available |
| YDR | `ydr.inspect` / `ydr.validate`: RSC7 version/flags, Drawable and all four LOD slots, models/geometries, bounds, vertex declarations/stride/semantic slots, vertex/index/triangle counts, ShaderFX hashes/parameters/samplers, embedded TextureDictionary descriptors and mip decode; versioned inspection/validation reports | rigid translation, texture rebind, shader rebind | `ydr.inspect`, `ydr.validate` (`--strict`), `spatial`, `preview`, `capabilities`; declarative `ydr.translate`, `ydr.rebind-texture`, and `ydr.rebind-shader` through `plan/apply` | Available for current controlled writer/preview scope. Semantic reopen is an in-memory RSC7 round-trip check; CodeWalker GUI and runtime acceptance remain false/not asserted. |
| YDD | dictionary/drawable inspection, validate, spatial context, bounded selected-drawable model preview | selected drawable translation, texture rebind, shader rebind | `inspect`, `validate`, `spatial`, parameterized `preview`, `capabilities`; declarative `ydd.translate`, `ydd.rebind-texture`, and `ydd.rebind-shader` through `plan/apply` | Available for current controlled writer/preview scope |
| YTD | structured per-texture inventory, DDS extraction, RGBA top-mip preview support in core | DDS replacement, relocation/repack, RGBA repack, compact rebuild | `inspect`, `validate`, `spatial`, `capabilities`; declarative `ytd.replace-dds`, `ytd.repack-dds`, `ytd.repack-rgba`, and `ytd.rebuild-compact` through `plan/apply` | Available for current Legacy writer scope |
| YBN | collision inspection, validate, spatial context, bounded normalized collision preview | conservative sphere/capsule/cylinder radius edits and same-child material edits | `inspect`, `validate`, `spatial`, `preview`, `capabilities`; declarative `ybn.edit-polygon` through `plan/apply` | Available for current controlled writer/preview scope |
| YMAP | inspect, validate, dependency analysis, spatial context | `ymap.set-transform`, `ymap.set-property`, `ymap.delete`, `ymap.duplicate`, `ymap.create` through non-destructive plan/apply; single and combined workspace export | `inspect`, `validate`, `spatial`, `capabilities`, `plan`, `apply`, parameterized `ymap.*`, `workspace preflight --json`, `workspace export --json`, `workspace scene --json` | Available for current read, authoring and export scope; workspace operations remain context-required |
| YFT | Legacy v162 FragType and optional mainDrawable inspection/validation only | none; physics children, damaged variants and articulation unsupported | `inspect`, `validate`, `capabilities`, `yft.info` | Read-only, constrained to Legacy v162/mainDrawable |
| YCD / YMT / YLD / YNV | RSC7 container-only inspection when decompression succeeds; no semantic format parser | none | `capabilities`; container-only `inspect`; `validate` unavailable | Container-only / semantic validation unsupported |
| YTYP | inspect and validate | no direct public mutation contract | `inspect`, `validate`, `spatial`, `capabilities` | Read-only |
| YMF | inspect and validate; serializer used by export pipeline | generated by workspace/export flows | `inspect`, `validate`, `capabilities` | Generated internally |
| Workspace | scan, providers, dependency closure, combined preflight, MLO audit, scene assembly | single and combined FiveM resource export | `workspace preflight --json`, `workspace export --json`, `workspace scene --json` | Available for current preflight/export scope |
| GTA installation | native Windows Legacy discovery and explicit-path validation | none | `gta discover --json` | Available for Legacy discovery |
| FiveM environment | default/explicit installation discovery, storage evidence, `CitizenFX.ini` metadata, validated GTA Legacy relationship | none | `fivem discover --json` | Available for current Legacy environment scope |
| GTA catalog | engine-owned loose-filesystem enumeration with Legacy-install provenance and explicit RPF boundary reporting | deterministic path catalog output | `gta catalog --json`; `gta vanilla-index` compatibility | Available for loose filesystem trees; partial on raw RPF installations |

## Global build and environment diagnostics

`ragelab doctor [--json]` is the read-only environment/build diagnostic. Its versioned report is defined by [ragelab.cli.doctor.v1.schema.json](schemas/ragelab.cli.doctor.v1.schema.json). It exposes build target/profile/features, Core revision and dirty-at-build state, the compiled module registry, semantic versus container-only asset format support, and environment checks with `info`, `warning` or `blocker` severity.

`ragelab capabilities --json` retains the existing `ragelab.cli.capabilities` v1 schema and adds build metadata, compiled modules, the actual Engine asset-format registry and optional external dependencies. See [ragelab.cli.capabilities.v1.schema.json](schemas/ragelab.cli.capabilities.v1.schema.json).

GPU availability affects only `render.asset` and `render.scene`. GTA V Legacy and FiveM installations are optional target data; their absence does not block Core parsers, validators or GLB/YDR authoring. Incomplete installations warn only on the related command IDs. Doctor does not create files/directories, test write access, modify PATH/environment/Registry, or change game installations. Its wgpu check creates and drops a transient device; native discovery uses the existing read-only GTA/FiveM discovery APIs. Full read-only semantics and build metadata timing are documented in [DOCTOR.md](DOCTOR.md).

## Canonical agent workflow

The intended automation sequence is:

```text
inspect
  -> capabilities
  -> plan
  -> apply
  -> validate
```

Not every writer has been moved to the declarative operation document yet. Legacy CLI commands remain available during the 0.x series while structured equivalents are added.

## Known Legacy catalog boundary

Native catalog construction now works for directly visible filesystem assets and reports GTA Legacy installation provenance when available. A raw GTA installation remains `partialRpfBoundary` because RageLab does not yet enumerate/decrypt the contents of the RPF archive set.

This limitation is explicit in the structured catalog report and is not hidden behind discovery or dependency classification.

Managed browser uploads, HTTP process state, and web-only transport behavior are not CLI parity targets.

## Safety boundary

A capability is considered complete only when the underlying Rust implementation owns its validation and safety policy. CLI, MCP, and Studio adapters must not invent layout support or relax writer eligibility.
