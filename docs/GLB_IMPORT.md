# GLB 2.0 import and structural YDR export

RageLab has a CLI path from a static GLB 2.0 scene to a GTA V Legacy YDR v165. The path decodes supported geometry, extracts embedded PNG images, validates an optional GTA material-authoring sidecar, writes vertex/index buffers, embeds source images in a TextureDictionary and can clone a supported Legacy ShaderFX template from an external YFT or YDR.

The optional ShaderFX path is deliberately narrow: it accepts five exact Legacy signatures, `normal.sps`, `spec.sps`, `normal_spec_tnt.sps`, `emissivenight.sps` and `emissive.sps`. The first binds albedo/normal/specular and maps normal-map scale to the verified bumpiness parameter; the second binds albedo/specular and preserves its other parameters; the third binds albedo/tint palette/normal/specular while keeping the observed zero tint-palette selector fixed; the two emissive presets bind one combined albedo/emissive image and map glTF emission factor/strength into named shader parameters while preserving other lanes. These are exact template paths, not a general PBR converter. Glass, masked/blended and metallic-roughness/occlusion authoring are not enabled. Geometry, dictionary and shader blocks are semantically reopened by RageLab; CodeWalker Core has independently parsed the 28 individual outputs and one corrected representative was confirmed by the user in Model Viewer. The 28 new outputs were not each GUI-tested, and GTA V runtime acceptance has not been established.

## CLI

Inspect the source inventory:

~~~powershell
ragelab glb inspect .\pumpkin.glb
ragelab glb inspect .\pumpkin.glb --json
~~~

Decode the selected static scene and report per-primitive geometry:

~~~powershell
ragelab glb geometry .\pumpkin.glb --json
~~~

Create and edit the material-authoring sidecar:

~~~powershell
ragelab glb profile init .\pumpkin.glb --output .\pumpkin.gta-materials.json
ragelab glb profile validate .\pumpkin.glb .\pumpkin.gta-materials.json --json
~~~

Export a structural YDR. When `--profile` is omitted, the CLI creates the default profile in memory and validates it against the source GLB:

~~~powershell
ragelab glb export .\pumpkin.glb --output .\pumpkin.ydr --json
ragelab glb export .\pumpkin.glb --profile .\pumpkin.gta-materials.json --output .\pumpkin.ydr
ragelab glb export .\pumpkin.glb --profile .\pumpkin.gta-materials.json --shader-template .\template.yft --shader-index 0 --output .\pumpkin.ydr --json
# The supplied YFT or YDR is read-only; only an exact supported Legacy ShaderFX signature is cloned.
~~~

Inspect the active scene hierarchy before authoring. `glb tree` lists stable node indices and the number of meshes/primitives beneath each group. Select one or more non-overlapping node subtrees with repeatable `--node`; a selected node includes its descendants. The selected nodes retain their transforms from every ancestor in the active scene. Selecting both a parent and one of its descendants is rejected rather than double-counting geometry.

~~~powershell
ragelab glb tree .\\halloween.glb
ragelab glb tree .\\halloween.glb --json
ragelab glb export .\\halloween.glb --node 12 --output .\\witch-props.ydr --json
# For CodeWalker Model Viewer, supply a verified Legacy ShaderFX template:
ragelab glb export .\\halloween.glb --node 12 --profile .\\halloween.normal-sps.profile.json --shader-template .\\prop_picnictable_01.yft --shader-index 0 --output .\\witch-props-codewalker.ydr --json
ragelab glb export .\\halloween.glb --node 4 --node 7 --profile .\\halloween.normal-sps.profile.json --shader-template .\\prop_picnictable_01.yft --shader-index 0 --output .\\pumpkins-and-ghosts.ydr --json
~~~

Without `--node`, export remains full-active-scene and keeps report schema version 1. An export with `--node` emits report schema version 2 and includes the selected roots, included node/mesh indices and primitive count; the contract is published at [ragelab.glb.ydr-export.v2.schema.json](schemas/ragelab.glb.ydr-export.v2.schema.json). For the first CLI implementation the decoder still traverses and decodes the complete active scene before the Engine filters the selected node subtrees, so structural decode errors elsewhere in the scene can still abort a selected export. Writer-level attribute restrictions are checked after filtering and therefore block only when that primitive is selected. `TEXCOORD_0`, `TEXCOORD_1` and `TEXCOORD_2` are preserved in the Legacy vertex declaration. Source images are still decoded through the existing full-image path; the selection does not yet prune the source TextureDictionary to only the images used by selected meshes.

