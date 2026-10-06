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
