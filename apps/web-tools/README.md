# RageLab Web Tools

Portable browser tools backed by the Rust Core through `@ragelab/wasm`.

## Data boundary

- Inputs are bytes explicitly supplied by the user.
- The browser does not parse RAGE formats independently; YTD/YDR/YDD/YFT/YMAP/YTYP semantics come from Rust/WASM.
- Installed GTA discovery, RPF access, GTA keys and native wgpu remain desktop/local-bridge concerns.
- Web writes are in-memory until the user explicitly downloads output.
- Supported writes fail closed and must semantic-reopen through the Rust Core before download is enabled.
- GTA Legacy and the original `patins` resource are never modified by Web Tools.

## Development

```powershell
bun run wasm:sync
bun install
bun run check
bun run build
bun run test:browser
```

`wasm:sync` regenerates `vendor/wasm` from the current Core revision. The generated package is versioned with this app so a checkout has an exact Web/Core contract; regenerate it whenever the WASM API changes.

## Card 17 — YTD

The YTD tool supports drag/drop or file-open, texture metadata, Rust-decoded mip/channel preview, top-mip PNG export, capability-gated PNG replacement, semantic reopen validation and explicit rebuilt-YTD download. Unsupported replacement formats remain inspect-only and display the Core-provided reason.