The CLI does not overwrite an existing output unless `--overwrite` is supplied. The output path must be distinct from the input GLB and profile file. The engine builds the complete resource and semantically reopens it through `YdrDocument::from_bytes` before the CLI opens the destination. The Studio Tauri adapter follows the same policy: absolute paths are required, an existing output is rejected by default, and `overwrite=true` is explicit.

The authoring use case lives in `ragelab-engine/src/glb.rs`. It accepts source bytes/name, optional profile bytes and an explicit YFT/YDR template selection with template bytes. The engine has no filesystem or environment-path discovery; CLI and Tauri each resolve paths and pass byte-first requests to that service. The Studio commands are `core_glb_inspect`, `core_glb_geometry`, `core_glb_profile_init`, `core_glb_profile_validate` and `core_glb_export`. Rust DTOs and nested GLB profile/report types generate the TypeScript bindings with `ts-rs`; run `bun run generate:glb-types` from `ragelab-studio` to regenerate `src/lib/generated/glb`. `src/lib/native.ts` imports those generated types and retains compatibility aliases for frontend callers.

The JSON response uses schema `ragelab.glb.ydr-export`. Full-scene exports retain schema version 1; exports with `--node` use version 2 and include the selection report described above. Both versions report geometry/texture counts, profile validation, ShaderFX/template provenance, material/shader bindings, semantic reopen, source/profile/output hashes and the explicit `runtimeRenderable=false` state. Version 1 is published at [ragelab.glb.ydr-export.v1.schema.json](schemas/ragelab.glb.ydr-export.v1.schema.json); version 2 is published at [ragelab.glb.ydr-export.v2.schema.json](schemas/ragelab.glb.ydr-export.v2.schema.json). Parsing the emitted YDR does not establish acceptance by GTA V runtime or CodeWalker.

## Geometry conversion

The static scene decoder applies node hierarchy transforms, converts glTF Y-up positions/normals/tangents to GTA Z-up, expands TRIANGLES/TRIANGLE_STRIP/TRIANGLE_FAN, corrects mirrored winding, and calculates per-primitive bounds.

The current YDR writer serializes these vertex attributes:

- POSITION: Float3.
- NORMAL: Float3.
- TANGENT: Float4.
- COLOR_0: Float4.
- TEXCOORD_0, TEXCOORD_1 and TEXCOORD_2: Float2.

The reader reopens the output and compares positions, normals, tangents, colors, UV0, UV1, UV2 and indices against the serialized input. The writer emits a single High LOD model with 16-bit indices; a primitive therefore cannot exceed the Legacy 16-bit vertex/index range (65,535 vertices). Multi-LOD generation, skinning, morph targets and animation are not implemented.

TEXCOORD_0/1/2 are written to their corresponding Legacy vertex declaration slots (6/7/8). For texture roles configured in the profile, the referenced UV set must exist on every decoded primitive using that material. UV sets above 2 are rejected rather than discarded.

## Embedded images and mipmaps

The current image decoder accepts PNG images embedded as either:

- a GLB BIN-backed bufferView with a declared `mimeType`; or
- a base64 `data:image/png` URI.

It converts grayscale, grayscale-alpha, RGB and RGBA PNG data to RGBA8, expands palette images through the PNG decoder, validates image dimensions and byte ranges, and applies per-image and aggregate memory limits. External image URIs, external buffers, JPEG and other MIME formats are rejected for this export path. All source images must be decodable, including images not currently assigned to a material role.

The YDR writer stores the resulting textures in a TextureDictionary reachable from the Drawable's ShaderGroup pointer. The dictionary descriptors live in the RSC7 system segment and image payloads in the graphics segment. Supported output encodings are:

- `auto`: BC1 for fully opaque images; BC3 when any pixel has transparency.
- `bc1`: BC1; fractional alpha is rejected to avoid silently quantizing it.
- `bc3`: BC3 with alpha support.
- `rgba8`: uncompressed RGBA8.

A complete deterministic mip chain down to 1x1 is generated for each texture. The CLI retains readable, index-prefixed image names in the dictionary and fails on invalid dimensions, duplicate/colliding JOAAT name hashes, unsupported formats or oversized payloads.

## Material profile semantics

