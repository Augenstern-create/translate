#!/usr/bin/env python3
"""Verify the frozen E0 baseline metadata without asserting translation quality."""
from __future__ import annotations
import argparse, hashlib, json, subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parents[2]

def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--lock", default="fixtures/research/baseline-lock.json")
    ap.add_argument("--out", default="tests/quality/reports/e0-baseline-verification.json")
    args = ap.parse_args()
    lock = json.loads((ROOT / args.lock).read_text(encoding="utf-8"))
    errors = []
    checked = {}
    groups = {"model": lock.get("model_hashes", {}), "resource": lock.get("resource_hashes", {})}
    mapping = {
        "encoder_model_quantized.onnx": ".build/model-cache/ort-model-linux/encoder_model_quantized.onnx",
        "decoder_model_merged_quantized.onnx": ".build/model-cache/ort-model-linux/decoder_model_merged_quantized.onnx",
        "tokenizer.json": ".build/model-cache/ort-model-linux/tokenizer.json",
        "config.json": ".build/model-cache/ort-model-linux/config.json",
        "dictionary_entries.sqlite": "resources/en-zh-ecdict/entries.sqlite",
        "dictionary_index.fst": "resources/en-zh-ecdict/index.fst",
        "quality_rules": "fixtures/rules/en-zh.quality.json",
        "terms": "fixtures/terms/en-zh.technical.json",
        "templates": "fixtures/templates/en-zh.core.json",
        "composition": "fixtures/rules/en-zh.composition.json",
        "golden": "fixtures/quality/golden-v1.jsonl",
    }
    for group, hashes in groups.items():
        for name, expected in hashes.items():
            path = ROOT / mapping.get(name, "")
            if not path.exists():
                errors.append(f"{group}.{name}: missing file")
                checked[f"{group}.{name}"] = None
            else:
                actual = sha(path)
                checked[f"{group}.{name}"] = actual
                if actual != expected:
                    errors.append(f"{group}.{name}: hash mismatch")
    try:
        head = subprocess.check_output(["git", "-C", str(ROOT.parent), "rev-parse", "HEAD"], text=True).strip()
    except Exception as exc:  # pragma: no cover
        head = f"UNKNOWN: {exc}"
    dirty = subprocess.run(["git", "-C", str(ROOT.parent), "diff", "--quiet"]).returncode != 0
    report = {
        "ok": not errors,
        "errors": errors,
        "checked_hashes": checked,
        "source_commit_in_lock": lock.get("source_commit"),
        "current_git_head": head,
        "working_tree_dirty": dirty,
        "quality_status": "NOT_EVALUATED",
        "note": "Hash/config verification only. Does not establish translation quality or human review.",
    }
    out = ROOT / args.out
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if not errors else 1

if __name__ == "__main__":
    raise SystemExit(main())
