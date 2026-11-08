#!/usr/bin/env python3
"""P14-Q2：用参考译文审计 Best 选择器（离线，不参与运行时）。

回答两个问题：
1. Best 选中的候选是否真的比另一个候选更接近参考译文（selector accuracy）？
2. 选错时平均损失多少（regret）？哪些样本选错？

注意：参考译文用于**审计**，选择器运行时不得读取参考。
"""
from __future__ import annotations
import argparse, json, re
from collections import Counter
from pathlib import Path


def norm(s: str) -> str:
    return "".join((s or "").lower().split())


def ngrams(s: str, n: int) -> Counter:
    return Counter(s[i : i + n] for i in range(max(0, len(s) - n + 1)))


def chrf(hyp: str, ref: str, max_n: int = 6, beta: float = 2.0) -> float:
    hyp, ref = norm(hyp), norm(ref)
    if not hyp or not ref:
        return 0.0
    scores = []
    for n in range(1, max_n + 1):
        h, r = ngrams(hyp, n), ngrams(ref, n)
        overlap = sum((h & r).values())
        if overlap == 0:
            scores.append(0.0)
            continue
        p = overlap / max(1, sum(h.values()))
        rec = overlap / max(1, sum(r.values()))
        b2 = beta * beta
        scores.append((1 + b2) * p * rec / (b2 * p + rec) if p + rec else 0.0)
    return sum(scores) / len(scores)


def load(path: Path) -> dict[str, dict]:
    return {json.loads(l)["id"]: json.loads(l) for l in path.read_text(encoding="utf-8").splitlines() if l.strip()}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--golden", default="fixtures/quality/paragraph-v1.jsonl")
    ap.add_argument("--whole", default="tests/quality/outputs/paragraph3-whole.jsonl")
    ap.add_argument("--sentence", default="tests/quality/outputs/paragraph3-sentence.jsonl")
    ap.add_argument("--best", default="tests/quality/outputs/paragraph3-best-final.jsonl")
    ap.add_argument("--out", default="tests/quality/reports/compare-selector")
    args = ap.parse_args()

    golden = load(Path(args.golden))
    whole = load(Path(args.whole))
    sentence = load(Path(args.sentence))
    best = load(Path(args.best))

    rows = []
    correct = 0
    regret_total = 0.0
    chosen_counts = Counter()
    for gid, g in golden.items():
        cw = chrf(whole[gid]["output"], g["reference"])
        cs = chrf(sentence[gid]["output"], g["reference"])
        cb = chrf(best[gid]["output"], g["reference"])
        route = best[gid]["diagnostics"]["route_reason"]
        if "best=整段" in route:
            chosen = "whole"
        elif "best=逐句" in route:
            chosen = "sentence"
        else:
            chosen = "other"
        chosen_counts[chosen] += 1
        oracle = "whole" if cw >= cs else "sentence"
        best_score = max(cw, cs)
        regret = max(0.0, best_score - cb)
        regret_total += regret
        if chosen == oracle:
            correct += 1
        rows.append(
            {
                "id": gid,
                "chosen": chosen,
                "oracle": oracle,
                "whole_chrf": round(cw, 4),
                "sentence_chrf": round(cs, 4),
                "best_chrf": round(cb, 4),
                "regret": round(regret, 4),
                "source": g["source"],
                "reference": g["reference"],
            }
        )
    wrong = sorted([r for r in rows if r["chosen"] != r["oracle"]], key=lambda r: -r["regret"])
    report = {
        "schema": "candidate-selection-audit/v1",
        "samples": len(rows),
        "selector_accuracy": round(correct / len(rows), 4),
        "mean_regret": round(regret_total / len(rows), 4),
        "chosen_counts": dict(chosen_counts),
        "wrong": wrong,
        "rows": rows,
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.with_suffix(".json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    md = [
        "# Best 候选选择审计（离线参考）",
        "",
        f"- 样本：{len(rows)}",
        f"- **选择器准确率**：{report['selector_accuracy']:.2%}（选中候选 = 两候选中 chrF 更高者）",
        f"- 平均 regret：{report['mean_regret']:.4f} chrF",
        f"- 选择分布：{dict(chosen_counts)}",
        "",
        "## 选错且损失最大的样本",
        "",
        "| ID | 选中 | Oracle | whole | sentence | best | regret |",
        "|---|---|---|---:|---:|---:|---:|",
    ]
    for r in wrong[:20]:
        md.append(
            f"| {r['id']} | {r['chosen']} | {r['oracle']} | {r['whole_chrf']} | "
            f"{r['sentence_chrf']} | {r['best_chrf']} | {r['regret']} |"
        )
    md += ["", "> 参考只用于审计；运行时选择器不读取参考。", ""]
    out.with_suffix(".md").write_text("\n".join(md), encoding="utf-8")
    print(json.dumps({k: report[k] for k in ["samples", "selector_accuracy", "mean_regret", "chosen_counts"]}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
