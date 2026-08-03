#!/usr/bin/env python3
"""核心与 MCP 自测：在独立临时目录中验证，不触碰真实记忆。"""
from __future__ import annotations

import atexit
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

from support import rust_binary

ROOT = Path(__file__).resolve().parents[1]
CORE = rust_binary()
MCP = [str(CORE), "mcp"]

passed = failed = 0


def check(name: str, cond: bool, detail: str = ""):
    global passed, failed
    if cond:
        passed += 1
    else:
        failed += 1
        print(f"  FAIL {name}  {detail}")


def run_cli(home: Path, *args: str, stdin: str = "") -> tuple[int, dict | str]:
    env = dict(os.environ, ENGRAMARK_HOME=str(home))
    r = subprocess.run([str(CORE), *args], input=stdin, capture_output=True,
                       text=True, env=env, timeout=60)
    out = r.stdout.strip() or r.stderr.strip()
    try:
        return r.returncode, json.loads(out)
    except json.JSONDecodeError:
        return r.returncode, out


def cache_generation(home: Path) -> int:
    with sqlite3.connect(home / "cache" / "memory.mcache") as conn:
        return int(conn.execute(
            "SELECT value FROM cache_meta WHERE key='generation'"
        ).fetchone()[0])


class McpClient:
    def __init__(self, home: Path, *, cwd: Path | None = None,
                 roots: list[Path] | None = None):
        env = dict(os.environ, ENGRAMARK_HOME=str(home))
        self.proc = subprocess.Popen(MCP, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, text=True, env=env,
                                     cwd=cwd)
        self.next_id = 0
        self.roots = roots or []

    def call(self, method: str, params: dict | None = None) -> dict:
        self.next_id += 1
        msg = {"jsonrpc": "2.0", "id": self.next_id, "method": method}
        if params is not None:
            msg["params"] = params
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            response = json.loads(line)
            if response.get("method") == "roots/list":
                self.proc.stdin.write(json.dumps({
                    "jsonrpc": "2.0",
                    "id": response["id"],
                    "result": {"roots": [
                        {"uri": root.resolve().as_uri(), "name": root.name}
                        for root in self.roots
                    ]},
                }) + "\n")
                self.proc.stdin.flush()
                continue
            if response.get("id") == self.next_id:
                return response

    def notify(self, method: str):
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.proc.stdin.flush()

    def close(self):
        self.proc.terminate()
        self.proc.wait(timeout=5)


CARD = """@0 fact published I3 T3 2026-08-01
= OrchidUI, core
~ user
# lock
OrchidUI（口头称 core）= ~/Library/.../user_default/OrchidUI/，示例扩展。
构建脚本位于 scripts/build.py，产物写入受控输出目录。
"""

SKILL = """@0 skill published I2 T2 2026-07-20
= 部署, SafeDeploy
~ self:opencode
部署 OrchidUI 改动的标准流程。
1. 签名构建 → 2. 经 SafeDeploy 部署
! 禁止绕过 SafeDeploy 直接调用底层设备接口
"""

CEF_CARD = """@0 fact published I3 T3 2026-08-02
= OrchidUI Web 当前 CEF 运行组件下载地址
~ user
# lock
OrchidUI Web 当前使用示例下载域名 downloads.example.com。
Mac 与 Windows 的 CEF 组件地址由 current.json 清单提供。
"""

CANDIDATE = """@0 fact candidate I2 T2 2026-08-01
= OrchidUI, 构建偏好
~ self:opencode
OrchidUI 构建产物应保存在独立输出目录。
"""


