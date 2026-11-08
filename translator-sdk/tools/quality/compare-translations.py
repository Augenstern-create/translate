#!/usr/bin/env python3
"""T5：翻译质量对照（自动指标 + 约束保真 + 回归样本）。

指标口径：
- **chrF-zh**：去掉所有空白后按字符 1–6-gram 计算 Fβ（β=2，偏重召回）；
  这是本项目的趋势指标，不是 sacreBLEU 的标准 chrF 数字。
- **BLEU-zh（辅助）**：字符级 BLEU-4，只作为参考，不单独当“翻译正确率”。
- **数字/占位符/否定/目标语言**：按确定性约束检查，失败即列出样本 id。
- 人工评审不在这里自动通过；报告里固定写 `PENDING`。

用法：
    python3 tools/quality/compare-translations.py \
        --golden fixtures/quality/golden-v1.jsonl \
        --baseline baseline=tests/quality/outputs/baseline.jsonl \
        --run fast=tests/quality/outputs/fast.jsonl \
        --run full=tests/quality/outputs/full.jsonl \
        --out tests/quality/reports/compare-v1
"""
from __future__ import annotations

import argparse
import json
import math
import re
from collections import Counter, defaultdict
from pathlib import Path

URL_RE = re.compile(r"https?://[^\s，。；、]+")
EMAIL_RE = re.compile(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}")
VAR_RE = re.compile(r"\{[^{}\s]+\}")
CODE_RE = re.compile(r"`[^`]+`")
CJK_RE = re.compile(r"[\u4e00-\u9fff]")
NEGATORS = set("不没無无未别別非否勿莫甭休毋")
NEG_CUES = [
    "not", "n't", "never", "none", "nobody", "nothing", "neither", "nor",
    "without", "cannot", "can't", "don't", "doesn't", "didn't", "isn't",
    "aren't", "wasn't", "weren't", "won't", "wouldn't", "shouldn't",
    "couldn't", "haven't", "hasn't", "hadn't", "no", "hardly", "barely",
    "rarely", "seldom",
]


def norm(text: str) -> str:
    return "".join((text or "").lower().split())


def ngrams(text: str, n: int) -> Counter:
    return Counter(text[i : i + n] for i in range(0, max(0, len(text) - n + 1)))


def chrf(hyp: str, ref: str, max_n: int = 6, beta: float = 2.0) -> float:
    hyp, ref = norm(hyp), norm(ref)
    if not hyp or not ref:
        return 0.0
    scores = []
    for n in range(1, max_n + 1):
        h, r = ngrams(hyp, n), ngrams(ref, n)
        overlap = sum((h & r).values())
        if overlap == 0:
            scores.append(0.0)
            continue
        precision = overlap / max(1, sum(h.values()))
        recall = overlap / max(1, sum(r.values()))
        beta2 = beta * beta
        scores.append(
            (1 + beta2) * precision * recall / (beta2 * precision + recall)
            if precision + recall > 0
            else 0.0
        )
    return sum(scores) / len(scores)


def bleu(hyp: str, ref: str, max_n: int = 4) -> float:
    hyp, ref = norm(hyp), norm(ref)
    if not hyp or not ref:
        return 0.0
    precisions = []
    for n in range(1, max_n + 1):
        h, r = ngrams(hyp, n), ngrams(ref, n)
        overlap = sum((h & r).values())
        total = max(0, len(hyp) - n + 1)
        precisions.append(overlap / total if total else 0.0)
    if min(precisions) == 0:
        return 0.0
    log_sum = sum(math.log(p) for p in precisions) / max_n
    brevity = min(1.0, len(hyp) / max(1, len(ref)))
    return brevity * math.exp(log_sum)


def extract_tokens(pattern: re.Pattern, text: str) -> list[str]:
    return [m.group(0) for m in pattern.finditer(text or "")]


