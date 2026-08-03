#!/usr/bin/env python3
"""Run the day-to-day Engramark test suite (Rust binary + black-box groups)."""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TESTS = ROOT / "tests"
RUST = ROOT / "rust"


def main() -> int:
    python = sys.executable
    cargo = os.environ.get("CARGO", "cargo")
    build = subprocess.run([cargo, "build"], cwd=RUST)
    if build.returncode:
        print("FAIL cargo build")
        return 1
    unit = subprocess.run([cargo, "test", "--locked"], cwd=RUST)
    if unit.returncode:
        print("FAIL cargo test")
        return 1
    tasks = [
        ("文档结构与链接", [python, str(TESTS / "test_documentation.py")]),
        ("核心、MCP 与并发", [python, str(TESTS / "test_core.py")]),
        ("事务、缓存与恢复", [python, str(TESTS / "test_architecture.py")]),
        ("Codex 适配器", [python, str(TESTS / "test_codex_adapter.py")]),
        ("OpenCode 雷达内核", [python, str(TESTS / "test_opencode_radar.py")]),
        ("OpenCode 适配器", ["node", str(TESTS / "test_opencode_adapter.mjs")]),
        ("宿主接线", [python, str(TESTS / "test_host_setup.py")]),
    ]
    binary = RUST / "target" / "debug" / (
        "engramark.exe" if os.name == "nt" else "engramark")
    env = dict(
        os.environ,
        PYTHONDONTWRITEBYTECODE="1",
        ENGRAMARK_TEST_BINARY=str(binary.resolve()),
    )
    failed = 0
    for label, command in tasks:
        print(f"\n=== {label} ===", flush=True)
        if subprocess.run(command, cwd=ROOT, env=env).returncode:
            failed += 1
    print(f"\n测试组结果：{len(tasks) - failed} 通过 / {failed} 失败")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
