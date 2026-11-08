#!/usr/bin/env python3
"""生成人工评审抽样表（T5 §5.3）。

不自动打分；只把金样本、参考译文、实际译文放进 CSV，留下空白列供评审人填写。
按类别分层抽样（每类最多 5 条），并额外纳入所有自动检查异常样本。

用法：
    python3 tools/quality/make-review-sample.py \
        --golden fixtures/quality/golden-v1.jsonl \
        --outputs tests/quality/outputs/full.jsonl \
        --csv tests/quality/reports/human-review-sample.csv \
        --md tests/quality/reports/human-review-template.md
"""
from __future__ import annotations

import argparse
import csv
import json
from collections import defaultdict
from pathlib import Path

RATING_FIELDS = [
    "semantic_accuracy_1_5",
    "completeness_1_5",
    "fluency_1_5",
    "terminology_1_5",
    "context_1_5",
    "format_1_5",
    "worst_error_critical_major_minor_acceptable",
    "reviewer_note",
]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--golden", required=True)
    ap.add_argument("--outputs", required=True)
    ap.add_argument("--csv", required=True)
    ap.add_argument("--md", required=True)
    ap.add_argument("--per-category", type=int, default=5)
    args = ap.parse_args()

    golden = {}
    for line in Path(args.golden).read_text(encoding="utf-8").splitlines():
        if line.strip():
            row = json.loads(line)
            golden[row["id"]] = row
    outputs = {}
    for line in Path(args.outputs).read_text(encoding="utf-8").splitlines():
        if line.strip():
            row = json.loads(line)
            outputs[row["id"]] = row

    by_category = defaultdict(list)
    for gid, row in golden.items():
        by_category[row["category"]].append(gid)

    selected = []
    for category in sorted(by_category):
        selected.extend(by_category[category][: args.per_category])
    for gid, out in outputs.items():
        quality = out.get("quality") or {}
        if out.get("error") or quality.get("fatal") or quality.get("issues"):
            if gid not in selected:
                selected.append(gid)

    selected = sorted(set(selected), key=lambda gid: gid)
    csv_path = Path(args.csv)
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with csv_path.open("w", newline="", encoding="utf-8-sig") as fh:
        writer = csv.DictWriter(
            fh,
            fieldnames=[
                "id",
                "category",
                "source",
                "reference",
                "output",
                "auto_flags",
                *RATING_FIELDS,
            ],
        )
        writer.writeheader()
        for gid in selected:
            g = golden.get(gid, {})
            out = outputs.get(gid, {})
            quality = out.get("quality") or {}
            flags = []
            if out.get("error"):
                flags.append(f"error:{out['error'].get('code')}")
            for issue in quality.get("issues") or []:
                flags.append(f"{issue.get('kind')}:{issue.get('severity')}")
            writer.writerow(
                {
                    "id": gid,
                    "category": g.get("category", ""),
                    "source": g.get("source", ""),
                    "reference": g.get("reference", ""),
                    "output": out.get("output") or "",
                    "auto_flags": ";".join(flags),
                    **{field: "" for field in RATING_FIELDS},
                }
            )

    md = [
        "# 人工评审模板（PENDING_HUMAN）",
        "",
        f"- 抽样数：{len(selected)} / {len(golden)}（每类最多 {args.per_category} 条 + 自动异常样本）",
        "- 评审列：语义准确性 / 信息完整性 / 中文自然度 / 术语正确性 / 上下文一致性 / 格式保真（1–5）",
        "- 错误等级：Critical / Major / Minor / Acceptable（必须区分“译法不同但含义正确”）",
        "- 状态：**未完成**。空白不代表通过；完成前不得把本轮质量标为 PASS。",
        "",
        "| ID | 类别 | 原文 | 参考译文 | 实际译文 | 自动标记 |",
        "|---|---|---|---|---|---|",
    ]
    for gid in selected:
        g = golden.get(gid, {})
        out = outputs.get(gid, {})
        quality = out.get("quality") or {}
        flags = []
        if out.get("error"):
            flags.append(f"error:{out['error'].get('code')}")
        for issue in quality.get("issues") or []:
            flags.append(f"{issue.get('kind')}:{issue.get('severity')}")
        md.append(
            f"| {gid} | {g.get('category', '')} | {g.get('source', '')} | {g.get('reference', '')} | "
            f"{out.get('output') or ''} | {';'.join(flags)} |"
        )
    md += ["", "评审结果请填写到同名 CSV，并把 `review_status` 从 `pending` 改为 `reviewed`。", ""]
    Path(args.md).write_text("\n".join(md), encoding="utf-8")
    print(f"selected {len(selected)} samples -> {csv_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
