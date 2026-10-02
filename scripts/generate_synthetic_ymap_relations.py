#!/usr/bin/env python3
"""Generate copyright-free YMAP parent/child relationship fixtures.

The fixtures reuse the deterministic synthetic META layout from `simple.ymap`
and patch only CMapData.name/parent before recompressing the RSC7 payload.
They exist to exercise resolver graph semantics rather than new binary layout.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

from generate_synthetic_ymap import joaat

REPO = Path(__file__).resolve().parents[1]
SOURCE = REPO / "fixtures" / "synthetic" / "simple.ymap"
OUTPUT = REPO / "fixtures" / "synthetic" / "ymap-relations"
CMAP_DATA_OFFSET = 0x500
NAME_OFFSET = CMAP_DATA_OFFSET + 8
PARENT_OFFSET = CMAP_DATA_OFFSET + 12


def write_variant(source: bytes, name: str, parent: str | None) -> Path:
    if source[:4] != b"RSC7":
        raise ValueError("synthetic source is not RSC7")

    payload = bytearray(zlib.decompress(source[16:], wbits=-15))
    struct.pack_into("<I", payload, NAME_OFFSET, joaat(name))
    struct.pack_into("<I", payload, PARENT_OFFSET, joaat(parent) if parent else 0)

    compressor = zlib.compressobj(level=9, wbits=-15)
    compressed = compressor.compress(bytes(payload)) + compressor.flush()
    output = OUTPUT / f"{name}.ymap"
    output.write_bytes(source[:16] + compressed)
    return output


def main() -> None:
    source = SOURCE.read_bytes()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    relationships = [
        ("relation_parent", None),
        ("relation_child_a", "relation_parent"),
        ("relation_child_b", "relation_parent"),
        ("relation_grandchild", "relation_child_a"),
    ]
    for name, parent in relationships:
        output = write_variant(source, name, parent)
        print(output.relative_to(REPO))


if __name__ == "__main__":
    main()
