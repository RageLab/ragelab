# Contributing

Contributions are welcome when they preserve RageLab's format-safety and automation contracts.

## Development requirements

Use the pinned Rust toolchain and run the full workspace gates before submitting a change:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Architecture rules

- Keep binary parsing and writing in the relevant Rust format crate.
- Keep workspace indexing and dependency resolution in `ragelab-assets`.
- Keep reusable application policy in `ragelab-engine`.
- Keep the CLI as an adapter rather than a second implementation.
- Do not move writer-safety decisions into UI or scripting layers.
- Preserve fail-closed behavior for unsupported layouts.

## Fixtures and third-party data

Use synthetic fixtures for committed regression tests.

Do not commit proprietary Rockstar assets, private game installations, extracted archives, private indexes, or third-party files that cannot be redistributed.

## Public interfaces

Changes to versioned JSON output must follow `docs/CLI_CONTRACT.md`.

A feature intended for RageLab Studio should first be available through the Rust core and, where applicable, the CLI.
