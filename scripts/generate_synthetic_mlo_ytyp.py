#!/usr/bin/env python3
"""Generate a copyright-free synthetic RSC7/META/YTYP MLO fixture."""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

SYSTEM_BASE = 0x50000000
SYSTEM_SIZE = 8192
SYSTEM_FLAGS = (1 << 27) | 4


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


def put_array(buf: bytearray, offset: int, block_id: int, block_offset: int, count: int) -> None:
    struct.pack_into("<QHHI", buf, offset, mptr(block_id, block_offset), count, count, 0)


def put_vec3(buf: bytearray, offset: int, xyz: tuple[float, float, float]) -> None:
    struct.pack_into("<fff", buf, offset, *xyz)


def write_structure_info(
    buf: bytearray,
    offset: int,
    *,
    name: str,
    key: int,
    size: int,
    entries_offset: int,
    entries: list[tuple[str, int, int]],
) -> int:
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
    return entries_offset + len(entries) * 16


def write_data_block(
    buf: bytearray,
    descriptor_offset: int,
    *,
    structure_name: str,
    data_offset: int,
    data: bytes,
) -> None:
    struct.pack_into(
        "<IiQ",
        buf,
        descriptor_offset,
        jenkins(structure_name),
        len(data),
        rptr(data_offset),
    )
    buf[data_offset : data_offset + len(data)] = data


