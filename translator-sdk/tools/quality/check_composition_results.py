#!/usr/bin/env python3
"""P19-G：独立组合测试结果校验（正例必须 rule + 精确译文；反例必须回退模型）。"""
from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path


def load(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", default="fixtures/quality/composition-v1.jsonl")
    ap.add_argument("--results", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    cases = load(Path(args.cases))
    results = {row.get("id"): row for row in load(Path(args.results))}
    failures = []
    positive = Counter()
    negative = Counter()

    for case in cases:
        cid = case["id"]
        row = results.get(cid)
        if row is None:
            failures.append({"id": cid, "reason": "missing_result"})
            continue
        route = row.get("route")
        output = row.get("output") or ""
        expect_rule = bool(case.get("expect_rule"))
        if expect_rule:
            positive["total"] += 1
            if route == "rule" and output == case.get("reference"):
                positive["pass"] += 1
            else:
                failures.append({
                    "id": cid,
                    "reason": "positive_mismatch",
                    "expected": case.get("reference"),
                    "actual": output,
                    "route": route,
                })
        else:
            negative["total"] += 1
            if route != "rule" and row.get("model_invoked"):
                negative["pass"] += 1
            else:
                failures.append({
                    "id": cid,
                    "reason": "negative_did_not_fallback",
                    "route": route,
                    "actual": output,
                })

    summary = {
        "schema": "p19-composition-validation/v1",
        "cases": len(cases),
        "positive": dict(positive),
        "negative": dict(negative),
        "failures": failures,
        "pass": not failures,
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    md = out.with_suffix(".md")
    lines = [
        "# P19-G 独立组合测试",
        "",
        f"- 正例：{positive['pass']}/{positive['total']}",
        f"- 反例回退：{negative['pass']}/{negative['total']}",
        f"- 失败：{len(failures)}",
        "",
    ]
    if failures:
        lines += ["## 失败", ""]
        for f in failures:
            lines.append(f"- `{f['id']}`：{f['reason']} {f.get('expected', '')} -> {f.get('actual', '')}")
    md.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({"positive": dict(positive), "negative": dict(negative), "failures": len(failures)}, ensure_ascii=False))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
