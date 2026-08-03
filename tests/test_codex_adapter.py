#!/usr/bin/env python3
"""Codex 适配器测试：通过 Rust 二进制的 hook 入口验证只检索已有正式记忆。"""
from __future__ import annotations

import atexit
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from support import rust_binary

ROOT = Path(__file__).resolve().parents[1]
BINARY = rust_binary()

passed = failed = 0


def check(name: str, cond: bool, detail: str = ""):
    global passed, failed
    if cond:
        passed += 1
    else:
        failed += 1
        print(f"  FAIL {name}  {detail}")


def run_hook(home: Path, event_name: str, event: dict) -> tuple[int, str]:
    env = dict(os.environ, ENGRAMARK_HOME=str(home))
    r = subprocess.run([str(BINARY), "hook", event_name], input=json.dumps(event),
                       capture_output=True, text=True, env=env, timeout=15)
    return r.returncode, r.stdout.strip()


def cli(home: Path, *args: str, stdin: str = "") -> dict:
    env = dict(os.environ, ENGRAMARK_HOME=str(home))
    r = subprocess.run([str(BINARY), *args], input=stdin, capture_output=True,
                       text=True, env=env, timeout=30)
    try:
        return json.loads(r.stdout.strip() or r.stderr.strip())
    except json.JSONDecodeError:
        return {}


CONTEXT_PREFIX = "Engramark 长期记忆命中（需要详情可调用 MCP memory_get）：\n"
MAX_LINES = 3
MAX_LINE_CODEPOINTS = 360
MAX_LINE_BYTES = 900
MAX_CONTEXT_BYTES = 1200


def _has_forbidden_character(text: str) -> bool:
    return any(ord(char) < 32 or 0x7F <= ord(char) <= 0x9F
               or char in (" ", " ") for char in text)


def valid_context(value: object) -> bool:
    """宿主侧复核合同：与 Rust 二进制内部校验同一规则。"""
    if (not isinstance(value, str) or not value.startswith(CONTEXT_PREFIX)
            or len(value.encode("utf-8")) > MAX_CONTEXT_BYTES):
        return False
    lines = value[len(CONTEXT_PREFIX):].split("\n")
    if not 1 <= len(lines) <= MAX_LINES:
        return False
    memory_ids: set[int] = set()
    for line in lines:
        match = re.match(r"^记忆提示：记忆 ([1-9]\d*)：", line)
        if match is None:
            return False
        memory_id = int(match.group(1))
        if (memory_id > 2**63 - 1 or memory_id in memory_ids
                or match.end() == len(line) or len(line) > MAX_LINE_CODEPOINTS
                or len(line.encode("utf-8")) > MAX_LINE_BYTES
                or _has_forbidden_character(line)
                or "[long-term-memory-index:" in line
                or "[/long-term-memory-index]" in line):
            return False
        memory_ids.add(memory_id)
    return True


CARD = """@0 fact published I3 T3 2026-08-01
= OrchidUI, core
~ user
# lock
OrchidUI（口头称 core）= ~/Library/.../user_default/OrchidUI/，示例扩展。
构建脚本位于 scripts/build.py，产物写入受控输出目录。
"""


def main() -> int:
    home = Path(tempfile.mkdtemp(prefix="engramark-codex-test-"))
    atexit.register(shutil.rmtree, home, ignore_errors=True)
    workspace = Path(tempfile.mkdtemp(prefix="engramark-codex-workspace-")) / "OrchidUI"
    (workspace / ".git").mkdir(parents=True)
    atexit.register(shutil.rmtree, workspace.parent, ignore_errors=True)
    cli(home, "save", CARD, "--lock")
    cwd = str(workspace)
    base = {"session_id": "thr_1", "transcript_path": None,
            "cwd": cwd, "hook_event_name": "", "model": "gpt"}

    print("[1] UserPromptSubmit 雷达注入")
    rc, out = run_hook(home, "codex-user-prompt-submit",
                       {**base, "hook_event_name": "UserPromptSubmit",
                        "prompt": "帮我改一下 core 的构建脚本"})
    check("exit 0", rc == 0)
    payload = json.loads(out) if out else {}
    ctx = payload.get("hookSpecificOutput", {}).get("additionalContext", "")
    check("注入自然语言记忆提示和微摘要", "记忆提示：记忆 1" in ctx and "OrchidUI" in ctx
          and "受控输出目录" in ctx and "@1" not in ctx and " I2 " not in ctx, ctx[:250])
    check("完整注入块不超过 1,200 UTF-8 字节",
          len(ctx.encode("utf-8")) <= 1200, str(len(ctx.encode("utf-8"))))
    check("宿主复核核心输出合同", valid_context(ctx), ctx[:250])
    check("复核规则拒绝超限和控制字符",
          not valid_context(CONTEXT_PREFIX + "记忆提示：记忆 1：" + "😀" * 220)
          and not valid_context(CONTEXT_PREFIX + "记忆提示：记忆 1：坏\x07标题"))

    rc, out = run_hook(home, "codex-user-prompt-submit",
                       {**base, "hook_event_name": "UserPromptSubmit",
                        "prompt": "随便聊点天气"})
    check("无命中则无输出", rc == 0 and out == "", out[:100])

    rc, out = run_hook(home, "codex-user-prompt-submit",
                       {**base, "hook_event_name": "UserPromptSubmit",
                        "prompt": "再看一次 core"})
    check("同会话冷却", rc == 0 and out == "", out[:100])

    check("不记录会话流水", not (home / "raw").exists(), str(home))

    print("[2] SessionStart resume/compact")
    rc, out = run_hook(home, "codex-session-start",
                       {**base, "hook_event_name": "SessionStart", "source": "resume"})
    payload = json.loads(out) if out else {}
    ctx = payload.get("hookSpecificOutput", {}).get("additionalContext", "")
    check("resume 注入自然语言记忆摘要", "记忆 1" in ctx and "OrchidUI" in ctx
          and "@1" not in ctx and not re.search(r"\b[ITF]\d", ctx)
          and "候选" not in ctx and "摘要：" in ctx and "正文预览：" not in ctx,
          ctx[:300])
    rc, out = run_hook(home, "codex-session-start",
                       {**base, "hook_event_name": "SessionStart", "source": "startup"})
    check("startup 不注入", rc == 0 and out == "", out[:100])

    rc, out = run_hook(home, "codex-user-prompt-submit",
                       {**base, "session_id": "thr_noise", "cwd": "/workspace/other",
                        "hook_event_name": "UserPromptSubmit", "prompt": "this is a core dump"})
    check("弱锚点跨项目不误注入", rc == 0 and out == "", out[:100])

    print("[3] 坏输入不炸宿主")
    rc, _ = run_hook(home, "codex-user-prompt-submit", {})
    check("空事件静默退出", rc == 0)

    print("[4] 旧会话兼容入口")
    results = [run_hook(home, name, base)
               for name in ("codex-post-tool-use", "codex-stop", "codex-session-end")]
    check("旧采集钩子只空操作", all(rc == 0 and out == "" for rc, out in results)
          and not (home / "raw").exists(), str(results))

    print(f"\n结果：{passed} 通过 / {failed} 失败")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
