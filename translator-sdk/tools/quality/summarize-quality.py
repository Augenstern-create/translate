#!/usr/bin/env python3
"""把词典质量与翻译对照结果汇总成一份 Markdown（T5）。

用法：
    python3 tools/quality/summarize-quality.py \
        --dictionary tests/quality/reports/dictionary.json \
        --compare tests/quality/reports/compare-v1.json \
        --out tests/quality/reports/summary.md
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dictionary", required=True)
    ap.add_argument("--compare", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--terms-output", default=None, help="terms 配置输出，用于列术语违规")
    args = ap.parse_args()

    d = json.loads(Path(args.dictionary).read_text(encoding="utf-8"))
    c = json.loads(Path(args.compare).read_text(encoding="utf-8"))

    lines = [
        "# 英中质量汇总（自动指标）",
        "",
        "- 人工评审：**PENDING_HUMAN**（未完成，不得当作通过）",
        f"- 词典：{', '.join(d.get('dictionaries', []))}",
        f"- 金样本：{c.get('golden')}（{c.get('sample_count')} 条）",
        "",
        "## 1. 词典",
        "",
        f"- 高频词覆盖：**{d['coverage']['found']}/{d['coverage']['target_total']} = {d['coverage']['rate']:.2%}**",
        f"- 平均义项数（命中词）：{d['coverage']['avg_senses_found']}，多义项词：{d['coverage']['multi_sense_found']}",
        f"- 多义词检查：**{d['ambiguity']['passed']}/{d['ambiguity']['total']}**",
        f"- 技术/固定短语：**{d['phrases']['passed']}/{d['phrases']['total']}**",
        f"- 项目补充条目：**{d['supplement']['passed']}/{d['supplement']['total']}**",
        "",
        "| 多义词 | 义项数 | 结果 |",
        "|---|---:|---|",
    ]
    for item in d["ambiguity"]["items"]:
        lines.append(f"| {item['word']} | {item['sense_count']} | {'PASS' if item['pass'] else 'FAIL'} |")

    lines += [
        "",
        "## 2. 翻译（相对基线的自动对照）",
        "",
        "| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for name, m in c["runs"].items():
        lines.append(
            f"| {name} | {m['chrf']:.4f} | {m['bleu']:.4f} | {m['error_rate']:.2%} | {m['fatal_integrity']} | "
            f"{m['number_fidelity'] if m['number_fidelity'] is not None else 'n/a'} | "
            f"{m['placeholder_accuracy'] if m['placeholder_accuracy'] is not None else 'n/a'} | "
            f"{m['url_accuracy'] if m['url_accuracy'] is not None else 'n/a'} | "
            f"{m['negation_accuracy'] if m['negation_accuracy'] is not None else 'n/a'} | "
            f"{m['target_language_accuracy'] if m['target_language_accuracy'] is not None else 'n/a'} |"
        )

    lines += ["", "### 分类 chrF", "", "| 类别 | " + " | ".join(c["runs"].keys()) + " |", "|---|" + "---:|" * len(c["runs"])]
    categories = sorted({k for m in c["runs"].values() for k in m["per_category"]})
    for cat in categories:
        lines.append(f"| {cat} | " + " | ".join(str(m["per_category"].get(cat, "n/a")) for m in c["runs"].values()) + " |")

    lines += ["", "### 相对基线变化", ""]
    for name, diff in c.get("diffs", {}).items():
        lines.append(
            f"- **{name}**：chrF {diff['chrf_delta']:+.4f}，BLEU {diff['bleu_delta']:+.4f}，"
            f"错误 {diff['error_delta']:+d}，新增致命 {len(diff['new_fatal'])}，"
            f"回归样本 {diff['regression_count']}，改善样本 {diff['improvement_count']}"
        )
    if args.terms_output and Path(args.terms_output).exists():
        violations = []
        for line in Path(args.terms_output).read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            term = (row.get("diagnostics") or {}).get("term")
            for v in (term or {}).get("violations") or []:
                violations.append((row["id"], row["source"], row.get("output") or "", v))
        lines += ["", "## 3. 术语约束（Warn 诊断；Enforce 配置会实际改写）", ""]
        lines.append(f"发现 **{len(violations)}** 处术语不一致：")
        lines.append("")
        lines.append("| ID | 源文 | 译文（Warn） | 违规 |")
        lines.append("|---|---|---|---|")
        for gid, source, output, violation in violations:
            lines.append(f"| {gid} | {source} | {output} | {violation} |")

    lines += [
        "",
        "## 4. 口径与限制",
        "",
        "- chrF/BLEU 只衡量与单一参考译文的接近程度，不等同于翻译正确率；不得单独作为验收结论。",
        "- 数字/占位符/URL/否定是确定性检查；否定与目标语言仍是启发式，可能误报。",
        "- 人工评审必须按 `tests/quality/reports/human-review-sample.csv` 填写；当前为 PENDING。",
        "",
    ]
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(lines), encoding="utf-8")
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
