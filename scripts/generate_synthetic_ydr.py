#!/usr/bin/env python3
"""Generate the deterministic synthetic YDR fixture used by RageLab tests."""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "fixtures" / "synthetic" / "ydr" / "simple.ydr"

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


def generate() -> bytes:
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

    compressor = zlib.compressobj(level=6, wbits=-15)
    compressed = compressor.compress(bytes(system)) + compressor.flush()

    output = bytearray()
    output += b"RSC7"
    output += struct.pack("<I", 165)
    output += struct.pack("<I", SYSTEM_FLAGS)
    output += struct.pack("<I", 0)
    output += compressed
    return bytes(output)


def main() -> None:
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_bytes(generate())
    print(f"wrote {OUTPUT} ({OUTPUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
