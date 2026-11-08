#!/usr/bin/env python3
"""P15-Q2：NLLB-200-distilled-600M 少量候选筛选（不接入 SDK）。"""
import json, os
from pathlib import Path
ROOT = Path(__file__).resolve().parents[2]
os.environ.setdefault("HF_HOME", str(ROOT / ".build/hf-cache"))
import torch
from transformers import AutoTokenizer, AutoModelForSeq2SeqLM
from collections import Counter

def norm(s): return "".join((s or "").lower().split())
def ngrams(s,n): return Counter(s[i:i+n] for i in range(max(0,len(s)-n+1)))
def chrf(h,r,maxn=6,beta=2):
 h,r=norm(h),norm(r)
 if not h or not r: return 0.0
 vals=[]
 for n in range(1,maxn+1):
  a,b=ngrams(h,n),ngrams(r,n); o=sum((a&b).values())
  if not o: vals.append(0.0); continue
  p=o/max(1,sum(a.values())); rec=o/max(1,sum(b.values())); b2=beta*beta
  vals.append((1+b2)*p*rec/(b2*p+rec) if p+rec else 0.0)
 return sum(vals)/len(vals)

def main():
 rows=[json.loads(l) for l in open(ROOT/"fixtures/quality/sentence-v1.jsonl") if l.strip()][:10]
 name="facebook/nllb-200-distilled-600M"
 tok=AutoTokenizer.from_pretrained(name, src_lang="eng_Latn")
 model=AutoModelForSeq2SeqLM.from_pretrained(name); model.eval()
 out=[]
 for i,row in enumerate(rows,1):
  batch=tok(row["source"], return_tensors="pt")
  with torch.no_grad():
   gen=model.generate(**batch, forced_bos_token_id=tok.convert_tokens_to_ids("zho_Hans"), max_new_tokens=128, num_beams=1)
  text=tok.batch_decode(gen, skip_special_tokens=True)[0]
  out.append({"id":row["id"],"source":row["source"],"reference":row["reference"],"output":text,"chrf":round(chrf(text,row["reference"]),4)})
  print(i,row["id"],text)
 path=ROOT/"tests/quality/outputs/sentence-v1-nllb-screen.jsonl"; path.parent.mkdir(parents=True,exist_ok=True)
 path.write_text("\n".join(json.dumps(r,ensure_ascii=False) for r in out)+"\n",encoding="utf-8")
 print("mean chrF",sum(r["chrf"] for r in out)/len(out))

if __name__=="__main__": main()
