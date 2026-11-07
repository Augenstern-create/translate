#!/usr/bin/env python3
"""Kaikki / 中文 Wiktionary JSONL → 规范化词典 JSONL（P9.1 小样本）。

只读取本地文件；不联网。默认只保留 lang_code=en 且 gloss 含 CJK 的条目。
输出一行一个对象，供 `pack-dictionary --format jsonl` 使用。
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def has_cjk(text: str) -> bool:
    return any(
        "\u4e00" <= c <= "\u9fff"
        or "\u3400" <= c <= "\u4dbf"
        or "\uf900" <= c <= "\ufaff"
        for c in text
    )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("output")
    ap.add_argument("--limit", type=int, default=1000)
    ap.add_argument("--stats", default=None)
    args = ap.parse_args()

    stats = {
        "source": "kaikki-zh-wiktionary",
        "total_rows": 0,
        "accepted_rows": 0,
        "skipped_language": 0,
        "skipped_missing_headword": 0,
        "skipped_invalid_schema": 0,
        "skipped_no_chinese_gloss": 0,
        "merged_duplicates": 0,
        "output_entries": 0,
    }
    entries: dict[str, dict] = {}
    with open(args.input, encoding="utf-8") as fh:
        for line in fh:
            stats["total_rows"] += 1
            try:
                row = json.loads(line)
            except Exception:
                stats["skipped_invalid_schema"] += 1
                continue
            if row.get("lang_code") != "en":
                stats["skipped_language"] += 1
                continue
            word = (row.get("word") or "").strip()
            if not word:
                stats["skipped_missing_headword"] += 1
                continue
            senses = row.get("senses") or []
            defs = []
            for sense in senses:
                for gloss in sense.get("glosses") or []:
                    gloss = (gloss or "").strip()
                    if gloss and has_cjk(gloss):
                        defs.append(gloss)
            if not defs:
                stats["skipped_no_chinese_gloss"] += 1
                continue
            pos = (row.get("pos") or "").strip()
            if word in entries:
                stats["merged_duplicates"] += 1
                old = entries[word]
                for d in defs:
                    if d not in old["definitions"]:
                        old["definitions"].append(d)
                if pos and pos not in old["pos"]:
                    old["pos"] = (old["pos"] + ", " + pos).strip(", ")
            else:
                entries[word] = {
                    "headword": word,
                    "pos": pos or None,
                    "definitions": sorted(set(defs)),
                }
            if len(entries) >= args.limit:
                break

    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as fh:
        for word in sorted(entries):
            fh.write(json.dumps(entries[word], ensure_ascii=False) + "\n")
    stats["accepted_rows"] = sum(1 for _ in entries)
    stats["output_entries"] = len(entries)
    if args.stats:
        Path(args.stats).write_text(json.dumps(stats, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(stats, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
