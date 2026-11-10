#!/usr/bin/env python3
"""P18-L: 统计日常知识测试集的词典命中率。

输入是 `quality-runner --lookup-input/--lookup-output` 的输出；本工具不自己
连接 FST/SQLite，避免复制一套词典查询逻辑。

命中定义（docs/70 §4）：
  - 正式词典查询返回至少一条词条；
  - 词条至少有一个非空中文释义；
  - 多义词测试项额外要求返回义项总数 >= min_senses；
  - 只存在部分单词、模型译文、学习候选都不算命中。
"""
from __future__ import annotations

import argparse
import json
from collections import Counter, defaultdict
from pathlib import Path


def has_chinese(text: str) -> bool:
    return any("\u4e00" <= c <= "\u9fff" for c in (text or ""))


def sense_text(sense) -> str:
    if isinstance(sense, str):
        return sense
    if isinstance(sense, dict):
        return sense.get("definition", "") or ""
    return ""


def entry_senses(entry: dict) -> list[str]:
    return [sense_text(s) for s in (entry.get("senses") or [])]


def load_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--lookup-output", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--label", default="daily")
    args = ap.parse_args()

    dataset = load_jsonl(Path(args.dataset))
    lookup = {row.get("word", "").strip().lower(): row for row in load_jsonl(Path(args.lookup_output))}

    category = defaultdict(lambda: {"total": 0, "hit": 0, "exact": 0, "lemma": 0})
    split = defaultdict(lambda: {"total": 0, "hit": 0})
    misses = []
    exact_total = 0
    lemma_total = 0

    for item in dataset:
        unit = item["unit"].strip().lower()
        min_senses = int(item.get("min_senses") or 1)
        row = lookup.get(unit, {})
        entries = [
            e for e in (row.get("entries") or [])
            if any(has_chinese(sense_text(s)) for s in (e.get("senses") or []))
        ]
        senses = sum(len([s for s in entry_senses(e) if has_chinese(s)]) for e in entries)
        # 前缀补全不算命中：exact 模式输出 exact/lemma_hit；没有这些字段时
        # 退回“词头完全相等”判定，绝不把前缀条目当完整短语。
        exact = bool(row.get("exact")) or any(
            (e.get("headword") or "").strip().lower() == unit for e in entries
        )
        lemma = bool(row.get("lemma_hit"))
        hit = bool(entries) and senses >= min_senses and (exact or lemma)

        cat = category[item["category"]]
        cat["total"] += 1
        split[item["split"]]["total"] += 1
        if hit:
            cat["hit"] += 1
            split[item["split"]]["hit"] += 1
            if exact:
                cat["exact"] += 1
                exact_total += 1
            if lemma:
                cat["lemma"] += 1
                lemma_total += 1
        else:
            misses.append({
                "id": item["id"],
                "unit": item["unit"],
                "category": item["category"],
                "split": item["split"],
                "reference": item.get("reference"),
                "min_senses": min_senses,
            })

    total = len(dataset)
    hit = total - len(misses)
    result = {
        "schema": "daily-knowledge-coverage/v1",
        "label": args.label,
        "dataset": args.dataset,
        "total": total,
        "hit": hit,
        "miss": len(misses),
        "coverage": round(hit / total, 4) if total else None,
        "frozen": {
            "total": split["frozen"]["total"],
            "hit": split["frozen"]["hit"],
            "coverage": round(split["frozen"]["hit"] / split["frozen"]["total"], 4)
            if split["frozen"]["total"] else None,
        },
        "construction": {
            "total": split["construction"]["total"],
            "hit": split["construction"]["hit"],
            "coverage": round(split["construction"]["hit"] / split["construction"]["total"], 4)
            if split["construction"]["total"] else None,
        },
        "exact_hits": exact_total,
        "lemma_hits": lemma_total,
        "categories": {
            k: {
                **v,
                "coverage": round(v["hit"] / v["total"], 4) if v["total"] else None,
            }
            for k, v in sorted(category.items())
        },
        "misses": misses,
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    lines = [
        f"# 日常知识库覆盖 — {args.label}",
        "",
        f"- 知识单元：{total}",
        f"- 命中：{hit}（{result['coverage'] * 100:.2f}%）",
        f"- 冻结验收集：{result['frozen']['hit']}/{result['frozen']['total']}（{result['frozen']['coverage'] * 100:.2f}%）",
        f"- 原形/精确命中：{exact_total}；词形回退命中：{lemma_total}",
        f"- 未命中：{len(misses)}",
        "",
        "| 类别 | 总数 | 命中 | 覆盖率 |",
        "|---|---:|---:|---:|",
    ]
    for cat, v in result["categories"].items():
        lines.append(f"| {cat} | {v['total']} | {v['hit']} | {v['coverage'] * 100:.2f}% |")
    lines += ["", "## 未命中示例（最多 50）", ""]
    for miss in misses[:50]:
        lines.append(f"- `{miss['unit']}`（{miss['category']} / {miss['split']}）")
    md = out.with_suffix(".md")
    md.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({k: result[k] for k in ("label", "total", "hit", "coverage", "frozen", "construction", "exact_hits", "lemma_hits")}, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
