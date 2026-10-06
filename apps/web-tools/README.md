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

## Cards 18–19 — model and YMAP preview

YDR/YDD/YFT preview consumes typed Rust/WASM geometry/material packets; Three.js only renders those buffers. YMAP preview keeps the same supplied-only dependency model: caller-supplied YTYP/YDR/YDD/YFT/YTD bytes are resolved by Rust/WASM, missing or ambiguous providers fail closed, and supported metadata edits enable download only after semantic reopen.

## Optional local bridge

The bridge is disabled by default and is never auto-discovered or auto-connected. To opt in, start a separate Core process and explicitly allow the browser origin:

```powershell
ragelab bridge serve `
  --game-root "C:\Program Files\Rockstar Games\Grand Theft Auto V Legacy" `
  --index "C:\path\to\legacy-v5.bin" `
  --keys "C:\path\to\gta-rpf-keys" `
  --origin "http://127.0.0.1:4173"
```

The command prints a loopback URL and a random per-launch capability. Paste both into the **Optional local bridge** panel and select **Connect explicitly**. The page does not persist the capability and clears its password field after successful connection.

The bridge is read-only: it exposes bounded search, opaque asset reads and bounded YMAP dependency bundles. Raw GTA key material, archive paths, nested RPF paths and unrestricted filesystem paths are not returned to the browser. Bytes received from the bridge still pass through the existing Rust/WASM YTD/model/YMAP contracts; the browser does not gain a second RAGE parser or resolver.

Web writes remain unchanged: edits are in memory, output requires an explicit download action, and supported writes remain gated by semantic reopen. The bridge itself has no write endpoint.

See `docs/WEB_BRIDGE_SECURITY.md` for the threat model, limits and audit behavior.
