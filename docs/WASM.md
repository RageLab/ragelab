# RageLab WASM portable Core

`ragelab-wasm` is the browser-facing adapter for the portable RageLab format subset. It operates only on bytes explicitly supplied by the caller. Rust format crates remain authoritative; JavaScript does not reimplement RAGE parsing or write semantics.

## Supported browser formats

The facade exposes parse/inspect/validate for YMAP, YTYP, YDR, YDD, YFT and YTD. Model geometry is returned through `Float32Array`/`Uint32Array`; decoded texture pixels and rewritten assets use `Uint8Array`.

Safe writes are fail-closed:

- YMAP: transform, archetype, flags and parent edits through the existing YMAP edit command contract.
- YDR/YDD: only proven existing shader/texture rebinds.
- YTD: PNG replacement through `ragelab-authoring`, preserving supported Legacy target format and semantic-reopening the output.
- YTYP/YFT: inspect-only until a portable writer is independently proven.

Errors are structured JS objects with schema `ragelab.wasm.error`. Capabilities are available from `capabilities()`.

## Desktop-only boundary

The WASM crate does **not** depend on `ragelab-engine`, `ragelab-assets` or `ragelab-rpf`. These remain desktop-only:

- RPF archive access and nested provider resolution.
- installed GTA discovery and persistent game/world indexes.
- GTA/RPF key material.
- unrestricted filesystem operations and explicit-output orchestration.
- native wgpu renderer/window integration.

Web Tools are therefore useful with user-supplied files without exposing an installed GTA tree or key store to page JavaScript.

## Reproducible frontend package

Prerequisites are an installed `wasm32-unknown-unknown` Rust target and a `wasm-bindgen` CLI matching the pinned crate version.

From the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-wasm.ps1
```

The generated ESM package is written to `target/ragelab-wasm-pkg` and contains JS glue, TypeScript declarations, the WASM binary, `package.json` and `manifest.json` with SHA-256 hashes. `target/` is ignored and generated artifacts are never committed.

A frontend workspace can depend on the generated directory as a local package. Initialization must happen in the browser before calling the exported facade.

## Tests

Host portable-unit tests:

```powershell
cargo test -p ragelab-authoring -p ragelab-wasm
```

Real JS/WASM integration tests:

```powershell
$env:CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER = "wasm-bindgen-test-runner"
cargo test -p ragelab-wasm --target wasm32-unknown-unknown --test wasm
```

The integration suite executes the compiled `.wasm` through `wasm-bindgen-test-runner`, uses copyright-free synthetic fixtures, covers all six supported formats, typed model/texture buffers, YMAP/YDR/YDD/YTD safe writes, semantic reopen and fail-closed capability/error schemas. The YFT fixture is generated in memory and intentionally contains no main drawable; model preview rejection is part of the contract.

After generating the web package, a plain JavaScript consumer smoke can be run with:

```powershell
node .\scripts\smoke-wasm-package.mjs target\ragelab-wasm-pkg
```

That harness imports the generated `--target web` ESM package, initializes the WASM binary, validates synthetic files, consumes typed model/texture arrays, performs the shared YTD PNG roundtrip, and verifies RPF remains fail-closed.
