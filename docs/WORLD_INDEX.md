# Persistent Legacy world index

RageLab extends the persistent GTA V Legacy RPF asset index into a versioned world index. The world index is built by the same ordered base/update/DLC scanner used for asset and archetype resolution; there is no second implementation of RPF load order.

## Schema v4

Schema v4 adds persistent YMAP/YTYP world metadata to the existing installation fingerprint, file winners, archetype winners and GTXD texture-parent records.

Each YMAP record preserves:

- outer RPF + nested RPF + entry provenance;
- load rank;
- file-name hash and parsed CMapData name hash;
- parent map hash;
- optional CMapData `flags` and `contentFlags`;
- entity and streaming extents when proven by META fields;
- map physics-dictionary hashes;
- deduplicated entity archetype hashes;
- normalized entity index, archetype, position, quaternion, optional Legacy scale fields, flags and parent index.

Each YTYP record preserves its provider, file/internal name hash, dependency hashes and archetype hashes. Existing per-archetype records remain authoritative for resolving drawable/texture/physics providers.

Winner selection follows the same RPF `load_rank` rules as the rest of the game index. Higher-ranked providers replace lower-ranked providers; same-rank conflicts remain explicit candidates rather than being guessed away.

## Installation fingerprint and invalidation

The index remains tied to `GtaRpfInstallationFingerprint`:

- GTA5.exe size/mtime;
- update.rpf size/mtime;
- ordered outer-RPF count;
- deterministic signature over outer-RPF relative paths, sizes and mtimes.

The persistent schema version is part of the cache filename/validation. Schema-v3 data is not read as schema v4.

## Spatial acceleration

The persisted broadphase is a uniform XY grid with 512-meter cells.

Maps are inserted using proven streaming extents when available, otherwise proven entity extents. Maps with missing, invalid or excessively large extents are placed in a global fallback set. This is deliberate fail-safe behavior: an incomplete map may make a query less selective, but it is not silently omitted.

Per-map insertion is bounded to 4096 cells. Queries spanning more than 16384 cells fall back to all world-map keys instead of allocating an unbounded cell traversal.

The index also persists parent -> child map relationships.

## Queries

Core exposes read-only queries over the persisted index:

- box query;
- point/radius query;
- frustum query using a caller-provided broadphase AABB plus six planes.

Queries return matching map provenance and optionally persisted entity records. Query execution does not reopen RPF/YMAP files or assemble a global scene.

Entity filtering currently uses the proven entity origin. The normalized YTYP model does not expose archetype geometry bounds, so RageLab does not invent per-entity world AABBs. Map-level streaming/entity extents remain the broadphase authority until stronger archetype-bound evidence exists.

## CLI

Build a persistent index:

```bash
ragelab gta rpf-index <game-root> --keys <key-cache> --output legacy-v4.bin --json
```

Query a radius:

```bash
ragelab gta world-query legacy-v4.bin \
  --point -1625 -854 0 \
  --radius 750 \
  --entities \
  --json
```

Query a box:

```bash
ragelab gta world-query legacy-v4.bin \
  --box -2000 -1200 -200 -1200 -500 300 \
  --json
```

`--repeat N` reruns the in-memory spatial query after one index load and reports `indexLoadMs`, `queryTotalMs` and `queryAverageMs`. This is intended for warm-query regression measurements; it does not reopen RPFs between iterations.

## Legacy validation

Validation against the installed GTA V Legacy tree used the existing read-only RPF key cache.

Cold schema-v4 build:

- 123 ordered/scanned outer archives;
- 5,199 nested RPFs;
- 297,372 indexed asset files;
- 19,383 parsed YMAPs;
- 10,609 load-order-winning world-map keys;
- 3,074,696 persisted entity records;
- 2,746 parsed YTYPs;
- 1,698 winning YTYP records;
- 159,327 archetype keys;
- 158,448,454-byte persisted index;
- approximately 77.6 seconds wall time in the debug validation build.

The build emitted 23 explicit warnings for unsupported/corrupt resources, primarily destruction YTYP variants whose normalized structure differs from the supported archetype layout and a small number of PSIN resources stored under YMAP/YTYP names. Those entries remained unparsed rather than being inferred.

### sm_23 cross-check

A spatial query around the patins/sm_23 area returned both the heist-era and current patch providers with their independent load ranks.

The current `sm_23_strm_0.ymap` winner is:

- outer archive: `update/x64/dlcpacks/patchday27ng/dlc.rpf`;
- nested archive: `x64/levels/gta5/_cityw/santamon_01/santamon_metadata.rpf`;
- load rank: 92;
- map hash/name: `0xA95C4E81`;
- parent: `0x8373A62F`;
- entities: 339;
- root flags: 0;
- content flags: 65;
- entity extents min: `[-1730.8256, -985.5216, 6.4199]`;
- entity extents max: `[-1521.1582, -722.0092, 16.4638]`;
- streaming extents min: `[-1845.6899, -1053.6802, -112.8136]`;
- streaming extents max: `[-1433.0793, -623.7620, 129.9835]`.

Direct enumeration of that nested RPF returned exactly the expected four `sm_23*` YMAP entries. A temporary direct extraction of `sm_23_strm_0.ymap` parsed to the same name hash, parent hash, 339 entities and entity extents. The temporary extracted file was deleted after validation.

The earlier `hei_sm_23_strm_0.ymap` remains independently indexed from `mpheist` at load rank 26 and also contains 339 entities; it is not incorrectly collapsed into the patchday map because its CMapData identity differs.

### Query timing

Debug-build measurements on the validation workstation:

- loading the 158 MB persisted index: about 5.9 seconds;
- 1-meter radius query around the `sm_23_strm_0` entity center: about 2.95 ms for one run, with 1,111 broadphase map keys;
- 750-meter radius query repeated 1,000 times: about 12.94 ms/query average, with 2,998 broadphase map keys and 2,130 final map hits.

These are regression measurements for this installation/debug build, not universal performance guarantees. Query timing includes constructing/sorting the returned Rust report but excludes RPF I/O because all spatial records are persisted.

## Safety

- GTA files and RPFs are read-only inputs.
- Building an index writes only the requested cache/output file.
- Existing output requires explicit `--overwrite`.
- Unsupported/corrupt YMAP or YTYP entries produce build warnings; they are not synthesized.
- World queries reject non-finite/invalid bounds and radii.
