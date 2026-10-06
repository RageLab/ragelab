import type { TextureAuthoringEntry, TextureAuthoringReport } from "./contracts";
import {
  exportTopMipPng,
  inspectYtdBytes,
  replaceTexturePng,
  textureMip,
} from "./core";

export class YtdDocument {
  readonly name: string;
  readonly originalBytes: Uint8Array;
  private bytesValue: Uint8Array;
  private reportValue: TextureAuthoringReport;
  dirty = false;
  semanticReopen = false;

  private constructor(name: string, bytes: Uint8Array, report: TextureAuthoringReport) {
    this.name = name;
    this.originalBytes = new Uint8Array(bytes);
    this.bytesValue = new Uint8Array(bytes);
    this.reportValue = report;
  }

  static open(name: string, bytes: Uint8Array): YtdDocument {
    const report = inspectYtdBytes(bytes);
    return new YtdDocument(name, bytes, report);
  }

  get bytes(): Uint8Array {
    return new Uint8Array(this.bytesValue);
  }

  get report(): TextureAuthoringReport {
    return this.reportValue;
  }

  texture(index: number): TextureAuthoringEntry {
    const texture = this.reportValue.textures[index];
    if (!texture) {
      throw new RangeError(`texture index ${index} is out of bounds`);
    }
    return texture;
  }

  mip(textureIndex: number, mipIndex: number) {
    return textureMip(this.bytesValue, textureIndex, mipIndex);
  }

  exportPng(textureIndex: number): Uint8Array {
    const texture = this.texture(textureIndex);
    if (!texture.export.pngTopMip) {
      throw new Error(`PNG export unavailable: ${texture.replacement.reason ?? texture.format}`);
    }
    return exportTopMipPng(this.bytesValue, textureIndex);
  }

  replacePng(textureIndex: number, pngBytes: Uint8Array): void {
    const texture = this.texture(textureIndex);
    if (!texture.replacement.png) {
      throw new Error(texture.replacement.reason ?? "PNG replacement is unsupported");
    }

    // Transactional: current bytes/report are changed only after the Rust Core
    // write has semantically reopened and produced a fresh authoring report.
    const next = replaceTexturePng(this.bytesValue, textureIndex, pngBytes);
    this.bytesValue = next.bytes;
    this.reportValue = next.report;
    this.dirty = true;
    this.semanticReopen = true;
  }

  reset(): void {
    this.bytesValue = new Uint8Array(this.originalBytes);
    this.reportValue = inspectYtdBytes(this.originalBytes);
    this.dirty = false;
    this.semanticReopen = false;
  }
}
