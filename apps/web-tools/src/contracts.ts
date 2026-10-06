export interface TextureExportCapabilities {
  pngTopMip: boolean;
  classicDdsFullMips: boolean;
}

export interface TextureReplacementCapabilities {
  layoutPreservingDds: boolean;
  relocatedDds: boolean;
  png: boolean;
  targetFormatPreserved: boolean;
  dimensionChanges: boolean;
  mipChainRegenerated: boolean;
  reason: string | null;
}

export interface TextureAuthoringEntry {
  index: number;
  name: string;
  dictionaryHash: string;
  nameHash: string;
  dictionaryHashMatchesName: boolean;
  width: number;
  height: number;
  depth: number;
  stride: number;
  format: string;
  formatRaw: string;
  mipLevels: number;
  usage: number;
  usageFlags: string;
  extraFlags: string;
  encodedBytes: number;
  export: TextureExportCapabilities;
  replacement: TextureReplacementCapabilities;
}

export interface TextureAuthoringReport {
  schema: "ragelab.texture-authoring";
  schemaVersion: number;
  textureCount: number;
  textures: TextureAuthoringEntry[];
  rules: string[];
}

export interface TexturePacketMetadata {
  schema: "ragelab.wasm.texture";
  schemaVersion: number;
  index: number;
  mipIndex: number;
  name: string;
  nameHash: string;
  width: number;
  height: number;
  format: string;
  mipLevels: number;
}

export interface TexturePacketLike {
  metadata(): unknown;
  rgba(): Uint8Array;
  free?(): void;
}

export type ModelFormat = "ydr" | "ydd" | "yft";

export interface ModelBounds {
  center: [number, number, number];
  radius: number;
  min: [number, number, number];
  max: [number, number, number];
}

export interface ModelShaderTextureReference {
  parameterHash: string;
  textureName: string | null;
}

export interface ModelShaderMetadata {
  index: number;
  nameHash: string;
  fileHash: string;
  diffuseTextureName: string | null;
  textureReferences: ModelShaderTextureReference[];
}

export interface ModelPrimitiveMetadata {
  index: number;
  modelIndex: number;
  geometryIndex: number;
  shaderIndex: number | null;
  topology: string;
  positionFloatOffset: number;
  vertexCount: number;
  normalFloatOffset: number | null;
  uvFloatOffset: number | null;
  indexOffset: number;
  indexCount: number;
}

export interface ModelMetadata {
  schema: "ragelab.wasm.model";
  schemaVersion: number;
  format: ModelFormat;
  selectorIndex: number | null;
  selectorHash: string | null;
  selectorName: string | null;
  name: string | null;
  lod: string;
  coordinateConvention: string;
  bounds: ModelBounds;
  primitiveCount: number;
  vertexCount: number;
  indexCount: number;
  embeddedTextureCount: number;
  shaders: ModelShaderMetadata[];
  primitives: ModelPrimitiveMetadata[];
}

export interface ModelPacketLike {
  metadata(): unknown;
  positions(): Float32Array;
  normals(): Float32Array;
  uv0(): Float32Array;
  indices(): Uint32Array;
  free?(): void;
}

export interface ModelPacketData {
  metadata: ModelMetadata;
  positions: Float32Array;
  normals: Float32Array;
  uv0: Float32Array;
  indices: Uint32Array;
}

export interface YddEntryReport {
  index: number;
  nameHash: string;
  name: string | null;
}

export interface YddInspectReport {
  schema: "ragelab.wasm.ydd";
  schemaVersion: number;
  entries: YddEntryReport[];
}

export interface YftInspectReport {
  schema: "ragelab.wasm.yft";
  schemaVersion: number;
  name: string | null;
  hasMainDrawable: boolean;
}

export interface YmapEntityReport {
  index: number;
  archetypeHash: string;
  position: [number, number, number];
  rotation: [number, number, number, number];
  scaleXY: number | null;
  scaleZ: number | null;
  flags: number;
  parentIndex: number | null;
}

export interface YmapInspectReport {
  schema: "ragelab.wasm.ymap";
  schemaVersion: number;
  nameHash: string | null;
  parentHash: string | null;
  flags: number | null;
  contentFlags: number | null;
  physicsDictionaries: string[];
  entitiesExtentsMin: [number, number, number] | null;
  entitiesExtentsMax: [number, number, number] | null;
  streamingExtentsMin: [number, number, number] | null;
  streamingExtentsMax: [number, number, number] | null;
  entities: YmapEntityReport[];
}

