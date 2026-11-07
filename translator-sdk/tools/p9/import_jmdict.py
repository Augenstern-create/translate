#!/usr/bin/env python3
"""JMdict XML → 规范化词典 JSONL（P9.1 小样本）。

只读取本地 XML；不联网。保留 ent_seq/k_ele/r_ele/sense/POS/gloss。
headword 取首个汉字词形（无汉字则首个假名），首个假名作为 traditional 备用键。
"""
from __future__ import annotations

import argparse
import json
import xml.etree.ElementTree as ET
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("output")
    ap.add_argument("--limit", type=int, default=1000)
    ap.add_argument("--stats", default=None)
    ap.add_argument("--priority", default="食べる,学校,行く,今日,はし,橋,箸,端")
    args = ap.parse_args()

    stats = {
        "source": "jmdict",
        "total_entries": 0,
        "accepted_entries": 0,
        "skipped_no_gloss": 0,
        "skipped_no_headword": 0,
        "output_entries": 0,
    }
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    priority = {w.strip() for w in args.priority.split(",") if w.strip()}
    rows = []
    with open(out, "w", encoding="utf-8") as fh:
        for event, elem in ET.iterparse(args.input, events=("end",)):
            if elem.tag != "entry":
                continue
            stats["total_entries"] += 1
            kebs = [e.text.strip() for e in elem.findall("./k_ele/keb") if e.text and e.text.strip()]
            rebs = [e.text.strip() for e in elem.findall("./r_ele/reb") if e.text and e.text.strip()]
            headword = kebs[0] if kebs else (rebs[0] if rebs else "")
            if not headword:
                stats["skipped_no_headword"] += 1
                elem.clear()
                continue
            defs = []
            pos = []
            for sense in elem.findall("./sense"):
                for p in sense.findall("./pos"):
                    if p.text and p.text.strip() and p.text.strip() not in pos:
                        pos.append(p.text.strip())
                for g in sense.findall("./gloss"):
                    text = (g.text or "").strip()
                    if text and text not in defs:
                        defs.append(text)
            if not defs:
                stats["skipped_no_gloss"] += 1
                elem.clear()
                continue
            row = {
                "headword": headword,
                "traditional": rebs[0] if rebs and rebs[0] != headword else None,
                "pos": ", ".join(pos) if pos else None,
                "definitions": defs,
                "source_entry_id": elem.findtext("./ent_seq"),
            }
            rows.append(row)
            elem.clear()
        # 先保留用户指定的质量样本词，再用其余词条补足上限
        selected = []
        selected_keys = set()
        for row in rows:
            keys = {row["headword"], row.get("traditional") or ""}
            if keys & priority and row["headword"] not in selected_keys:
                selected.append(row)
                selected_keys.add(row["headword"])
        for row in rows:
            if len(selected) >= args.limit:
                break
            if row["headword"] not in selected_keys:
                selected.append(row)
                selected_keys.add(row["headword"])
        for row in selected[: args.limit]:
            fh.write(json.dumps(row, ensure_ascii=False) + "\n")
    stats["accepted_entries"] = len(selected[: args.limit])
    stats["output_entries"] = stats["accepted_entries"]
    if args.stats:
        Path(args.stats).write_text(json.dumps(stats, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(stats, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
