# GLB batch pipeline

\`ragelab glb batch <manifest.json>\` executes a versioned list of GLB-to-YDR exports through the same byte-first \`ragelab-engine\` use case as \`glb export\`. It does not spawn the CLI recursively, infer material intent, select a shader, or discover templates.

## Manifest v1

The manifest is UTF-8 JSON with schema \`ragelab.glb.batch-manifest\`, version 1. Its strict JSON Schema is [ragelab.glb.batch-manifest.v1.schema.json](schemas/ragelab.glb.batch-manifest.v1.schema.json).

Example:

~~~json
{
  "schema": "ragelab.glb.batch-manifest",
  "schemaVersion": 1,
  "failurePolicy": "continue",
  "concurrency": 2,
  "report": "reports/batch.json",
  "items": [
    {
      "id": "crate-a",
      "source": "input/crate-a.glb",
      "output": "output/crate-a.ydr",
      "profile": "profiles/crate-a.json",
      "shaderTemplate": null,
      "shaderIndex": 0,
      "nodes": [],
      "report": "reports/crate-a.export-report.json"
    },
    {
      "id": "crate-b",
      "source": "input/crate-b.glb",
      "output": "output/crate-b.ydr",
      "profile": "profiles/crate-b.json",
      "shaderTemplate": "templates/normal.yft",
      "shaderIndex": 0,
      "nodes": [0, 2],
      "report": "reports/crate-b.export-report.json"
    }
  ]
}
~~~

Run it with:

~~~powershell
ragelab glb batch .\batch.json
ragelab glb batch .\batch.json --json
~~~

All paths in the manifest are relative to the manifest's directory. Absolute paths, roots, drive prefixes, \`.\`/\`..\` path components and paths that resolve through a symlink/reparse point outside that directory are rejected. Input files must exist. Output parent directories must already exist and resolve inside the manifest root; the command does not create directory trees. Existing destination files, including symlinks, are rejected. Case-insensitive collisions are detected on Windows.

The manifest has 1..128 items and concurrency 1..8. The manifest is limited to 4 MiB, each GLB to 512 MiB, each profile to 16 MiB, and each optional template to 256 MiB. A profile path is required for every item and is parsed using the strict authoring-profile contract. \`shaderTemplate\` is a required field that must be either a \`.yft\`/\`.ydr\` path or explicit \`null\`. \`shaderIndex\` and \`nodes\` are always required; if no template is selected, \`shaderIndex\` must be 0. An empty \`nodes\` array selects the full active scene; otherwise the listed node subtrees are exported using the same overlap/range validation as \`glb export --node\`.

The template, material profile, shader index and node selection are per-item decisions. The Batch command never supplies implicit profile files or guesses which template/material/shader is appropriate. \`shaderTemplate: null\` means structural geometry/texture export without cloning a populated ShaderFX template; it does not assert runtime renderability.

## Preflight and execution

The command performs a complete global preflight before publishing any final output:

1. Parse the strict manifest and validate the version, item IDs and explicit options.
2. Resolve and canonicalize all inputs and output parents under the manifest root.
3. Detect aliases/collisions among the aggregate report, every YDR, every item report, all inputs and the manifest.
4. Read every source/profile/template under the documented size budgets.
5. Run the actual Engine GLB-to-YDR export in memory for every item, including profile validation, optional template signature checks, selected-node validation and YDR semantic reopen.
6. Build and serialize every per-item export-report v1 in memory.

If any item or path fails preflight, the command exits before creating any destination file. Preflight is sequential and bounded to one item at a time; successful output bytes are discarded after verification. Execution then repeats the conversion using a bounded worker pool. Source/profile/template SHA-256 values are compared with preflight fingerprints before publication to detect input changes between the two passes.

The worker count is bounded by the manifest's \`concurrency\`. Results and reports preserve manifest order regardless of completion order. Progress is written to stderr, not mixed into JSON stdout.

\`failurePolicy\` is explicit:

- \`continue\`: a runtime execution/publication failure marks that item \`failed\`; other items continue.
- \`fail_fast\`: once a worker observes a failure, no new item is scheduled. Items already running finish; items not started are marked \`cancelled\`.

Preflight failures are global and always stop the batch before writes, regardless of the policy. The policy controls failures after preflight (for example, a destination becoming unavailable or an input changing). Successful items are not rolled back when a later item fails. YDR and item-report publication are atomic no-replace operations; if an item report cannot be published after its YDR, the command attempts to remove that item's YDR and records rollback failure if removal also fails. A failure publishing the final aggregate report can occur after item outputs have already been published; inspect the per-item reports and output paths in that exceptional case.

## Reports and exit status

Each item writes the same \`ragelab.glb.export-report\` v1 sidecar produced by \`glb export --report\`. The aggregate report is \`ragelab.glb.batch-report\` v1, validated by [ragelab.glb.batch-report.v1.schema.json](schemas/ragelab.glb.batch-report.v1.schema.json). It contains the manifest provenance, selected failure policy and concurrency, stable manifest-order item statuses, source/output hashes, and totals for \`succeeded\`, \`failed\` and \`cancelled\`.

The aggregate report is written last using same-directory temporary-file publication and hard-link no-replace semantics. Neither YDR outputs nor individual/aggregate reports are overwritten.

- Exit 0: every item succeeded.
- Exit 1: one or more items failed or were cancelled after the global preflight; the aggregate report is retained, and \`--json\` places the full report in the common CLI error envelope's \`data\`.
- Exit 2: malformed command/manifest declaration or unsafe paths.
- Exit 3: a requested export feature/template is recognized but unsupported.
- Exit 4: invalid GLB/profile/YDR data or failed semantic validation.

The report's semantic reopen is a RageLab parser/serializer gate. It is not evidence of CodeWalker GUI acceptance or GTA V/FiveM runtime acceptance. Runtime flags remain false unless an independent downstream gate is executed and recorded.
