#!/usr/bin/env python3
"""P20-C：语义组合对抗集校验（正例允许多个合法语序，反例必须回退）。"""
from __future__ import annotations
import argparse, json
from pathlib import Path


def rows(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", default="fixtures/quality/semantic-adversarial-v1.jsonl")
    ap.add_argument("--results", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    cases = rows(Path(args.cases))
    results = {r.get("id"): r for r in rows(Path(args.results))}
    failures = []
    pos = {"total": 0, "pass": 0}
    neg = {"total": 0, "pass": 0}
    for case in cases:
        row = results.get(case["id"])
        if row is None:
            failures.append({"id": case["id"], "reason": "missing"})
            continue
        if case["expect_rule"]:
            pos["total"] += 1
            allowed = case.get("allowed") or []
            if row.get("route") == "rule" and row.get("output") in allowed and "semantic:" in (row.get("engine_id") or ""):
                pos["pass"] += 1
            else:
                failures.append({"id": case["id"], "reason": "positive_mismatch", "output": row.get("output"), "route": row.get("route"), "engine": row.get("engine_id"), "allowed": allowed})
        else:
            neg["total"] += 1
            if row.get("route") != "rule" and row.get("model_invoked"):
                neg["pass"] += 1
            else:
                failures.append({"id": case["id"], "reason": "negative_did_not_fallback", "output": row.get("output"), "route": row.get("route")})
    summary = {"schema": "p20-semantic-validation/v1", "positive": pos, "negative": neg, "failures": failures, "pass": not failures}
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    md = out.with_suffix(".md")
    lines = ["# P20-C 语义组合对抗测试", "", f"- 正例：{pos['pass']}/{pos['total']}", f"- 反例回退：{neg['pass']}/{neg['total']}", f"- 失败：{len(failures)}", ""]
    for f in failures:
        lines.append(f"- `{f['id']}`：{f}")
    md.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({"positive": pos, "negative": neg, "failures": len(failures)}, ensure_ascii=False))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
