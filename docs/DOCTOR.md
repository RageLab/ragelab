# Doctor and global capabilities

RageLab exposes two read-only discovery commands:

~~~powershell
ragelab doctor
ragelab doctor --json
ragelab capabilities --json
~~~

## Doctor report v1

\`ragelab doctor --json\` returns the dedicated \`ragelab.cli.doctor\` v1 document, validated by [ragelab.cli.doctor.v1.schema.json](schemas/ragelab.cli.doctor.v1.schema.json). It does not wrap this payload in the generic command envelope; its top-level \`schema\` and \`schemaVersion\` are the compatibility contract.

The report includes:

- build version, Core Git revision and dirty-at-build flag, API compatibility version, Cargo profile, target triple/OS/architecture/environment/families, and enabled Cargo features;
- the compiled Engine module list generated from the same declaration that compiles and re-exports those modules;
- the semantic/container-only asset format registry generated from the Engine's actual asset format dispatch, plus GLB 2.0 when the compiled GLB pipeline module is present;
- checks with stable IDs, category, severity (\`info\`, \`warning\`, \`blocker\`), status, summary, supporting details, actionable recommendation, and affected command IDs;
- renderer, current-directory metadata, and optional GTA V Legacy/FiveM discovery summaries with candidate provenance;
- explicit external dependencies and the commands they affect.

Overall status is \`healthy\`, \`warning\`, or \`blocked\`. The severity counts in \`summary\` are authoritative; the CLI exit code indicates that the diagnostic command completed and the report was emitted. Automation should inspect \`status\` and \`summary.blockers\`, rather than interpreting any warning as a failure of unrelated commands.

A missing GPU affects only \`render.asset\`/\`render.scene\`. A missing GTA V Legacy installation affects commands that need game/RPF data. Missing FiveM affects FiveM environment discovery and related optional workflows. They are not global Core blockers. Incomplete detected candidates are warnings scoped to the corresponding commands. A missing required Engine module or required semantic format registration is a blocker because it indicates an incomplete RageLab binary.

YFT is listed as semantic only within the existing Legacy v162/mainDrawable parser scope. YCD, YMT, YLD and YNV remain explicitly \`containerOnly\`; a valid RSC7 container does not imply semantic parsing.

## Read-only boundary

Doctor is non-destructive:

- it does not create directories or files;
- it does not perform write-permission probes;
- it does not modify environment variables, PATH, Windows Registry, GTA/FiveM configuration or installations;
- GTA/FiveM discovery uses the existing bounded read-only discovery APIs and reports their candidate provenance;
- current-directory inspection reads filesystem metadata only;
- the wgpu probe initializes the same transient offscreen adapter/device used by render commands and drops it after the readiness check; it does not persist settings or render output.

The report explicitly includes \`readOnly: true\`, \`actionsTaken: []\`, \`writeProbePerformed: false\`, and \`implicitOutputDirectoryRequired: false\`. A successful metadata check is not a guarantee that every future input/output path is readable or writable; the relevant command still validates its explicit paths at execution time.

## Global capabilities v1

\`ragelab capabilities --json\` retains \`ragelab.cli.capabilities\` schema v1 and its existing command/alias/response-envelope fields. The current schema is [ragelab.cli.capabilities.v1.schema.json](schemas/ragelab.cli.capabilities.v1.schema.json).

Additional fields expose:

- \`build\`: the same build metadata reported by Doctor;
- \`compiledModules\`: module IDs from the Engine's compile-time declaration;
- \`assetFormats\`: parser support and limitation derived from the actual \`AssetFormat\` registry;
- \`externalDependencies\`: optional environment/device dependencies, their affected command IDs, and configuration guidance.

The per-file form \`ragelab capabilities <file> --json\` remains unchanged and continues to report operations for the specific asset.

## Build metadata semantics

Revision and dirty state are captured by the Engine build script. \`dirty\` means the build source tree had tracked or non-ignored untracked changes at compile time; it is not a live \`git status\` result queried by Doctor. Target/profile/features describe the Cargo compilation that produced the running binary. A rebuild is required to refresh these fields.

Cargo features are reported as compiled by the Engine crate. An empty list means no Engine Cargo features were enabled; it does not mean the target has no operating-system features or libraries.
