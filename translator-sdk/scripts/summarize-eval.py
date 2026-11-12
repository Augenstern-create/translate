#!/usr/bin/env python3
"""把 quality-runner 的 JSON 报告汇总成 Markdown（只读，不重新跑评测）。

用法：
    python3 scripts/summarize-eval.py /tmp/bench-test-deepseek.json /tmp/bench-test-glm.json

输出：Markdown 表格 + 失败用例清单（按类别分组）+ 指标与费用口径说明。
"""

import json
import sys
from collections import Counter, defaultdict


def load(path):
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def pct(value):
    return f"{value:.2f}"


def main(paths):
    reports = [load(p) for p in paths]
    out = ["## 评测汇总", ""]
    out.append("| 报告 | 通过 | chrF++ | BLEU-4 | p50 | p95 | max | HTTP 调用 | 输入/输出 token | 估算费用 |")
    out.append("|---|---|---|---|---|---|---|---|---|---|")
    for path, report in zip(paths, reports):
        m = report.get("metrics") or {}
        cost = report.get("estimated_cost_usd")
        out.append(
            "| {} | {}/{} | {} | {} | {} ms | {} ms | {} ms | {} | {}/{} | {} |".format(
                path.split("/")[-1],
                report["passed"],
                report["total"],
                pct(m.get("chrf_pp", 0.0)) if m else "—",
                pct(m.get("bleu4", 0.0)) if m else "—",
                report["latency_p50_ms"],
                report["latency_p95_ms"],
                report["latency_max_ms"],
                report["http_calls"],
                report["tokens_input"],
                report["tokens_output"],
                "未配置单价" if cost is None else f"${cost:.6f}",
            )
        )

    out += ["", "### 按类别", ""]
    # 每个报告一列，避免把不同模型的"通过数"加在一起产生误导
    names = [path.split("/")[-1].replace(".json", "") for path in paths]
    categories = defaultdict(lambda: defaultdict(lambda: (0, 0)))
    for name, report in zip(names, reports):
        for item in report.get("by_category", []):
            categories[item["category"]][name] = (item["passed"], item["failed"])
    header = "| 类别 | " + " | ".join(f"{n}（通过/失败）" for n in names) + " |"
    out.append(header)
    out.append("|" + "---|" * (len(names) + 1))
    for category in sorted(categories):
        cells = [f"{p}/{f}" for p, f in (categories[category][n] for n in names)]
        out.append(f"| {category} | " + " | ".join(cells) + " |")

    for path, report in zip(paths, reports):
        failures = [c for c in report.get("cases", []) if not c["ok"]]
        out += ["", f"### 失败用例（{path.split('/')[-1]}，共 {len(failures)} 条）", ""]
        if not failures:
            out.append("无。")
            continue
        counts = Counter(c["category"] for c in failures)
        out.append("| 类别 | 失败数 |")
        out.append("|---|---|")
        for name, count in counts.most_common():
            out.append(f"| {name} | {count} |")
        out += ["", "前 20 条：", ""]
        out.append("| id | 类别 | 失败原因 | 译文（截断） |")
        out.append("|---|---|---|---|")
        for case in failures[:20]:
            reasons = "；".join(case["failures"])[:120]
            output = (case.get("output") or "").replace("|", "\\|")[:60]
            out.append(f"| {case['id']} | {case['category']} | {reasons} | {output} |")

    out += ["", "> 人工评审状态：**PENDING_HUMAN**。自动指标与规则检查不替代人工验收。"]
    print("\n".join(out))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    main(sys.argv[1:])