def main() -> int:
    home = Path(tempfile.mkdtemp(prefix="engramark-test-"))
    atexit.register(shutil.rmtree, home, ignore_errors=True)
    print(f"测试 HOME: {home}")

    print("[1] 写入与检索")
    rc, out = run_cli(home, "save", CARD, "--lock")
    check("save fact", rc == 0 and out.get("id") == 1, str(out))
    rc, out = run_cli(home, "save", SKILL)
    check("save skill", rc == 0 and out.get("id") == 2, str(out))
    rc, out = run_cli(home, "search", "OrchidUI")
    check("search 命中", rc == 0 and any("@1" in line for line in out.get("results", [])), str(out))
    rc, out = run_cli(home, "save", CEF_CARD, "--lock")
    check("save CEF fact", rc == 0 and out.get("id") == 3, str(out))
    rc, out = run_cli(home, "search", "CEF 服务 下载地址", "--explain")
    check("自然问题多词召回 CEF", rc == 0 and out.get("results")
          and out["results"][0].startswith("@3"), str(out))
    rc, out = run_cli(home, "search", "CEF服务下载地址")
    check("中英无空格召回 CEF", rc == 0 and any("@3" in line for line in out.get("results", [])), str(out))
    rc, out = run_cli(home, "search", "天气 菜谱 完全无关")
    check("低证据硬拒答", rc == 0 and out.get("results") == [], str(out))
    rc, out = run_cli(home, "get", "1")
    check("get 全文", rc == 0 and "示例扩展" in out["cards"][0]["text"], str(out))
    rc, out = run_cli(home, "get", "1", "2", "3", "4", "5", "6")
    check("get 超 5 个 id 被拒", rc != 0, str(out))

    print("[2] 雷达扫描与冷却")
    rc, out = run_cli(home, "scan", "--session", "s1", "--project", "OrchidUI-a1b2c3",
                      stdin="帮我看看 core 的构建脚本")
    hits = out.get("hits", [])
    check("scan 命中 core", rc == 0 and any(h["id"] == 1 for h in hits), str(out))
    check("注入行短", all(len(line) < 300 for line in out.get("lines", [])), str(out))
    rc, out = run_cli(home, "scan", "--session", "s1", stdin="core core core")
    check("同会话冷却", rc == 0 and out.get("hits") == [], str(out))
    rc, out = run_cli(home, "scan", "--session", "s2", stdin="this is a test of core dump")
    ids = [h["id"] for h in out.get("hits", [])]
    check("弱锚点 core 跨项目不乱注入", 1 not in ids, str(out))
    rc, out = run_cli(home, "scan", "--session", "s-cef", stdin="CEF 服务下载地址")
    check("强锚点 CEF 自动命中", any(h["id"] == 3 for h in out.get("hits", [])), str(out))
    rc, out = run_cli(home, "scan", "--session", "s3", stdin="这个 testcase 不含锚点")
    check("无关文本不命中", out.get("hits") == [], str(out))

    radar_home = Path(tempfile.mkdtemp(prefix="engramark-radar-budget-test-"))
    atexit.register(shutil.rmtree, radar_home, ignore_errors=True)
    for label in ("A", "B"):
        shared_card = f"""@0 fact published I3 T3 2026-08-02
= SharedAnchor9
~ user
共享锚点卡 {label}。
{'😀' * 120}
"""
        rc, saved = run_cli(radar_home, "save", shared_card)
        check(f"建立共享锚点卡 {label}", rc == 0, str(saved))
    legacy_state = radar_home / "cache" / "radar-state" / "shared.json"
    legacy_state.parent.mkdir(parents=True, exist_ok=True)
    legacy_state.write_text(json.dumps({
        "SharedAnchor9": 9_999_999_999, "@1": 9_999_999_999, "@2": 9_999_999_999,
    }), encoding="utf-8")
    rc, first_shared = run_cli(
        radar_home, "scan", "--session", "shared", "--budget", "1",
        stdin="请查看 SharedAnchor9")
    first_ids = {hit["id"] for hit in first_shared.get("hits", [])}
    check("首轮只展示预算内卡片", rc == 0 and len(first_ids) == 1, str(first_shared))
    check("Codex 完整注入块受字节预算约束",
          len(first_shared.get("context", "").encode("utf-8")) <= 1200
          and all(len(line.encode("utf-8")) <= 900
                  for line in first_shared.get("lines", [])), str(first_shared))
    state = json.loads(legacy_state.read_text())
    check("冷却状态只记录实际展示卡片",
          state.get("version") == 2 and len(state.get("cooldown", {})) == 1
          and all(key.isdigit() for key in state.get("cooldown", {})), str(state))
    rc, second_shared = run_cli(
        radar_home, "scan", "--session", "shared", "--budget", "1",
        stdin="再次查看 SharedAnchor9")
    second_ids = {hit["id"] for hit in second_shared.get("hits", [])}
    check("共享锚点不连带冷却未展示卡片",
          rc == 0 and len(second_ids) == 1 and first_ids.isdisjoint(second_ids),
          str(second_shared))
    poisoned_state = radar_home / "cache" / "radar-state" / "poisoned.json"
    poisoned_state.write_text(
        '{"version":2,"cooldown":{"1":Infinity,"2":9999999999}}', encoding="utf-8")
    rc, recovered = run_cli(
        radar_home, "scan", "--session", "poisoned", "--budget", "1",
        stdin="请查看 SharedAnchor9")
    check("未来或非有限冷却时间失败开放", rc == 0 and len(recovered.get("hits", [])) == 1,
          str(recovered))
    oversized_state = radar_home / "cache" / "radar-state" / "oversized.json"
    oversized_state.write_text(" " * (1024 * 1024 + 1), encoding="utf-8")
    rc, recovered = run_cli(
        radar_home, "scan", "--session", "oversized", "--budget", "1",
        stdin="请查看 SharedAnchor9")
    check("超大冷却状态失败开放", rc == 0 and len(recovered.get("hits", [])) == 1,
          str(recovered))

    print("[3] 候选状态机")
    rc, out = run_cli(home, "propose", CANDIDATE, "--source", "self:opencode")
    cid = out.get("id")
    check("propose 写候选", rc == 0 and cid == 4, str(out))
    rc, out = run_cli(home, "search", "OrchidUI", "--scope", "published")
    check("候选不进 published 检索", all("@4" not in line for line in out.get("results", [])), str(out))
    rc, out = run_cli(home, "scan", "--session", "s9", stdin="core")
    check("候选不进雷达", all(h["id"] != 4 for h in out.get("hits", [])), str(out))
    rc, out = run_cli(home, "publish", str(cid))
    check("publish 提升", rc == 0 and out.get("ok"), str(out))
    rc, out = run_cli(home, "propose", CANDIDATE.replace("独立输出目录", "受控缓存目录"), "--source", "self:x")
    rc, out = run_cli(home, "reject", str(out.get("id")))
    check("reject 丢弃", rc == 0 and out.get("ok"), str(out))
    rc, out = run_cli(home, "save", CARD, "--lock")
    check("完全重复写入去重", rc == 0 and out.get("id") == 1 and out.get("deduplicated"), str(out))

    print("[4] 反馈与锁")
    rc, out = run_cli(home, "feedback", "1", "-")
    check("锁定卡拒绝负反馈", rc != 0 and "锁定" in str(out.get("error", "")), str(out))
    rc, out = run_cli(home, "feedback", "2", "-")
    check("未锁卡负反馈降 T", rc == 0 and out.get("trust") == 1, str(out))
    rc, out = run_cli(home, "feedback", "2", "+")
    check("同日冷却", rc != 0, str(out))
    rc, out = run_cli(home, "feedback", "4", "+")
    check("正反馈支持小数 T", rc == 0 and out.get("trust") == 2.5, str(out))
    rc, out = run_cli(home, "get", "0")
    check("公开编号拒绝零值", rc != 0 and "大于等于 1" in str(out), str(out))
    rc, out = run_cli(home, "get", "--", "-1")
    check("公开编号拒绝负值", rc != 0 and "大于等于 1" in str(out), str(out))
    rc, out = run_cli(home, "rebuild")
    rc, out = run_cli(home, "search", "构建产物 独立输出目录")
    check("小数 T 重建后仍可解析", rc == 0 and any("@4" in line for line in out.get("results", [])), str(out))

    print("[5] MCP 协议")
    removed_home = home / "removed-data-home"
    stale_env = dict(os.environ, ENGRAMARK_HOME=str(removed_home))
    stale = subprocess.run(MCP, input="", capture_output=True,
                           text=True, env=stale_env, timeout=5)
    check("空输入退出不复活已删除的数据目录",
          stale.returncode == 0 and not removed_home.exists(), stale.stderr)
    pure_project = subprocess.run(
        [str(CORE), "project-id", str(home), "--authoritative"],
        capture_output=True, text=True, env=stale_env, timeout=5,
    )
    check("纯项目解析不初始化已删除的数据目录",
          pure_project.returncode == 0 and not removed_home.exists(), pure_project.stderr)

    failing_client = McpClient(home)
    failing_client.call("initialize", {"protocolVersion": "2025-11-25",
                                       "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}})
    index_as_dir = home / "cache" / "memory.mcache"
    saved_index = index_as_dir.read_bytes()
    index_as_dir.unlink()
    index_as_dir.mkdir()
    try:
        r = failing_client.call("tools/call", {"name": "memory_search",
                                               "arguments": {"query": "测试"}})
    finally:
        index_as_dir.rmdir()
        index_as_dir.write_bytes(saved_index)
        failing_client.close()
    check("缓存故障返回可理解提示而非 -32603",
          "result" in r and r["result"].get("isError") is True
          and "原始记忆没有丢失" in r["result"]["content"][0]["text"], str(r))
    run_cli(home, "rebuild")

    uninitialized = McpClient(home)
    r = uninitialized.call("tools/list")
    check("初始化前拒绝 MCP 请求", r.get("error", {}).get("code") == -32002, str(r))
    uninitialized.close()
    client = McpClient(home)
    r = client.call("initialize", {"protocolVersion": "2025-06-18",
                                   "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}})
    initialized = r.get("result", {})
    check("initialize", initialized.get("serverInfo", {}).get("name") == "engramark"
          and initialized.get("protocolVersion") == "2025-06-18", str(r))
    check("MCP 版本使用唯一版本源", initialized.get("serverInfo", {}).get("version")
          == (ROOT / "VERSION").read_text().strip(), str(initialized))
    instructions = initialized.get("instructions", "")
    check("MCP 服务指引自足", "memory_save" in instructions
          and len(instructions) <= 512, str(initialized))
    check("只按用户意图保存", "记一下" in instructions and "不得因为信息看起来有价值" in instructions
          and "明确要求先存为候选" in instructions, instructions)
    check("多语言语义保存不依赖固定唤醒词", "不限定关键词或语言" in instructions
          and "remember this" in instructions and "make a note" in instructions, instructions)
    client.notify("notifications/initialized")
    r = client.call("tools/list")
    tools = r.get("result", {}).get("tools", [])
    names = [t["name"] for t in tools]
    check("tools/list 11 个", len(names) == 11, str(names))
    check("工具均有中文标题和闭世界注解", all(
        tool.get("title") and tool.get("annotations", {}).get("openWorldHint") is False
        for tool in tools), str(tools)[:300])
    check("所有 Schema 拒绝额外参数", all(
        tool["inputSchema"].get("additionalProperties") is False for tool in tools), "")
    by_name = {tool["name"]: tool for tool in tools}
    check("保存工具要求多语言明确意图", "任何语言" in by_name["memory_save"]["description"]
          and "不限定关键词" in by_name["memory_save"]["description"], "")
    check("候选工具禁止主动使用", "用户明确要求" in by_name["memory_propose"]["description"]
          and "不得由 AI 主动创建" in by_name["memory_propose"]["description"], "")
    check("搜索接口只保留 query", set(
        by_name["memory_search"]["inputSchema"]["properties"]) == {"query"}, "")
    check("结构化写入不暴露线格式", "text" not in
          by_name["memory_save"]["inputSchema"]["properties"]
          and "decision" in by_name["memory_save"]["inputSchema"]["properties"]["type"]["enum"], "")
    update_properties = by_name["memory_update"]["inputSchema"]["properties"]
    check("更新字段没有危险默认值", all(
        "default" not in update_properties[key] for key in ("body", "entities", "type")), "")
    check("读取工具如实标注副作用", by_name["memory_get"]["annotations"]["readOnlyHint"] is False,
          str(by_name["memory_get"]["annotations"]))
    search_card = home / "cards" / "0001.mem"
    search_mtime = search_card.stat().st_mtime_ns
    r = client.call("tools/call", {"name": "memory_search", "arguments": {"query": "OrchidUI"}})
    text = r["result"]["content"][0]["text"]
    check("MCP search 人类可读", "记忆 1" in text and "@1" not in text
          and "正文预览：" in text and not re.search(r"\b[ITF]\d", text), text[:180])
    check("搜索预览保持只读", search_card.stat().st_mtime_ns == search_mtime, "")
    r = client.call("tools/call", {"name": "memory_search", "arguments": {
        "query": "CEF 服务 下载地址", "explain": True}})
    check("MCP 拒绝额外搜索参数", r["result"].get("isError") is True, str(r)[:180])
    r = client.call("tools/call", {"name": "memory_get", "arguments": {"ids": [1]}})
    text = r["result"]["content"][0]["text"]
    check("MCP get 人类可读", "记忆 1" in text and "示例扩展" in text
          and "@1" not in text and not re.search(r"\b[ITF]\d", text), text)
    r = client.call("tools/call", {"name": "memory_get", "arguments": {"ids": [1, 2, 3, 4, 5, 6]}})
    check("MCP get 超限报错", r["result"].get("isError") is True, str(r)[:120])
    r = client.call("tools/call", {"name": "memory_feedback", "arguments": {
        "id": 1, "outcome": "incorrect"}})
    check("MCP 锁定卡反馈被拒", r["result"].get("isError") is True, str(r)[:120])

    secret = "MCP_SECRET_8dc3f1"
    r = client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": f"{secret} 的架构决策", "body": "正文只应进入记忆卡。",
        "entities": [secret, "架构决策"], "type": "decision", "scope": "global",
        "lock": True}})
    text = r["result"]["content"][0]["text"]
    saved_id = int(re.search(r"记忆 (\d+)", text).group(1))
    saved_path = home / "cards" / f"{saved_id:04d}.mem"
    saved_text = saved_path.read_text()
    check("MCP 结构化保存默认与类型", "decision published I3 T3" in saved_text
          and "# scope global" in saved_text and "# lock" in saved_text, saved_text)
    saved_path.write_text(saved_text.replace(
        "# scope global\n",
        "# scope global\n# last-used 2026-07-01\n# valid-from 2026-01-01\n"
        "# valid-to 2099-12-31\n# supersedes @2\n",
    ), encoding="utf-8")
    run_cli(home, "rebuild")
    r = client.call("tools/call", {"name": "memory_update", "arguments": {
        "id": saved_id, "body": "更新后的正文。"}})
    updated_text = saved_path.read_text()
    check("PATCH 更新保留服务端元数据", "更新后的正文。" in updated_text
          and "# scope global" in updated_text and "# lock" in updated_text
          and "~ user" in updated_text and "decision published I3 T3" in updated_text
          and "# last-used 2026-07-01" in updated_text
          and "# valid-from 2026-01-01" in updated_text
          and "# valid-to 2099-12-31" in updated_text
          and "# supersedes @2" in updated_text,
          updated_text)
    before_stat = saved_path.stat().st_mtime_ns
    r = client.call("tools/call", {"name": "memory_update", "arguments": {
        "id": saved_id, "body": "更新后的正文。"}})
    check("同值 PATCH 不写盘", "内容没有变化" in r["result"]["content"][0]["text"]
          and saved_path.stat().st_mtime_ns == before_stat, str(r))
    r = client.call("tools/call", {"name": "memory_update", "arguments": {"id": saved_id}})
    check("空 PATCH 被拒绝", r["result"].get("isError") is True, str(r))
    r = client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "非法实体不会写入", "entities": ["a,b"], "scope": "global"}})
    check("实体分隔符注入被拒绝", r["result"].get("isError") is True, str(r))
    r = client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "非法\n标题", "scope": "global"}})
    check("多行标题被拒绝", r["result"].get("isError") is True, str(r))
    index_path = home / "cache" / "memory.mcache"
    index_path.unlink()
    r = client.call("tools/call", {"name": "memory_update", "arguments": {
        "id": saved_id, "entities": ["非法,实体"]}})
    check("无效更新在缓存读取前失败", r["result"].get("isError") is True
          and not index_path.exists(), str(r))

    r = client.call("tools/call", {"name": "memory_propose", "arguments": {
        "title": "显式保存复用候选", "body": "用户随后明确确认",
        "entities": ["候选"], "type": "fact", "scope": "global"}})
    promoted_id = int(re.search(r"记忆 (\d+)", r["result"]["content"][0]["text"]).group(1))
    r = client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "显式保存复用候选", "body": "用户随后明确确认",
        "entities": ["候选", "用户确认"], "type": "decision", "scope": "global"}})
    promoted_path = home / "cards" / f"{promoted_id:04d}.mem"
    promoted_text = promoted_path.read_text()
    check("明确保存候选时复用编号并采用正式元数据",
          f"记忆 {promoted_id}" in r["result"]["content"][0]["text"]
          and "已复用现有内容并保存" in r["result"]["content"][0]["text"]
          and "decision published I3 T3" in promoted_text
          and "~ user" in promoted_text and "用户确认" in promoted_text,
          str((r, promoted_text)))
    r = client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "显式保存复用候选", "body": "用户随后明确确认",
        "entities": ["候选", "用户确认"], "type": "decision", "scope": "global"}})
    check("重复明确保存保持无操作",
          "无需重复写入" in r["result"]["content"][0]["text"], str(r))

    r = client.call("tools/call", {"name": "memory_propose", "arguments": {
        "title": "MCP 候选生命周期测试", "body": "候选正文", "scope": "global"}})
    candidate_id = int(re.search(r"记忆 (\d+)", r["result"]["content"][0]["text"]).group(1))
    candidate_path = home / "cards" / f"{candidate_id:04d}.mem"
    check("MCP 候选使用较弱默认值", "fact candidate I2 T2" in candidate_path.read_text(), "")
    r1 = client.call("tools/call", {"name": "memory_publish", "arguments": {"id": candidate_id}})
    publish_generation = cache_generation(home)
    publish_mtime = candidate_path.stat().st_mtime_ns
    r2 = client.call("tools/call", {"name": "memory_publish", "arguments": {"id": candidate_id}})
    check("发布候选幂等", "已发布候选" in r1["result"]["content"][0]["text"]
          and "已经是正式记忆" in r2["result"]["content"][0]["text"]
          and cache_generation(home) == publish_generation
          and candidate_path.stat().st_mtime_ns == publish_mtime, str((r1, r2)))
    r = client.call("tools/call", {"name": "memory_feedback", "arguments": {
        "id": candidate_id, "outcome": "correct"}})
    check("反馈使用语义枚举", r["result"].get("isError") is not True
          and "内容正确" in r["result"]["content"][0]["text"], str(r))
    r1 = client.call("tools/call", {"name": "memory_archive", "arguments": {"id": candidate_id}})
    archive_generation = cache_generation(home)
    archive_mtime = candidate_path.stat().st_mtime_ns
    r2 = client.call("tools/call", {"name": "memory_archive", "arguments": {"id": candidate_id}})
    check("归档幂等", "已归档" in r1["result"]["content"][0]["text"]
          and "已经归档" in r2["result"]["content"][0]["text"]
          and cache_generation(home) == archive_generation
          and candidate_path.stat().st_mtime_ns == archive_mtime, str((r1, r2)))
    r1 = client.call("tools/call", {"name": "memory_delete", "arguments": {
        "id": candidate_id, "confirm": True}})
    delete_generation = cache_generation(home)
    delete_mtime = candidate_path.stat().st_mtime_ns
    r2 = client.call("tools/call", {"name": "memory_delete", "arguments": {
        "id": candidate_id, "confirm": True}})
    check("删除幂等", "已删除" in r1["result"]["content"][0]["text"]
          and "已经删除" in r2["result"]["content"][0]["text"]
          and cache_generation(home) == delete_generation
          and candidate_path.stat().st_mtime_ns == delete_mtime, str((r1, r2)))
    r = client.call("tools/call", {"name": "memory_propose", "arguments": {
        "title": "拒绝幂等测试", "scope": "global"}})
    reject_id = int(re.search(r"记忆 (\d+)", r["result"]["content"][0]["text"]).group(1))
    reject_path = home / "cards" / f"{reject_id:04d}.mem"
    r1 = client.call("tools/call", {"name": "memory_reject", "arguments": {"id": reject_id}})
    reject_generation = cache_generation(home)
    reject_mtime = reject_path.stat().st_mtime_ns
    r2 = client.call("tools/call", {"name": "memory_reject", "arguments": {"id": reject_id}})
    check("拒绝候选幂等", "已丢弃候选" in r1["result"]["content"][0]["text"]
          and "已经被丢弃" in r2["result"]["content"][0]["text"]
          and cache_generation(home) == reject_generation
          and reject_path.stat().st_mtime_ns == reject_mtime, str((r1, r2)))
    r = client.call("tools/call", {"name": "memory_audit", "arguments": {}})
    audit_text = r["result"]["content"][0]["text"]
    check("MCP audit 人类可读", "检查完成" in audit_text and "candidates" not in audit_text
          and not audit_text.lstrip().startswith("{"), audit_text)
    client.close()

    workspace_a = Path(tempfile.mkdtemp(prefix="engramark-workspace-a-"))
    workspace_b = Path(tempfile.mkdtemp(prefix="engramark-workspace-b-"))
    unknown_workspace = Path(tempfile.mkdtemp(prefix="engramark-no-project-"))
    for workspace in (workspace_a, workspace_b):
        (workspace / ".git").mkdir()
    nested_workspace = workspace_a / "nested"
    nested_workspace.mkdir()
    workspace_link = workspace_a.parent / f"{workspace_a.name}-link"
    workspace_link.symlink_to(workspace_a, target_is_directory=True)
    atexit.register(workspace_link.unlink, missing_ok=True)
    for workspace in (workspace_a, workspace_b, unknown_workspace):
        atexit.register(shutil.rmtree, workspace, ignore_errors=True)
    _, project_a_result = run_cli(home, "project-id", str(workspace_a))
    _, nested_result = run_cli(home, "project-id", str(nested_workspace))
    _, linked_result = run_cli(home, "project-id", str(workspace_link))
    expected_project_a = project_a_result.get("project")
    check("项目目录解析规范化子目录与符号链接",
          expected_project_a == nested_result.get("project") == linked_result.get("project"),
          str((project_a_result, nested_result, linked_result)))
    program_client = McpClient(home, cwd=Path("/"),
                               roots=[ROOT / "rust" / "target"])
    program_client.call("initialize", {"protocolVersion": "2025-06-18",
                                       "capabilities": {"roots": {}},
                                       "clientInfo": {"name": "program", "version": "1"}})
    program_client.notify("notifications/initialized")
    program_client.call("ping")
    r = program_client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "程序目录不提供项目上下文", "scope": "project"}})
    check("程序安装目录不能成为项目上下文", r["result"].get("isError") is True, str(r))
    program_client.close()

    client_a = McpClient(home, cwd=Path("/"), roots=[workspace_a])
    client_a.call("initialize", {"protocolVersion": "2025-06-18",
                                  "capabilities": {"roots": {"listChanged": True}},
                                  "clientInfo": {"name": "roots-a", "version": "1"}})
    client_a.notify("notifications/initialized")
    client_a.call("ping")
    r = client_a.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "ProjectAlphaOnly 独占记忆", "body": "只属于项目 A",
        "entities": ["ProjectAlphaOnly"], "scope": "project"}})
    project_id = int(re.search(r"记忆 (\d+)", r["result"]["content"][0]["text"]).group(1))
    project_card = (home / "cards" / f"{project_id:04d}.mem").read_text()
    check("roots 提供项目上下文",
          f"# scope project:{expected_project_a}" in project_card, project_card)

    nested_client = McpClient(home, cwd=Path("/"), roots=[nested_workspace])
    nested_client.call("initialize", {"protocolVersion": "2025-06-18",
                                       "capabilities": {"roots": {}},
                                       "clientInfo": {"name": "nested-root", "version": "1"}})
    nested_client.notify("notifications/initialized")
    nested_client.call("ping")
    r = nested_client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "子目录 root 仍属于项目 A", "scope": "project"}})
    nested_id = int(re.search(r"记忆 (\d+)", r["result"]["content"][0]["text"]).group(1))
    nested_card = (home / "cards" / f"{nested_id:04d}.mem").read_text()
    check("root 与工作目录共用项目解析结果",
          f"# scope project:{expected_project_a}" in nested_card, nested_card)
    nested_client.close()
    client_b = McpClient(home, cwd=Path("/"), roots=[workspace_b])
    client_b.call("initialize", {"protocolVersion": "2025-06-18",
                                  "capabilities": {"roots": {}},
                                  "clientInfo": {"name": "roots-b", "version": "1"}})
    client_b.notify("notifications/initialized")
    client_b.call("ping")
    r = client_b.call("tools/call", {"name": "memory_search", "arguments": {
        "query": "ProjectAlphaOnly"}})
    check("项目搜索硬隔离", "没有找到" in r["result"]["content"][0]["text"], str(r))
    r = client_b.call("tools/call", {"name": "memory_get", "arguments": {"ids": [project_id]}})
    check("按编号读取也隔离", r["result"].get("isError") is True, str(r))
    isolated_calls = [
        client_b.call("tools/call", {"name": "memory_update", "arguments": {
            "id": project_id, "body": "越权更新"}}),
        client_b.call("tools/call", {"name": "memory_feedback", "arguments": {
            "id": project_id, "outcome": "correct"}}),
        client_b.call("tools/call", {"name": "memory_archive", "arguments": {
            "id": project_id}}),
        client_b.call("tools/call", {"name": "memory_delete", "arguments": {
            "id": project_id, "confirm": True}}),
    ]
    check("项目变更操作全部隔离", all(
        item["result"].get("isError") is True for item in isolated_calls
    ), str(isolated_calls))
    audit_b = client_b.call("tools/call", {"name": "memory_audit", "arguments": {}})
    check("项目审计也隔离", "ProjectAlphaOnly" not in
          audit_b["result"]["content"][0]["text"], str(audit_b))
    deleted = client_a.call("tools/call", {"name": "memory_delete", "arguments": {
        "id": project_id, "confirm": True}})
    after_delete = client_b.call("tools/call", {"name": "memory_get", "arguments": {
        "ids": [project_id]}})
    check("项目墓碑保留隔离边界", deleted["result"].get("isError") is not True
          and after_delete["result"].get("isError") is True, str((deleted, after_delete)))
    client_a.close()
    client_b.close()

    cwd_client = McpClient(home, cwd=workspace_a)
    cwd_client.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "cwd", "version": "1"}})
    cwd_client.notify("notifications/initialized")
    r = cwd_client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "工作目录项目上下文", "scope": "project"}})
    check("具体工作目录提供项目上下文", r["result"].get("isError") is not True, str(r))
    cwd_client.close()

    ambiguous = McpClient(home, cwd=Path("/"), roots=[workspace_a, workspace_b])
    ambiguous.call("initialize", {"protocolVersion": "2025-06-18",
                                    "capabilities": {"roots": {}},
                                    "clientInfo": {"name": "multi-root", "version": "1"}})
    ambiguous.notify("notifications/initialized")
    ambiguous.call("ping")
    r = ambiguous.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "多根目录不能猜项目", "scope": "project"}})
    check("多个 roots 不猜项目", r["result"].get("isError") is True, str(r))
    ambiguous.close()

    plain_root = McpClient(home, cwd=Path("/"), roots=[unknown_workspace])
    plain_root.call("initialize", {"protocolVersion": "2025-06-18",
                                    "capabilities": {"roots": {}},
                                    "clientInfo": {"name": "plain-root", "version": "1"}})
    plain_root.notify("notifications/initialized")
    plain_root.call("ping")
    r = plain_root.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "客户端明确选择的普通工作区", "scope": "project"}})
    check("唯一 roots 可作为明确项目边界", r["result"].get("isError") is not True, str(r))
    plain_root.close()

    unknown_client = McpClient(home, cwd=unknown_workspace)
    unknown_client.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                        "clientInfo": {"name": "unknown", "version": "1"}})
    unknown_client.notify("notifications/initialized")
    r = unknown_client.call("tools/call", {"name": "memory_save", "arguments": {
        "title": "不能静默降级", "scope": "project"}})
    check("未知项目拒绝项目写入", r["result"].get("isError") is True
          and "项目" in r["result"]["content"][0]["text"]
          and "global" in r["result"]["content"][0]["text"], str(r))
    unknown_client.close()

    legacy = McpClient(home)
    r = legacy.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                    "clientInfo": {"name": "legacy", "version": "1"}})
    check("已退役协议按未知版本处理", r["result"]["protocolVersion"] == "2025-11-25", str(r))
    legacy.close()
    future = McpClient(home)
    r = future.call("initialize", {"protocolVersion": "2099-01-01", "capabilities": {},
                                    "clientInfo": {"name": "future", "version": "1"}})
    check("未知协议不被回显", r["result"]["protocolVersion"] == "2025-11-25", str(r))
    future.close()

    privacy_query = "QUERY_SECRET_51d08b"
    privacy_client = McpClient(home)
    privacy_client.call("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                        "clientInfo": {"name": "privacy", "version": "1"}})
    privacy_client.call("tools/call", {"name": "memory_search", "arguments": {
        "query": privacy_query}})
    privacy_client.close()

    logs = "\n".join(
        path.read_text(encoding="utf-8", errors="replace")
        for path in (home / "logs").glob("*.log")
    )
    check("MCP 与审计日志不记录记忆内容", secret not in logs
          and "正文只应进入记忆卡" not in logs and privacy_query not in logs, logs[-500:])
    check("诊断与审计日志保持私有权限", os.name == "nt" or (
          (home / "logs").stat().st_mode & 0o777 == 0o700 and all(
              path.stat().st_mode & 0o777 == 0o600 for path in (home / "logs").glob("*.log")
          )), "")

    print("[6] 索引重建一致性")
    rc1, out1 = run_cli(home, "search", "")
    rc, out = run_cli(home, "rebuild")
    rc2, out2 = run_cli(home, "search", "")
    check("重建后排序一致", out1 == out2, "")

    print("[7] 并发写入")
    env = dict(os.environ, ENGRAMARK_HOME=str(home))
    procs = []
    for i in range(8):
        text = (f"@0 fact published I1 T2 2026-08-02\n= Concurrent{i}\n~ self:test\n"
                f"并发写入测试卡 {i}。\n")
        procs.append(subprocess.Popen([str(CORE), "save", text], stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True, env=env))
    concurrent_out = []
    for proc in procs:
        stdout, stderr = proc.communicate(timeout=60)
        try:
            concurrent_out.append(json.loads(stdout.strip() or stderr.strip()))
        except json.JSONDecodeError:
            concurrent_out.append({})
    concurrent_ids = [item.get("id") for item in concurrent_out if item.get("ok")]
    check("并发分配 id 不冲突", len(concurrent_ids) == 8 and len(set(concurrent_ids)) == 8,
          str(concurrent_out))

    duplicate = """@0 fact published I1 T2 2026-08-02
= ConcurrentDuplicate
~ self:test
并发重复内容只应保存一张。
"""
    procs = [subprocess.Popen([str(CORE), "save", duplicate], stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True, env=env) for _ in range(4)]
    duplicate_out = []
    for proc in procs:
        stdout, stderr = proc.communicate(timeout=60)
        duplicate_out.append(json.loads(stdout.strip() or stderr.strip()))
    duplicate_ids = [item.get("id") for item in duplicate_out]
    check("并发重复写入复用同一 id", len(set(duplicate_ids)) == 1,
          str(duplicate_out))
    rc, out = run_cli(home, "rebuild")
    check("并发写入后索引可重建", rc == 0 and out.get("ok"), str(out))

    print("[8] JSON 安全 ID 上限与测试时钟注入")
    rc, out = run_cli(home, "publish", str(2**53))
    check("超界公开 id 返回稳定错误", rc == 1 and not out.get("ok")
          and "安全上限" in str(out.get("error", "")), str(out))
    over_card = "@9007199254740992 fact candidate I2 T2 2026-08-02\n~ user\n超界卡片。\n"
    rc, out = run_cli(home, "propose", over_card)
    check("超界卡头被拒绝", rc == 1 and "安全上限" in str(out.get("error", "")), str(out))
    over_ref = ("@0 fact candidate I2 T2 2026-08-02\n~ user\n# supersedes @9007199254740992\n"
                "引用超界编号。\n")
    rc, out = run_cli(home, "propose", over_ref)
    check("超界 supersedes 被拒绝", rc == 1 and "安全上限" in str(out.get("error", "")), str(out))
    env = dict(os.environ, ENGRAMARK_HOME=str(home), ENGRAMARK_TEST_NOW="2030-01-02T03:04:05")
    r = subprocess.run([str(CORE), "get", "1"], capture_output=True, text=True,
                       env=env, timeout=60)
    check("注入时钟驱动 last-used 写回", r.returncode == 0
          and (home / "cards" / "0001.mem").read_text(encoding="utf-8").count(
              "# last-used 2030-01-02") == 1, r.stdout[-200:] + r.stderr[-200:])
    client = McpClient(home)
    try:
        client.call("initialize", {"protocolVersion": "2025-11-25",
                                   "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}})
        client.notify("notifications/initialized")
        listed = client.call("tools/list")
        schemas = {tool["name"]: tool["inputSchema"] for tool in listed["result"]["tools"]}
        id_schema = schemas["memory_get"]["properties"]["ids"]["items"]
        check("MCP id schema 声明安全上限",
              id_schema.get("maximum") == 9007199254740991, str(id_schema))
        resp = client.call("tools/call", {"name": "memory_archive", "arguments": {"id": 2**53}})
        text = resp["result"]["content"][0]["text"]
        check("MCP 超界 id 返回稳定错误", resp["result"].get("isError") is True
              and "安全上限" in text, text)
    finally:
        client.close()

    print(f"\n结果：{passed} 通过 / {failed} 失败")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
