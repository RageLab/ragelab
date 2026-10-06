# Architecture

RageLab separates binary format logic, workspace operations, application policy, and adapters.

## Dependency direction

```text
CLI / future MCP / RageLab Studio
              |
              v
        ragelab-engine
              |
              v
        ragelab-assets
              |
      +-------+-------+
      |               |
      v               v
 format crates   shared binary layers
```

The dependency direction is deliberate. Adapters may depend on core libraries; core libraries must not depend on a CLI, desktop UI, HTTP transport, or agent protocol.

## Layers

### Binary and format crates

`ragelab-resource`, `ragelab-meta`, `ragelab-pso`, and `ragelab-rbf` provide reusable binary primitives.

Typed format crates own format-specific parsing, normalization, validation, and writer rules:

- `ragelab-ymap`
- `ragelab-ytyp`
- `ragelab-ymf`
- `ragelab-ytd`
- `ragelab-ybn`
- `ragelab-ydr`
- `ragelab-ydd`

Binary-layout safety decisions belong in these crates.

### Workspace layer

`ragelab-assets` owns filesystem-oriented operations:

- deterministic asset indexing;
- provider lookup;
- dependency closure;
- workspace-level relationship discovery;
- resource assembly and export.

It does not own presentation or transport concerns.

### Application layer

`ragelab-engine` owns transport-neutral product operations and policies shared by every adapter.

This includes declarative operation parsing, planning, write eligibility orchestration, non-destructive output policy, post-write semantic verification, the public asset service used for type detection/inspection/validation/capability discovery, the bounded diagnostic preview service for Legacy YDR/YDD/YBN, YMAP scene assembly plus its serialized `SceneManifestReport`, the versioned shared render package with typed binary buffers for native/Three.js/WASM consumers, and workspace export preflight/export services with serialized reports. Export path resolution and workspace-containment checks are engine policy, not adapter logic. These services return serializable Rust reports so CLI, Studio, and future MCP adapters consume the same domain policy without reparsing assets, duplicating scene/export vocabulary, or reimplementing truncation and export gate rules independently.

Format crates remain authoritative for binary-layout safety and writer capability decisions. `ragelab-engine` composes those typed decisions into transport-neutral product reports and operations. For Legacy YMAP placement, `ragelab-ymap` also exposes parsed `scaleXY`/`scaleZ`; the engine propagates those values into `SpatialTransform` and fails closed on incomplete scale evidence.

If a rule must behave identically in the CLI, an MCP server, and RageLab Studio, it belongs at or below this layer.

The application layer also owns the persistent GTA V Legacy RPF/world index. The index reuses the engine-owned ordered archive scan and installation fingerprint, persists YMAP/YTYP spatial/dependency metadata, and exposes bounded box/radius/frustum queries without global scene assembly. Missing spatial evidence degrades selectivity but never silently promotes inferred world bounds. See [WORLD_INDEX.md](WORLD_INDEX.md).

### Native renderer

`ragelab-render` is the reusable native/wgpu rendering layer. It consumes only the engine-owned `RenderPackage` contract and must not depend on RAGE format crates. The native viewport supports camera-preserving streaming package swaps plus bounded inactive GPU residency using the same stable asset/texture identities as Core package merging. `ragelab-engine::world_stream` drives nearby-map selection, chunk CPU caching, local-overlay precedence, deterministic eviction and active-package merging over the persistent world index without assembling the full installed world. See [RENDERER.md](RENDERER.md) and [WORLD_STREAMING.md](WORLD_STREAMING.md).

### CLI

`apps/ragelab` is the primary automation adapter. It exposes diagnostics and product operations without requiring a graphical environment, including `render asset`, `render scene`, and `render compare` over the shared native renderer.

The CLI must not contain an independent implementation of format parsing, renderer semantics, or writer safety.

### RageLab Studio

RageLab Studio is a separate Tauri application. It is a human interface over RageLab capabilities.

The Studio may own presentation, interaction state, rendering, and desktop integration. It must not become a second implementation of RAGE binary formats.

## Capability rule

A product feature is not complete if it exists only in RageLab Studio.

New capabilities should be implemented in this order:

1. core parser/model or writer;
2. validation and fail-closed capability decision;
3. transport-neutral engine operation when applicable;
4. CLI exposure;
5. agent/MCP exposure;
6. Studio presentation.

## Safety invariants

- Workspace-level `unsafe_code = "forbid"`.
- Pointer, offset, length, and allocation reads are bounds checked.
- Unsupported writer layouts are rejected explicitly.
- Source assets remain unchanged unless an operation explicitly documents in-place behavior.
- Proprietary assets and private validation data are never committed.
- Structured output schemas are versioned independently from human-readable output.
