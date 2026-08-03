#!/usr/bin/env python3
"""安装生命周期：本地包 → 安装 → 接线 → MCP → 钩子/雷达 → 检索 → 写入 →
备份 → 升级重装 → 卸载，全程在隔离 HOME 中验证且卸载后数据完整。"""
from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"

passed = failed = 0


def check(name: str, cond: bool, detail: str = "") -> None:
    global passed, failed
    if cond:
        passed += 1
    else:
        failed += 1
        print(f"  FAIL {name}  {detail}")


def run(command: list[str], home: Path, timeout: int = 60, **kwargs) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ, HOME=str(home), USERPROFILE=str(home),
               LOCALAPPDATA=str(home / "AppData" / "Local"))
    env.pop("CODEX_HOME", None)
    return subprocess.run(command, capture_output=True, text=True, env=env,
                          timeout=timeout, **kwargs)


def main() -> int:
    machine = platform.machine().lower()
    if sys.platform == "darwin":
        target = "macos-arm64" if machine in {"arm64", "aarch64"} else "macos-x86_64"
        suffix = "tar.gz"
    elif sys.platform.startswith("linux") and machine in {"x86_64", "amd64"}:
        target, suffix = "linux-x86_64", "tar.gz"
    elif os.name == "nt" and machine in {"x86_64", "amd64"}:
        target, suffix = "windows-x86_64", "zip"
    else:
        print(f"SKIP 不支持的安装生命周期平台：{sys.platform}/{machine}")
        return 77
    packages = sorted(DIST.glob(f"engramark-*-{target}.{suffix}"))
    if not packages:
        print(f"SKIP dist 中没有 {target} 安装包")
        return 77
    package = packages[-1]
    powershell = os.environ.get("ENGRAMARK_TEST_POWERSHELL", "pwsh")
    home = Path(tempfile.mkdtemp(prefix="engramark-install-test-"))
    try:
        (home / ".codex").mkdir()
        (home / ".config" / "opencode").mkdir(parents=True)
        (home / "AppData" / "Local").mkdir(parents=True)
        def installer_for(bundle: Path) -> list[str]:
            return (
                [powershell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
                 str(ROOT / "install.ps1"),
                 "-Package", str(bundle), "-Home", str(home)]
                if os.name == "nt" else
                ["sh", str(ROOT / "install.sh"), "--package", str(bundle),
                 "--home", str(home)]
            )

        malicious = home / ("unsafe.zip" if os.name == "nt" else "unsafe.tar.gz")
        outside = home.parent / f"{home.name}-archive-escape"
        if os.name == "nt":
            with zipfile.ZipFile(malicious, "w") as archive:
                archive.writestr("engramark/../../archive-escape", "unsafe")
        else:
            with tarfile.open(malicious, "w:gz") as archive:
                entry = tarfile.TarInfo("engramark/unsafe-link")
                entry.type = tarfile.SYMTYPE
                entry.linkname = str(outside)
                archive.addfile(entry)
        rejected = run(installer_for(malicious), home, timeout=60)
        check("安装器拒绝链接或目录穿越包", rejected.returncode != 0 and not outside.exists(),
              rejected.stderr[-300:])

        install_command = installer_for(package)
        install = run(install_command, home, timeout=300)
        check("安装成功", install.returncode == 0, install.stderr[-500:])
        app = (home / "AppData" / "Local" / "Engramark" if os.name == "nt"
               else home / ".local" / "share" / "engramark").resolve()
        binary = app / "bin" / ("engramark.exe" if os.name == "nt" else "engramark")
        data = (home / "engramark").resolve()
        check("二进制与数据目录就位", binary.is_file() and (data / "cards").is_dir(), "")
        check("安装包不含旧 Python 运行时",
              not (app / "runtime").exists() and not any(app.rglob("*.py")), "")
        check("安装提示重启宿主", "重启宿主" in install.stdout, install.stdout[-200:])

        env = dict(os.environ, HOME=str(home), ENGRAMARK_HOME=str(data))
        env.pop("CODEX_HOME", None)

        def cli(*args: str, stdin: str = "") -> tuple[int, dict]:
            r = subprocess.run([str(binary), *args], input=stdin, capture_output=True,
                               text=True, env=env, timeout=60)
            text = r.stdout.strip() or r.stderr.strip()
            try:
                return r.returncode, json.loads(text)
            except json.JSONDecodeError:
                return r.returncode, {"raw": text}

        codex_config = (home / ".codex" / "config.toml").read_text()
        open_config = (home / ".config" / "opencode" / "opencode.jsonc").read_text()
        encoded_binary = json.dumps(str(binary), ensure_ascii=False)
        check("宿主接线指向新二进制", encoded_binary in codex_config
              and encoded_binary in open_config and "mcp_server.py" not in codex_config, "")
        check("OpenCode 插件已安装",
              (home / ".config" / "opencode" / "plugins" / "engramark.js").is_file(), "")
        hooks_text = (home / ".codex" / "hooks.json").read_text()
        check("Codex 钩子指向二进制入口", "hook codex-user-prompt-submit" in hooks_text
              and "hook codex-session-start" in hooks_text
              and "user_prompt_submit.py" not in hooks_text, "")

        rc, saved = cli("save", "@0 fact published I3 T3 2026-08-01\n= LifecycleTest\n~ user\n"
                        "安装生命周期测试卡，提及 LifecycleTest。\n", "--lock")
        check("写入成功", rc == 0 and saved.get("id") == 1, str(saved))
        rc, found = cli("search", "LifecycleTest")
        check("检索命中", rc == 0 and any("LifecycleTest" in line for line in found.get("results", [])),
              str(found))

        proc = subprocess.Popen([str(binary), "mcp"], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, text=True, env=env)
        try:
            proc.stdin.write(json.dumps({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                           "clientInfo": {"name": "install", "version": "1"}}}) + "\n")
            proc.stdin.write(json.dumps({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "memory_search", "arguments": {"query": "LifecycleTest"}}}) + "\n")
            proc.stdin.flush()
            responses = [json.loads(proc.stdout.readline()), json.loads(proc.stdout.readline())]
        finally:
            proc.terminate()
            proc.wait(timeout=5)
        check("MCP 初始化与搜索", responses[0]["result"]["serverInfo"]["name"] == "engramark"
              and "LifecycleTest" in responses[1]["result"]["content"][0]["text"],
              str(responses)[:300])

        hook = subprocess.run([str(binary), "hook", "codex-user-prompt-submit"],
                              input=json.dumps({"session_id": "s", "cwd": str(home / ".codex"),
                                                "prompt": "LifecycleTest 在哪里"}),
                              capture_output=True, text=True, env=env, timeout=15)
        check("Codex 钩子注入", hook.returncode == 0
              and "LifecycleTest" in hook.stdout, hook.stdout[:200])

        backup = home / "snapshot"
        rc, report = cli("backup", str(backup))
        check("备份成功", rc == 0 and (backup / "manifest.json").is_file(), str(report))

        reinstall = run(install_command, home, timeout=300)
        check("升级重装成功且数据保留", reinstall.returncode == 0
              and (data / "cards" / "0001.mem").exists(), reinstall.stderr[-300:])
        rc, found = cli("search", "LifecycleTest")
        check("重装后检索仍命中", rc == 0 and any("LifecycleTest" in line for line in found.get("results", [])),
              str(found))

        uninstall_command = (
            [powershell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
             str(app / "bin" / "uninstall.ps1"),
             "-Home", str(home)]
            if os.name == "nt" else
            ["sh", str(app / "bin" / "uninstall.sh"), "--home", str(home)]
        )
        uninstall = run(uninstall_command, home)
        check("卸载成功", uninstall.returncode == 0, uninstall.stderr[-300:])
        check("程序目录已移除", not app.exists(), "")
        check("记忆数据完整保留", (data / "cards" / "0001.mem").exists()
              and "LifecycleTest" in (data / "cards" / "0001.mem").read_text(), "")
        check("宿主接线已拆除", "engramark" not in (home / ".codex" / "config.toml").read_text().lower()
              and not (home / ".config" / "opencode" / "plugins" / "engramark.js").exists(), "")
    finally:
        shutil.rmtree(home, ignore_errors=True)
    print(f"\n结果：{passed} 通过 / {failed} 失败")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
