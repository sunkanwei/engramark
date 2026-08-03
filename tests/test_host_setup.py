#!/usr/bin/env python3
"""Smoke-test repeatable host wiring and precise unwiring."""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

from support import rust_binary

ROOT = Path(__file__).resolve().parents[1]
SETUP = rust_binary()


def run(home: Path, action: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ, HOME=str(home), PYTHONDONTWRITEBYTECODE="1")
    env.pop("CODEX_HOME", None)
    return subprocess.run(
        [str(SETUP), "host-setup", action, "--home", str(home),
         "--app-root", str(home / ".local" / "share" / "engramark"),
         "--data-home", str(home / "engramark"), "--codex", "yes",
         "--opencode", "yes"],
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
    )


def run_project(home: Path, action: str, project: Path) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ, HOME=str(home), PYTHONDONTWRITEBYTECODE="1")
    env.pop("CODEX_HOME", None)
    return subprocess.run(
        [str(SETUP), "host-setup", action, "--home", str(home),
         "--app-root", str(home / ".local" / "share" / "engramark"),
         "--data-home", str(home / "engramark"), "--project", str(project)],
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
    )


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="engramark-host-setup-") as temporary:
        home = Path(temporary)
        codex = home / ".codex"
        opencode = home / ".config" / "opencode"
        codex.mkdir(parents=True)
        opencode.mkdir(parents=True)
        (codex / "config.toml").write_text(
            'model = "keep"\n\n[features]\n'
            '# engramark-begin（旧版托管块）\nmemories = false\n# engramark-end\n\n'
            '[mcp_servers.engramark]\ncommand = "/old/engramark/runtime/python"\n'
            'args = ["/old/engramark/bin/mcp_server.py"]\n',
            encoding="utf-8",
        )
        (codex / "config.toml.engramark-bak").write_text(
            '[features]\nmemories = true\n', encoding="utf-8",
        )
        (codex / "hooks.json").write_text(json.dumps({
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo keep"}]}]},
        }), encoding="utf-8")
        (codex / "AGENTS.md").write_text("# Rules\n\nKeep this.\n", encoding="utf-8")
        (opencode / "opencode.jsonc").write_text(
            '{\n  // keep\n  "mcp": {"existing": {"type": "remote", "url": "https://example.invalid"}},\n}\n',
            encoding="utf-8",
        )
        (opencode / "AGENTS.md").write_text("# Rules\n\nKeep this.\n", encoding="utf-8")
        old_plugin = opencode / "plugins" / "engramark.js"
        old_plugin.parent.mkdir()
        old_plugin.write_text("// engramark-managed-opencode-plugin-v2\n", encoding="utf-8")
        parent_modes = {
            path: path.stat().st_mode & 0o777
            for path in (codex, opencode, old_plugin.parent)
        } if os.name != "nt" else {}

        before = {
            path: path.read_bytes()
            for path in (codex / "config.toml", codex / "hooks.json", codex / "AGENTS.md",
                         opencode / "opencode.jsonc", opencode / "AGENTS.md")
        }
        checked = run(home, "check")
        if (checked.returncode or not old_plugin.exists()
                or any(path.read_bytes() != value for path, value in before.items())):
            print("FAIL 宿主预检修改了配置")
            return 1

        first = run(home, "install")
        if first.returncode:
            print(first.stdout, first.stderr)
            return 1
        managed = {path: path.read_bytes() for path in before}
        managed_plugin = old_plugin.read_bytes()
        if run(home, "install").returncode or any(
            path.read_bytes() != value for path, value in managed.items()
        ) or old_plugin.read_bytes() != managed_plugin:
            print("FAIL 重复接线改变了配置")
            return 1
        if any(path.stat().st_mode & 0o777 != mode for path, mode in parent_modes.items()):
            print("FAIL 原子配置写入改变了既有父目录权限")
            return 1

        app = home / ".local" / "share" / "engramark"
        codex_text = (codex / "config.toml").read_text(encoding="utf-8")
        open_text = (opencode / "opencode.jsonc").read_text(encoding="utf-8")
        binary = str(app / "bin" / ("engramark.exe" if os.name == "nt" else "engramark"))
        if binary not in codex_text or binary not in open_text:
            print("FAIL 宿主接线没有指向固定程序目录")
            return 1
        if (codex_text.count("[mcp_servers.engramark]") != 1
                or "旧版托管块" in codex_text):
            print("FAIL Codex 旧接线没有安全升级")
            return 1
        if not old_plugin.is_file():
            print("FAIL OpenCode 请求级雷达插件没有安装")
            return 1
        plugin_text = old_plugin.read_text(encoding="utf-8")
        if ("engramark-managed-opencode-plugin-v4" not in plugin_text
                or str(app) not in plugin_text or str(home / "engramark") not in plugin_text
                or "tool.execute" in plugin_text or "experimental.session.compacting" in plugin_text
                or "raw-append" in plugin_text or "ENGRAMARK_SOURCE_ROOT" in plugin_text
                or "process.env.ENGRAMARK_HOME ||" in plugin_text):
            print("FAIL OpenCode 插件不是无流水的受管请求雷达")
            return 1
        codex_agent = (codex / "AGENTS.md").read_text(encoding="utf-8")
        opencode_agent = (opencode / "AGENTS.md").read_text(encoding="utf-8")
        if not all("任何语言" in text and "remember this" in text
                   and "不得因为内容看起来重要而主动保存" in text
                   for text in (codex_agent, opencode_agent)):
            print("FAIL 宿主没有安装多语言显式保存规则")
            return 1
        if "existing" not in open_text or "echo keep" not in (codex / "hooks.json").read_text():
            print("FAIL 宿主原配置没有保留")
            return 1

        removed = run(home, "uninstall")
        if removed.returncode:
            print(removed.stdout, removed.stderr)
            return 1
        if "engramark" in (codex / "config.toml").read_text(encoding="utf-8").lower():
            print("FAIL Codex 接线没有移除")
            return 1
        if "memories = true" not in (codex / "config.toml").read_text(encoding="utf-8"):
            print("FAIL Codex 原生记忆设置没有还原")
            return 1
        if "existing" not in (opencode / "opencode.jsonc").read_text(encoding="utf-8"):
            print("FAIL OpenCode 原配置被破坏")
            return 1
        if old_plugin.exists():
            print("FAIL OpenCode 请求级雷达插件没有卸载")
            return 1
        old_plugin.parent.mkdir(exist_ok=True)
        user_plugin = ("// user plugin mentioning engramark-managed-opencode-plugin-v4\n"
                       "export default function userPlugin() {}\n")
        old_plugin.write_text(user_plugin, encoding="utf-8")
        preserved_plugin = run(home, "uninstall")
        if (preserved_plugin.returncode != 0
                or old_plugin.read_text(encoding="utf-8") != user_plugin):
            print("FAIL 卸载误删或阻塞于 OpenCode 同名用户插件")
            return 1
        refused_plugin = run(home, "install")
        if (refused_plugin.returncode == 0
                or old_plugin.read_text(encoding="utf-8") != user_plugin):
            print("FAIL OpenCode 同名用户插件被覆盖")
            return 1
        old_plugin.unlink()
        user_target = home / "user-opencode-plugin.js"
        user_target.write_text(user_plugin, encoding="utf-8")
        old_plugin.symlink_to(user_target)
        preserved_link = run(home, "uninstall")
        refused_link = run(home, "install")
        if (preserved_link.returncode != 0 or refused_link.returncode == 0
                or not old_plugin.is_symlink()
                or user_target.read_text(encoding="utf-8") != user_plugin):
            print("FAIL OpenCode 同名插件符号链接未被安全保留或拒绝")
            return 1
        old_plugin.unlink()

        project = home / "workspace"
        project.mkdir()
        project_config = project / ".codex" / "config.toml"
        checked = run_project(home, "project-check", project)
        if checked.returncode or project_config.exists():
            print("FAIL 项目预检修改了配置")
            return 1
        project_config.parent.mkdir()
        original_project_config = '# keep\nmodel = "keep"\n'
        project_config.write_text(original_project_config, encoding="utf-8")
        enabled = run_project(home, "project-enable", project)
        if enabled.returncode:
            print(enabled.stdout, enabled.stderr)
            return 1
        enabled_text = project_config.read_text(encoding="utf-8")
        if (str(project) not in enabled_text or "mcp_servers.engramark" not in enabled_text
                or 'model = "keep"' not in enabled_text):
            print("FAIL 项目配置没有安全加入 cwd 覆盖")
            return 1
        enabled_bytes = project_config.read_bytes()
        if run_project(home, "project-enable", project).returncode or project_config.read_bytes() != enabled_bytes:
            print("FAIL 项目启用重复执行发生漂移")
            return 1
        disabled = run_project(home, "project-disable", project)
        if disabled.returncode or project_config.read_text(encoding="utf-8") != original_project_config:
            print("FAIL 项目撤销没有精确保留原配置")
            return 1

        conflict = home / "conflict-workspace"
        conflict_config = conflict / ".codex" / "config.toml"
        conflict_config.parent.mkdir(parents=True)
        conflict_original = '[mcp_servers.engramark]\ncwd = "/keep"\n'
        conflict_config.write_text(conflict_original, encoding="utf-8")
        refused = run_project(home, "project-enable", conflict)
        if refused.returncode == 0 or conflict_config.read_text() != conflict_original:
            print("FAIL 项目启用覆盖了用户已有的同名 MCP 配置")
            return 1

        desktop = home / "Desktop"
        desktop.mkdir()
        if (run_project(home, "project-enable", desktop).returncode == 0
                or run_project(home, "project-enable", home / "engramark").returncode == 0):
            print("FAIL 项目启用接受了宽泛目录或记忆数据目录")
            return 1
        missing = run_project(home, "project-enable", home / "missing-workspace")
        if missing.returncode != 2 or "存在的具体项目目录" not in missing.stderr:
            print("FAIL 不存在的项目目录没有返回可理解错误")
            return 1

    print("PASS 两个宿主与项目 cwd 可重复接线，卸载后原配置仍保留")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
