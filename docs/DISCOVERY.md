# Game Discovery

RageLab provides local installation discovery as a transport-neutral Rust capability. The CLI is an adapter over that core logic; Studio and future agent adapters must reuse the same discovery and validation rules.

## Active compatibility scope

The current discovery target is **GTA V Legacy / Gen8**.

Enhanced / Gen9 is intentionally outside the active compatibility contract. RageLab may detect Enhanced markers in a candidate directory so that it can reject the directory for Legacy workflows, but that does not imply Enhanced asset support.

## CLI

```bash
ragelab gta discover --json
```

The structured response uses the common `ragelab.cli.response` envelope. `data` contains a versioned `ragelab.gta.discovery` report.

Important fields:

- `target` — product, edition, and known Steam app ID for the active discovery target;
- `validLegacyInstallations` — number of candidates that pass every required Legacy validation check;
- `candidates` — discovered or explicitly supplied roots, including rejected candidates;
- `provenance` — every source that independently identified a candidate;
- `edition` — `legacy`, `enhanced`, `ambiguous`, or `unknown` based on executable markers;
- `checks` — machine-readable required/optional filesystem validation.

RageLab does not silently choose one launcher when the same root is found multiple ways. Equivalent roots are deduplicated and their provenance is merged.

## Windows providers

Native Windows discovery currently considers:

1. `RAGELAB_GTA5_LEGACY` — explicit user/automation override;
2. Rockstar Games registry values under `Grand Theft Auto V` in 32-bit and 64-bit HKLM views;
3. Steam app manifest `271590`, including additional Steam libraries declared by `libraryfolders.vdf`;
4. existing common Steam, Rockstar Games, and Epic Games install locations.

Provider absence is not an error. Stale registry or manifest candidates are returned as invalid candidates when enough provenance exists to inspect them.

On non-Windows platforms, the explicit `RAGELAB_GTA5_LEGACY` override remains available for validation and automation, while native launcher discovery is currently Windows-only.

## Legacy validation

A candidate is valid for the current RageLab Legacy workflow only when:

- the root directory exists;
- `GTA5.exe` exists;
- `GTA5_Enhanced.exe` does not exist;
- `common.rpf` exists;
- `x64a.rpf` exists;
- `update/update.rpf` exists.

`PlayGTAV.exe` is reported as an optional launcher check because asset tooling does not need to launch the game.

If both Legacy and Enhanced executable markers exist, the candidate is `ambiguous` and fails closed. RageLab does not infer that a renamed/copied executable changes an installation's asset generation.

## Explicit validation API

`ragelab-engine` also exposes `validate_gta_v_installation(path)`. This lets Studio or another native adapter validate a user-selected directory without duplicating filesystem rules.

## FiveM discovery

RageLab also exposes native FiveM environment discovery:

```bash
ragelab fivem discover --json
```

The structured response uses the common `ragelab.cli.response` envelope. `data` contains a versioned `ragelab.fivem.discovery` report.

FiveM discovery is evidence-based. RageLab does not infer the GTA installation from folder names or launcher assumptions. The relationship to GTA is established only from the `[Game]` `IVPath` value in `FiveM.app/CitizenFX.ini`, then validated through the same GTA V Legacy validator used by `gta discover`.

Important fields:

- `validInstallations` — FiveM candidates that pass the required local installation checks;
- `legacyLinkedInstallations` — candidates whose configured `IVPath` resolves to a valid GTA V Legacy installation;
- `appRoot` — the normalized `FiveM.app` directory;
- `citizenFxIni` — the configuration file used as relationship evidence;
- `savedBuildNumber` and `updateChannel` — reported when present in `[Game]`; neither is treated as an asset-format compatibility guarantee;
- `storagePaths` — known local FiveM storage locations and whether they exist;
- `gta.status` — explicit relationship state such as `validLegacy`, `invalidLegacy`, `enhanced`, `ambiguous`, `unknown`, `invalidConfiguredPath`, or `missingConfiguration`;
- `gta.installation` — the complete GTA validation result when an absolute `IVPath` was available.

### FiveM providers

Discovery currently considers:

1. `RAGELAB_FIVEM` — explicit user/automation override; it may point to the FiveM root or directly to `FiveM.app`;
2. on Windows, `%LOCALAPPDATA%\FiveM` when that directory exists.

Custom FiveM installations outside the default location should use `RAGELAB_FIVEM` or the Rust validation API. Provider absence is not an error.

### FiveM validation

A FiveM candidate is valid when:

- the normalized FiveM root exists;
- `FiveM.app` exists;
- `FiveM.app/CitizenFX.ini` exists and is readable.

`FiveM.exe` is reported as an optional check. The following conventional paths are reported as storage evidence, not requirements:

- `FiveM.app/data`;
- `FiveM.app/data/game-storage`;
- `FiveM.app/data/cache`;
- `FiveM.app/citizen`.

An `IVPath` must be absolute before RageLab will validate it as a GTA relationship. Relative or malformed paths fail closed and are never resolved heuristically. If the configured GTA root is Enhanced, ambiguous, incomplete, or unknown, that status is reported without promoting it to Legacy compatibility.

`ragelab-engine` exposes `validate_fivem_installation(path)` for Studio and other native adapters.

## Catalog boundary

Installation discovery and vanilla asset catalog construction are separate capabilities.

The current `gta vanilla-index` flow can build a catalog from:

- an extracted GTA asset tree; or
- a path listing generated by tooling that can enumerate the game archives.

RageLab does **not** currently open the raw/encrypted RPF archive set to enumerate every vanilla asset. Discovering a valid game root therefore does not by itself create a complete native vanilla file catalog. Direct RPF catalog construction is tracked separately so this limitation remains explicit rather than being hidden behind discovery.

## Privacy and repository policy

Discovery reads local registry/manifests and checks filesystem markers. It does not copy game assets into the RageLab repository. Local installation paths and generated proprietary catalogs must not be committed.
