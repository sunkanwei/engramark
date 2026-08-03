#!/usr/bin/env python3
"""Generate the Rust full-casefold table from Unicode 16.0.0 CaseFolding.txt.

Python str.casefold() applies the full (C + F) non-Turkic mappings. The output
file is deterministic and embedded in the engramark binary together with the
Unicode license.
"""
from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "CaseFolding.txt"
TARGET = HERE.parent.parent / "src" / "casefold_table.rs"
EXPECTED_HEADER = "# CaseFolding-16.0.0.txt"


def main() -> int:
    lines = SOURCE.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0] != EXPECTED_HEADER:
        raise SystemExit(f"CaseFolding.txt 不是 Unicode 16.0.0：{lines[0] if lines else '空文件'}")
    common: dict[int, str] = {}
    full: dict[int, str] = {}
    for line in lines:
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        code, status, mapping, *_ = [part.strip() for part in line.split(";")]
        cp = int(code, 16)
        if status == "C":
            common[cp] = mapping
        elif status == "F":
            full[cp] = mapping
    table: dict[int, str] = dict(common)
    table.update(full)
    out = [
        "// Generated from Unicode 16.0.0 CaseFolding.txt (C+F, non-Turkic).",
        "// See assets/unicode/generate_casefold.py and assets/unicode/unicode-license.txt.",
        "pub const UNICODE_DATA_VERSION: (u8, u8, u8) = (16, 0, 0);",
        "pub const CASEFOLD: &[(u32, [u32; 3])] = &[",
    ]
    for cp in sorted(table):
        mapping = [int(x, 16) for x in table[cp].split()]
        assert 1 <= len(mapping) <= 3, (cp, mapping)
        slot = mapping + [0] * (3 - len(mapping))
        out.append(f"    (0x{cp:X}, [0x{slot[0]:X}, 0x{slot[1]:X}, 0x{slot[2]:X}]),")
    out.append("];\n")
    TARGET.write_text("\n".join(out), encoding="utf-8")
    print(f"{TARGET}: {len(table)} mappings")
    return 0


if __name__ == "__main__":
    sys.exit(main())
