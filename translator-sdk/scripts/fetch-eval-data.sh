#!/usr/bin/env bash
# 下载评测原始数据（**不**转换、**不**提交到 git）。
#
# 设计原则：
#   1. 只下载**公开且允许再分发**的数据；受限/需登录的数据一律跳过并记录原因；
#   2. 原始文件放在 fixtures/eval/external/（git 忽略），转换结果另放；
#   3. 记录 URL、版本、许可证、下载日期与 sha256，供复核与复现。
#
# 用法：bash scripts/fetch-eval-data.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
ext="$root/fixtures/eval/external"
mkdir -p "$ext/wmt24pp" "$ext/ntrex"
stamp="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

download() { # url dest
  local url="$1" dest="$2"
  if [[ -s "$dest" ]]; then
    echo "已存在：$dest"
  else
    echo "下载：$url"
    curl -sSL --fail --max-time 300 -o "$dest" "$url"
  fi
}

# ---- WMT24++（Apache-2.0，可直接再分发）----
download "https://huggingface.co/datasets/google/wmt24pp/resolve/main/en-zh_CN.jsonl" \
  "$ext/wmt24pp/en-zh_CN.jsonl"

# ---- NTREX-128（CC-BY-SA-4.0）----
base="https://raw.githubusercontent.com/MicrosoftTranslator/NTREX/main"
download "$base/NTREX-128/newstest2019-src.eng.txt" "$ext/ntrex/newstest2019-src.eng.txt"
download "$base/NTREX-128/newstest2019-ref.zho-CN.txt" "$ext/ntrex/newstest2019-ref.zho-CN.txt"
download "$base/DOCUMENT_IDS.tsv" "$ext/ntrex/DOCUMENT_IDS.tsv"
download "$base/LICENSE.md" "$ext/ntrex/LICENSE.md"

# ---- 受限数据：**不下载**，只记录 ----
# FLORES+（openlanguagedata/flores_plus）与 FLORES-200（facebook/flores）在 HuggingFace
# 上是 gated 数据集：未登录/未接受条款时返回 401。按项目规则不得绕过，故不下载。
# WMT26 General MT：只公开 blind set（无参考译文），无法计算 chrF/BLEU，故不导入。

python3 - "$ext" "$stamp" <<'PY'
import hashlib, json, os, sys

ext, stamp = sys.argv[1], sys.argv[2]

def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()

sources = [
    {
        "dataset_id": "wmt24pp",
        "name": "WMT24++ (en-zh_CN)",
        "url": "https://huggingface.co/datasets/google/wmt24pp/resolve/main/en-zh_CN.jsonl",
        "homepage": "https://huggingface.co/datasets/google/wmt24pp",
        "version": "HF snapshot 2026-07-30",
        "license": "Apache-2.0",
        "redistributable": True,
        "files": [("wmt24pp/en-zh_CN.jsonl", "zh_CN: 简体中文")],
    },
    {
        "dataset_id": "ntrex128",
        "name": "NTREX-128 (eng-zho-CN)",
        "url": "https://github.com/MicrosoftTranslator/NTREX",
        "homepage": "https://github.com/MicrosoftTranslator/NTREX",
        "version": "main @ 2026-11-12",
        "license": "CC-BY-SA-4.0",
        "redistributable": True,
        "files": [
            ("ntrex/newstest2019-src.eng.txt", "英语源文"),
            ("ntrex/newstest2019-ref.zho-CN.txt", "简体中文参考译文"),
            ("ntrex/DOCUMENT_IDS.tsv", "文档边界"),
            ("ntrex/LICENSE.md", "数据集许可证"),
        ],
    },
]

skipped = [
    {
        "dataset_id": "flores_plus",
        "name": "FLORES+ (openlanguagedata/flores_plus)",
        "reason": "HuggingFace gated：未登录/未接受条款返回 401（Access restricted）。按规则不绕过。",
        "probe": "HTTP 401",
    },
    {
        "dataset_id": "flores200",
        "name": "FLORES-200 (facebook/flores, CC-BY-SA-4.0)",
        "reason": "HuggingFace gated（gated: auto）：downloads 返回 401，需要账号与条款同意。按规则不绕过；许可证本身允许再分发。",
        "probe": "HTTP 401",
    },
    {
        "dataset_id": "wmt26_genmt",
        "name": "WMT26 General MT",
        "reason": "目前只公开 blind set（wmt26_genmt_blindset.jsonl，仅源文、无参考译文），无法计算 chrF/BLEU；带参考的测试集通常赛后发布。",
        "probe": "https://data.statmt.org/wmt26/ 目录列表",
    },
    {
        "dataset_id": "tatoeba_challenge",
        "name": "Tatoeba Challenge (en-zh)",
        "reason": "可用（Tatoeba 句子 CC-BY 2.0 FR），但为众包句子、领域/质量分布不可控，且与 WMT/NTREX 风格差异大；第一版不纳入基准，仅作为潜在补充记录在案。",
        "probe": "未下载",
    },
]

out = {"generated_at": stamp, "sources": [], "skipped": skipped}
for src in sources:
    entry = {k: v for k, v in src.items() if k != "files"}
    entry["files"] = []
    for rel, note in src["files"]:
        path = os.path.join(ext, rel)
        if not os.path.exists(path):
            continue
        entry["files"].append({
            "path": rel,
            "note": note,
            "bytes": os.path.getsize(path),
            "sha256": sha256(path),
        })
    out["sources"].append(entry)

manifest = os.path.join(ext, "SOURCES.json")
with open(manifest, "w", encoding="utf-8") as fh:
    json.dump(out, fh, ensure_ascii=False, indent=2)
    fh.write("\n")
print("清单写入：", manifest)
PY
