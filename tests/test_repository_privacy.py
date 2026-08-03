#!/usr/bin/env python3
"""验证 Git 只跟踪可公开内容，个人记忆与凭据不会进入仓库。"""
from __future__ import annotations

import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

PRIVATE_PREFIXES = (
    "cache/", "candidates/", "logs/", "raw/", "state/", "locks/",
    "backups/", "exports/", "snapshots/",
)
PRIVATE_EXACT = {"install-manifest.txt", "runtime/python", "uninstall.sh"}
IGNORE_SAMPLES = (
    "cards/9999.mem",
    "state/migration-backups/example/cards/0001.mem",
    "cache/memory.mcache",
    "raw/project/session.jsonl",
    "logs/mcp.log",
    "runtime/python",
    "install-manifest.txt",
    "backups/private/cards/0001.mem",
    ".env",
    ".serena/project.yml",
)
PERSONAL_MARKERS = (
    "Nexus" + "UI", "Nexus" + "UIweb", "Remote" + "Deploy",
    "runtime-test." + "sunkanwei.cn", "/Users/" + "sunkanwei",
    "/Users/" + "someone", "OpenCode" + "X 项目",
)
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"\bghp_[A-Za-z0-9]{20,}\b"),
    re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b"),
    re.compile(r"\bsk-[A-Za-z0-9_-]{12,}\b"),
)
RETIRED_IMPLEMENTATION = (
    "bin/engramark.py",
    "bin/host_setup.py",
    "bin/mcp_server.py",
    "migration/baseline.json",
    "tests/golden_generate.py",
    "tests/test_differential.py",
    "tests/test_performance.py",
)


def git(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)


def main() -> int:
    if not (ROOT / ".git").exists():
        print("SKIP 仓库隐私边界：当前副本没有 .git")
        return 77

    failures: list[str] = []
    # 同时检查已跟踪文件和准备首次加入 Git 的未跟踪文件，避免测试本身
    # 只有在暂存之后才看得到泄漏。
    listed = git("ls-files", "-z", "--cached", "--others", "--exclude-standard")
    if listed.returncode:
        print(listed.stderr.strip())
        return 1
    tracked = sorted({
        path for path in listed.stdout.split("\0")
        if path and (ROOT / path).exists()
    })

    leaked_paths = [
        path for path in tracked
        if path in PRIVATE_EXACT
        or any(path.startswith(prefix) for prefix in PRIVATE_PREFIXES)
        or (path.startswith("cards/") and path != "cards/.gitkeep")
        or path.startswith("runtime/")
    ]
    if leaked_paths:
        failures.append("Git 跟踪了私有路径：" + ", ".join(leaked_paths))

    retired_paths = [path for path in RETIRED_IMPLEMENTATION if (ROOT / path).exists()]
    if retired_paths:
        failures.append("仓库重新出现已退役的迁移实现：" + ", ".join(retired_paths))

    for sample in IGNORE_SAMPLES:
        check = git("check-ignore", "--no-index", "--quiet", "--", sample)
        if check.returncode != 0:
            failures.append(f"忽略规则未覆盖：{sample}")

    keep = git("check-ignore", "--no-index", "--quiet", "--", "cards/.gitkeep")
    if keep.returncode == 0:
        failures.append("cards/.gitkeep 不应被忽略")

    for relative in tracked:
        path = ROOT / relative
        if not path.is_file() or path.stat().st_size > 3 * 1024 * 1024:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        for marker in PERSONAL_MARKERS:
            if marker in text:
                failures.append(f"{relative} 包含个人标记：{marker}")
        for pattern in SECRET_PATTERNS:
            if pattern.search(text):
                failures.append(f"{relative} 包含疑似凭据：{pattern.pattern}")

    if failures:
        for failure in failures:
            print(f"FAIL {failure}")
        return 1
    print(f"PASS 已检查 {len(tracked)} 个待公开路径；私有目录、个人标记和凭据规则均通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
