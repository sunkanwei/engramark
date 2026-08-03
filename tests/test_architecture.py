#!/usr/bin/env python3
"""架构验收：格式、事务恢复、缓存一致性、锁切换与迁移（黑盒驱动 Rust 二进制）。"""
from __future__ import annotations

import base64
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

from support import rust_binary

ROOT = Path(__file__).resolve().parents[1]
BINARY = rust_binary()
passed = failed = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global passed, failed
    if condition:
        passed += 1
    else:
        failed += 1
        print(f"  FAIL {name}  {detail}")


def cli(home: Path, *args: str, crash: str = "", stdin: str = "") -> tuple[int, dict | str]:
    env = dict(os.environ, ENGRAMARK_HOME=str(home))
    if crash:
        env["ENGRAMARK_CRASH_STAGE"] = crash
    result = subprocess.run([str(BINARY), *args], input=stdin, capture_output=True, text=True,
                            env=env, timeout=60)
    output = result.stdout.strip() or result.stderr.strip()
    try:
        return result.returncode, json.loads(output)
    except json.JSONDecodeError:
        return result.returncode, output


def memory(entity: str, title: str, *, status: str = "published", trust: str = "2") -> str:
    return (f"@0 fact {status} I2 T{trust} 2026-08-02\n"
            f"= {entity}\n~ self:test\n{title}\n")


def latest_tx(home: Path) -> tuple[Path, dict]:
    path = next((home / "state" / "transactions").glob("*.txn"))
    return path, json.loads(path.read_text())