export interface SuppliedCatalogEntry {
  index: number;
  name: string;
  format: "ytyp" | "ydr" | "ydd" | "yft" | "ytd";
  nameHash: string;
}

export interface SuppliedModelResolution {
  ytypDependencyIndex: number;
  archetypeIndex: number;
  modelDependencyIndex: number;
  modelFormat: ModelFormat;
  drawableIndex: number | null;
  assetHash: string;
  textureDictionaryHash: string | null;
}

export interface SuppliedSceneEntity extends YmapEntityReport {
  resolution: SuppliedModelResolution | null;
}

export interface SuppliedSceneDiagnostic {
  entityIndex: number | null;
  code: string;
  message: string;
}

export interface SuppliedYmapSceneReport {
  schema: "ragelab.wasm.supplied-ymap-scene";
  schemaVersion: number;
  ymap: YmapInspectReport;
  catalog: SuppliedCatalogEntry[];
  entities: SuppliedSceneEntity[];
  diagnostics: SuppliedSceneDiagnostic[];
}

export interface ModelDependency {
  name: string;
  bytes: Uint8Array;
}

export interface ResolvedModelTexture {
  metadata: TexturePacketMetadata;
  rgba: Uint8Array;
  source: "embedded" | "dependency";
  sourceName: string;
}

export interface DiffuseResolution {
  texture: ResolvedModelTexture | null;
  diagnostics: string[];
}

export interface WasmErrorReport {
  schema: "ragelab.wasm.error";
  schemaVersion: number;
  format: string;
  code: string;
  message: string;
}

export function isTextureReport(value: unknown): value is TextureAuthoringReport {
  if (!value || typeof value !== "object") return false;
  const report = value as Partial<TextureAuthoringReport>;
  return report.schema === "ragelab.texture-authoring"
    && Array.isArray(report.textures)
    && typeof report.textureCount === "number";
}

export function isTextureMetadata(value: unknown): value is TexturePacketMetadata {
  if (!value || typeof value !== "object") return false;
  const metadata = value as Partial<TexturePacketMetadata>;
  return metadata.schema === "ragelab.wasm.texture"
    && typeof metadata.index === "number"
    && typeof metadata.mipIndex === "number"
    && typeof metadata.width === "number"
    && typeof metadata.height === "number";
}

export function isModelMetadata(value: unknown): value is ModelMetadata {
  if (!value || typeof value !== "object") return false;
  const metadata = value as Partial<ModelMetadata>;
  return metadata.schema === "ragelab.wasm.model"
    && (metadata.format === "ydr" || metadata.format === "ydd" || metadata.format === "yft")
    && typeof metadata.primitiveCount === "number"
    && typeof metadata.vertexCount === "number"
    && typeof metadata.indexCount === "number"
    && Array.isArray(metadata.shaders)
    && Array.isArray(metadata.primitives);
}

export function isYddInspectReport(value: unknown): value is YddInspectReport {
  if (!value || typeof value !== "object") return false;
  const report = value as Partial<YddInspectReport>;
  return report.schema === "ragelab.wasm.ydd" && Array.isArray(report.entries);
}

export function isYftInspectReport(value: unknown): value is YftInspectReport {
  if (!value || typeof value !== "object") return false;
  const report = value as Partial<YftInspectReport>;
  return report.schema === "ragelab.wasm.yft" && typeof report.hasMainDrawable === "boolean";
}

export function isYmapInspectReport(value: unknown): value is YmapInspectReport {
  if (!value || typeof value !== "object") return false;
  const report = value as Partial<YmapInspectReport>;
  return report.schema === "ragelab.wasm.ymap" && Array.isArray(report.entities);
}

export function isSuppliedYmapSceneReport(value: unknown): value is SuppliedYmapSceneReport {
  if (!value || typeof value !== "object") return false;
  const report = value as Partial<SuppliedYmapSceneReport>;
  return report.schema === "ragelab.wasm.supplied-ymap-scene"
    && isYmapInspectReport(report.ymap)
    && Array.isArray(report.catalog)
    && Array.isArray(report.entities)
    && Array.isArray(report.diagnostics);
}

export function wasmError(error: unknown): WasmErrorReport {
  if (error && typeof error === "object") {
    const candidate = error as Partial<WasmErrorReport>;
    if (
      candidate.schema === "ragelab.wasm.error"
      && typeof candidate.code === "string"
      && typeof candidate.message === "string"
    ) {
      return candidate as WasmErrorReport;
    }
  }
  return {
    schema: "ragelab.wasm.error",
    schemaVersion: 1,
    format: "web",
    code: "unexpectedError",
    message: error instanceof Error ? error.message : String(error),
  };
}