def py_numbers(text: str) -> list[str]:
    """与 Rust `quality.rs` 对齐的简化数字提取：千分位逗号/空格、前导零、百分号。"""
    text = text.translate(str.maketrans("０１２３４５６７８９", "0123456789"))
    text = re.sub(r"(?<=\d)[, ](?=\d{3}(?!\d))", "", text)
    tokens = re.findall(r"\d+(?:\.\d+)?%?", text)
    out = []
    for token in tokens:
        suffix = "%" if token.endswith("%") else ""
        body = token[:-1] if suffix else token
        if "." in body:
            int_part, frac = body.split(".", 1)
            int_part = int_part.lstrip("0") or "0"
            out.append(f"{int_part}.{frac}{suffix}")
        else:
            out.append((body.lstrip("0") or "0") + suffix)
    return out


def constraint_results(source: str, output: str, constraints: list[str]) -> dict:
    output = output or ""
    results = {}
    if "url_preserved" in constraints:
        urls = extract_tokens(URL_RE, source)
        results["url_preserved"] = all(url in output for url in urls)
    if "placeholder_preserved" in constraints:
        tokens = (
            extract_tokens(VAR_RE, source)
            + extract_tokens(URL_RE, source)
            + extract_tokens(EMAIL_RE, source)
            + extract_tokens(CODE_RE, source)
        )
        results["placeholder_preserved"] = all(tok in output for tok in tokens)
    if "negation_preserved" in constraints:
        lower = source.lower()
        has_cue = any(
            (cue in lower) if cue == "n't" else re.search(rf"\b{re.escape(cue)}\b", lower)
            for cue in NEG_CUES
        )
        results["negation_preserved"] = (not has_cue) or any(ch in NEGATORS for ch in output)
    if "number_preserved" in constraints:
        nums = py_numbers(source)
        out_nums = py_numbers(output)
        results["number_preserved"] = all(
            out_nums.count(num) >= nums.count(num) for num in set(nums)
        )
    if "target_language_zh" in constraints:
        results["target_language_zh"] = bool(CJK_RE.search(output))
    return results


def load_golden(path: Path) -> dict[str, dict]:
    rows = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            row = json.loads(line)
            rows[row["id"]] = row
    return rows


def load_output(path: Path) -> dict[str, dict]:
    rows = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            row = json.loads(line)
            rows[row["id"]] = row
    return rows


