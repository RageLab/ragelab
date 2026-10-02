#!/usr/bin/env python3
"""Generate a tiny copyright-free synthetic RSC7/META/YTYP fixture.

The fixture contains one `CBaseArchetypeDef` and only the fields currently
consumed by `ragelab-ytyp`. It exists to test parser behavior without shipping
Rockstar game assets.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

SYSTEM_BASE = 0x50000000
SYSTEM_SIZE = 8192
SYSTEM_FLAGS = (1 << 27) | 4  # one 8192-byte page


def jenkins(text: str) -> int:
    h = 0
    for b in text.encode("ascii"):
        h = (h + b) & 0xFFFFFFFF
        h = (h + ((h << 10) & 0xFFFFFFFF)) & 0xFFFFFFFF
        h ^= h >> 6
    h = (h + ((h << 3) & 0xFFFFFFFF)) & 0xFFFFFFFF
    h ^= h >> 11
    h = (h + ((h << 15) & 0xFFFFFFFF)) & 0xFFFFFFFF
    return h & 0xFFFFFFFF


def joaat(text: str) -> int:
    return jenkins(text.lower())


def rptr(offset: int) -> int:
    return SYSTEM_BASE + offset


def mptr(block_id: int, offset: int = 0) -> int:
    return (block_id & 0xFFF) | ((offset & 0xFFFFF) << 12)


def write_structure_info(
    buf: bytearray,
    offset: int,
    *,
    name: str,
    key: int,
    size: int,
    entries_offset: int,
    entries: list[tuple[str, int, int]],
) -> None:
    struct.pack_into(
        "<IIIIQihh",
        buf,
        offset,
        jenkins(name),
        key,
        0x400,
        0,
        rptr(entries_offset),
        size,
        0,
        len(entries),
    )
    for i, (field_name, data_offset, data_type) in enumerate(entries):
        eoff = entries_offset + i * 16
        struct.pack_into(
            "<IiBBhI",
            buf,
            eoff,
            jenkins(field_name),
            data_offset,
            data_type,
            0,
            0,
            0,
        )


def write_data_block(
    buf: bytearray,
    descriptor_offset: int,
    *,
    structure_hash: int,
    data_offset: int,
    data: bytes,
) -> None:
    struct.pack_into(
        "<IiQ", buf, descriptor_offset, structure_hash, len(data), rptr(data_offset)
    )
    buf[data_offset : data_offset + len(data)] = data


def main() -> None:
    system = bytearray(SYSTEM_SIZE)
    struct.pack_into("<II", system, 0x00, 0x405BC808, 1)

    struct_infos_offset = 0x100
    cmaptypes_entries_offset = 0x180
    archetype_entries_offset = 0x200
    data_blocks_offset = 0x380
    name_offset = 0x400

    cmaptypes_data_offset = 0x500
    pointer_data_offset = 0x580
    archetype_data_offset = 0x600

    # META header: root block #1, two structure descriptors, three data blocks.
    struct.pack_into(
        "<ihBBiiQQQQQhhhh8I",
        system,
        0x10,
        0x50524430,
        0x79,
        0,
        0,
        0,
        1,
        rptr(struct_infos_offset),
        0,
        rptr(data_blocks_offset),
        rptr(name_offset),
        0,
        2,
        0,
        3,
        0,
        *([0] * 8),
    )
    system[name_offset : name_offset + 15] = b"synthetic_ytyp\0"

    write_structure_info(
        system,
        struct_infos_offset,
        name="CMapTypes",
        key=2608875220,
        size=80,
        entries_offset=cmaptypes_entries_offset,
        entries=[
            ("archetypes", 24, 0x52),
            ("name", 40, 0x4A),
            ("dependencies", 48, 0x52),
        ],
    )
    write_structure_info(
        system,
        struct_infos_offset + 32,
        name="CBaseArchetypeDef",
        key=2411387556,
        size=144,
        entries_offset=archetype_entries_offset,
        entries=[
            ("name", 88, 0x4A),
            ("textureDictionary", 92, 0x4A),
            ("clipDictionary", 96, 0x4A),
            ("drawableDictionary", 100, 0x4A),
            ("physicsDictionary", 104, 0x4A),
            ("assetType", 108, 0x62),
            ("assetName", 112, 0x4A),
        ],
    )

    cmaptypes = bytearray(80)
    struct.pack_into("<QHHI", cmaptypes, 24, mptr(2), 1, 1, 0)
    struct.pack_into("<I", cmaptypes, 40, joaat("simple"))
    # dependencies remains an empty/null Array_uint at offset 48.

    pointer_block = struct.pack("<Q", mptr(3))

    archetype = bytearray(144)
    struct.pack_into("<I", archetype, 88, joaat("test_archetype"))
    struct.pack_into("<I", archetype, 92, joaat("test_textures"))
    struct.pack_into("<I", archetype, 96, joaat("test_clip"))
    struct.pack_into("<I", archetype, 100, joaat("test_drawable_dict"))
    struct.pack_into("<I", archetype, 104, joaat("test_collision"))
    struct.pack_into("<i", archetype, 108, 2)  # ASSET_TYPE_DRAWABLE
    struct.pack_into("<I", archetype, 112, joaat("test_drawable"))

    write_data_block(
        system,
        data_blocks_offset,
        structure_hash=jenkins("CMapTypes"),
        data_offset=cmaptypes_data_offset,
        data=bytes(cmaptypes),
    )
    write_data_block(
        system,
        data_blocks_offset + 16,
        structure_hash=jenkins("POINTER"),
        data_offset=pointer_data_offset,
        data=pointer_block,
    )
    write_data_block(
        system,
        data_blocks_offset + 32,
        structure_hash=jenkins("CBaseArchetypeDef"),
        data_offset=archetype_data_offset,
        data=bytes(archetype),
    )

    compressor = zlib.compressobj(level=9, wbits=-15)
    compressed = compressor.compress(bytes(system)) + compressor.flush()
    rsc7 = b"RSC7" + struct.pack("<III", 2, SYSTEM_FLAGS, 0) + compressed

    out = Path(__file__).resolve().parents[1] / "fixtures" / "synthetic" / "simple.ytyp"
    out.write_bytes(rsc7)
    print(out)
    print(f"compressed={len(rsc7)} decompressed={len(system)}")


if __name__ == "__main__":
    main()