The versioned sidecar is bound to the source file SHA-256, byte length and material/texture counts. It separates:

- **Texture roles**: albedo, normal, metallic-roughness, specular, specularColor, occlusion and emissive.
- **Shader intent**: lit, unlit or emissive.
- **Surface mode**: opaque, masked, blended or glass.

It also records base-color, metallic, roughness, emissive, alpha, double-sided, transmission, IOR and normal-map convention settings. The CLI validates profile schema, source identity, numeric ranges, material and texture references, UV-set compatibility and embedded-image availability before creating the YDR.

The profile remains an authoring contract; the supported ShaderFX path is opt-in and requires a readable Legacy YFT or YDR template supplied by the user. The source resource is never modified and is not copied into the repository.

### Verified template: `normal.sps`

Observed in `prop_picnictable_01.yft`: `nameHash=0x38DD00DF`, `fileHash=0x14A780FD`, render bucket 0, mask `0x0000FF01`, parameter size 304, data size 384, 11 ordered parameters and 3 texture parameters. The CLI checks the exact name/file hashes, header signature, ordered parameter hashes/types and each sampler's `TextureBase` fields/VFT before cloning. It rebinds `DiffuseSampler` (`0xF1FE2B71`) to albedo, `BumpSampler` (`0x46B7C64F`) to normal and `SpecSampler` (`0x608799C6`) to specular or specularColor. `normalTexture.scale` maps to the `bumpiness` Vector4 (`0xF6712B81`). A primitive without a material receives a separate clone with all three sampler pointers cleared rather than being mapped to material 0.

### Verified template: `spec.sps`

Observed at shader index 0 of `prop_roadpole_01a.yft`, a Legacy sample previously extracted read-only from the `x64f/v_construction` provider: `nameHash=0x7204A391` (`spec`), `fileHash=0x85F50BE9` (`spec.sps`), render bucket 0, mask `0x0000FF01`, parameter size 320, data size 400, 11 ordered descriptors and 2 texture parameters. The signature check includes the ordered hash/type/descriptor-byte sequence, reserved header fields, parameter sizes and both complete `TextureBase` layouts. It rebinds only `DiffuseSampler` (`0xF1FE2B71`) to albedo and `SpecSampler` (`0x608799C6`) to the profile's specular texture. The other nine Vector4 values are retained from the template unchanged; no PBR factor is mapped into them. No normal sampler exists in this signature, so a profile assigning a normal map is rejected. `specularColor` is also rejected; only the explicit `specular` role is accepted. An unmaterialed primitive receives a separate clone with both sampler pointers cleared.

### Verified template: `normal_spec_tnt.sps`

Observed at shader index 0 of the read-only `prop_bollard_02a.yft` sample: `nameHash=0x6CD4735B`, `fileHash=0x304AB527`, render bucket 0, mask `0x0000FF01`, parameter size 352, data size 448, 13 ordered parameters and 4 texture parameters. The template identity resolves to `normal_spec_tnt.sps`. The signature gate checks header/reserved fields, the complete ordered hash/type/descriptor-byte sequence, the selector vector `tintPaletteSelector=0`, and all four `TextureBase` structures including their exact VFT, flags and role-specific `UsageData`.

The enabled bindings are `DiffuseSampler` (`0xF1FE2B71`)→albedo, `TintPaletteSampler` (`0xF648A067`)→`textureRoles.tintPalette`, `BumpSampler` (`0x46B7C64F`)→normal and `SpecSampler` (`0x608799C6`)→specular. The sample's `TextureBase.UsageData` values are retained exactly: diffuse `0x20015874`, tint palette `0x20002019`, normal `0x20015876`, specular `0x20015877`. CodeWalker decodes the low five bits as `TextureUsage` and the upper bits as flags; these observed values resolve to DIFFUSE (20), TINTPALETTE (25), NORMAL (22) and SPECULAR (23). The writer changes only each sampler name/pointer and preserves the source usage bits.

The Vector4 parameter `tintPaletteSelector` (`0xFDD796CF`) is observed as `[0,0,0,0]`. It is preserved from the template and the validator rejects a source template with a non-zero selector. The profile exposes a separate optional `textureRoles.tintPalette` texture binding; it does not expose selector components or promise support for alternate palette indices. One clone is emitted per material, with a neutral clone for unmaterialed geometry.