def evaluate(golden: dict[str, dict], output: dict[str, dict]) -> dict:
    per_category = defaultdict(list)
    rows = []
    errors = 0
    fatal = 0
    critical_issues = 0
    number_total = 0
    number_kept = 0
    constraint_total = 0
    constraint_pass = 0
    placeholder_total = 0
    placeholder_pass = 0
    url_total = 0
    url_pass = 0
    negation_total = 0
    negation_pass = 0
    target_total = 0
    target_pass = 0
    for gid, g in golden.items():
        out = output.get(gid)
        if out is None or out.get("error"):
            errors += 1
            rows.append({"id": gid, "category": g["category"], "error": (out or {}).get("error"), "chrf": 0.0, "bleu": 0.0})
            continue
        hyp = out.get("output") or ""
        ref = g["reference"]
        score = chrf(hyp, ref)
        bleu_score = bleu(hyp, ref)
        per_category[g["category"]].append(score)
        quality = out.get("quality") or {}
        if quality.get("fatal"):
            fatal += 1
        for issue in quality.get("issues") or []:
            if issue.get("severity") == "critical":
                critical_issues += 1
        number_total += quality.get("numbers_in_source") or 0
        number_kept += quality.get("numbers_preserved") or 0
        constraints = constraint_results(g["source"], hyp, g.get("critical_constraints") or [])
        for key, ok in constraints.items():
            constraint_total += 1
            constraint_pass += int(ok)
            if key == "placeholder_preserved":
                placeholder_total += 1
                placeholder_pass += int(ok)
            if key == "url_preserved":
                url_total += 1
                url_pass += int(ok)
            if key == "negation_preserved":
                negation_total += 1
                negation_pass += int(ok)
            if key == "target_language_zh":
                target_total += 1
                target_pass += int(ok)
        rows.append(
            {
                "id": gid,
                "category": g["category"],
                "source": g["source"],
                "reference": ref,
                "output": hyp,
                "chrf": round(score, 4),
                "bleu": round(bleu_score, 4),
                "error": None,
                "fatal": bool(quality.get("fatal")),
                "constraints": constraints,
                "quality_issues": quality.get("issues") or [],
            }
        )
    scored = [r["chrf"] for r in rows if not r.get("error")]
    return {
        "samples": len(golden),
        "errors": errors,
        "error_rate": round(errors / max(1, len(golden)), 4),
        "fatal_integrity": fatal,
        "critical_quality_issues": critical_issues,
        "chrf": round(sum(scored) / len(scored), 4) if scored else 0.0,
        "bleu": round(sum(r["bleu"] for r in rows if not r.get("error")) / max(1, len(scored)), 4),
        "exact_match": round(sum(1 for r in rows if not r.get("error") and norm(r["output"]) == norm(r["reference"])) / max(1, len(golden)), 4),
        "number_fidelity": round(number_kept / number_total, 4) if number_total else None,
        "constraint_accuracy": round(constraint_pass / constraint_total, 4) if constraint_total else None,
        "placeholder_accuracy": round(placeholder_pass / placeholder_total, 4) if placeholder_total else None,
        "url_accuracy": round(url_pass / url_total, 4) if url_total else None,
        "negation_accuracy": round(negation_pass / negation_total, 4) if negation_total else None,
        "target_language_accuracy": round(target_pass / target_total, 4) if target_total else None,
        "per_category": {
            k: round(sum(v) / len(v), 4) for k, v in sorted(per_category.items()) if v
        },
        "rows": rows,
    }


def diff_runs(name: str, base: dict, run: dict) -> dict:
    base_rows = {r["id"]: r for r in base["rows"]}
    run_rows = {r["id"]: r for r in run["rows"]}
    regressions = []
    improvements = []
    new_fatal = []
    fixed_fatal = []
    for gid, row in run_rows.items():
        b = base_rows.get(gid)
        if not b:
            continue
        if row.get("error") and not b.get("error"):
            regressions.append({"id": gid, "reason": "new_error", "source": b.get("source"), "reference": b.get("reference"), "baseline": b.get("output"), "run": None})
            continue
        if b.get("error") and not row.get("error"):
            improvements.append({"id": gid, "reason": "fixed_error", "source": row.get("source")})
            continue
        if row.get("error"):
            continue
        delta = row["chrf"] - b["chrf"]
        if delta <= -0.03:
            regressions.append(
                {
                    "id": gid,
                    "reason": "chrf_drop",
                    "delta": round(delta, 4),
                    "source": row["source"],
                    "reference": row["reference"],
                    "baseline": b["output"],
                    "run": row["output"],
                }
            )
        elif delta >= 0.03:
            improvements.append({"id": gid, "reason": "chrf_gain", "delta": round(delta, 4)})
        if row.get("fatal") and not b.get("fatal"):
            new_fatal.append(gid)
        if b.get("fatal") and not row.get("fatal"):
            fixed_fatal.append(gid)
    regressions.sort(key=lambda r: r.get("delta", 0))
    return {
        "run": name,
        "chrf_delta": round(run["chrf"] - base["chrf"], 4),
        "bleu_delta": round(run["bleu"] - base["bleu"], 4),
        "error_delta": run["errors"] - base["errors"],
        "number_fidelity_delta": (
            None
            if run["number_fidelity"] is None or base["number_fidelity"] is None
            else round(run["number_fidelity"] - base["number_fidelity"], 4)
        ),
        "new_fatal": new_fatal,
        "fixed_fatal": fixed_fatal,
        "regressions": regressions[:50],
        "regression_count": len(regressions),
        "improvements": improvements[:50],
        "improvement_count": len(improvements),
    }


