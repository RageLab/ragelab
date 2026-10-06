import init, {
  exportYtdTexturePng,
  inspectYtd,
  replaceYtdTexturePng,
  validateAsset,
  ytdTextureMip,
} from "@ragelab/wasm";
import {
  isTextureMetadata,
  isTextureReport,
  type TextureAuthoringReport,
  type TexturePacketLike,
  type TexturePacketMetadata,
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