This template is restricted to lit + opaque, white `baseColorFactor`, metallic=0, roughness=1, neutral emissive values, no double-sided, transmission or IOR. It accepts only albedo, tintPalette, normal and specular. Metallic-roughness, occlusion, emissive and specularColor roles are rejected. A non-default normal scale is mapped to the same `bumpiness` hash used by the verified `normal.sps` preset. The template's exact sample signature requires TextureBase VFT `0x40617568` and `unknown32=0x80`; other VFT/flag variants are not accepted.

For all five presets, one shader clone is produced per profile material and geometry-to-shader indices are checked by semantic reopen. The `emissive.sps` clone path accepts YDR sources as well as YFT sources; the other preset paths continue to accept YFT/YDR only when the file's embedded ShaderFX block passes the exact validator. `runtimeRenderable` remains false. These signatures are exact sample-specific acceptance gates, not wildcard matching by shader family. The optional profile field `tintPalette` is backward compatible: older sidecars without this property deserialize as `None`.

### Verified template: `emissivenight.sps`

Observed at shader index 2 of the read-only `prop_generator_03b.yft` sample: `nameHash=0xB08C6435`, `fileHash=0x557DB6E4`, JOAAT identities `emissivenight` / `emissivenight.sps`, render bucket 0, mask `0x0000FF01`, parameter size 144, data size 208, 5 ordered parameters and one texture sampler. The exact signature gate checks the full ordered descriptor tuple list (hash/type/unknown1), reserved header fields, payload sizes and the single `TextureBase` with VFT `0x4061A8F8`, unknown4=1, unknown30=1, unknown32=2, UsageData=0 and the remaining reserved fields/name.

The profile path requires `shaderIntent=emissive` and `surfaceMode=opaque`. The one `DiffuseSampler` (`0xF1FE2B71`) receives one combined texture source: `textureRoles.emissive` is preferred, otherwise `textureRoles.albedo` is used. If both roles are supplied, they must identify the same texture index and UV set; separate albedo and emissive images are rejected because this signature has only one sampler. The source YFT assigns `Prop_Bulb_White` to one primitive (model 2 / geometry 0, 112 vertices, 84 triangles).

The parameter mapping is deliberately small:
- `matMaterialColorScale` (`0x948A54F2`), a one-Vector4 parameter identified by the CodeWalker Gen9 conversion registry as a 16-byte CBuffer, receives `emissiveFactor[0..3]` in XYZ. Its W lane remains copied from the template.
- `emissiveMultiplier` (`0x5EEBED48`), a one-Vector4 parameter identified in that registry as a four-byte scalar CBuffer, receives `emissiveStrength` in X. YZW remain copied from the template.
- Other parameters, including `E9437406` and `4620A35D`, are preserved from the source template without authoring semantics.

This path requires a non-zero finite/float32-representable `emissiveFactor` and positive finite/float32-representable `emissiveStrength`; `baseColorFactor` must remain white, metallic=0, roughness=1, and double-sided/transmission/IOR are rejected. Normal, metallic-roughness, specular, specularColor, occlusion and tintPalette roles are rejected. Only the one combined diffuse/emissive source is supported.

Evidence limits: the preset identity and CBuffer names/layout are cross-referenced by the upstream CodeWalker conversion XML; the real YFT sample provides the exact hashes, values, texture name and geometry assignment. The source TextureBase has UsageData=0 (UNKNOWN), not a typed `TextureUsage::EMISSIVE` value. This does **not** prove that the game renders the material emissively or that the output looks correct. No separate asset instance with the same signature has yet been verified; keep that as a provenance limitation. `emissiveStrength`→`emissiveMultiplier` is the narrowly enabled mapping; it is not a claim that this reproduces every GTA emissive material or GTA lighting/bloom behavior.

### Verified template: `emissive.sps`

Observed in the read-only Arena light YDRs, with two independent drawable assets:
- `arena_light_a.ydr`, shader index 0: `nameHash=0x8F245469`, `fileHash=0x9D183303`, 7 ordered parameters, parameter/data sizes 208/272, one DiffuseSampler.
- `arena_lamps_01a.ydr`, shader index 1: the same shader hashes, ordered descriptor signature and block sizes.
- `arena_light_a.ydr`, shader index 3: a second instance in the same drawable with the same signature but a third observed TextureBase variant.

