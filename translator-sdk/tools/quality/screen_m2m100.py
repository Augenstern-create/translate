#!/usr/bin/env python3
"""P15-Q2：不同架构候选筛选（M2M100 418M，少量真实译文）。

只做候选筛选，不接入 SDK。运行：
    HF_HOME=.build/hf-cache .build/pyenv/bin/python tools/quality/screen_m2m100.py
"""
import json, os, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
os.environ.setdefault("HF_HOME", str(ROOT / ".build/hf-cache"))

import torch
from transformers import M2M100ForConditionalGeneration, M2M100Tokenizer


def norm(s):
    return "".join((s or "").lower().split())


def ngrams(s, n):
    from collections import Counter
    return Counter(s[i:i+n] for i in range(max(0, len(s)-n+1)))


def chrf(h, r, maxn=6, beta=2):
    h, r = norm(h), norm(r)
    if not h or not r:
        return 0.0
    vals = []
    for n in range(1, maxn+1):
        a, b = ngrams(h, n), ngrams(r, n)
        o = sum((a & b).values())
        if not o:
            vals.append(0.0); continue
        p = o / max(1, sum(a.values())); rec = o / max(1, sum(b.values()))
        b2 = beta * beta
        vals.append((1+b2)*p*rec/(b2*p+rec) if p+rec else 0.0)
    return sum(vals)/len(vals)


def main():
    rows = [json.loads(l) for l in open(ROOT / "fixtures/quality/sentence-v1.jsonl") if l.strip()][:20]
    name = "facebook/m2m100_418M"
    tok = M2M100Tokenizer.from_pretrained(name)
    model = M2M100ForConditionalGeneration.from_pretrained(name)
    model.eval()
    tok.src_lang = "en"
    out = []
    for i, row in enumerate(rows, 1):
        batch = tok(row["source"], return_tensors="pt", padding=True)
        with torch.no_grad():
            gen = model.generate(**batch, forced_bos_token_id=tok.get_lang_id("zh"), max_new_tokens=128, num_beams=1)
        text = tok.batch_decode(gen, skip_special_tokens=True)[0]
        out.append({"id": row["id"], "source": row["source"], "reference": row["reference"], "output": text, "chrf": round(chrf(text, row["reference"]), 4)})
        print(i, row["id"], text)
    path = ROOT / "tests/quality/outputs/sentence-v1-m2m100-screen.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in out) + "\n", encoding="utf-8")
    print("mean chrF", sum(r["chrf"] for r in out)/len(out))


if __name__ == "__main__":
    main()
