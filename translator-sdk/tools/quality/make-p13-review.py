#!/usr/bin/env python3
"""P13-Q4：生成重点人工评审包（Critical/语义/段落/文档）。

不自动打分；所有评审列留空，review_status=PENDING_HUMAN。
"""
from __future__ import annotations
import csv, json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
Q = ROOT / "tests/quality"

def load(path):
    return {json.loads(l)["id"]: json.loads(l) for l in Path(path).read_text(encoding="utf-8").splitlines() if l.strip()}

def golden(path):
    return {json.loads(l)["id"]: json.loads(l) for l in Path(path).read_text(encoding="utf-8").splitlines() if l.strip()}

def add(rows, kind, gid, g, out):
    q = (out or {}).get("quality") or {}
    flags = []
    if (out or {}).get("error"):
        flags.append("error:" + (out["error"].get("code") or ""))
    for issue in q.get("issues") or []:
        flags.append(f'{issue["kind"]}:{issue["severity"]}')
    rows.append({
        "id": gid, "kind": kind,
        "source": g.get("source", ""),
        "reference": g.get("reference", ""),
        "output": (out or {}).get("output") or "",
        "auto_flags": ";".join(flags),
        "semantic_accuracy_1_5": "", "completeness_1_5": "",
        "fluency_1_5": "", "terminology_1_5": "",
        "context_1_5": "", "format_1_5": "",
        "worst_error": "", "reviewer_note": "",
        "review_status": "PENDING_HUMAN",
    })

def main() -> int:
    rows = []
    crit_g = golden(ROOT / "fixtures/quality/critical-fixes.jsonl")
    crit_o = load(Q / "outputs/critical-fixes-final.jsonl")
    for gid, g in crit_g.items():
        add(rows, "critical", gid, g, crit_o.get(gid))

    sent_g = golden(ROOT / "fixtures/quality/sentence-v1.jsonl")
    sent_o = load(Q / "outputs/sentence-v1-int8-p13.jsonl")
    # 优先自动异常 + 按译文/参考长度比取 20 条（不做语义评分）
    ranked = sorted(
        sent_g,
        key=lambda i: len(sent_o.get(i, {}).get("output") or "")
        / max(1, len(sent_g[i]["reference"])),
    )
    selected = []
    for gid, out in sent_o.items():
        q = out.get("quality") or {}
        if out.get("error") or q.get("issues"):
            selected.append(gid)
    for gid in ranked:
        if gid not in selected:
            selected.append(gid)
    for gid in selected[:20]:
        add(rows, "sentence", gid, sent_g[gid], sent_o.get(gid))

    para_g = golden(ROOT / "fixtures/quality/paragraph-v1.jsonl")
    para_o = load(Q / "outputs/paragraph3-best-final.jsonl")
    for gid in list(para_g)[:10]:
        add(rows, "paragraph", gid, para_g[gid], para_o.get(gid))

    doc_g = golden(ROOT / "fixtures/quality/document-v1.jsonl")
    doc_o = load(Q / "outputs/document-v1-v4.jsonl")
    for gid, g in doc_g.items():
        add(rows, "document", gid, g, doc_o.get(gid))

    out = Q / "reports/p13-review-packet.csv"
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", newline="", encoding="utf-8-sig") as fh:
        w = csv.DictWriter(fh, fieldnames=list(rows[0]))
        w.writeheader(); w.writerows(rows)
    print(f"rows={len(rows)} -> {out}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