The signature gate requires the ordered descriptors `F1FE2B71(type0,u1=0), 948A54F2(type1,u1=165), E9437406(type1,u1=164), 4620A35D(type1,u1=163), 5EEBED48(type1,u1=162), BA54C190(type1,u1=161), D79BFC1E(type1,u1=160)`, zero descriptor unknown2/unknown4 fields, the exact header/reserved fields, and the one-sampler TextureBase. Only these three complete TextureBase variants are accepted:
- VFT `0x40619668`, unknown32 `0x80`, UsageData `0x2000B414` (`arena_light_a`, shader 0).
- VFT `0x40619668`, unknown32 `0x80`, UsageData `0x2000B014` (`arena_lamps_01a`, shader 1).
- VFT `0x4061A8F8`, unknown32 `0x0002`, UsageData `0` (`arena_light_a`, shader 3).
All other reserved TextureBase fields must be zero, unknown4=1, unknown30=1, and the embedded name must match the sampler name. No automatic VFT/UsageData normalization occurs.

The material mapping is intentionally the same narrow one as the sibling `emissivenight.sps`: one DiffuseSampler receives `textureRoles.emissive` preferentially, or `textureRoles.albedo` when emissive is absent. If both roles are supplied, they must refer to the same texture index and UV set. `matMaterialColorScale` (`0x948A54F2`) receives emissiveFactor RGB in Vector4 XYZ while W remains copied; `emissiveMultiplier` (`0x5EEBED48`) receives emissiveStrength in X while YZW remain copied. The two remaining vectors are preserved untouched. The upstream CodeWalker Gen9 conversion registry identifies the CBuffer fields as a four-byte emissive multiplier and a 16-byte material color scale; the real YDRs independently confirm the exact Legacy shader hashes and matching parameter order/values.

The same fail-closed material policy as `emissivenight.sps` applies: `shaderIntent=emissive`, opaque only, white baseColorFactor, metallic=0, roughness=1, non-zero finite emissiveFactor, positive finite emissiveStrength, no double-sided/transmission/IOR, and one combined texture source only. Normal, metallic-roughness, specular, specularColor, occlusion and tintPalette roles are rejected. The three observed TextureBase variants are a whitelist, not proof that all `emissive.sps` assets share these layouts.

Provenance: `arena_light_a.ydr` and `arena_lamps_01a.ydr` were extracted read-only from the nested `mpchristmas2018/x64/levels/gta5/props/prop_arena_lights_01.rpf` in the GTA Legacy RPF tree. Staged copies live only under `.tmp/yft-samples` outside Git. The archives and GTA installation were not modified. This gives two independent YDR assets and three concrete shader instances, but no CodeWalker/GTA/FiveM runtime acceptance; `runtimeRenderable=false` remains mandatory.

### Current fail-closed material limits

The three lit presets (`normal.sps`, `spec.sps` and `normal_spec_tnt.sps`) require `shader_intent=lit`, `surface_mode=opaque`, `baseColorFactor=[1,1,1,1]`, `metallic=0`, `roughness=1`, `emissiveFactor=[0,0,0]` and `emissiveStrength=1`; double-sided, transmission and IOR are rejected. Bake intended tint into the albedo image. `normal.sps` accepts albedo, normal and one of specular/specularColor, rejecting metallic-roughness, occlusion, emissive and tintPalette. `spec.sps` accepts only albedo and specular, rejecting normal, metallic-roughness, occlusion, emissive, specularColor and tintPalette. `normal_spec_tnt.sps` accepts albedo, tintPalette, normal and specular, rejecting metallic-roughness, occlusion, emissive and specularColor; its selector stays at the source value zero, so other palette slots cannot be authored. The `HardAlphaBlend` Vector4 in `spec.sps` is preserved but not authorable. No alpha/masked/blended or glass/transmission behavior is claimed. The two emissive presets use separate exact signatures and the same restricted emissive role/factor policy documented above.

## External acceptance gates

The following gates remain explicitly **NOT RUN** for outputs produced by this exporter:

- **CodeWalker acceptance:** no exported YDR was opened, inspected or previewed in CodeWalker during this stage.
- **GTA V Legacy runtime:** no exported YDR was installed, streamed or rendered in the game.
- **FiveM runtime:** no exported YDR was added to a resource or streamed/rendered by FiveM.

RageLab's semantic reopen checks its own parsed structures, parameter payloads, embedded dictionary and geometry-to-shader indices. Those checks establish binary roundtrip consistency only; they do not establish shader compilation, texture sampling, render-bucket behavior, alpha behavior, visual correctness or engine acceptance. The CLI therefore continues to report `runtimeRenderable=false`.

