#!/usr/bin/env python3
"""把多次 quality-runner 报告合并成一份"逐句对照"JSONL，便于人工评审与 diff。

输入：
  --dataset <bench/test.jsonl>   评测集（提供 source / reference / 领域 / 文档）
  --run <label>=<report.json>    可重复；label 用作输出字段名（如 ds / glm）

输出（stdout 或 --out）：每行一条
  {"id","dataset_id","domain","document_id","source","reference","<label>": {"output","ok","error","failures","elapsed_ms"}}

设计原则：
- 只做**确定性合并**：不重新翻译、不改写模型输出；
- 缺失/失败的用例保留空输出并带上 error，不能悄悄丢掉；
- 旧报告没有 dataset_id 时按 id 前缀（`wmt24pp-` / `ntrex128-`）推断并与数据集核对。
"""

import argparse
import json
import sys


def load_jsonl(path):
    rows = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            case = json.loads(line)
            rows[case["id"]] = case
    return rows


def load_report(path):
    with open(path, encoding="utf-8") as fh:
        report = json.load(fh)
    return {case["id"]: case for case in report.get("cases", [])}


def infer_dataset(case_id):
    if case_id.startswith("wmt24pp-"):
        return "wmt24pp"
    if case_id.startswith("ntrex128-"):
        return "ntrex128"
    return "handcrafted"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", required=True)
    parser.add_argument("--run", action="append", required=True, metavar="LABEL=REPORT")
    parser.add_argument("--out")
    args = parser.parse_args()

    dataset = load_jsonl(args.dataset)
    runs = []
    for item in args.run:
        label, _, path = item.partition("=")
        if not path:
            raise SystemExit(f"--run 需要 LABEL=REPORT，收到：{item}")
        runs.append((label, load_report(path)))

    ordered = sorted(dataset.values(), key=lambda c: c["id"])
    lines = []
    for case in ordered:
        cid = case["id"]
        record = {
            "id": cid,
            "dataset_id": case.get("dataset_id") or infer_dataset(cid),
            "domain": case.get("domain", ""),
            "document_id": case.get("document_id", ""),
            "source": case["source"],
            "reference": case.get("reference", ""),
        }
        if record["dataset_id"] == "handcrafted":
            record["dataset_id"] = infer_dataset(cid)
        for label, cases in runs:
            item = cases.get(cid)
            if item is None:
                record[label] = {"output": "", "ok": False, "error": "missing", "failures": [], "elapsed_ms": 0}
                continue
            record[label] = {
                "output": item.get("output", ""),
                "ok": item.get("ok", False),
                "error": item.get("error"),
                "failures": item.get("failures", []),
                "elapsed_ms": item.get("elapsed_ms", 0),
            }
        lines.append(json.dumps(record, ensure_ascii=False))

    text = "\n".join(lines) + "\n"
    if args.out:
        with open(args.out, "w", encoding="utf-8") as fh:
            fh.write(text)
        print(f"{len(lines)} 条 → {args.out}", file=sys.stderr)
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
