import init, {
  exportYtdTexturePng,
  inspectYdd,
  inspectYft,
  inspectYmap,
  inspectYtd,
  replaceYtdTexturePng,
  resolveSuppliedYmapScene,
  validateAsset,
  yddEmbeddedTextureByName,
  yddModel,
  ydrEmbeddedTextureByName,
  ydrModel,
  yftEmbeddedTextureByName,
  yftModel,
  ymapSetFlags,
  ymapSetTransform,
  ytdTextureByName,
  ytdTextureMip,
} from "@ragelab/wasm";
import {
  isModelMetadata,
  isTextureMetadata,
  isTextureReport,
  isSuppliedYmapSceneReport,
  isYddInspectReport,
  isYftInspectReport,
  isYmapInspectReport,
  type DiffuseResolution,
  type ModelDependency,
  type ModelFormat,
  type ModelPacketData,
  type ModelPacketLike,
  type ResolvedModelTexture,
  type TextureAuthoringReport,
  type TexturePacketLike,
  type TexturePacketMetadata,
  type SuppliedYmapSceneReport,
  type YddInspectReport,
  type YftInspectReport,
  type YmapInspectReport,
  wasmError,
} from "./contracts";

let initPromise: Promise<unknown> | null = null;

export async function initCore(): Promise<void> {
  initPromise ??= init();
  await initPromise;
}

export function inspectYtdBytes(bytes: Uint8Array): TextureAuthoringReport {
  const report = inspectYtd(bytes);
  if (!isTextureReport(report)) {
    throw new Error("Rust/WASM returned an invalid YTD authoring report");
  }
  return report;
}

export function textureMip(
  bytes: Uint8Array,
  textureIndex: number,
  mipIndex: number,
): { metadata: TexturePacketMetadata; rgba: Uint8Array } {
  const packet = ytdTextureMip(bytes, textureIndex, mipIndex) as TexturePacketLike;
  const metadata = packet.metadata();
  if (!isTextureMetadata(metadata)) {
    throw new Error("Rust/WASM returned invalid texture metadata");
  }
  return { metadata, rgba: packet.rgba() };
}

export function exportTopMipPng(bytes: Uint8Array, textureIndex: number): Uint8Array {
  return exportYtdTexturePng(bytes, textureIndex);
}

function copyTexturePacket(
  packet: TexturePacketLike,
  source: ResolvedModelTexture["source"],
  sourceName: string,
): ResolvedModelTexture {
  try {
    const metadata = packet.metadata();
    if (!isTextureMetadata(metadata)) {
      throw new Error("Rust/WASM returned invalid texture metadata");
    }
    return {
      metadata,
      rgba: new Uint8Array(packet.rgba()),
      source,
      sourceName,
    };
  } finally {
    packet.free?.();
  }
}

export function modelPacket(
  format: ModelFormat,
  bytes: Uint8Array,
  drawableIndex = 0,
): ModelPacketData {
  const packet = (
    format === "ydr"
      ? ydrModel(bytes)
      : format === "ydd"
        ? yddModel(bytes, drawableIndex)
        : yftModel(bytes)
  ) as ModelPacketLike;
  try {
    const metadata = packet.metadata();
    if (!isModelMetadata(metadata)) {
      throw new Error("Rust/WASM returned invalid model metadata");
    }
    return {
      metadata,
      positions: new Float32Array(packet.positions()),
      normals: new Float32Array(packet.normals()),
      uv0: new Float32Array(packet.uv0()),
      indices: new Uint32Array(packet.indices()),
    };
  } finally {
    packet.free?.();
  }
}

export function inspectYmapBytes(bytes: Uint8Array): YmapInspectReport {
  const report = inspectYmap(bytes);
  if (!isYmapInspectReport(report)) {
    throw new Error("Rust/WASM returned an invalid YMAP report");
  }
  return report;
}

export function resolveSuppliedScene(
  ymapBytes: Uint8Array,
  dependencies: ModelDependency[],
): SuppliedYmapSceneReport {
  const report = resolveSuppliedYmapScene(
    ymapBytes,
    dependencies.map((dependency) => ({
      name: dependency.name,
      bytes: dependency.bytes,
    })),
  );
  if (!isSuppliedYmapSceneReport(report)) {
    throw new Error("Rust/WASM returned an invalid supplied YMAP scene report");
  }
  return report;
}

function validateEditedYmap(bytes: Uint8Array): YmapInspectReport {
  const validation = validateAsset("ymap", bytes) as { valid?: unknown };
  if (validation?.valid !== true) {
    throw new Error("edited YMAP semantic reopen did not return valid=true");
  }
  return inspectYmapBytes(bytes);
}