def markdown(report: dict) -> str:
    lines = ["# 翻译质量自动对照", "", f"- 金样本：{report['golden']}", f"- 样本数：{report['sample_count']}", "- 人工评审：**PENDING**", ""]
    lines.append("| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for name, m in report["runs"].items():
        lines.append(
            f"| {name} | {m['chrf']:.4f} | {m['bleu']:.4f} | {m['error_rate']:.2%} | {m['fatal_integrity']} | "
            f"{m['number_fidelity'] if m['number_fidelity'] is not None else 'n/a'} | "
            f"{m['placeholder_accuracy'] if m['placeholder_accuracy'] is not None else 'n/a'} | "
            f"{m['url_accuracy'] if m['url_accuracy'] is not None else 'n/a'} | "
            f"{m['negation_accuracy'] if m['negation_accuracy'] is not None else 'n/a'} | "
            f"{m['target_language_accuracy'] if m['target_language_accuracy'] is not None else 'n/a'} |"
        )
    lines += ["", "## 分类 chrF", "", "| 类别 | " + " | ".join(report["runs"].keys()) + " |", "|---|" + "---:|" * len(report["runs"])]
    categories = sorted({c for m in report["runs"].values() for c in m["per_category"]})
    for cat in categories:
        cells = [str(m["per_category"].get(cat, "n/a")) for m in report["runs"].values()]
        lines.append(f"| {cat} | " + " | ".join(cells) + " |")
    if report.get("diffs"):
        lines += ["", "## 相对基线的变化", ""]
        for name, d in report["diffs"].items():
            lines.append(
                f"- **{name}**：chrF {d['chrf_delta']:+.4f}，BLEU {d['bleu_delta']:+.4f}，"
                f"错误 {d['error_delta']:+d}，新增致命 {len(d['new_fatal'])}，"
                f"回归样本 {d['regression_count']}，改善样本 {d['improvement_count']}"
            )
            for item in d["regressions"][:8]:
                lines.append(
                    f"  - `{item['id']}` {item.get('reason')} {item.get('delta', '')}："
                    f"{item.get('source', '')} → 基线 `{item.get('baseline', '')}` / 本次 `{item.get('run', '')}`"
                )
    lines += ["", "> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；", "> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。", ""]
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--golden", required=True)
    ap.add_argument("--baseline", required=True, help="name=path")
    ap.add_argument("--run", action="append", default=[], help="name=path（可重复）")
    ap.add_argument("--out", required=True, help="输出前缀（生成 .json 与 .md）")
    args = ap.parse_args()

    golden = load_golden(Path(args.golden))
    named = [args.baseline] + args.run
    runs = {}
    paths = {}
    for item in named:
        name, path = item.split("=", 1)
        runs[name] = evaluate(golden, load_output(Path(path)))
        paths[name] = path

    baseline_name = args.baseline.split("=", 1)[0]
    diffs = {
        name: diff_runs(name, runs[baseline_name], metrics)
        for name, metrics in runs.items()
        if name != baseline_name
    }
    report = {
        "schema": "translation-quality/v1",
        "golden": args.golden,
        "sample_count": len(golden),
        "baseline": baseline_name,
        "paths": paths,
        "runs": runs,
        "diffs": diffs,
        "review_status": "PENDING_HUMAN",
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.with_suffix(".json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    out.with_suffix(".md").write_text(markdown(report), encoding="utf-8")
    for name, m in runs.items():
        print(
            f"{name:16s} chrF={m['chrf']:.4f} BLEU={m['bleu']:.4f} "
            f"errors={m['errors']}/{m['samples']} fatal={m['fatal_integrity']} "
            f"num={m['number_fidelity']} placeholders={m['placeholder_accuracy']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
