#!/usr/bin/env python3
"""T3/T5：在固定金样本上批量跑多组配置。

配置（docs/57 §9）：
    baseline   原始 ORT 模型，不挂词典/规则/术语，Precise（仅质量检查）
    fast       原始 ORT 模型，Fast
    rules      原始 ORT 模型 + 规则包（缩写/弯引号/NFKC）
    terms      原始 ORT 模型 + 输出侧术语表
    full       词典 + 规则 + 术语，Auto 路由，Precise
    full-fast  词典 + 规则 + 术语，Auto 路由，Fast

默认 `--strict-integrity false`：质量对照要看**所有**译文与问题，
而不是让关键数字检查提前把样本变成错误。严格模式由单独的契约测试/运行验证。

用法：
    python3 tools/quality/run-translation-golden.py \
        --runner target/release/quality-runner \
        --model-dir .build/model-cache/ort-model-linux \
        --dict resources/en-zh-ecdict \
        --rules fixtures/rules/en-zh.quality.json \
        --terms fixtures/terms/en-zh.technical.json \
        --outdir tests/quality
"""
from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

CONFIGS = {
    "baseline": {"mode": "precise", "policy": "local", "rules": False, "terms": False, "dict": False, "text_kind": "sentence"},
    "fast": {"text_kind": "sentence", "mode": "fast", "policy": "local", "rules": False, "terms": False, "dict": False},
    "rules": {"text_kind": "sentence", "mode": "precise", "policy": "local", "rules": True, "terms": False, "dict": False},
    "terms": {"text_kind": "sentence", "mode": "precise", "policy": "local", "rules": False, "terms": True, "dict": False},
    "full": {"text_kind": "sentence", "mode": "precise", "policy": "auto", "rules": True, "terms": True, "dict": True},
    "full-fast": {"text_kind": "sentence", "mode": "fast", "policy": "auto", "rules": True, "terms": True, "dict": True},
    # 解码上界比较（T3）：只改 max_new_tokens，其它不变。
    "baseline-96": {"text_kind": "sentence", "mode": "precise", "policy": "local", "rules": False, "terms": False, "dict": False, "max_new_tokens": 96},
    "baseline-256": {"text_kind": "sentence", "mode": "precise", "policy": "local", "rules": False, "terms": False, "dict": False, "max_new_tokens": 256},
    # 术语约束从 Warn 改为 Enforce（只改术语策略，其它不变）。
    "terms-enforce": {"text_kind": "sentence", "mode": "precise", "policy": "local", "rules": False, "terms": True, "dict": False, "term_policy": "enforce"},
    "full-enforce": {"text_kind": "sentence", "mode": "precise", "policy": "auto", "rules": True, "terms": True, "dict": True, "term_policy": "enforce"},
    # P12-Q：Auto 四级策略 / 显式句子 / beam 对照
    "p12-auto": {"mode": "precise", "policy": "auto", "rules": True, "terms": True, "dict": True, "text_kind": "auto"},
    "p12-sentence": {"mode": "precise", "policy": "auto", "rules": True, "terms": True, "dict": True, "text_kind": "sentence"},
    "p12-beam2": {"mode": "precise", "policy": "local", "rules": False, "terms": False, "dict": False, "text_kind": "sentence", "beam": 2},
    "p12-beam4": {"mode": "precise", "policy": "local", "rules": False, "terms": False, "dict": False, "text_kind": "sentence", "beam": 4},
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runner", default=str(ROOT / "target/release/quality-runner"))
    ap.add_argument("--golden", default=str(ROOT / "fixtures/quality/golden-v1.jsonl"))
    ap.add_argument("--model-dir", default=str(ROOT / ".build/model-cache/ort-model-linux"))
    ap.add_argument("--dict", default=str(ROOT / "resources/en-zh-ecdict"))
    ap.add_argument("--rules", default=str(ROOT / "fixtures/rules/en-zh.quality.json"))
    ap.add_argument("--terms", default=str(ROOT / "fixtures/terms/en-zh.technical.json"))
    ap.add_argument("--outdir", default=str(ROOT / "tests/quality"))
    ap.add_argument("--configs", default="baseline,fast,rules,terms,full,full-fast")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--strict-integrity", default="false")
    ap.add_argument("--max-new-tokens", type=int, default=0)
    args = ap.parse_args()

    runner = Path(args.runner)
    if not runner.exists():
        print(f"找不到 runner：{runner}", file=sys.stderr)
        return 2
    if not Path(args.model_dir).is_dir():
        print(f"找不到模型目录：{args.model_dir}", file=sys.stderr)
        return 2

    outdir = Path(args.outdir)
    outputs = outdir / "outputs"
    reports = outdir / "reports"
    outputs.mkdir(parents=True, exist_ok=True)
    reports.mkdir(parents=True, exist_ok=True)

    selected = [c.strip() for c in args.configs.split(",") if c.strip()]
    unknown = [c for c in selected if c not in CONFIGS]
    if unknown:
        print(f"未知配置：{unknown}", file=sys.stderr)
        return 2

    started_all = time.time()
    for name in selected:
        cfg = CONFIGS[name]
        out = outputs / f"{name}.jsonl"
        summary = reports / f"{name}.summary.json"
        cmd = [
            str(runner),
            "--input",
            args.golden,
            "--output",
            str(out),
            "--label",
            name,
            "--mode",
            cfg["mode"],
            "--policy",
            cfg["policy"],
            "--source",
            "en",
            "--target",
            "zh-Hans",
            "--text-kind",
            cfg["text_kind"],
            "--strict-integrity",
            args.strict_integrity,
            "--summary",
            str(summary),
            "--ort-model-dir",
            args.model_dir,
        ]
        if cfg["dict"]:
            cmd.extend(["--dict", args.dict])
        if cfg["rules"]:
            cmd.extend(["--rules", args.rules])
        if cfg["terms"]:
            cmd.extend(["--terms", args.terms])
        if cfg.get("term_policy"):
            cmd.extend(["--term-policy", cfg["term_policy"]])
        if cfg.get("paragraph_strategy"):
            cmd.extend(["--paragraph-strategy", cfg["paragraph_strategy"]])
        if cfg.get("beam"):
            cmd.extend(["--beam", str(cfg["beam"])])
        if args.limit:
            cmd.extend(["--limit", str(args.limit)])
        max_new = cfg.get("max_new_tokens") or args.max_new_tokens
        if max_new:
            cmd.extend(["--max-new-tokens", str(max_new)])
        print(f"[{name}] {' '.join(cmd)}", file=sys.stderr)
        t0 = time.time()
        proc = subprocess.run(cmd, cwd=ROOT)
        if proc.returncode != 0:
            print(f"[{name}] 失败，退出码 {proc.returncode}", file=sys.stderr)
            return proc.returncode
        print(f"[{name}] 完成，耗时 {time.time() - t0:.1f}s", file=sys.stderr)

    print(f"全部完成：{len(selected)} 组，总耗时 {time.time() - started_all:.1f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