export function setYmapEntityFlags(
  bytes: Uint8Array,
  entityIndex: number,
  flags: number,
): { bytes: Uint8Array; report: YmapInspectReport } {
  const candidate = new Uint8Array(ymapSetFlags(bytes, entityIndex, flags));
  return { bytes: candidate, report: validateEditedYmap(candidate) };
}

export function setYmapEntityTransform(
  bytes: Uint8Array,
  entityIndex: number,
  position: [number, number, number],
  rotation: [number, number, number, number],
  scaleXY: number | null,
  scaleZ: number | null,
): { bytes: Uint8Array; report: YmapInspectReport } {
  const candidate = new Uint8Array(
    ymapSetTransform(
      bytes,
      entityIndex,
      new Float32Array(position),
      new Float32Array(rotation),
      scaleXY,
      scaleZ,
    ),
  );
  return { bytes: candidate, report: validateEditedYmap(candidate) };
}

export function inspectYddBytes(bytes: Uint8Array): YddInspectReport {
  const report = inspectYdd(bytes);
  if (!isYddInspectReport(report)) {
    throw new Error("Rust/WASM returned an invalid YDD report");
  }
  return report;
}

export function inspectYftBytes(bytes: Uint8Array): YftInspectReport {
  const report = inspectYft(bytes);
  if (!isYftInspectReport(report)) {
    throw new Error("Rust/WASM returned an invalid YFT report");
  }
  return report;
}

function embeddedDiffuse(
  format: ModelFormat,
  bytes: Uint8Array,
  drawableIndex: number,
  name: string,
): TexturePacketLike {
  if (format === "ydr") return ydrEmbeddedTextureByName(bytes, name) as TexturePacketLike;
  if (format === "ydd") {
    return yddEmbeddedTextureByName(bytes, drawableIndex, name) as TexturePacketLike;
  }
  return yftEmbeddedTextureByName(bytes, name) as TexturePacketLike;
}

export function resolveDiffuseTexture(
  format: ModelFormat,
  modelBytes: Uint8Array,
  drawableIndex: number,
  diffuseName: string,
  dependencies: ModelDependency[],
): DiffuseResolution {
  const diagnostics: string[] = [];

  try {
    return {
      texture: copyTexturePacket(
        embeddedDiffuse(format, modelBytes, drawableIndex, diffuseName),
        "embedded",
        "embedded TextureDictionary",
      ),
      diagnostics,
    };
  } catch (error) {
    const report = wasmError(error);
    if (report.code !== "missingDependency" && report.code !== "textureNotFound") {
      return {
        texture: null,
        diagnostics: [`embedded:${report.code}: ${report.message}`],
      };
    }
  }

  for (const dependency of dependencies) {
    try {
      return {
        texture: copyTexturePacket(
          ytdTextureByName(dependency.bytes, diffuseName) as TexturePacketLike,
          "dependency",
          dependency.name,
        ),
        diagnostics,
      };
    } catch (error) {
      const report = wasmError(error);
      if (report.code === "parseFailed") {
        diagnostics.push(`${dependency.name}:parseFailed: ${report.message}`);
        continue;
      }
      if (report.code === "textureNotFound") {
        continue;
      }
      diagnostics.push(`${dependency.name}:${report.code}: ${report.message}`);
      return { texture: null, diagnostics };
    }
  }

  diagnostics.push(
    `textureNotFound: ${JSON.stringify(diffuseName)} was not found in embedded data or supplied YTD dependencies`,
  );
  return { texture: null, diagnostics };
}

export function replaceTexturePng(
  bytes: Uint8Array,
  textureIndex: number,
  pngBytes: Uint8Array,
): { bytes: Uint8Array; report: TextureAuthoringReport } {
  const candidate = replaceYtdTexturePng(bytes, textureIndex, pngBytes);
  const validation = validateAsset("ytd", candidate) as { valid?: unknown };
  if (validation?.valid !== true) {
    throw new Error("semantic reopen validation did not return valid=true");
  }
  const report = inspectYtdBytes(candidate);
  return { bytes: new Uint8Array(candidate), report };
}

export function probeMipError(
  bytes: Uint8Array,
  textureIndex: number,
  mipIndex: number,
): { ok: true } | { ok: false; code: string; message: string } {
  try {
    textureMip(bytes, textureIndex, mipIndex);
    return { ok: true };
  } catch (error) {
    const report = wasmError(error);
    return { ok: false, code: report.code, message: report.message };
  }
}

export { wasmError };
