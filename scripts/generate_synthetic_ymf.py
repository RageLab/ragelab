#!/usr/bin/env python3
"""Generate a tiny PSO `_manifest.ymf` fixture without using the Rust code.

This intentionally duplicates only the externally documented PSO/YMF layout we
need for validation. It is not an implementation dependency of RageLab.
"""
from __future__ import annotations

import struct
from pathlib import Path

ROOT = 0x93A68A2F
IMAP_DEPS = 0xC11F3EE1
UINT_BLOCK = 0x06
MANIFEST_FLAGS_ENUM = 0x6452A05B

YMAP = 0x6C3AE62A
YTYP = 0xAA2B639B


def be_u32(value: int) -> bytes:
    return struct.pack(">I", value & 0xFFFFFFFF)


def be_i32(value: int) -> bytes:
    return struct.pack(">i", value)


def pointer(block_id: int, offset: int = 0) -> int:
    assert 0 < block_id <= 0xFFF
    assert 0 <= offset <= 0xFFFFF
    return (offset << 12) | block_id


def array_header(block_id: int, count: int, offset: int = 0) -> bytes:
    return struct.pack(">IIHHI", pointer(block_id, offset), 0, count, count, 0)


def struct_schema(type_hash: int, length: int, entries: list[tuple[int, int, int, int, int]]) -> tuple[int, bytes]:
    out = bytearray(b"\x00\x00")
    out += struct.pack(">H", len(entries))
    out += be_i32(length)
    out += b"\x00" * 4
    for name_hash, type_id, subtype, data_offset, ref in entries:
        out += be_u32(name_hash)
        out += bytes((type_id, subtype))
        out += struct.pack(">H", data_offset)
        out += be_u32(ref)
    return type_hash, bytes(out)


def enum_schema(type_hash: int, entries: list[tuple[int, int]]) -> tuple[int, bytes]:
    out = bytearray(b"\x01\x00")
    out += struct.pack(">H", len(entries))
    for name_hash, value in entries:
        out += be_u32(name_hash) + be_i32(value)
    return type_hash, bytes(out)


def psch() -> bytes:
    root_entries = [
        (0x100, 0x0C, 0, 0, 0xC25B3923), (0xB52CAE23, 0x0D, 0, 0, 0),
        (0x100, 0x0C, 0, 0, 0x59869C63), (0xF78AFB23, 0x0D, 0, 16, 2),
        (0x100, 0x0C, 0, 0, 0xD0AD6E62), (0x2BDA143F, 0x0D, 0, 32, 4),
        (0x100, 0x0C, 0, 0, IMAP_DEPS), (0xDD4C5CCC, 0x0D, 0, 48, 6),
        (0x100, 0x0C, 0, 0, 0x5A564E50), (0xD2611C99, 0x0D, 0, 64, 8),
        (0x100, 0x0C, 0, 0, 0x2C325290), (0x38767A8F, 0x0D, 0, 80, 10),
    ]
    imap_entries = [
        (0x31AF439F, 0x0B, 7, 0, 0),
        (0x100, 0x0E, 0, 0, MANIFEST_FLAGS_ENUM),
        (0x6452A05B, 0x0F, 0, 4, 0x00200001),
        (0x100, 0x0B, 7, 0, 0),
        (0x8FB42AE6, 0x0D, 0, 8, 3),
    ]
    items = [
        struct_schema(ROOT, 96, root_entries),
        struct_schema(IMAP_DEPS, 24, imap_entries),
        enum_schema(MANIFEST_FLAGS_ENUM, [(0x21569096, 0)]),
    ]
    header_size = 12 + len(items) * 8
    offsets = []
    cursor = header_size
    for type_hash, chunk in items:
        offsets.append((type_hash, cursor))
        cursor += len(chunk)
    out = bytearray(b"PSCH") + be_u32(cursor) + be_u32(len(items))
    for type_hash, offset in offsets:
        out += be_u32(type_hash) + be_i32(offset)
    for _, chunk in items:
        out += chunk
    return bytes(out)


def build() -> bytes:
    hash_block = be_u32(YTYP)
    imap_item = bytearray(24)
    imap_item[0:4] = be_u32(YMAP)
    imap_item[4:8] = be_i32(0)
    imap_item[8:24] = array_header(1, 1)

    root = bytearray(96)
    root[48:64] = array_header(2, 1)

    blocks = [(UINT_BLOCK, hash_block), (IMAP_DEPS, bytes(imap_item)), (ROOT, bytes(root))]
    offsets: list[int] = []
    cursor = 16
    for _, data in blocks:
        offsets.append(cursor)
        cursor += len(data)

    psin = bytearray(b"PSIN") + be_u32(cursor) + b"\x00" * 8
    for offset, (_, data) in zip(offsets, blocks):
        assert len(psin) == offset
        psin += data

    pmap_len = 16 + len(blocks) * 16
    pmap = bytearray(b"PMAP") + be_u32(pmap_len)
    pmap += be_i32(3) + struct.pack(">HH", len(blocks), 0x7070)
    for (type_hash, data), offset in zip(blocks, offsets):
        pmap += be_u32(type_hash) + be_i32(offset) + be_i32(0) + be_i32(len(data))

    return bytes(psin + pmap + psch())


def main() -> None:
    target = Path("fixtures/synthetic/_manifest.ymf")
    target.write_bytes(build())
    print(f"wrote {target} ({target.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