## Fail-closed boundaries

Export is blocked for unsupported required glTF extensions, unsupported vertex attributes, missing material UV sets, external images/buffers, unresolved extension texture sources, sparse/compressed geometry, non-triangle primitive modes and unsupported scene features. TEXCOORD sets above 2 are also blocked. The image decoder currently supports embedded PNG only.

A successful `glb export` means RageLab wrote a v165 RSC7 Drawable and semantically reopened its geometry, embedded TextureDictionary and (when supplied) ShaderFX parameters and geometry mappings. A structural export without `--shader-template` has no ShaderFX group and is not safe to open in the CodeWalker Model Viewer: CodeWalker's `BasicShader.SetGeomVars` reads `DrawableGeometry.Shader.Name`/`FileName` without a null guard, while its shaderless geometry batch can resolve to `Shader=null`. Use a verified Legacy YFT/YDR template for Model Viewer inspection. The supported template path copies parameter/TextureBase data from a separate Legacy YFT or YDR; no proprietary template binary is bundled. Other observed signatures remain catalog-only: `default.sps` (`0xE4DF46D5/0x18AD1594`) and `prop_generator_03b`'s 12-parameter detail-sampler shader (`0xD7C59E09/0x1E23223D`). The generator's 5-parameter emissivenight shader (`0xB08C6435/0x557DB6E4`) and Arena YDR emissive shader (`0x8F245469/0x9D183303`) are now separately supported under the narrow policies above. Earlier YFT samples were extracted by RageLab's native RPF reader, read-only: `prop_bollard_02a` from `x64h/v_utility`, `prop_roadpole_01a` from `x64f/v_construction` and `prop_generator_03b` from the `patchday3ng` override. The Arena YDR samples came from the nested `mpchristmas2018/x64/levels/gta5/props/prop_arena_lights_01.rpf`. All staged binaries remain outside Git; source RPFs and the GTA installation were not modified.

| Source / shader index | nameHash / fileHash | Header sizes (parameter/data) | Parameters / samplers | TextureBase evidence | Status |
|---|---|---:|---|---|---|
| `prop_roadpole_01a`, 0 | `7204A391/85F50BE9` | 320 / 400 | 11 / 2: Diffuse `F1FE2B71`, Spec `608799C6` | VFT `406187F8`; unknown4=1, unknown30=1, unknown32=2 | Implemented as `spec.sps` |
| `prop_picnictable_01`, 0 | `38DD00DF/14A780FD` | 304 / 384 | 11 / 3: Diffuse `F1FE2B71`, Bump `46B7C64F`, Spec `608799C6` | VFT `406187F8`; unknown4=1, unknown30=1, unknown32=2 | Implemented as `normal.sps` |
| `prop_bollard_02a`, 0 | `6CD4735B/304AB527` | 352 / 448 | 13 / 4: Diffuse `F1FE2B71`, TintPalette `F648A067`, Bump `46B7C64F`, Spec `608799C6` | VFT `40617568`; unknown4=1, unknown30=1, unknown32=`0x80`; role-specific UsageData preserved | Implemented as `normal_spec_tnt.sps` |
| `prop_generator_03b`, 1 | `D7C59E09/1E23223D` | 320 / 400 | 12 / 4: Diffuse `F1FE2B71`, Detail `CA4299E4`, Bump `46B7C64F`, Spec `608799C6` | VFT `4061A8F8`; unknown4=1, unknown30=1, unknown32=2, UsageData=0 | Catalog only; source samplers: `Prop_Generator_03`, `ENV_NOISE_HEAVY`, `Prop_Generator_03_n`, `Prop_Generator_03_s` |
| `prop_generator_03b`, 2 | `B08C6435/557DB6E4` | 144 / 208 | 5 / 1: Diffuse `F1FE2B71` | VFT `4061A8F8`; unknown4=1, unknown30=1, unknown32=2, UsageData=0 | Implemented narrowly as `emissivenight.sps`; one YFT sample, runtime behavior unverified |
| `arena_light_a`, 0 | `8F245469/9D183303` | 208 / 272 | 7 / 1: Diffuse `F1FE2B71` | VFT `40619668`; unknown4=1, unknown30=1, unknown32=`0x80`, UsageData=`0x2000B414` | Implemented as `emissive.sps`; Arena YDR sample |
| `arena_lamps_01a`, 1 | `8F245469/9D183303` | 208 / 272 | 7 / 1: Diffuse `F1FE2B71` | VFT `40619668`; unknown4=1, unknown30=1, unknown32=`0x80`, UsageData=`0x2000B014` | Implemented as `emissive.sps`; second Arena YDR asset |
| `arena_light_a`, 3 | `8F245469/9D183303` | 208 / 272 | 7 / 1: Diffuse `F1FE2B71` | VFT `4061A8F8`; unknown4=1, unknown30=1, unknown32=2, UsageData=0 | Implemented as `emissive.sps`; third TextureBase variant |
| `prop_roadpole_01a`, 1 | `E4DF46D5/18AD1594` | 208 / 272 | 7 / 1: Diffuse `F1FE2B71` | VFT `406187F8`; unknown4=1, unknown30=1, unknown32=2 | Catalog only; JOAAT name `default.sps` |

