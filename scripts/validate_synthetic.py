#!/usr/bin/env python3
"""Validate the committed synthetic fixtures without requiring the Rust toolchain.

This is intentionally an independent smoke check, not a replacement for
`cargo test`. It verifies the generated RSC7 envelope, selected META/schema
facts, semantic fixture values and the loose dependency-index filename contract.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "fixtures" / "synthetic"
STREAM = FIXTURES / "stream"


def jenkins(text: str) -> int:
    value = 0
    for byte in text.encode("ascii"):
        value = (value + byte) & 0xFFFFFFFF
        value = (value + ((value << 10) & 0xFFFFFFFF)) & 0xFFFFFFFF
        value ^= value >> 6
    value = (value + ((value << 3) & 0xFFFFFFFF)) & 0xFFFFFFFF
    value ^= value >> 11
    value = (value + ((value << 15) & 0xFFFFFFFF)) & 0xFFFFFFFF
    return value & 0xFFFFFFFF


def joaat(text: str) -> int:
    return jenkins(text.lower())


def rsc7_payload(path: Path) -> bytes:
    data = path.read_bytes()
    assert data[:4] == b"RSC7", f"{path}: missing RSC7 magic"
    version, system_flags, graphics_flags = struct.unpack_from("<III", data, 4)
    assert version == 2, f"{path}: unexpected resource version {version}"
    assert system_flags != 0, f"{path}: expected a system segment"
    assert graphics_flags == 0, f"{path}: synthetic fixture should have no graphics segment"
    payload = zlib.decompress(data[16:], -15)
    assert len(payload) == 8192, f"{path}: expected 8192 decompressed bytes"
    return payload


def validate_ymap() -> None:
    payload = rsc7_payload(FIXTURES / "simple.ymap")
    assert struct.unpack_from("<I", payload, 0x500 + 8)[0] == joaat("simple_map")
    assert struct.unpack_from("<I", payload, 0x720 + 8)[0] == joaat("test_archetype")
    assert struct.unpack_from("<I", payload, 0x7A0)[0] == joaat("test_collision")
    assert struct.pack("<I", jenkins("CMapData")) in payload
    assert struct.pack("<I", jenkins("CEntityDef")) in payload


def validate_ytyp() -> None:
    payload = rsc7_payload(FIXTURES / "simple.ytyp")
    assert struct.unpack_from("<I", payload, 0x500 + 40)[0] == joaat("simple")
    assert struct.unpack_from("<I", payload, 0x600 + 88)[0] == joaat("test_archetype")
    assert struct.unpack_from("<i", payload, 0x600 + 108)[0] == 2
    assert struct.unpack_from("<I", payload, 0x600 + 112)[0] == joaat("test_drawable")
    assert struct.pack("<I", jenkins("CMapTypes")) in payload
    assert struct.pack("<I", jenkins("CBaseArchetypeDef")) in payload


def validate_stream_contract() -> None:
    expected = {
        "simple.ymap": joaat("simple"),
        "simple.ytyp": joaat("simple"),
        "test_drawable.ydr": joaat("test_drawable"),
        "test_drawable_dict.ydd": joaat("test_drawable_dict"),
        "test_textures.ytd": joaat("test_textures"),
        "test_collision.ybn": joaat("test_collision"),
        "test_clip.ycd": joaat("test_clip"),
    }
    for filename, expected_hash in expected.items():
        path = STREAM / filename
        assert path.exists(), f"missing dependency fixture: {filename}"
        assert joaat(path.stem) == expected_hash

    assert (STREAM / "simple.ymap").read_bytes() == (FIXTURES / "simple.ymap").read_bytes()
    assert (STREAM / "simple.ytyp").read_bytes() == (FIXTURES / "simple.ytyp").read_bytes()


def main() -> None:
    validate_ymap()
    validate_ytyp()
    validate_stream_contract()
    print("synthetic fixture validation: ok")


if __name__ == "__main__":
    main()
