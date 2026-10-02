#!/usr/bin/env python3
"""Generate the deterministic synthetic YBN fixture used by RageLab writer tests."""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "fixtures" / "synthetic" / "ybn" / "simple.ybn"

SYSTEM_BASE = 0x50000000
SYSTEM_SIZE = 2048
SYSTEM_FLAGS = (1 << 26) | 1


def pointer(offset: int) -> int:
    return SYSTEM_BASE + offset


def write_u16(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<H", buffer, offset, value)


def write_i16(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<h", buffer, offset, value)


def write_u32(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", buffer, offset, value)


def write_u64(buffer: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<Q", buffer, offset, value)


def write_f32(buffer: bytearray, offset: int, value: float) -> None:
    struct.pack_into("<f", buffer, offset, value)


def write_vec3(buffer: bytearray, offset: int, value: tuple[float, float, float]) -> None:
    for axis, component in enumerate(value):
        write_f32(buffer, offset + axis * 4, component)


def write_bounds(buffer: bytearray, offset: int, bounds_type: int) -> None:
    buffer[offset + 0x10] = bounds_type
    write_f32(buffer, offset + 0x14, 2.0)
    write_vec3(buffer, offset + 0x20, (2.0, 3.0, 1.0))
    write_vec3(buffer, offset + 0x30, (-2.0, -3.0, -1.0))
    write_vec3(buffer, offset + 0x40, (0.0, 0.0, 0.0))
    write_vec3(buffer, offset + 0x50, (0.0, 0.0, 0.0))


def generate() -> bytes:
    system = bytearray(SYSTEM_SIZE)
    write_bounds(system, 0, 10)

    # Composite with one identity-transformed GeometryBVH child.
    write_u64(system, 0x70, pointer(0x400))
    write_u64(system, 0x78, pointer(0x410))
    write_u16(system, 0xA0, 1)
    write_u16(system, 0xA2, 1)
    write_u64(system, 0x400, pointer(0x200))
    write_vec3(system, 0x410, (1.0, 0.0, 0.0))
    write_vec3(system, 0x420, (0.0, 1.0, 0.0))
    write_vec3(system, 0x430, (0.0, 0.0, 1.0))
    write_vec3(system, 0x440, (0.0, 0.0, 0.0))

    write_bounds(system, 0x200, 8)
    write_u64(system, 0x200 + 0x88, pointer(0x500))
    write_vec3(system, 0x200 + 0x90, (1.0, 1.0, 1.0))
    write_vec3(system, 0x200 + 0xA0, (10.0, 20.0, 30.0))
    write_u64(system, 0x200 + 0xB0, pointer(0x480))
    write_u32(system, 0x200 + 0xD0, 3)
    write_u32(system, 0x200 + 0xD4, 1)
    write_u64(system, 0x200 + 0xF0, pointer(0x520))
    write_u64(system, 0x200 + 0x118, pointer(0x540))
    system[0x200 + 0x120] = 2

    # Three quantized vertices; vertex 1 is the sphere center.
    for offset, vertex in [
        (0x480, (0, 0, 0)),
        (0x486, (1, 0, 0)),
        (0x48C, (0, 1, 0)),
    ]:
        write_i16(system, offset, vertex[0])
        write_i16(system, offset + 2, vertex[1])
        write_i16(system, offset + 4, vertex[2])

    # Sphere polygon: kind 1, center vertex 1, radius 1.5.
    write_u16(system, 0x500, 1)
    write_u16(system, 0x502, 1)
    write_f32(system, 0x504, 1.5)

    # Two materials belonging to the same child.
    write_u32(system, 0x520, 69 | (7 << 8) | (3 << 24))
    write_u32(system, 0x524, 5 << 8)
    write_u32(system, 0x528, 70 | (8 << 8))
    write_u32(system, 0x52C, 6 << 8)
    system[0x540] = 0

    compressor = zlib.compressobj(level=6, wbits=-15)
    compressed = compressor.compress(bytes(system)) + compressor.flush()

    output = bytearray()
    output += b"RSC7"
    output += struct.pack("<I", 43)
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
