#!/usr/bin/env python3
"""E0 heuristic error triage over research-audit output.

This is *not* a human quality judgment and not a semantic classifier. It only
surfaces observable symptoms (route, repetition, ASCII punctuation, number/URL
loss, extreme length change) so a human reviewer can assign final error codes.
"""
from __future__ import annotations
import argparse, json, re
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
URL_RE = re.compile(r"https?://[^\s，。；、]+")
NUM_RE = re.compile(r"\d+(?:[.,]\d+)?")
CJK_RE = re.compile(r"[\u4e00-\u9fff]")


def repeated_ngram(text: str, n: int = 2) -> bool:
    chars = [c for c in text if CJK_RE.match(c)]
    if len(chars) < n * 2:
        return False
    grams = Counter("".join(chars[i:i+n]) for i in range(len(chars)-n+1))
    return any(v >= 2 for v in grams.values())


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--audit", default="tests/quality/reports/research-diagnostic-v1.json")
    ap.add_argument("--out", default="tests/quality/reports/research-diagnostic-v1-errors.json")
    args = ap.parse_args()
    audit = json.loads((ROOT / args.audit).read_text(encoding="utf-8"))
    rows = []
    by_category = defaultdict(Counter)
    by_route = Counter()
    for item in audit.get("items", []):
        src = item.get("source") or ""
        out = item.get("translation") or ""
        flags = []
        if NUM_RE.findall(src) and not set(NUM_RE.findall(src)).issubset(set(NUM_RE.findall(out))):
            flags.append("NUMBER_LOSS_SUSPECT")
        if URL_RE.findall(src) and not URL_RE.findall(out):
            flags.append("URL_LOSS_SUSPECT")
        if repeated_ngram(out):
            flags.append("REPETITION_SUSPECT")
        if "," in out and "，" not in out:
            flags.append("ASCII_COMMA")
        if len(src) > 40 and len(out) < max(4, len(src) * 0.25):
            flags.append("LENGTH_COMPRESSION_SUSPECT")
        if not CJK_RE.search(out):
            flags.append("NO_CHINESE")
        if item.get("fatal"):
            flags.append("FATAL")
        row = {
            "id": item.get("id"),
            "category": item.get("category"),
            "route": item.get("route"),
            "source": src,
            "translation": out,
            "heuristic_flags": flags,
            "human_review_status": "PENDING_HUMAN",
            "final_error_codes": [],
            "note": "Heuristic symptom only; final error code requires human review.",
        }
        rows.append(row)
        by_route[item.get("route") or "unknown"] += 1
        for f in flags:
            by_category[item.get("category") or "unknown"][f] += 1
    report = {
        "audit": args.audit,
        "samples": len(rows),
        "route_counts": dict(by_route),
        "flag_by_category": {k: dict(v) for k, v in sorted(by_category.items())},
        "quality_status": "PENDING_HUMAN",
        "rows": rows,
    }
    (ROOT / args.out).write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: report[k] for k in ("samples", "route_counts", "flag_by_category", "quality_status")}, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
