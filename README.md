# RageLab

RageLab is an open-source Rust toolkit for inspecting, validating, transforming, and exporting assets used by the RAGE ecosystem.

The command-line interface is the primary product surface. RageLab is designed for deterministic automation, scripting, and software agents without requiring Blender, a browser, or a graphical editor. RageLab Studio is a separate desktop application built on top of the same Rust capabilities.

## Principles

- **Core first.** Format parsing, writing, dependency resolution, and safety policy live in Rust libraries.
- **Automation first.** Product capabilities must be callable without a graphical interface.
- **Fail closed.** Unsupported binary layouts remain read-only instead of relying on unsafe assumptions.
- **Non-destructive by default.** Editing operations write new outputs unless an operation explicitly documents otherwise.
- **Structured contracts.** Machine-readable CLI contracts are versioned and intended for stable integration.
- **No proprietary assets.** The repository contains only source code, documentation, and synthetic fixtures.

## Current format coverage

RageLab currently contains Rust support for:

- RSC7 resources
- META and PSO containers
- YMAP
- YTYP
- YMF
- YTD
- YBN
- YDR
- YDD
- GLB 2.0 input (inspection, pre-export validation with in-memory YDR semantic preflight, static geometry decoding, embedded PNG extraction and YDR v165 export with optional exact Legacy `normal.sps`, `spec.sps`, `normal_spec_tnt.sps`, `emissivenight.sps` or `emissive.sps` cloning from an external YFT/YDR; narrow sampler/material bindings including optional `tintPalette` and emissiveFactor/Strength, full PBR and GTA runtime renderability not established)

Current writer coverage is intentionally narrower than reader coverage. Each writer operation is gated by format-specific validation and provenance requirements.

## CLI

Build the CLI:

```bash
cargo build -p ragelab
```

The primary automation flow is:

```bash
ragelab inspect asset.ydr --json
ragelab ydr inspect asset.ydr --json
ragelab ydr validate asset.ydr --strict --json
ragelab glb inspect model.glb --json
ragelab glb validate model.glb --json
ragelab glb geometry model.glb --json
ragelab glb profile init model.glb --output model.gta-materials.json
ragelab glb profile validate model.glb model.gta-materials.json --json
ragelab glb export model.glb --profile model.gta-materials.json --output model.ydr --json
ragelab glb export model.glb --profile model.gta-materials.json --shader-template template.yft --shader-index 0 --output model.ydr --json
ragelab glb batch batch.json --json
ragelab capabilities asset.ydr --json
ragelab spatial asset.ydr --json
ragelab preview asset.ydr --json
ragelab plan operation.json --json
ragelab apply operation.json --json
ragelab validate output.ydr --json
```

Declarative mutations are versioned, fail closed, and non-destructive by default. The operation document contract is documented in [docs/OPERATIONS.md](docs/OPERATIONS.md).

Canonical format and workspace commands use namespaces:

```bash
ragelab ydr inspect asset.ydr --json
ragelab ydr validate asset.ydr [--strict] [--json]
ragelab ydr info asset.ydr
ragelab ymap info map.ymap
ragelab ytd info textures.ytd
ragelab workspace deps ./stream map.ymap
ragelab workspace preflight ./stream map.ymap --json
ragelab workspace export ./stream map.ymap --output ./build/my-resource --json
ragelab workspace scene ./stream map.ymap --json
ragelab gta discover --json
ragelab fivem discover --json
ragelab gta catalog /path/to/assets --output ./build/gta-paths.txt --json
```

Global discovery:

```bash
ragelab --help
ragelab version --json
ragelab capabilities --json
```

Legacy flat command names remain available as compatibility aliases during the 0.x series. New integrations should use the canonical namespace form.

Structured commands return versioned JSON and deterministic process exit codes. The contract is documented in [docs/CLI_CONTRACT.md](docs/CLI_CONTRACT.md), native game discovery in [docs/DISCOVERY.md](docs/DISCOVERY.md), GTA catalog coverage in [docs/CATALOG.md](docs/CATALOG.md), headless preview behavior in [docs/PREVIEW.md](docs/PREVIEW.md), workspace export behavior in [docs/EXPORT.md](docs/EXPORT.md), current format/adapter coverage in [docs/CAPABILITIES.md](docs/CAPABILITIES.md), the GLB import/material-authoring and reproducible export-report workflow in [docs/GLB_IMPORT.md](docs/GLB_IMPORT.md), and the strict multi-asset batch pipeline in [docs/GLB_BATCH.md](docs/GLB_BATCH.md).

## Repository layout

```text
apps/
  ragelab/           CLI

crates/
  ragelab-engine/    transport-neutral application policy
  ragelab-gltf/      GLB 2.0 inspection, geometry decoding, PNG extraction and material profiles
  ragelab-assets/    workspace indexing, dependency resolution, export
  ragelab-resource/  RSC7 container support
  ragelab-meta/      META primitives
  ragelab-pso/       PSO primitives
  ragelab-rbf/       RBF support
  ragelab-ymap/
  ragelab-ytyp/
  ragelab-ymf/
  ragelab-ytd/
  ragelab-ybn/
  ragelab-ydr/
  ragelab-ydd/

fixtures/
  synthetic/
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for package boundaries.

## Development

The workspace targets Rust 1.80 and forbids unsafe Rust at workspace level.

Standard locked gates:

```bash
python scripts/validate_synthetic.py
python scripts/validate_synthetic_ymf.py
python scripts/check_hardening.py
cargo fmt --all -- --check
cargo check --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

Synthetic fixtures are used for reproducible parser and writer regression coverage. Proprietary game data must not be committed. Fixture policy, deterministic mutation coverage, parser/resource budgets, and the separation between structural, semantic, CodeWalker, GUI, and runtime acceptance are documented in [docs/QUALITY.md](docs/QUALITY.md).

## RageLab Studio

RageLab Studio is maintained in a separate repository. It provides desktop visualization and editing workflows through Tauri, Svelte, and Three.js. Binary format knowledge remains in RageLab; the Studio must not reimplement RAGE parsers or writers in TypeScript.

## Legal

RageLab is an independent open-source project and is not affiliated with or endorsed by Rockstar Games or Take-Two Interactive.

Grand Theft Auto, Rockstar Games, RAGE, and related names and marks belong to their respective owners.

RageLab is licensed under the Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