Ordered parameter hash sequences from those observed samples:

- Bollard palette shader: `F1FE2B71, F648A067, 46B7C64F, 608799C6, E9437406, 4620A35D, 32DD9FF5, F6712B81, FF11711D, F418334F, 87744680, 27B9B2FA, FDD796CF`.
- Generator detail shader descriptors in order are:
  `(F1FE2B71,type=0,u1=0), (CA4299E4,0,2), (46B7C64F,0,3), (608799C6,0,4), (B51E2E8F,1,168), (4620A35D,1,166), (32DD9FF5,1,165), (F6712B81,1,164), (FF11711D,1,163), (F418334F,1,162), (87744680,1,161), (27B9B2FA,1,160)`. All descriptor unknown2/unknown4 values are zero. Its Vector4 payloads by hash are: `B51E2E8F=[0.024,0.024,8,8]`, `4620A35D=[0,0,0,0]`, `32DD9FF5=[1,0,0,0]`, `F6712B81=[1,0,0,0]`, `FF11711D=[1,0,0,0]`, `F418334F=[1,0,0,0]`, `87744680=[120,0,0,0]`, `27B9B2FA=[0.96,0,0,0]`.
- Generator one-sampler shader descriptors in order are `(F1FE2B71,type=0,u1=0), (948A54F2,1,163), (E9437406,1,162), (4620A35D,1,161), (5EEBED48,1,160)`; all descriptor unknown2/unknown4 values are zero. Vector payloads: `948A54F2=[1,0,0,1]`, `E9437406=[1,0,0,0]`, `4620A35D=[0,0,0,0]`, `5EEBED48=[6,0,0,0]`.
- Road-pole default shader: `F1FE2B71, 948A54F2, E9437406, 4620A35D, 32DD9FF5, BA54C190, D79BFC1E`.

The generator shader at index 1 resolves by JOAAT to `normal_spec_detail` / `normal_spec_detail.sps` (`0xD7C59E09/0x1E23223D`). It is assigned to four primitives: (model0/geometry1: 482 vertices/306 triangles), (model1/geometry1: 152/94), (model3/geometry1: 20/10) and (model6/geometry1: 52/30). All four expose UV0 only in the current preview; no UV1 stream is decoded for them. Each of its four TextureBase objects uses VFT `0x4061A8F8`, unknown4=1, unknown30=1, unknown32=2, reserved fields zero and UsageData=0. The `DetailSampler` source is `ENV_NOISE_HEAVY`; its name and `detailSettings=[0.024,0.024,8,8]` suggest a detail/noise layer, but the asset alone does not prove which UV transform/blend channels are consumed or whether the texture should be authored as albedo/detail/normal. No profile binding is enabled.

The `prop_generator_03b` shader at index 2 is assigned to exactly one preview primitive in the parsed main drawable: model index 2, geometry index 0, 112 vertices and 84 triangles. That shader's sole texture reference is `Prop_Bulb_White`; its `emissiveMultiplier` Vector4 is `[6,0,0,0]` and `matMaterialColorScale` is `[1,0,0,1]`. Its TextureBase uses VFT `0x4061A8F8`, unknown4=1, unknown30=1, unknown32=2 and UsageData=0 (UNKNOWN, not `TextureUsage::EMISSIVE`). The CLI now supports a narrow `emissivenight.sps` path by mapping glTF `emissiveFactor` RGB to `matMaterialColorScale` XYZ and `emissiveStrength` to `emissiveMultiplier` X, while preserving un-authored lanes and the other parameters. This mapping is corroborated by the CodeWalker Gen9 conversion XML's named CBuffer layouts, but only this one real YFT sample has been observed so far. It does not prove GTA runtime rendering, bloom or visual correctness; those remain unverified.


