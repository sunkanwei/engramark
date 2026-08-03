#!/usr/bin/env python3
"""可调规模的合成卡检索回归（黑盒驱动 Rust 二进制），默认模拟两千张手动积累。"""
from __future__ import annotations

import json
import os
import re
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

from support import rust_binary

ROOT = Path(__file__).resolve().parents[1]
BINARY = rust_binary(release=True)
CARD_COUNT = max(100, int(os.environ.get("ENGRAMARK_SCALE_CARDS", "2000")))
TARGET_ID = CARD_COUNT - 2
ATLAS_ID = CARD_COUNT - 1
BEACON_ID = CARD_COUNT
ID_RE = re.compile(r"^@(\d+)\b")


def card(cid: int, entity: str, title: str, body: str = "", scope: str = "") -> str:
    lines = [f"@{cid} fact published I1 T2 2026-08-01", f"= {entity}", "~ self:test"]
    if scope:
        lines.append(f"# scope project:{scope}")
    lines.append(title)
    if body:
        lines.append(body)
    return "\n".join(lines) + "\n"


def cli(home: Path, *args: str, stdin: str = "") -> dict:
    result = subprocess.run([str(BINARY), *args], input=stdin, capture_output=True,
                            text=True, env={**os.environ, "ENGRAMARK_HOME": str(home)},
                            timeout=60)
    text = result.stdout.strip() or result.stderr.strip()
    return json.loads(text) if text else {}


def search_ids(home: Path, query: str, limit: int = 5, project: str = "") -> list[int]:
    out = cli(home, "search", query, "--limit", str(limit), "--project", project)
    ids = []
    for line in out.get("results", []):
        match = ID_RE.match(line.removeprefix("可能相关："))
        if match:
            ids.append(int(match.group(1)))
    return ids


def main() -> int:
    home = Path(tempfile.mkdtemp(prefix="engramark-scale-"))
    cards = home / "cards"
    cards.mkdir(parents=True)
    try:
        for cid in range(1, TARGET_ID):
            topic = f"topic{cid:04d}"
            (cards / f"{cid:04d}.mem").write_text(card(
                cid,
                topic,
                f"合成项目 {cid} 的构建服务配置。",
                f"用于干扰检索的普通资料，组件编号 {cid}，不含目标事实。",
            ), encoding="utf-8")

        (cards / f"{TARGET_ID:04d}.mem").write_text(
            f"""@{TARGET_ID} fact published I3 T3 2026-08-02
= OrchidUI Web 当前 CEF 运行组件下载地址
~ user
# lock
OrchidUI Web 当前 CEF 下载域名 downloads.example.com。
Mac 与 Windows 文件均由 https://downloads.example.com/current.json 统一清单提供。
""", encoding="utf-8")
        (cards / f"{ATLAS_ID:04d}.mem").write_text(card(
            ATLAS_ID, "Atlas, core", "Atlas 项目的 core 构建规则。", scope="Atlas"
        ), encoding="utf-8")
        (cards / f"{BEACON_ID:04d}.mem").write_text(card(
            BEACON_ID, "Beacon, core", "Beacon 项目的 core 构建规则。", scope="Beacon"
        ), encoding="utf-8")

        started = time.perf_counter()
        cli(home, "rebuild")
        rebuild_ms = (time.perf_counter() - started) * 1000

        positives = [
            "CEF 服务下载地址",
            "Mac Windows CEF 组件在哪里下载",
            "downloads.example.com",
            "OrchidUI Web 的 CEF current.json",
        ]
        recall = sum(
            TARGET_ID in search_ids(home, query, limit=5, project="OrchidUIweb-scale")
            for query in positives
        ) / len(positives)

        negatives = [
            "今天的天气和晚餐菜谱", "如何学习水彩画", "附近咖啡店推荐", "旅行行李清单",
            "健身训练计划", "电影字幕校对", "花园浇水提醒", "年度税务资料整理",
        ]
        rejection = sum(not search_ids(home, query, limit=5) for query in negatives) / len(negatives)
        radar_false = sum(
            bool(cli(home, "scan", "--session", f"neg-{i}", "--project", "unrelated",
                     stdin=query).get("lines"))
            for i, query in enumerate(negatives)
        )
        radar_positive = bool(cli(home, "scan", "--session", "pos", "--project", "unrelated",
                                  stdin="CEF 下载地址").get("lines"))

        scoped = search_ids(home, "core 构建规则", limit=5, project="Atlas")
        cross_project = search_ids(home, "Beacon 项目的 core 构建规则", limit=5, project="Atlas")

        for _ in range(3):
            search_ids(home, positives[0], limit=5, project="OrchidUIweb-scale")
        search_latencies = []
        for index in range(20):
            started = time.perf_counter()
            search_ids(home, positives[index % len(positives)], limit=5,
                       project="OrchidUIweb-scale")
            search_latencies.append((time.perf_counter() - started) * 1000)
        search_sorted = sorted(search_latencies)

        radar_latencies = []
        radar_output_bytes = []
        for index in range(20):
            started = time.perf_counter()
            scanned = cli(home, "scan", "--session", f"scale-{index}",
                          "--project", "OrchidUIweb-scale", stdin="CEF 下载地址")
            radar_latencies.append((time.perf_counter() - started) * 1000)
            radar_output_bytes.append(len(scanned.get("context", "").encode("utf-8")))
        radar_sorted = sorted(radar_latencies)

        report = {
            "cards": len(list(cards.glob("*.mem"))),
            "rebuild_ms": round(rebuild_ms, 1),
            "recall_at_5": round(recall, 4),
            "unrelated_rejection": round(rejection, 4),
            "radar_false_injections": radar_false,
            "strong_anchor_radar": radar_positive,
            "scope_top1": scoped[0] if scoped else None,
            "cross_project_visible": BEACON_ID in cross_project,
            "hot_search_p50_ms": round(statistics.median(search_sorted), 2),
            "hot_search_p95_ms": round(search_sorted[18], 2),
            "hot_radar_p50_ms": round(statistics.median(radar_sorted), 2),
            "hot_radar_p95_ms": round(radar_sorted[18], 2),
            "radar_output_max_bytes": max(radar_output_bytes),
        }
        print(json.dumps(report, ensure_ascii=False, indent=2))
        ok = (
            report["cards"] == CARD_COUNT
            and recall == 1.0
            and rejection == 1.0
            and radar_false == 0
            and radar_positive
            and bool(scoped) and scoped[0] == ATLAS_ID
            and not report["cross_project_visible"]
            and search_sorted[18] < 100.0
            and radar_sorted[18] < 250.0
            and max(radar_output_bytes) <= 1200
        )
        return 0 if ok else 1
    finally:
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
