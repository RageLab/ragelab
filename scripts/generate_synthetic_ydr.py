#!/usr/bin/env python3
"""Generate the deterministic synthetic YDR fixture used by RageLab tests."""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT_DIR = ROOT / "fixtures" / "synthetic" / "ydr"
SIMPLE_OUTPUT = OUTPUT_DIR / "simple.ydr"
EDITABLE_OUTPUT = OUTPUT_DIR / "editable.ydr"
EMBEDDED_OUTPUT = OUTPUT_DIR / "embedded.ydr"
EXTERNAL_OUTPUT = OUTPUT_DIR / "external-diffuse.ydr"

SYSTEM_BASE = 0x50000000
SYSTEM_SIZE = 2048
SYSTEM_FLAGS = (1 << 26) | 1
DECL_TYPES = 0x7755_5555_5599_6996


def pointer(offset: int) -> int:
    return SYSTEM_BASE + offset


def write_u16(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<H", buffer, offset, value)


def write_u32(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", buffer, offset, value)


def write_u64(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<Q", buffer, offset, value)


def write_f32(buffer: bytearray, offset: int, value: float) -> None:
    struct.pack_into("<f", buffer, offset, value)


def generate(*, editable: bool = False, embedded: bool = False, external: bool = False) -> bytes:
    system = bytearray(SYSTEM_SIZE)

    # Drawable root.
    for offset, value in [
        (0x20, 0.5),
        (0x24, 0.5),
        (0x28, 0.0),
        (0x2C, 1.0),
        (0x30, 0.0),
        (0x34, 0.0),
        (0x38, 0.0),
        (0x40, 1.0),
        (0x44, 1.0),
        (0x48, 0.0),
    ]:
        write_f32(system, offset, value)
    write_u64(system, 0x50, pointer(0x0D0))
    write_u64(system, 0xA8, pointer(0x340))

    # High LOD ResourcePointerList64.
    write_u64(system, 0x0D0, pointer(0x0E0))
    write_u16(system, 0x0D8, 1)
    write_u16(system, 0x0DA, 1)
    write_u64(system, 0x0E0, pointer(0x0F0))

    # DrawableModel.
    write_u64(system, 0x0F0 + 0x08, pointer(0x120))
    write_u16(system, 0x0F0 + 0x10, 1)
    write_u16(system, 0x0F0 + 0x12, 1)
    write_u64(system, 0x0F0 + 0x18, pointer(0x680))
    write_u64(system, 0x120, pointer(0x130))

    # Model and geometry min/max Vector4 storage.
    for base in [0x680, 0x6A0]:
        for offset, value in [
            (0, 0.0),
            (4, 0.0),
            (8, 0.0),
            (12, 0.0),
            (16, 1.0),
            (20, 1.0),
            (24, 0.0),
            (28, 0.0),
        ]:
            write_f32(system, base + offset, value)

    # DrawableGeometry.
    write_u64(system, 0x130 + 0x18, pointer(0x1D0))
    write_u64(system, 0x130 + 0x38, pointer(0x250))
    write_u32(system, 0x130 + 0x58, 3)
    write_u16(system, 0x130 + 0x60, 3)
    write_u16(system, 0x130 + 0x70, 36)
    write_u64(system, 0x130 + 0x78, pointer(0x2C0))

    # VertexBuffer.
    write_u16(system, 0x1D0 + 0x08, 36)
    write_u64(system, 0x1D0 + 0x10, pointer(0x2C0))
    write_u32(system, 0x1D0 + 0x18, 3)
    write_u64(system, 0x1D0 + 0x30, pointer(0x2B0))

    # IndexBuffer.
    write_u32(system, 0x250 + 0x08, 3)
    write_u64(system, 0x250 + 0x10, pointer(0x330))

    # VertexDeclaration: Position Float3, Normal Float3, Colour0, TexCoord0 Float2.
    write_u32(system, 0x2B0, 0x59)
    write_u16(system, 0x2B4, 36)
    system[0x2B7] = 4
    write_u64(system, 0x2B8, DECL_TYPES)

    vertices = [
        ((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (0.0, 0.0)),
        ((1.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0)),
        ((0.0, 1.0, 0.0), (0.0, 0.0, 1.0), (0.0, 1.0)),
    ]
    for index, (position, normal, uv) in enumerate(vertices):
        base = 0x2C0 + index * 36
        for component, value in enumerate(position):
            write_f32(system, base + component * 4, value)
        for component, value in enumerate(normal):
            write_f32(system, base + 12 + component * 4, value)
        system[base + 24 : base + 28] = bytes([255, 255, 255, 255])
        write_f32(system, base + 28, uv[0])
        write_f32(system, base + 32, uv[1])

    write_u16(system, 0x330, 0)
    write_u16(system, 0x332, 1)
    write_u16(system, 0x334, 2)
    system[0x340:0x34E] = b"test_drawable\0"

    if editable:
        add_editable_shader_layout(system)
    if embedded:
        add_embedded_diffuse_layout(system)
    if external:
        add_external_diffuse_layout(system)

    compressor = zlib.compressobj(level=6, wbits=-15)
    compressed = compressor.compress(bytes(system)) + compressor.flush()

    output = bytearray()
    output += b"RSC7"
    output += struct.pack("<I", 165)
    output += struct.pack("<I", SYSTEM_FLAGS)
    output += struct.pack("<I", 0)
    output += compressed
    return bytes(output)


def add_embedded_diffuse_layout(system: bytearray) -> None:
    """Add one diffuse shader bound to one embedded 4x4 BC1 texture."""

    # Drawable -> ShaderGroup, with both one shader and an embedded dictionary.
    write_u64(system, 0x10, pointer(0x380))
    write_u64(system, 0x380 + 0x08, pointer(0x500))
    write_u64(system, 0x380 + 0x10, pointer(0x3C0))
    write_u16(system, 0x380 + 0x18, 1)
    write_u16(system, 0x380 + 0x1A, 1)
    write_u64(system, 0x3C0, pointer(0x3D0))

    # One ShaderFX with one texture parameter using the proven DiffuseSampler hash.
    write_u64(system, 0x3D0, pointer(0x400))
    write_u32(system, 0x3D0 + 0x08, 0x1111_2222)
    system[0x3D0 + 0x10] = 1
    write_u32(system, 0x3D0 + 0x18, 0x3333_4444)
    system[0x3D0 + 0x27] = 1
    system[0x400] = 0
    write_u64(system, 0x408, pointer(0x440))
    write_u32(system, 0x410, 0xF1FE_2B71)
    write_u64(system, 0x440 + 0x28, pointer(0x620))

    # Geometry 0 uses shader 0.
    write_u64(system, 0x0F0 + 0x20, pointer(0x4C0))
    write_u16(system, 0x4C0, 0)

    # Embedded TextureDictionary with one named BC1 texture.
    write_u64(system, 0x500 + 0x20, pointer(0x550))
    write_u16(system, 0x500 + 0x28, 1)
    write_u16(system, 0x500 + 0x2A, 1)
    write_u64(system, 0x500 + 0x30, pointer(0x560))
    write_u16(system, 0x500 + 0x38, 1)
    write_u16(system, 0x500 + 0x3A, 1)
    write_u32(system, 0x550, 0x1234_5678)
    write_u64(system, 0x560, pointer(0x580))

    write_u64(system, 0x580 + 0x28, pointer(0x620))
    write_u16(system, 0x580 + 0x50, 4)
    write_u16(system, 0x580 + 0x52, 4)
    write_u16(system, 0x580 + 0x54, 1)
    write_u16(system, 0x580 + 0x56, 2)
    write_u32(system, 0x580 + 0x58, 0x3154_5844)  # DXT1 / BC1
    system[0x580 + 0x5D] = 1
    write_u64(system, 0x580 + 0x70, pointer(0x640))
    system[0x620:0x62E] = b"embedded_diff\0"
    system[0x640:0x648] = bytes([0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0])


def add_external_diffuse_layout(system: bytearray) -> None:
    """Add one proven diffuse shader whose texture must come from an external YTD."""

    write_u64(system, 0x10, pointer(0x380))
    write_u64(system, 0x380 + 0x10, pointer(0x3C0))
    write_u16(system, 0x380 + 0x18, 1)
    write_u16(system, 0x380 + 0x1A, 1)
    write_u64(system, 0x3C0, pointer(0x3D0))

    write_u64(system, 0x3D0, pointer(0x400))
    write_u32(system, 0x3D0 + 0x08, 0x1111_2222)
    system[0x3D0 + 0x10] = 1
    write_u32(system, 0x3D0 + 0x18, 0x3333_4444)
    system[0x3D0 + 0x27] = 1
    system[0x400] = 0
    write_u64(system, 0x408, pointer(0x440))
    write_u32(system, 0x410, 0xF1FE_2B71)
    write_u64(system, 0x440 + 0x28, pointer(0x500))
    system[0x500:0x50D] = b"test_diffuse\0"

    write_u64(system, 0x0F0 + 0x20, pointer(0x4C0))
    write_u16(system, 0x4C0, 0)


def add_editable_shader_layout(system: bytearray) -> None:
    """Add two existing shaders and two compatible texture bindings."""

    # Drawable -> ShaderGroup -> two ShaderFX objects.
    write_u64(system, 0x10, pointer(0x380))
    write_u64(system, 0x380 + 0x10, pointer(0x3C0))
    write_u16(system, 0x380 + 0x18, 2)
    write_u16(system, 0x380 + 0x1A, 2)
    write_u64(system, 0x3C0, pointer(0x3D0))
    write_u64(system, 0x3C8, pointer(0x560))

    # Shader 0 owns two texture parameters with the same parameter hash.
    write_u64(system, 0x3D0, pointer(0x400))
    write_u32(system, 0x3D0 + 0x08, 0x1111_2222)
    system[0x3D0 + 0x10] = 2
    write_u32(system, 0x3D0 + 0x18, 0x3333_4444)
    system[0x3D0 + 0x27] = 2

    system[0x400] = 0
    write_u64(system, 0x408, pointer(0x440))
    system[0x410] = 0
    write_u64(system, 0x418, pointer(0x4A0))
    write_u32(system, 0x420, 0x5555_6666)
    write_u32(system, 0x424, 0x5555_6666)

    write_u64(system, 0x440 + 0x28, pointer(0x500))
    write_u64(system, 0x4A0 + 0x28, pointer(0x520))
    system[0x500:0x50D] = b"test_diffuse\0"
    system[0x520:0x52C] = b"test_normal\0"

    # Shader 1 is an existing target shader. No new shader data is created by edits.
    write_u32(system, 0x560 + 0x08, 0x9999_AAAA)
    write_u32(system, 0x560 + 0x18, 0xBBBB_CCCC)

    # Geometry 0 initially uses shader 0.
    write_u64(system, 0x0F0 + 0x20, pointer(0x540))
    write_u16(system, 0x540, 0)


def main() -> None:
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    variants = [
        (SIMPLE_OUTPUT, dict(editable=False, embedded=False)),
        (EDITABLE_OUTPUT, dict(editable=True, embedded=False)),
        (EMBEDDED_OUTPUT, dict(editable=False, embedded=True)),
        (EXTERNAL_OUTPUT, dict(editable=False, embedded=False, external=True)),
    ]
    for output, options in variants:
        output.write_bytes(generate(**options))
        print(f"wrote {output} ({output.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
