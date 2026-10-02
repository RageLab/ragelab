#!/usr/bin/env python3
"""Generate a tiny deterministic legacy GTA V PC YTD fixture.

The resource contains one synthetic 4x4 RGBA8 texture. It is intentionally
constructed from scratch and contains no Rockstar/third-party asset data.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "fixtures" / "synthetic" / "ytd" / "simple.ytd"

SYSTEM_BASE = 0x5000_0000
GRAPHICS_BASE = 0x6000_0000
PAGE_512_FLAGS = 1 << 27
RESOURCE_VERSION = 13

TEXTURE_NAME = "synthetic_diffuse"
TEXTURE_OFFSET = 0x80
NAME_OFFSET = 0x120
HASHES_OFFSET = 0x40
POINTERS_OFFSET = 0x48
PAGES_INFO_OFFSET = 0x140


def joaat(name: str) -> int:
    value = 0
    for byte in name.encode("ascii").lower():
        value = (value + byte) & 0xFFFF_FFFF
        value = (value + ((value << 10) & 0xFFFF_FFFF)) & 0xFFFF_FFFF
        value ^= value >> 6
    value = (value + ((value << 3) & 0xFFFF_FFFF)) & 0xFFFF_FFFF
    value ^= value >> 11
    value = (value + ((value << 15) & 0xFFFF_FFFF)) & 0xFFFF_FFFF
    return value & 0xFFFF_FFFF


def pack_list_header(buffer: bytearray, offset: int, pointer: int, count: int, capacity: int) -> None:
    struct.pack_into("<QHHI", buffer, offset, pointer, count, capacity, 0)


def build() -> bytes:
    system = bytearray(512)
    graphics = bytearray(512)

    name_hash = joaat(TEXTURE_NAME)

    # TextureDictionary / ResourceFileBase.
    struct.pack_into("<IIQ", system, 0x00, 0, 1, SYSTEM_BASE + PAGES_INFO_OFFSET)
    struct.pack_into("<IIII", system, 0x10, 0, 0, 1, 0)
    pack_list_header(system, 0x20, SYSTEM_BASE + HASHES_OFFSET, 1, 1)
    pack_list_header(system, 0x30, SYSTEM_BASE + POINTERS_OFFSET, 1, 1)
    struct.pack_into("<I", system, HASHES_OFFSET, name_hash)
    struct.pack_into("<Q", system, POINTERS_OFFSET, SYSTEM_BASE + TEXTURE_OFFSET)

    # ResourcePagesInfo: 16-byte header followed by eight reserved bytes per
    # system/graphics page. The fixture has one 512-byte page in each segment.
    struct.pack_into("<IIBBHI", system, PAGES_INFO_OFFSET, 0, 0, 1, 1, 0, 0)

    # Legacy 0x90-byte Texture. Unknown fields remain zero unless the reference
    # layout has a conventional default useful for the fixture.
    struct.pack_into("<II", system, TEXTURE_OFFSET + 0x00, 0, 1)
    struct.pack_into("<Q", system, TEXTURE_OFFSET + 0x28, SYSTEM_BASE + NAME_OFFSET)
    struct.pack_into("<H", system, TEXTURE_OFFSET + 0x30, 1)
    usage_data = 20  # TextureUsage.DIFFUSE, no usage flags.
    struct.pack_into("<I", system, TEXTURE_OFFSET + 0x40, usage_data)
    struct.pack_into("<I", system, TEXTURE_OFFSET + 0x48, 0)
    struct.pack_into("<HHHH", system, TEXTURE_OFFSET + 0x50, 4, 4, 1, 16)
    struct.pack_into("<I", system, TEXTURE_OFFSET + 0x58, 32)  # D3DFMT_A8B8G8R8 / RGBA8.
    struct.pack_into("<B", system, TEXTURE_OFFSET + 0x5D, 1)  # one mip level
    struct.pack_into("<Q", system, TEXTURE_OFFSET + 0x70, GRAPHICS_BASE)

    encoded_name = TEXTURE_NAME.encode("ascii") + b"\0"
    system[NAME_OFFSET : NAME_OFFSET + len(encoded_name)] = encoded_name

    # 4x4 RGBA8 top mip: deterministic gradient-like pixels.
    pixels = bytes(
        component
        for y in range(4)
        for x in range(4)
        for component in (x * 64, y * 64, 255 - x * 32, 255)
    )
    assert len(pixels) == 64
    graphics[: len(pixels)] = pixels

    payload = bytes(system + graphics)
    compressor = zlib.compressobj(level=9, wbits=-15)
    compressed = compressor.compress(payload) + compressor.flush()
    header = b"RSC7" + struct.pack(
        "<III", RESOURCE_VERSION, PAGE_512_FLAGS, PAGE_512_FLAGS
    )
    return header + compressed


def main() -> None:
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    data = build()
    OUTPUT.write_bytes(data)
    print(f"synthetic YTD: {OUTPUT}")
    print(f"size={len(data)} bytes texture={TEXTURE_NAME} hash=0x{joaat(TEXTURE_NAME):08X}")


if __name__ == "__main__":
    main()
