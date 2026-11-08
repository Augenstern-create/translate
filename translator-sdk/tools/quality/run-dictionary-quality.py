#!/usr/bin/env python3
"""T1/T5：英中词典覆盖率、多义词、短语质量测量。

调用 `quality-runner --lookup-input` 批量查词（走真实 FST + SQLite 查询路径），
不直接读 SQLite，避免“测的不是线上实现”。

用法：
    python3 tools/quality/run-dictionary-quality.py \
        --runner target/release/quality-runner \
        --dict resources/en-zh-ecdict \
        --out tests/quality/reports/dictionary.json

不联网、不写工作区外路径。
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_jsonl(path: Path) -> list[dict]:
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line:
            rows.append(json.loads(line))
    return rows


def exact_entries(result: dict, word: str) -> list[dict]:
    target = word.strip().lower()
    out = []
    for entry in result.get("entries", []):
        head = (entry.get("headword") or "").strip().lower()
        normalized = (entry.get("normalized") or "").strip().lower()
        if head == target or normalized == target:
            out.append(entry)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runner", default=str(ROOT / "target/release/quality-runner"))
    ap.add_argument("--dict", action="append", default=[])
    ap.add_argument("--coverage", default=str(ROOT / "fixtures/quality/google-10000-english-usa.txt"))
    ap.add_argument("--ambiguous", default=str(ROOT / "fixtures/quality/ambiguous.json"))
    ap.add_argument("--phrases", default=str(ROOT / "fixtures/quality/phrases.json"))
    ap.add_argument("--supplement", default=str(ROOT / "fixtures/dictionary/en-zh.supplement.jsonl"))
    ap.add_argument("--out", default=str(ROOT / "tests/quality/reports/dictionary.json"))
    ap.add_argument("--lookup-limit", type=int, default=12)
    ap.add_argument("--keep-words", default=None, help="保留查询词表（默认写到输出同目录）")
    args = ap.parse_args()

    runner = Path(args.runner)
    if not runner.exists():
        print(f"找不到 runner：{runner}\n请先 cargo build -p quality-runner --release", file=sys.stderr)
        return 2
    dicts = [Path(d) for d in args.dict]
    if not dicts:
        print("至少需要一个 --dict", file=sys.stderr)
        return 2

    coverage = [
        line.strip()
        for line in Path(args.coverage).read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.startswith("#")
    ]
    ambiguous = json.loads(Path(args.ambiguous).read_text(encoding="utf-8"))["words"]
    phrases = json.loads(Path(args.phrases).read_text(encoding="utf-8"))["phrases"]
    supplement = load_jsonl(Path(args.supplement))

    words: list[str] = []
    seen: set[str] = set()

    def push(word: str) -> None:
        word = word.strip()
        if word and word.lower() not in seen:
            seen.add(word.lower())
            words.append(word)

    for word in coverage:
        push(word)
    for item in ambiguous:
        push(item["word"])
    for item in phrases:
        push(item["phrase"])
    for item in supplement:
        push(item["headword"])

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    word_path = Path(args.keep_words) if args.keep_words else out_path.with_name("lookup-words.txt")
    word_path.write_text("\n".join(words) + "\n", encoding="utf-8")
    lookup_out = out_path.with_name("lookup-results.jsonl")

    cmd = [
        str(runner),
        "--lookup-input",
        str(word_path),
        "--lookup-output",
        str(lookup_out),
        "--lookup-limit",
        str(args.lookup_limit),
    ]
    for d in dicts:
        cmd.extend(["--dict", str(d)])
    started = time.time()
    proc = subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT)
    elapsed = time.time() - started
    if proc.returncode != 0:
        print(proc.stdout, file=sys.stderr)
        print(proc.stderr, file=sys.stderr)
        return proc.returncode

    results = {row["word"].strip().lower(): row for row in load_jsonl(lookup_out)}

    found_cov: list[str] = []
    missing_cov: list[str] = []
    sense_counts: list[int] = []
    for word in coverage:
        row = results.get(word.lower())
        entries = exact_entries(row, word) if row else []
        if entries:
            found_cov.append(word)
            sense_counts.append(sum(len(e.get("senses") or []) for e in entries))
        else:
            missing_cov.append(word)

    ambiguity = []
    for item in ambiguous:
        row = results.get(item["word"].lower())
        entries = exact_entries(row, item["word"]) if row else []
        senses = [s for e in entries for s in (e.get("senses") or [])]
        groups = []
        ok = bool(entries) and len(senses) >= item.get("min_senses", 1)
        for group in item.get("required_groups", []):
            hit = any(any(term in sense for term in group) for sense in senses)
            groups.append({"terms": group, "hit": hit})
            ok = ok and hit
        ambiguity.append(
            {
                "word": item["word"],
                "min_senses": item.get("min_senses", 1),
                "senses": senses,
                "sense_count": len(senses),
                "groups": groups,
                "pass": ok,
            }
        )

    phrase_rows = []
    for item in phrases:
        row = results.get(item["phrase"].lower())
        entries = exact_entries(row, item["phrase"]) if row else []
        senses = [s for e in entries for s in (e.get("senses") or [])]
        hit = any(item["expected"] in sense for sense in senses)
        phrase_rows.append(
            {
                "phrase": item["phrase"],
                "expected": item["expected"],
                "senses": senses,
                "pass": hit,
            }
        )

    supplement_missing = []
    supplement_pass = 0
    for item in supplement:
        row = results.get(item["headword"].lower())
        entries = exact_entries(row, item["headword"]) if row else []
        senses = [s for e in entries for s in (e.get("senses") or [])]
        expected = item["definitions"][0]
        if any(expected in sense for sense in senses):
            supplement_pass += 1
        else:
            supplement_missing.append({"headword": item["headword"], "expected": expected, "senses": senses})

    report = {
        "schema": "dictionary-quality/v1",
        "dictionaries": [str(d) for d in dicts],
        "runner": str(runner),
        "lookup_words": len(words),
        "lookup_results": len(results),
        "elapsed_seconds": round(elapsed, 2),
        "coverage": {
            "target_total": len(coverage),
            "found": len(found_cov),
            "missing": len(missing_cov),
            "rate": round(len(found_cov) / len(coverage), 4) if coverage else None,
            "missing_words": missing_cov[:200],
            "avg_senses_found": round(sum(sense_counts) / len(sense_counts), 2) if sense_counts else 0,
            "multi_sense_found": sum(1 for n in sense_counts if n >= 2),
        },
        "ambiguity": {
            "total": len(ambiguity),
            "passed": sum(1 for a in ambiguity if a["pass"]),
            "items": ambiguity,
        },
        "phrases": {
            "total": len(phrase_rows),
            "passed": sum(1 for p in phrase_rows if p["pass"]),
            "items": phrase_rows,
        },
        "supplement": {
            "total": len(supplement),
            "passed": supplement_pass,
            "missing": supplement_missing,
        },
        "review_status": "PENDING_HUMAN",
    }

    out_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "coverage": report["coverage"]["rate"],
        "coverage_found": f'{report["coverage"]["found"]}/{report["coverage"]["target_total"]}',
        "ambiguity": f'{report["ambiguity"]["passed"]}/{report["ambiguity"]["total"]}',
        "phrases": f'{report["phrases"]["passed"]}/{report["phrases"]["total"]}',
        "supplement": f'{report["supplement"]["passed"]}/{report["supplement"]["total"]}',
        "elapsed_seconds": report["elapsed_seconds"],
        "out": str(out_path),
    }, ensure_ascii=False, indent=2))
    return 0 if report["coverage"]["rate"] and report["coverage"]["rate"] >= 0.95 else 1


if __name__ == "__main__":
    raise SystemExit(main())
