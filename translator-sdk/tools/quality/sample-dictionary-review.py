#!/usr/bin/env python3
"""生成词典人工评审抽样（T1 完成标准：人工质量审查）。

输入 `run-dictionary-quality.py` 产出的 `lookup-results.jsonl`。
按固定随机种子抽 50 条（可复现），并纳入 10 个多义词；输出 CSV + Markdown 模板。
不自动评分，`review_status` 固定 PENDING_HUMAN。
"""
from __future__ import annotations

import argparse
import csv
import json
import random
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--lookup", required=True)
    ap.add_argument("--csv", required=True)
    ap.add_argument("--md", required=True)
    ap.add_argument("--sample", type=int, default=50)
    args = ap.parse_args()

    rows = []
    for line in Path(args.lookup).read_text(encoding="utf-8").splitlines():
        if line.strip():
            rows.append(json.loads(line))

    found = [r for r in rows if r.get("found") and r.get("entries")]
    random.seed(20261108)
    sample = random.sample(found, min(args.sample, len(found)))
    # 多义词全部纳入
    for word in ["bank", "spring", "charge", "run", "match", "right", "book", "present", "light", "close"]:
        row = next((r for r in rows if r["word"].lower() == word), None)
        if row and row not in sample:
            sample.append(row)
    sample.sort(key=lambda r: r["word"].lower())

    csv_path = Path(args.csv)
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with csv_path.open("w", newline="", encoding="utf-8-sig") as fh:
        writer = csv.DictWriter(
            fh,
            fieldnames=[
                "word",
                "pos",
                "senses",
                "meaning_accuracy_1_5",
                "coverage_of_senses_1_5",
                "chinese_naturalness_1_5",
                "pos_correct",
                "worst_error_critical_major_minor_acceptable",
                "reviewer_note",
                "review_status",
            ],
        )
        writer.writeheader()
        for row in sample:
            entries = row["entries"]
            first = entries[0]
            writer.writerow(
                {
                    "word": row["word"],
                    "pos": "|".join(first.get("pos") or []),
                    "senses": " || ".join(first.get("senses") or []),
                    "meaning_accuracy_1_5": "",
                    "coverage_of_senses_1_5": "",
                    "chinese_naturalness_1_5": "",
                    "pos_correct": "",
                    "worst_error_critical_major_minor_acceptable": "",
                    "reviewer_note": "",
                    "review_status": "PENDING_HUMAN",
                }
            )

    md = [
        "# 词典人工评审模板（PENDING_HUMAN）",
        "",
        f"- 抽样：{len(sample)} 条（固定种子 20261108 + 10 个多义词）",
        "- 评分：释义准确性 / 义项覆盖 / 中文自然度（1–5）；词性是否正确；最差错误等级。",
        "- 状态：**未完成**。空白不代表通过。",
        "",
        "| 词条 | 词性 | 义项 |",
        "|---|---|---|",
    ]
    for row in sample:
        first = row["entries"][0]
        md.append(
            f"| {row['word']} | {'|'.join(first.get('pos') or [])} | "
            + " || ".join(first.get("senses") or [])
            + " |"
        )
    md += ["", "结果填写到同名 CSV，并在最终报告中更新 `review_status`。", ""]
    Path(args.md).write_text("\n".join(md), encoding="utf-8")
    print(f"sampled {len(sample)} entries -> {csv_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
