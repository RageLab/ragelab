#!/usr/bin/env python3
"""Generate a tiny copyright-free synthetic RSC7/META/YMAP fixture.

This does not attempt to model every GTA field. It creates only the fields
currently consumed by ragelab-ymap so parser regressions can be tested without
committing Rockstar assets.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

SYSTEM_BASE = 0x50000000
SYSTEM_SIZE = 8192
SYSTEM_FLAGS = (1 << 27) | 4  # one page, base 0x200 << 4 == 8192


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


def write_structure_info(buf: bytearray, offset: int, *, name: str, key: int, size: int, entries_offset: int, entries: list[tuple[str, int, int]]):
    struct.pack_into("<IIIIQi h h", buf, offset,
                     jenkins(name), key, 0x400, 0, rptr(entries_offset), size, 0, len(entries))
    for i, (field_name, data_offset, data_type) in enumerate(entries):
        eoff = entries_offset + i * 16
        struct.pack_into("<IiBBhI", buf, eoff,
                         jenkins(field_name), data_offset, data_type, 0, 0, 0)


def write_data_block(buf: bytearray, descriptor_offset: int, *, structure_hash: int, data_offset: int, data: bytes):
    struct.pack_into("<IiQ", buf, descriptor_offset, structure_hash, len(data), rptr(data_offset))
    buf[data_offset:data_offset + len(data)] = data


def main() -> None:
    system = bytearray(SYSTEM_SIZE)

    # Generic ResourceFileBase prefix values commonly emitted by META builders.
    struct.pack_into("<II", system, 0x00, 0x405BC808, 1)

    struct_infos_offset = 0x100
    cmap_entries_offset = 0x180
    centity_entries_offset = 0x200
    data_blocks_offset = 0x300
    name_offset = 0x380

    cmap_data_offset = 0x500
    pointer_data_offset = 0x700
    entity_data_offset = 0x720
    hash_data_offset = 0x7A0

    # META header (fields after the inherited 16-byte ResourceFileBase prefix).
    struct.pack_into("<i h B B i i Q Q Q Q Q h h h h 8I", system, 0x10,
                     0x50524430, 0x79, 0, 0, 0, 1,
                     rptr(struct_infos_offset), 0, rptr(data_blocks_offset), rptr(name_offset), 0,
                     2, 0, 4, 0,
                     *([0] * 8))
    system[name_offset:name_offset + 10] = b"synthetic\0"

    write_structure_info(
        system,
        struct_infos_offset,
        name="CMapData",
        key=3448101671,
        size=512,
        entries_offset=cmap_entries_offset,
        entries=[
            ("name", 8, 0x4A),
            ("parent", 12, 0x4A),
            ("entities", 96, 0x52),
            ("physicsDictionaries", 160, 0x52),
        ],
    )
    write_structure_info(
        system,
        struct_infos_offset + 32,
        name="CEntityDef",
        key=1825799514,
        size=128,
        entries_offset=centity_entries_offset,
        entries=[
            ("archetypeName", 8, 0x4A),
            ("flags", 12, 0x15),
            ("position", 32, 0x33),
            ("rotation", 48, 0x34),
            ("parentIndex", 72, 0x14),
        ],
    )

    cmap = bytearray(512)
    struct.pack_into("<II", cmap, 8, joaat("simple_map"), 0)
    struct.pack_into("<QHHI", cmap, 96, mptr(2), 1, 1, 0)
    struct.pack_into("<QHHI", cmap, 160, mptr(4), 1, 1, 0)

    pointer_block = struct.pack("<Q", mptr(3))

    entity = bytearray(128)
    struct.pack_into("<II", entity, 8, joaat("test_archetype"), 0x55AA55AA)
    struct.pack_into("<fff", entity, 32, 1.0, 2.0, 3.0)
    struct.pack_into("<ffff", entity, 48, 0.0, 0.0, 0.0, 1.0)
    struct.pack_into("<i", entity, 72, -1)

    physics = struct.pack("<I", joaat("test_collision"))

    write_data_block(system, data_blocks_offset + 0 * 16,
                     structure_hash=jenkins("CMapData"), data_offset=cmap_data_offset, data=bytes(cmap))
    write_data_block(system, data_blocks_offset + 1 * 16,
                     structure_hash=jenkins("POINTER"), data_offset=pointer_data_offset, data=pointer_block)
    write_data_block(system, data_blocks_offset + 2 * 16,
                     structure_hash=jenkins("CEntityDef"), data_offset=entity_data_offset, data=bytes(entity))
    write_data_block(system, data_blocks_offset + 3 * 16,
                     structure_hash=jenkins("HASH"), data_offset=hash_data_offset, data=physics)

    compressor = zlib.compressobj(level=9, wbits=-15)
    compressed = compressor.compress(bytes(system)) + compressor.flush()
    rsc7 = b"RSC7" + struct.pack("<III", 2, SYSTEM_FLAGS, 0) + compressed

    out = Path(__file__).resolve().parents[1] / "fixtures" / "synthetic" / "simple.ymap"
    out.write_bytes(rsc7)
    print(out)
    print(f"compressed={len(rsc7)} decompressed={len(system)}")


if __name__ == "__main__":
    main()
