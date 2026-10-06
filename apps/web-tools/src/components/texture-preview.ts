import type { TexturePacketMetadata } from "../contracts";

export type ChannelMode = "rgba" | "rgb" | "r" | "g" | "b" | "a";

export function renderTexture(
  canvas: HTMLCanvasElement,
  metadata: TexturePacketMetadata,
  rgba: Uint8Array,
  channel: ChannelMode,
): void {
  const expected = metadata.width * metadata.height * 4;
  if (rgba.length !== expected) {
    throw new Error(
      `RGBA payload ${rgba.length} does not match ${metadata.width}×${metadata.height}×4`,
    );
  }

  const output = new Uint8ClampedArray(rgba);
  for (let offset = 0; offset < output.length; offset += 4) {
    const r = output[offset];
    const g = output[offset + 1];
    const b = output[offset + 2];
    const a = output[offset + 3];
    switch (channel) {
      case "rgba":
        break;
      case "rgb":
        output[offset + 3] = 255;
        break;
      case "r":
        output[offset] = r;
        output[offset + 1] = r;
        output[offset + 2] = r;
        output[offset + 3] = 255;
        break;
      case "g":
        output[offset] = g;
        output[offset + 1] = g;
        output[offset + 2] = g;
        output[offset + 3] = 255;
        break;
      case "b":
        output[offset] = b;
        output[offset + 1] = b;
        output[offset + 2] = b;
        output[offset + 3] = 255;
        break;
      case "a":
        output[offset] = a;
        output[offset + 1] = a;
        output[offset + 2] = a;
        output[offset + 3] = 255;
        break;
    }
  }

  canvas.width = metadata.width;
  canvas.height = metadata.height;
  const context = canvas.getContext("2d", { alpha: true });
  if (!context) throw new Error("2D canvas context is unavailable");
  context.putImageData(new ImageData(output, metadata.width, metadata.height), 0, 0);
}
