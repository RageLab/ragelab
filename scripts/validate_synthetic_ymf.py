#!/usr/bin/env python3
"""Independent structural validator for the synthetic PSO YMF fixture."""
from __future__ import annotations

import struct
from pathlib import Path

EXPECTED_YMAP = 0x6C3AE62A
EXPECTED_YTYP = 0xAA2B639B
ROOT = 0x93A68A2F
IMAP_DEPS = 0xC11F3EE1
UINT_BLOCK = 0x06


def u32(data: bytes, offset: int) -> int:
    return struct.unpack_from(">I", data, offset)[0]


def i32(data: bytes, offset: int) -> int:
    return struct.unpack_from(">i", data, offset)[0]


def u16(data: bytes, offset: int) -> int:
    return struct.unpack_from(">H", data, offset)[0]


def sections(data: bytes) -> dict[bytes, bytes]:
    out: dict[bytes, bytes] = {}
    cursor = 0
    while cursor < len(data):
        ident = data[cursor:cursor+4]
        length = u32(data, cursor + 4)
        assert length >= 8 and cursor + length <= len(data), (ident, length)
        out[ident] = data[cursor:cursor+length]
        cursor += length
    assert cursor == len(data)
    return out


def array_header(owner: bytes, offset: int) -> tuple[int, int, int]:
    word = u32(owner, offset)
    block_id = word & 0xFFF
    relative = word >> 12
    count = u16(owner, offset + 8)
    assert count == u16(owner, offset + 10)
    return block_id, relative, count


def main() -> None:
    data = Path("fixtures/synthetic/_manifest.ymf").read_bytes()
    parsed = sections(data)
    assert set(parsed) == {b"PSIN", b"PMAP", b"PSCH"}
    psin, pmap, psch = parsed[b"PSIN"], parsed[b"PMAP"], parsed[b"PSCH"]
    assert psin[8:16] == b"\x00" * 8
    assert i32(pmap, 8) == 3
    assert u16(pmap, 12) == 3
    assert u16(pmap, 14) == 0x7070

    blocks: dict[int, tuple[int, int, int]] = {}
    for index in range(3):
        base = 16 + index * 16
        blocks[index + 1] = (u32(pmap, base), i32(pmap, base + 4), i32(pmap, base + 12))
    assert blocks[1][0] == UINT_BLOCK
    assert blocks[2][0] == IMAP_DEPS
    assert blocks[3][0] == ROOT

    def block_bytes(block_id: int) -> bytes:
        _, offset, length = blocks[block_id]
        return psin[offset:offset+length]

    root = block_bytes(3)
    block_id, rel, count = array_header(root, 48)
    assert (block_id, rel, count) == (2, 0, 1)
    imap = block_bytes(2)
    assert u32(imap, 0) == EXPECTED_YMAP
    assert i32(imap, 4) == 0
    hash_block_id, hash_rel, hash_count = array_header(imap, 8)
    assert (hash_block_id, hash_rel, hash_count) == (1, 0, 1)
    assert u32(block_bytes(1), 0) == EXPECTED_YTYP

    assert psch[:4] == b"PSCH"
    assert u32(psch, 8) == 3
    assert u32(psch, 12) == ROOT
    assert u32(psch, 20) == IMAP_DEPS
    assert u32(psch, 28) == 0x6452A05B
    print("synthetic YMF OK")
    print(f"size={len(data)} bytes ymap=0x{EXPECTED_YMAP:08X} ytyp=0x{EXPECTED_YTYP:08X}")


if __name__ == "__main__":
    main()
