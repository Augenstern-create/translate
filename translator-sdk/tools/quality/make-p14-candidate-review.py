#!/usr/bin/env python3
"""P14-Q1：盲评 Best 候选（A/B 隐藏策略）。

生成 CSV 与密钥；评审人只看到 A/B 译文，不看到“整段/逐句”标签。
"""
from __future__ import annotations
import csv, json, random
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
Q = ROOT / "tests/quality"

def load(path):
    return {json.loads(l)["id"]: json.loads(l) for l in Path(path).read_text(encoding="utf-8").splitlines() if l.strip()}

def main():
    golden = load(ROOT / "fixtures/quality/paragraph-v1.jsonl")
    whole = load(Q / "outputs/paragraph3-whole.jsonl")
    sentence = load(Q / "outputs/paragraph3-sentence.jsonl")
    best = load(Q / "outputs/paragraph3-best-final.jsonl")
    rng = random.Random(20261108)
    rows, key = [], {}
    for gid, g in golden.items():
        if rng.random() < 0.5:
            a, b, a_side = whole[gid]["output"], sentence[gid]["output"], "whole"
        else:
            a, b, a_side = sentence[gid]["output"], whole[gid]["output"], "sentence"
        b_side = "sentence" if a_side == "whole" else "whole"
        best_text = best[gid]["output"]
        chosen = "whole" if "best=整段" in best[gid]["diagnostics"]["route_reason"] else "sentence"
        rows.append({
            "id": gid, "source": g["source"], "reference": g["reference"],
            "candidate_A": a, "candidate_B": b,
            "A_semantic_1_5": "", "B_semantic_1_5": "",
            "A_completeness_1_5": "", "B_completeness_1_5": "",
            "A_fluency_1_5": "", "B_fluency_1_5": "",
            "A_terminology_1_5": "", "B_terminology_1_5": "",
            "which_is_better_A_B_tie": "", "best_selected_agrees_yes_no": "",
            "reviewer_note": "", "review_status": "PENDING_HUMAN",
        })
        key[gid] = {"A_side": a_side, "B_side": b_side, "best_choice": chosen, "best_text": best_text}
    out = Q / "reports/p14-candidate-review.csv"
    with out.open("w", newline="", encoding="utf-8-sig") as fh:
        w = csv.DictWriter(fh, fieldnames=list(rows[0])); w.writeheader(); w.writerows(rows)
    (Q / "reports/p14-candidate-key.json").write_text(json.dumps(key, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"rows={len(rows)} -> {out}")

if __name__ == "__main__":
    main()
