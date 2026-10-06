#!/usr/bin/env python3
"""Generate a tiny deterministic Legacy YFT with no main drawable.

This is a fail-closed browser/parser fixture only. It contains no Rockstar data.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "fixtures" / "synthetic" / "yft" / "no-main-drawable.yft"
YFT_LEGACY_VERSION = 162
SYSTEM_FLAGS = 0xA800_0000


def build() -> bytes:
    system = bytes(512)
    compressor = zlib.compressobj(level=9, wbits=-15)
    compressed = compressor.compress(system) + compressor.flush()
    return b"RSC7" + struct.pack("<III", YFT_LEGACY_VERSION, SYSTEM_FLAGS, 0) + compressed


def main() -> None:
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    payload = build()
    OUTPUT.write_bytes(payload)
    print(f"synthetic YFT: {OUTPUT}")
    print(f"size={len(payload)} bytes main_drawable=null")


if __name__ == "__main__":
    main()