def test_state_and_recovery() -> None:
    home = Path(tempfile.mkdtemp(prefix="engramark-v3-txn-"))
    try:
        rc, first = cli(home, "save", memory("Alpha", "Alpha 初始事实。"))
        check("事务基线写入", rc == 0 and first.get("id") == 1, str(first))

        rc, _ = cli(home, "save", memory("BeforeJournal", "日志后崩溃。"), crash="after-journal")
        check("日志落盘后崩溃被注入", rc == 97)
        rc, recovered = cli(home, "recover")
        check("源未写入时确定性终止事务", rc == 0 and
              recovered["recovered"][0]["action"] == "aborted-before-source", str(recovered))

        rc, _ = cli(home, "save", memory("AfterSource", "源写入后崩溃。"), crash="after-source")
        check("源写入后崩溃被注入", rc == 97)
        rc, found = cli(home, "search", "AfterSource")
        check("源新缓存旧时自动重放", rc == 0 and found.get("results"), str(found))
        check("重放后清理事务日志", not list((home / "state" / "transactions").glob("*.txn")))

        rc, _ = cli(home, "save", memory("AfterCache", "缓存提交后崩溃。"), crash="after-cache")
        check("缓存提交后崩溃被注入", rc == 97)
        rc, recovered = cli(home, "recover")
        check("源与缓存均新时只清日志", rc == 0 and
              recovered["recovered"][0]["action"] == "already-committed", str(recovered))

        rc, _ = cli(home, "save", memory("CacheAhead", "模拟缓存领先。"), crash="after-cache")
        tx_path, tx = latest_tx(home)
        item = tx["files"][0]
        target = home / item["path"]
        if item["before_exists"]:
            target.write_bytes(base64.b64decode(item["before_b64"]))
        else:
            target.unlink()
        rc, recovered = cli(home, "recover")
        check("缓存新而源旧时以源修复缓存", rc == 0 and
              recovered["recovered"][0]["action"] == "repaired-cache-to-source", str(recovered))
        rc, result = cli(home, "search", "CacheAhead")
        check("缓存领先修复后不泄露新值", rc == 0 and result.get("results") == [], str(result))

        save_rc, failed_save = cli(home, "save", memory("DiskFullSource", "源写入磁盘满。"),
                                   crash="disk-full-source")
        rc, recovered = cli(home, "recover")
        check("源文件磁盘满时保留并确定性终止事务", save_rc != 0 and not failed_save.get("ok", True)
              and recovered and recovered["recovered"][0]["action"] == "aborted-before-source",
              f"{failed_save} / {recovered}")

        save_rc, failed_save = cli(home, "save", memory("DiskFullCache", "缓存提交磁盘满。"),
                                   crash="disk-full-cache")
        rc, recovered = cli(home, "recover")
        check("缓存磁盘满时从新源重放", save_rc != 0 and not failed_save.get("ok", True)
              and recovered and recovered["recovered"][0]["action"] == "completed-source-and-cache",
              f"{failed_save} / {recovered}")

        rc, _ = cli(home, "save", memory("Traversal", "恶意恢复路径。"), crash="after-journal")
        tx_path, tx = latest_tx(home)
        outside = home.parent / f"{home.name}-outside.mem"
        tx["files"][0]["path"] = "../" + outside.name
        unsigned = {key: value for key, value in tx.items() if key != "checksum"}
        canonical = json.dumps(
            unsigned, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
        tx["checksum"] = hashlib.sha256(b"MEMTXN\0v1" + canonical).hexdigest()
        tx_path.write_text(
            json.dumps(tx, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8")
        rc, result = cli(home, "recover")
        check("事务恢复拒绝目录穿越", rc != 0 and "路径" in str(result)
              and not outside.exists() and tx_path.exists(), str(result))
        tx_path.unlink()

        rc, _ = cli(home, "save", memory("Conflict", "人工冲突。"), crash="after-source")
        tx_path, tx = latest_tx(home)
        conflict = home / tx["files"][0]["path"]
        conflict.write_text(conflict.read_text() + "人工改写\n", encoding="utf-8")
        rc, result = cli(home, "recover")
        check("既非旧哈希也非新哈希时停止自动恢复", rc != 0 and
              "停止自动恢复" in str(result), str(result))
        check("人工冲突保留恢复证据", tx_path.exists())
        tx_path.unlink()

        rc, _ = cli(home, "feedback", "1", "+", crash="after-source")
        mark = home / "state" / "feedback" / "1.mark"
        check("反馈卡片写入后崩溃被注入", rc == 97 and not mark.exists())
        rc, recovered = cli(home, "recover")
        check("反馈卡片与冷却标记作为同一事务恢复", rc == 0 and mark.is_file()
              and recovered["recovered"][0]["action"] == "completed-source-and-cache",
              str(recovered))
        rc, result = cli(home, "feedback", "1", "+")
        check("反馈恢复后冷却标记生效", rc != 0 and "冷却" in str(result), str(result))
    finally:
        shutil.rmtree(home, ignore_errors=True)


def test_cache_and_ids() -> None:
    home = Path(tempfile.mkdtemp(prefix="engramark-v3-cache-"))
    try:
        cli(home, "save", memory("Keep", "需要保留的事实。"))
        cli(home, "propose", memory("Candidate", "候选内容。", status="candidate"))
        cli(home, "reject", "2")
        cli(home, "save", memory("Next", "墓碑后继续编号。"))
        check("墓碑编号永不复用", (home / "cards" / "0002.mem").exists()
              and (home / "cards" / "0003.mem").exists()
              and "tombstone" in (home / "cards" / "0002.mem").read_text())
        before = cli(home, "search", "Keep")[1]
        (home / "cache" / "memory.mcache").unlink()
        after = cli(home, "search", "Keep")[1]
        check("删除缓存可从 cards+state 重建同一检索金样", before == after, f"{before} != {after}")

        conn = sqlite3.connect(home / "cache" / "memory.mcache")
        generation_before = int(conn.execute(
            "SELECT value FROM cache_meta WHERE key='generation'"
        ).fetchone()[0])
        fingerprint_before = conn.execute(
            "SELECT value FROM cache_meta WHERE key='sqlite_capability_fingerprint'"
        ).fetchone()[0]
        conn.execute(
            "UPDATE cache_meta SET value=? WHERE key='sqlite_capability_fingerprint'",
            (json.dumps({"python_executable": "/old/runtime/python"}),),
        )
        conn.commit()
        conn.close()
        rc, upgraded = cli(home, "search", "Keep")
        conn = sqlite3.connect(home / "cache" / "memory.mcache")
        upgraded_meta = dict(conn.execute("SELECT key,value FROM cache_meta"))
        conn.close()
        check("旧能力指纹缓存自动从记忆卡重建", rc == 0 and upgraded.get("results")
              and int(upgraded_meta["generation"]) > generation_before
              and upgraded_meta["sqlite_capability_fingerprint"] == fingerprint_before
              and json.loads(fingerprint_before)["fingerprint_format"] == 3,
              str(upgraded))
        sequence_path = home / "state" / "id-sequence"
        sequence_mtime = sequence_path.stat().st_mtime_ns
        rc, report = cli(home, "diagnose", "--full")
        check("完整诊断通过", rc == 0 and report.get("ok"), str(report))
        check("诊断只读且不改写编号高水位",
              sequence_path.stat().st_mtime_ns == sequence_mtime)

        card_one = home / "cards" / "0001.mem"
        original_one = card_one.read_bytes()
        card_one.write_text("这不是合法卡片\n", encoding="utf-8")
        rc, rebuild_report = cli(home, "rebuild")
        rc_search, missing = cli(home, "search", "Keep")
        rc_diag, bad_report = cli(home, "diagnose")
        check("坏卡退出旧索引并持续报告位置", rc == 0
              and rebuild_report.get("invalid_cards")
              and rc_search == 0 and not missing.get("results")
              and rc_diag == 0 and not bad_report.get("ok")
              and bad_report.get("invalid_cards"), str(bad_report))
        card_one.write_bytes(original_one)
        cli(home, "rebuild")

        conn = sqlite3.connect(home / "cache" / "memory.mcache")
        segment = conn.execute(
            "SELECT id FROM fts_data WHERE id > 10 ORDER BY id DESC LIMIT 1"
        ).fetchone()
        if segment is None:
            raise RuntimeError("FTS 索引没有可供损坏测试使用的段")
        conn.execute("DELETE FROM fts_data WHERE id=?", segment)
        conn.commit()
        conn.close()
        rc, report = cli(home, "diagnose")
        check("业务校验能发现 FTS 与 cards 不一致", rc != 0, str(report))
        cli(home, "rebuild")

        backup = home.parent / f"{home.name}-backup"
        rc, report = cli(home, "backup", str(backup))
        check("一致备份只含卡片、编号和清单", rc == 0
              and sorted(p.name for p in backup.iterdir()) == ["cards", "id-sequence", "manifest.json"],
              str(report))
        rc, extra = cli(home, "save", memory("RollbackExtra", "回滚后应成为墓碑。"))
        rc, report = cli(home, "rollback", str(backup), "--confirm")
        extra_text = (home / "cards" / f"{extra['id']:04d}.mem").read_text()
        check("回滚前自动安全备份且不复用新编号", rc == 0
              and Path(report["safety_backup"]).is_dir()
              and "tombstone" in extra_text
              and int((home / "state" / "id-sequence").read_text()) == extra["id"], str(report))
        shutil.rmtree(backup, ignore_errors=True)
    finally:
        shutil.rmtree(home, ignore_errors=True)


def test_radar_and_trigram() -> None:
    home = Path(tempfile.mkdtemp(prefix="engramark-v3-radar-"))
    try:
        cli(home, "save", memory("ShortAnchor", "标题不含目标。\nbodyonlytoken 只在完整正文。"))
        rc, unicode_hit = cli(home, "search", "bodyonlytoken")
        check("Unicode FTS 索引完整正文",
              rc == 0 and bool(unicode_hit.get("results")), str(unicode_hit))
        import struct
        conn = sqlite3.connect(home / "cache" / "memory.mcache")
        magic, version, count, length = struct.unpack(
            ">4sHHI", bytes(conn.execute("SELECT blob FROM radar_cache").fetchone()[0])[:12])
        payload = b"future-required"
        section = struct.pack(">HHI", 999, 1, len(payload))
        broken = (struct.pack(">4sHHI", magic, version, count + 1,
                              length + 40 + len(payload))
                  + bytes(conn.execute("SELECT blob FROM radar_cache").fetchone()[0])[12:]
                  + section + hashlib.sha256(payload).digest() + payload)
        conn.execute("UPDATE radar_cache SET blob=?", (broken,))
        conn.commit()
        conn.close()
        rc, found = cli(home, "scan", "--session", "radar-rebuild", stdin="ShortAnchor")
        check("未知必需区段损坏雷达后从真源自动重建", rc == 0 and found.get("lines"), str(found))
    finally:
        shutil.rmtree(home, ignore_errors=True)


def test_swap_lock() -> None:
    home = Path(tempfile.mkdtemp(prefix="engramark-v3-swap-"))
    try:
        cli(home, "save", memory("ConcurrentRead", "并发查询与重建。"))
        env = dict(os.environ, ENGRAMARK_HOME=str(home))
        readers = [subprocess.Popen(
            [sys.executable, "-c", (
                "import os,subprocess,sys;"
                f"c={str(BINARY)!r};"
                "[(lambda r: sys.exit(2) if r.returncode else None)(subprocess.run([c,'search','ConcurrentRead'],capture_output=True,text=True,env=os.environ)) for _ in range(20)]")],
            env=env) for _ in range(4)]
        rebuilders = [subprocess.Popen([str(BINARY), "rebuild"], env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                       text=True) for _ in range(3)]
        codes = [p.wait(timeout=60) for p in [*readers, *rebuilders]]
        check("查询与重建并发切换无新旧库冲突", all(code == 0 for code in codes), str(codes))
    finally:
        shutil.rmtree(home, ignore_errors=True)


def test_migration() -> None:
    home = Path(tempfile.mkdtemp(prefix="engramark-v3-migrate-"))
    try:
        (home / "cards").mkdir()
        (home / "candidates").mkdir()
        (home / "cards" / "0001.mem").write_text(
            memory("Old", "缺格式版本标记的正式卡。").replace("@0", "@1"), encoding="utf-8")
        (home / "candidates" / "0002.mem").write_text(
            memory("Draft", "旧目录候选卡。", status="candidate").replace("@0", "@2"),
            encoding="utf-8")
        rc, report = cli(home, "migrate-v1")
        check("旧候选迁入统一 cards 目录", rc == 0
              and (home / "cards" / "0002.mem").exists()
              and not (home / "candidates").exists(), str(report))
        check("迁移生成备份与差异", rc == 0
              and Path(report["backup"]).joinpath("migration.diff").exists(), str(report))
        check("迁移后写入显式格式版本", "# format 1" in
              (home / "cards" / "0001.mem").read_text())
        rc, report = cli(home, "migrate-v1")
        check("已规范的卡片重复迁移无变更", rc == 0 and report["changed"] == 0, str(report))

        (home / "candidates").mkdir()
        conflict = home / "candidates" / "0001.mem"
        conflict.write_text(
            memory("Conflict", "冲突候选。", status="candidate").replace("@0", "@1"),
            encoding="utf-8")
        original = (home / "cards" / "0001.mem").read_bytes()
        rc, report = cli(home, "migrate-v1")
        check("旧候选编号冲突时停止且不覆盖正式卡", rc != 0 and conflict.exists()
              and (home / "cards" / "0001.mem").read_bytes() == original, str(report))
    finally:
        shutil.rmtree(home, ignore_errors=True)


def main() -> int:
    print("[1] 可恢复事务")
    test_state_and_recovery()
    print("[2] 缓存、编号与备份")
    test_cache_and_ids()
    print("[3] FTS 与雷达字节码")
    test_radar_and_trigram()
    print("[4] 跨进程安全切换")
    test_swap_lock()
    print("[5] 旧数据与卡片格式迁移")
    test_migration()
    print(f"\n结果：{passed} 通过 / {failed} 失败")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