The generator detail candidate at index 1 remains catalog-only. Its exact descriptors and `detailSettings=[0.024,0.024,8,8]` are documented above, but the sampler source `ENV_NOISE_HEAVY`, UV0-only geometry and UsageData=0 do not prove the UV transform or blend/channel semantics. The same hash/type names can occur in different templates, but the signatures are not interchangeable. For example, the generator's `normal.sps` uses TextureBase VFT `0x4061A8F8`, while the enabled normal template sample uses `0x406187F8`; the bollard sample has `0x40617568` and `unknown32=0x80`. Those variants are deliberately rejected by the current exact-template gates rather than being normalized or rewritten.

The CodeWalker cross-reference resolves parameter-name hashes such as `TintPaletteSampler`, `tintPaletteSelector`, `DetailSampler`, `detailSettings`, `emissiveMultiplier`, `matMaterialColorScale`, `HardAlphaBlend` and specular controls. Its Gen9 conversion XML maps `emissivenight.sps`'s `emissiveMultiplier` to a four-byte CBuffer and `matMaterialColorScale` to a 16-byte CBuffer; this supports the limited field mapping above. A parameter name or Gen9 conversion entry alone is not proof of Gen8 shader rendering behavior. The CodeWalker viewport HLSL is not used as a ShaderFX template source.

A successful export does **not** mean the resource was accepted by GTA V runtime, loaded in FiveM, rendered correctly, or accepted by CodeWalker. The `TextureBase` VFT differs across observed assets (including `0x40617568`, `0x4061A8F8` and `0x406187F8`); current signature gates intentionally require the exact VFT present in each verified source template rather than assuming all layouts are interchangeable. Additional templates and runtime validation remain separate work.

## Independent CodeWalker parser validation — 2026-10-30

The five enabled presets were exported from one deterministic synthetic GLB (one triangle, three vertices, UV1, one embedded 1x1 PNG) into temporary YDR v165 files outside the repository. The source templates remained read-only: normal from `prop_picnictable_01.yft`, spec from `prop_roadpole_01a.yft`, tint palette from `prop_bollard_02a.yft`, emissivenight from `prop_generator_03b.yft`, and emissive from shader index 0 of `arena_light_a.ydr`.

An isolated .NET 9 harness references the official CodeWalker Core project at revision `485d56bec00262ed7fa472261cce7bbc6202b96e`. It calls `YdrFile.Load(byte[])` and verifies the decoded Drawable, model/geometry arrays, vertex/index buffers, ShaderGroup, ordered ShaderFX metadata, sampler TextureBase records and embedded TextureDictionary. CodeWalker.Core built successfully for `netstandard2.0`; the harness built with zero warnings/errors.

Results: **5/5 YDRs parsed successfully**. Each contains one model, one geometry with 3 vertices and 3 indices, one ShaderFX with the expected exact signature and one embedded texture (1x1 DXT1). CodeWalker recovered the expected parameter counts (11 normal, 11 spec, 13 tint, 5 emissivenight, 7 emissive), sampler counts (3, 2, 4, 1, 1) and source-specific sampler TextureBase VFT/UsageData. This is independent structural parser acceptance, not GUI inspection or engine acceptance.

The independent read exposed a TextureDictionary descriptor-default mismatch: RageLab had left `TextureBase.Unknown_4h` and `Unknown_30h` as zero. CodeWalker's `TextureBase` constructor defaults these fields to 1 (while a newly constructed Legacy descriptor retains VFT=0 and Unknown_32h=0). The YDR writer now serializes those defaults explicitly; the YTD parser exposes VFT/Unknown_4/Unknown_30/Unknown_32 in `DictionaryTextureInfo`, and a writer regression asserts them after semantic reopen. The shader sampler TextureBase records continue to preserve their exact template-specific VFT/UsageData.

This gate covers CodeWalker Core parsing only. The CodeWalker GUI, GTA V Legacy runtime load/render and FiveM load/render were not run. `runtimeRenderable=false` remains mandatory until a safe, explicit engine acceptance test is completed.
