# Project Origin

RageLab began as a clean extraction of the reusable Rust core and command-line tooling from `FlokiTV/rage-rs`.

The initial extraction source was commit `3c2ff7a` from the `main` branch on 2026-10-02.

The new repository intentionally starts with independent Git history and excludes the previous web application, HTTP server, deployment runtime, and local operational state.

RageLab preserves the format-safety model developed in the source project while establishing a separate public product identity, package namespace, CLI contract, and release lifecycle.
