"""Shared native test-binary selection.

The daily runner explicitly points every black-box test at the binary it just
built.  Individual tests default to the debug binary so an unrelated, stale
release artifact can never mask source changes.
"""
from __future__ import annotations

import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def rust_binary(*, release: bool = False) -> Path:
    configured = os.environ.get("ENGRAMARK_TEST_BINARY")
    if configured:
        path = Path(configured).expanduser().resolve()
        if not path.is_file():
            raise RuntimeError(f"ENGRAMARK_TEST_BINARY 不存在：{path}")
        return path
    name = "engramark.exe" if os.name == "nt" else "engramark"
    profile = "release" if release else "debug"
    path = ROOT / "rust" / "target" / profile / name
    if not path.is_file():
        raise RuntimeError(f"缺少 {profile} 测试二进制：{path}")
    return path
