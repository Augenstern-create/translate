#!/usr/bin/env python3
"""E0 research audit: run an existing quality-runner binary on a research JSONL.

This tool intentionally does not implement a second translation stack. It calls the
quality-runner, joins its per-unit records back to the research dataset, and emits
sample-level evidence for human review. It does not invent references or quality
labels; unreviewed rows remain PENDING_HUMAN.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read_jsonl(path: Path):
    rows = []
    with path.open(encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line and not line.startswith("#"):
                rows.append(json.loads(line))
    return rows


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", default="fixtures/research/diagnostic-v1.jsonl")
    ap.add_argument("--output", default="tests/quality/research/diagnostic-v1.raw.jsonl")
    ap.add_argument("--report", default="tests/quality/reports/research-diagnostic-v1.json")
    ap.add_argument("--runner", default="./target/release/quality-runner")
    ap.add_argument("--label", default="research-diagnostic-v1")
    ap.add_argument("--model-dir", default=".build/model-cache/ort-model-linux")
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    dataset_path = ROOT / args.dataset
    raw_path = ROOT / args.output
    report_path = ROOT / args.report
    raw_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.parent.mkdir(parents=True, exist_ok=True)
    dataset = read_jsonl(dataset_path)
    if args.limit:
        dataset = dataset[: args.limit]

    # quality-runner reads source only; references and labels stay in the research set.
    runner_input = raw_path.with_suffix(".input.jsonl")
    with runner_input.open("w", encoding="utf-8") as fh:
        for row in dataset:
            fh.write(json.dumps({"id": row["id"], "source": row["source"]}, ensure_ascii=False) + "\n")

    cmd = [
        args.runner,
        "--input", str(runner_input),
        "--output", str(raw_path),
        "--label", args.label,
        "--mode", "precise",
        "--policy", "auto",
        "--text-kind", "sentence",
        "--source", "en",
        "--target", "zh-Hans",
        "--strict-integrity", "false",
        "--term-policy", "enforce",
        "--dict", "resources/en-zh-ecdict",
        "--rules", "fixtures/rules/en-zh.quality.json",
        "--terms", "fixtures/terms/en-zh.technical.json",
        "--templates", "fixtures/templates/en-zh.core.json",
        "--composition", "fixtures/rules/en-zh.composition.json",
        "--semantic-composition", "fixtures/rules/en-zh.composition.json",
        "--ort-model-dir", args.model_dir,
    ]
    subprocess.run(cmd, cwd=ROOT, check=True)

    raw = {row["id"]: row for row in read_jsonl(raw_path)}
    rows = []
    category_route = defaultdict(Counter)
    route_counts = Counter()
    status_counts = Counter()
    for item in dataset:
        rec = raw.get(item["id"], {})
        route = rec.get("route") or "unknown"
        engine = rec.get("engine_id") or "unknown"
        diag = rec.get("diagnostics") or {}
        units = diag.get("units") or []
        route_reason = diag.get("route_reason") or rec.get("fallback_reason")
        review = item.get("human_review_status", "pending")
        row = {
            "id": item["id"],
            "source": item["source"],
            "category": item.get("category"),
            "references": item.get("references", []),
            "semantic_interpretations": item.get("semantic_interpretations", []),
            "ambiguity_expected": item.get("ambiguity_expected", False),
            "translation": rec.get("output"),
            "route": route,
            "route_reason": route_reason,
            "engine_id": engine,
            "model_invoked": rec.get("model_invoked"),
            "unit_count": len(units),
            "units": units,
            "fatal": bool((rec.get("quality") or {}).get("fatal")),
            "quality_issues": (rec.get("quality") or {}).get("issues", []),
            "human_review_status": review,
            "error_labels": [],
            "error_labels_note": "No automatic semantic error label is asserted; human review required.",
        }
        rows.append(row)
        route_counts[route] += 1
        category_route[item.get("category", "unknown")][route] += 1
        status_counts[review] += 1

    report = {
        "dataset": str(dataset_path.relative_to(ROOT)),
        "raw_output": str(raw_path.relative_to(ROOT)),
        "samples": len(rows),
        "route_counts": dict(route_counts),
        "category_route": {k: dict(v) for k, v in sorted(category_route.items())},
        "review_status_counts": dict(status_counts),
        "quality_status": "PENDING_HUMAN",
        "note": "Router/engineering evidence only. No semantic quality PASS and no invented references.",
        "items": rows,
    }
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: report[k] for k in ("samples", "route_counts", "category_route", "quality_status")}, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
