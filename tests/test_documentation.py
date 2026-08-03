#!/usr/bin/env python3
"""验证待发布 Markdown 的标题与相对链接。"""
from __future__ import annotations

import re
import subprocess
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
LINK_RE = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
LANGUAGE_PAIRS = (
    ("README.md", "README.zh-CN.md"),
    ("docs/user-guide.md", "docs/使用指南.md"),
    ("docs/installation.md", "docs/安装指南.md"),
    ("docs/architecture.md", "docs/架构设计.md"),
    ("docs/release-guide.md", "docs/发布指南.md"),
    ("docs/testing.md", "docs/测试与验收.md"),
    ("THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.zh-CN.md"),
)
WINDOWS_INSTALL_DOCS = (
    "README.md",
    "README.zh-CN.md",
    "docs/installation.md",
    "docs/安装指南.md",
)


def has_local_link(text: str, target: str) -> bool:
    return f"]({target})" in text or f'href="{target}"' in text


def publishable_markdown() -> list[Path]:
    if not (ROOT / ".git").exists():
        return sorted(
            path for path in ROOT.rglob("*.md")
            if path.relative_to(ROOT).parts[0] != "runtime"
        )
    result = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    )
    return sorted(
        ROOT / relative
        for relative in set(result.stdout.split("\0"))
        if relative.endswith(".md") and (ROOT / relative).is_file()
    )


def main() -> int:
    failures: list[str] = []
    markdown = publishable_markdown()
    if not (ROOT / "README.md").is_file():
        failures.append("仓库根目录必须保留 README.md 作为默认项目入口")
    for english_relative, chinese_relative in LANGUAGE_PAIRS:
        english = ROOT / english_relative
        chinese = ROOT / chinese_relative
        if not english.is_file() or not chinese.is_file():
            failures.append(
                f"中英文文档必须成对存在：{english_relative} ↔ {chinese_relative}"
            )
            continue
        english_text = english.read_text(encoding="utf-8")
        chinese_text = chinese.read_text(encoding="utf-8")
        if not has_local_link(english_text, chinese.name):
            failures.append(f"{english_relative} 缺少简体中文切换链接")
        if not has_local_link(chinese_text, english.name):
            failures.append(f"{chinese_relative} 缺少 English 切换链接")
    for relative in WINDOWS_INSTALL_DOCS:
        text = (ROOT / relative).read_text(encoding="utf-8")
        if "-ExecutionPolicy Bypass -File $script" not in text:
            failures.append(f"{relative} 的 Windows 安装命令必须兼容客户端默认执行策略")
        if "Set-ExecutionPolicy" in text:
            failures.append(f"{relative} 不得要求用户永久修改 PowerShell 执行策略")
    for path in markdown:
        text = path.read_text(encoding="utf-8")
        headings: list[str] = []
        in_fence = False
        for line in text.splitlines():
            if line.startswith("```"):
                in_fence = not in_fence
            elif not in_fence and line.startswith("# "):
                headings.append(line)
        if len(headings) != 1 or not text.startswith("# "):
            failures.append(f"{path.relative_to(ROOT)} 必须以唯一一级标题开头")
        for raw_target in LINK_RE.findall(text):
            target = raw_target.strip().split("#", 1)[0]
            if not target or re.match(r"^[a-z][a-z0-9+.-]*:", target, re.I):
                continue
            resolved = (path.parent / unquote(target)).resolve()
            if not resolved.exists():
                failures.append(
                    f"{path.relative_to(ROOT)} 包含失效链接：{raw_target}"
                )

    if failures:
        for failure in failures:
            print(f"FAIL {failure}")
        return 1
    print(f"PASS 已检查 {len(markdown)} 个 Markdown 文件的标题与相对链接")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