def main() -> None:
    system = bytearray(SYSTEM_SIZE)
    struct.pack_into("<II", system, 0x00, 0x405BC808, 1)

    struct_infos_offset = 0x100
    entries_offset = 0x200
    data_blocks_offset = 0x500
    name_offset = 0x600

    structures = [
        ("CMapTypes", 80, [
            ("archetypes", 24, 0x52),
            ("name", 40, 0x4A),
            ("dependencies", 48, 0x52),
        ]),
        ("CMloArchetypeDef", 240, [
            ("bbMin", 32, 0x33),
            ("bbMax", 48, 0x33),
            ("name", 88, 0x4A),
            ("textureDictionary", 92, 0x4A),
            ("clipDictionary", 96, 0x4A),
            ("drawableDictionary", 100, 0x4A),
            ("physicsDictionary", 104, 0x4A),
            ("assetType", 108, 0x62),
            ("assetName", 112, 0x4A),
            ("entities", 152, 0x52),
            ("rooms", 168, 0x52),
            ("portals", 184, 0x52),
            ("entitySets", 200, 0x52),
        ]),
        ("CMloRoomDef", 112, [
            ("name", 8, 0x44),
            ("bbMin", 32, 0x33),
            ("bbMax", 48, 0x33),
            ("flags", 76, 0x63),
            ("portalCount", 80, 0x15),
            ("floorId", 84, 0x14),
            ("attachedObjects", 96, 0x52),
        ]),
        ("CMloPortalDef", 64, [
            ("roomFrom", 8, 0x15),
            ("roomTo", 12, 0x15),
            ("flags", 16, 0x63),
            ("mirrorPriority", 20, 0x15),
            ("opacity", 24, 0x15),
            ("audioOcclusion", 28, 0x15),
            ("corners", 32, 0x52),
            ("attachedObjects", 48, 0x52),
        ]),
        ("CMloEntitySet", 48, [
            ("name", 8, 0x4A),
            ("locations", 16, 0x52),
            ("entities", 32, 0x52),
        ]),
        ("CEntityDef", 128, [
            ("archetypeName", 8, 0x4A),
            ("flags", 12, 0x63),
            ("position", 32, 0x33),
            ("rotation", 48, 0x34),
            ("scaleXY", 64, 0x21),
            ("scaleZ", 68, 0x21),
            ("parentIndex", 72, 0x14),
        ]),
    ]

    next_entries = entries_offset
    for i, (name, size, entries) in enumerate(structures):
        next_entries = write_structure_info(
            system,
            struct_infos_offset + i * 32,
            name=name,
            key=0x1000 + i,
            size=size,
            entries_offset=next_entries,
            entries=entries,
        )

    blocks = [
        ("CMapTypes", 0x700, bytearray(80)),
        ("POINTER", 0x780, bytearray(8)),
        ("CMloArchetypeDef", 0x800, bytearray(240)),
        ("STRING", 0x900, bytearray(b"kitchen\0hall\0")),
        ("CMloRoomDef", 0x920, bytearray(224)),
        ("CMloPortalDef", 0xA10, bytearray(64)),
        ("VECTOR", 0xA60, bytearray(64)),
        ("UINT", 0xAA0, bytearray(8)),
        ("CMloEntitySet", 0xAB0, bytearray(48)),
        ("POINTER", 0xAE0, bytearray(8)),
        ("CEntityDef", 0xAF0, bytearray(128)),
        ("POINTER", 0xB80, bytearray(8)),
    ]

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
        len(structures),
        0,
        len(blocks),
        0,
        *([0] * 8),
    )
    system[name_offset : name_offset + 19] = b"synthetic_mlo_ytyp\0"

    cmap = blocks[0][2]
    put_array(cmap, 24, 2, 0, 1)
    struct.pack_into("<I", cmap, 40, joaat("synthetic_mlo"))

    struct.pack_into("<Q", blocks[1][2], 0, mptr(3))

    mlo = blocks[2][2]
    put_vec3(mlo, 32, (0.0, 0.0, 0.0))
    put_vec3(mlo, 48, (9.0, 3.0, 2.5))
    struct.pack_into("<I", mlo, 88, joaat("v_test_mlo"))
    struct.pack_into("<i", mlo, 108, 4)  # assetless
    put_array(mlo, 152, 12, 0, 1)
    put_array(mlo, 168, 5, 0, 2)
    put_array(mlo, 184, 6, 0, 1)
    put_array(mlo, 200, 9, 0, 1)

    rooms = blocks[4][2]
    put_array(rooms, 8, 4, 0, 8)
    put_vec3(rooms, 32, (0.0, 0.0, 0.0))
    put_vec3(rooms, 48, (4.0, 3.0, 2.5))
    struct.pack_into("<IIIi", rooms, 76, 1, 1, 0, 0)
    room1 = 112
    put_array(rooms, room1 + 8, 4, 8, 5)
    put_vec3(rooms, room1 + 32, (4.0, 0.0, 0.0))
    put_vec3(rooms, room1 + 48, (9.0, 3.0, 2.5))
    struct.pack_into("<IIIi", rooms, room1 + 76, 2, 1, 1, 0)
    put_array(rooms, room1 + 96, 8, 0, 1)

    portal = blocks[5][2]
    struct.pack_into("<IIIIII", portal, 8, 1, 2, 4, 3, 128, 7)
    put_array(portal, 32, 7, 0, 4)

    corners = blocks[6][2]
    for i, corner in enumerate([
        (4.0, 0.0, 0.0),
        (4.0, 3.0, 0.0),
        (4.0, 3.0, 2.5),
        (4.0, 0.0, 2.5),
    ]):
        put_vec3(corners, i * 16, corner)

    struct.pack_into("<II", blocks[7][2], 0, 0, 1)

    entity_set = blocks[8][2]
    struct.pack_into("<I", entity_set, 8, 0xABCD)
    put_array(entity_set, 16, 8, 4, 1)
    put_array(entity_set, 32, 10, 0, 1)
    struct.pack_into("<Q", blocks[9][2], 0, mptr(11))

    entity = blocks[10][2]
    struct.pack_into("<II", entity, 8, joaat("test_archetype"), 0)
    put_vec3(entity, 32, (1.0, 2.0, 3.0))
    struct.pack_into("<ffff", entity, 48, 0.0, 0.0, 0.0, 1.0)
    struct.pack_into("<ff", entity, 64, 1.0, 1.0)
    struct.pack_into("<i", entity, 72, -1)
    struct.pack_into("<Q", blocks[11][2], 0, mptr(11))

    for i, (structure_name, data_offset, data) in enumerate(blocks):
        write_data_block(
            system,
            data_blocks_offset + i * 16,
            structure_name=structure_name,
            data_offset=data_offset,
            data=bytes(data),
        )

    compressor = zlib.compressobj(level=9, wbits=-15)
    compressed = compressor.compress(bytes(system)) + compressor.flush()
    rsc7 = b"RSC7" + struct.pack("<III", 2, SYSTEM_FLAGS, 0) + compressed

    out = Path(__file__).resolve().parents[1] / "fixtures" / "synthetic" / "mlo.ytyp"
    out.write_bytes(rsc7)
    print(out)
    print(f"compressed={len(rsc7)} decompressed={len(system)}")


if __name__ == "__main__":
    main()
