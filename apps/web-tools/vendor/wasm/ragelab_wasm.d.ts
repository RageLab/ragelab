/* tslint:disable */
/* eslint-disable */
export function capabilities(): any;
export function validateAsset(format: string, bytes: Uint8Array): any;
export function inspectYmap(bytes: Uint8Array): any;
export function inspectYtyp(bytes: Uint8Array): any;
export function resolveSuppliedYmapScene(ymap_bytes: Uint8Array, supplied: any): any;
export function inspectYtd(bytes: Uint8Array): any;
export function inspectYdrMaterials(bytes: Uint8Array): any;
export function inspectYddMaterials(bytes: Uint8Array): any;
export function inspectYdd(bytes: Uint8Array): any;
export function inspectYft(bytes: Uint8Array): any;
export function ydrModel(bytes: Uint8Array): WasmModelPacket;
export function yddModel(bytes: Uint8Array, drawable_index: number): WasmModelPacket;
export function yftModel(bytes: Uint8Array): WasmModelPacket;
export function ytdTexture(bytes: Uint8Array, index: number): WasmTexturePacket;
export function ytdTextureByName(bytes: Uint8Array, name: string): WasmTexturePacket;
export function ydrEmbeddedTexture(bytes: Uint8Array, texture_index: number): WasmTexturePacket;
export function ydrEmbeddedTextureByName(bytes: Uint8Array, name: string): WasmTexturePacket;
export function yddEmbeddedTexture(bytes: Uint8Array, drawable_index: number, texture_index: number): WasmTexturePacket;
export function yddEmbeddedTextureByName(bytes: Uint8Array, drawable_index: number, name: string): WasmTexturePacket;
export function yftEmbeddedTexture(bytes: Uint8Array, texture_index: number): WasmTexturePacket;
export function yftEmbeddedTextureByName(bytes: Uint8Array, name: string): WasmTexturePacket;
export function ytdTextureMip(bytes: Uint8Array, index: number, mip_index: number): WasmTexturePacket;
export function exportYtdTexturePng(bytes: Uint8Array, index: number): Uint8Array;
export function replaceYtdTexturePng(bytes: Uint8Array, index: number, png_bytes: Uint8Array): Uint8Array;
export function ymapSetTransform(bytes: Uint8Array, index: number, position: Float32Array, rotation: Float32Array, scale_xy?: number | null, scale_z?: number | null): Uint8Array;
export function ymapSetArchetype(bytes: Uint8Array, index: number, archetype_hash: number): Uint8Array;
export function ymapSetFlags(bytes: Uint8Array, index: number, flags: number): Uint8Array;
export function ymapSetParent(bytes: Uint8Array, index: number, parent_index?: number | null): Uint8Array;
export function ydrRebindShader(bytes: Uint8Array, model_index: number, geometry_index: number, target_shader_index: number): Uint8Array;
export function ydrRebindTexture(bytes: Uint8Array, source_shader_index: number, source_parameter_index: number, target_shader_index: number, target_parameter_index: number): Uint8Array;
export function yddRebindShader(bytes: Uint8Array, drawable_index: number, model_index: number, geometry_index: number, target_shader_index: number): Uint8Array;
export function yddRebindTexture(bytes: Uint8Array, drawable_index: number, source_shader_index: number, source_parameter_index: number, target_shader_index: number, target_parameter_index: number): Uint8Array;
export class WasmModelPacket {
  private constructor();
  free(): void;
  metadata(): any;
  positions(): Float32Array;
  normals(): Float32Array;
  uv0(): Float32Array;
  indices(): Uint32Array;
}
export class WasmTexturePacket {
  private constructor();
  free(): void;
  metadata(): any;
  rgba(): Uint8Array;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly __wbg_wasmmodelpacket_free: (a: number, b: number) => void;
  readonly wasmmodelpacket_metadata: (a: number, b: number) => void;
  readonly wasmmodelpacket_positions: (a: number) => number;
  readonly wasmmodelpacket_normals: (a: number) => number;
  readonly wasmmodelpacket_uv0: (a: number) => number;
  readonly wasmmodelpacket_indices: (a: number) => number;
  readonly __wbg_wasmtexturepacket_free: (a: number, b: number) => void;
  readonly wasmtexturepacket_metadata: (a: number, b: number) => void;
  readonly wasmtexturepacket_rgba: (a: number) => number;
  readonly capabilities: (a: number) => void;
  readonly validateAsset: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly inspectYmap: (a: number, b: number, c: number) => void;
  readonly inspectYtyp: (a: number, b: number, c: number) => void;
  readonly resolveSuppliedYmapScene: (a: number, b: number, c: number, d: number) => void;
  readonly inspectYtd: (a: number, b: number, c: number) => void;
  readonly inspectYdrMaterials: (a: number, b: number, c: number) => void;
  readonly inspectYddMaterials: (a: number, b: number, c: number) => void;
  readonly inspectYdd: (a: number, b: number, c: number) => void;
  readonly inspectYft: (a: number, b: number, c: number) => void;
  readonly ydrModel: (a: number, b: number, c: number) => void;
  readonly yddModel: (a: number, b: number, c: number, d: number) => void;
  readonly yftModel: (a: number, b: number, c: number) => void;
  readonly ytdTexture: (a: number, b: number, c: number, d: number) => void;
  readonly ytdTextureByName: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly ydrEmbeddedTexture: (a: number, b: number, c: number, d: number) => void;
  readonly ydrEmbeddedTextureByName: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly yddEmbeddedTexture: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly yddEmbeddedTextureByName: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly yftEmbeddedTexture: (a: number, b: number, c: number, d: number) => void;
  readonly yftEmbeddedTextureByName: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly ytdTextureMip: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly exportYtdTexturePng: (a: number, b: number, c: number, d: number) => void;
  readonly replaceYtdTexturePng: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly ymapSetTransform: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => void;
  readonly ymapSetArchetype: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly ymapSetFlags: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly ymapSetParent: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly ydrRebindShader: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly ydrRebindTexture: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
  readonly yddRebindShader: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
  readonly yddRebindTexture: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;
/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
