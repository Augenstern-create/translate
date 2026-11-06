#!/usr/bin/env python3
"""把 PerfHarness 的一行 JSON 汇总成 p50/p95，并保留原始数组。"""
from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parents[3]
OUT = SDK / "tests/android-harness/out"


def pct(values: list[float], p: float) -> float:
    ordered = sorted(values)
    if not ordered:
        raise ValueError("empty")
    # nearest-rank：与报告里写的口径一致，避免插值口径争议
    import math
    k = max(1, math.ceil(p / 100.0 * len(ordered)))
    return ordered[min(k, len(ordered)) - 1]


def summary(values: list[float]) -> dict:
    return {
        "n": len(values),
        "min": round(min(values), 3),
        "p50": round(pct(values, 50), 3),
        "p95": round(pct(values, 95), 3),
        "max": round(max(values), 3),
        "mean": round(statistics.fmean(values), 3),
    }


def main() -> int:
    raw = json.loads((OUT / "j14-raw.json").read_text(encoding="utf-8"))
    result = {
        "stage": "P7.2/J-14",
        "device": raw["device"],
        "iterations": raw["iterations"],
        "rss_source": raw["rss_source"],
        "latency_ms": {
            k: summary(raw[f"{k}_ms"]) for k in ("create", "lookup", "detect", "close")
        },
        "rss_kb": {
            "after_create_first": raw["rss_kb_after_create"][0],
            "after_close_last": raw["rss_kb_after_close"][-1],
            "growth": raw["rss_kb_after_close"][-1] - raw["rss_kb_after_create"][0],
            "vmhwm": raw["vmhwm_kb"],
            "peak_observed": raw["peak_observed_kb"],
        },
        "caveats": [
            "x86_64 模拟器（gms2 / android-34），不是物理设备",
            "JNI v1 未编译真实 NMT，本数据不代表 Android NMT 内存预算",
            "RSS 为 Java+native 同进程口径（/proc/self/status VmRSS）",
            "lookup 命中词典 mmap/页缓存，热路径数字偏乐观",
        ],
    }
    (OUT / "j14-summary.json").write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
