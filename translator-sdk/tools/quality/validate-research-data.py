#!/usr/bin/env python3
"""Validate E0 research data separation and integrity."""
from __future__ import annotations
import argparse, hashlib, json
from pathlib import Path
ROOT = Path(__file__).resolve().parents[2]

def rows(path):
    out=[]
    for n,line in enumerate(path.read_text(encoding='utf-8').splitlines(),1):
        line=line.strip()
        if line and not line.startswith('#'):
            out.append((n,json.loads(line)))
    return out

def main() -> int:
    ap=argparse.ArgumentParser()
    ap.add_argument('--lock', default='fixtures/research/research-data-lock.json')
    args=ap.parse_args()
    lock=json.loads((ROOT/args.lock).read_text(encoding='utf-8'))
    errors=[]
    ids=set()
    for key in ('development','diagnostic','diagnostic_probes','frozen'):
        item=lock.get(key) or {}
        path=item.get('path')
        if not path:
            errors.append(f'{key}: missing path')
            continue
        p=ROOT/path
        if not p.exists():
            errors.append(f'{key}: missing {path}')
            continue
        actual=hashlib.sha256(p.read_bytes()).hexdigest()
        if item.get('sha256') and actual != item['sha256']:
            errors.append(f'{key}: hash mismatch')
        data=rows(p)
        for n,row in data:
            if row.get('id') in ids:
                errors.append(f'{key}:{n}: duplicate id {row.get("id")}')
            ids.add(row.get('id'))
            if row.get('split') and row.get('split') not in ('development','diagnostic','diagnostic_probe','frozen'):
                errors.append(f'{key}:{n}: bad split')
            if key=='frozen' and not row.get('references'):
                errors.append(f'{key}:{n}: frozen row missing reference')
    print(json.dumps({'ok': not errors, 'errors': errors, 'unique_ids': len(ids)}, ensure_ascii=False, indent=2))
    return 0 if not errors else 1

if __name__ == '__main__':
    raise SystemExit(main())
