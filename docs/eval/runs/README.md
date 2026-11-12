# 公开基准逐句译文（bench/test.jsonl，2 213 条）

本目录保存**完整模型译文**，不是汇总数字。每条用例同时给出原文与参考译文，
可以直接人工阅读、diff 两个模型，或作为后续回归基线。

| 文件 | 内容 |
|---|---|
| `bench-test-deepseek-v4.1-flash.json` | `quality-runner` 原始报告（含 cases、metrics、usage） |
| `bench-test-glm-5.3-flash.json` | 同上 |
| `bench-test-both-models.jsonl` | 合并后的**逐句对照**：`id / dataset_id / domain / document_id / source / reference / ds / glm` |

## 运行条件（这批译文的生成环境）

| 项 | 值 |
|---|---|
| 日期 | 2026-11-12 |
| 数据集 | `translator-sdk/fixtures/eval/bench/test.jsonl`（WMT24++ 958 + NTREX-128 1 255） |
| 后端 | OpenAI 兼容转发站（`starhub`），`api_key_env` 读取，密钥未入库 |
| 模型 | `deepseek-v4.1-flash`、`glm-5.3-flash` |
| 关闭思考 | `disable_thinking = true` + `thinking_style = "thinking_type"`（deepseek 实测 completion 45→6 token） |
| 并发 | `[http] max_concurrent_requests = 3`、`max_retries = 4`（站点单 key 并发上限≈3） |
| 预算 | `max_http_calls_per_request = 3`，`strict_integrity = true` |
| 运行 | `cargo run --release -p quality-runner -- --dataset fixtures/eval/bench/test.jsonl --config <toml> --jobs 3 --out <json>` |

## 汇总数字（详见 docs/eval/BENCH-BASELINE.md）

| 模型 | 通过 | chrF++ | BLEU-4 | p50 | p95 | 实测费用 |
|---|---|---|---|---|---|---|
| deepseek-v4.1-flash | 2 210 / 2 213 | 30.72 | 36.36 | 2 258 ms | 3 462 ms | $0.0898 |
| glm-5.3-flash | 2 211 / 2 213 | 30.12 | 35.21 | 3 564 ms | 7 714 ms | $0.0908 |

> 失败 5 条：4 条纯话题标签贴文（模型整条不译，参考译文译了标签）、
> 1 条服务端 `content_filter`。都保留在文件里，没有删样本。

## 怎么用

```bash
# 1) 只看 deepseek 与 glm 不一致的句子（人工评审优先看这些）
python3 - <<'PY'
import json, difflib
for line in open("docs/eval/runs/bench-test-both-models.jsonl", encoding="utf-8"):
    r = json.loads(line)
    a, b = r["ds"]["output"], r["glm"]["output"]
    if difflib.SequenceMatcher(None, a, b).ratio() < 0.6:
        print(r["id"], "|", r["source"][:60])
        print("  ds :", a)
        print("  glm:", b)
PY

# 2) 过滤某个领域 / 某篇文档
grep '"domain": "literary"' docs/eval/runs/bench-test-both-models.jsonl | head

# 3) 统计失败条目
grep -c '"ok": false' docs/eval/runs/bench-test-deepseek-v4.1-flash.json
```

## 说明

- 译文是**模型输出**，不是人工参考；参考译文在数据集的 `reference` 字段（WMT24++ / NTREX 官方）。
- 人工评审状态仍为 `PENDING_HUMAN`：这些文件是评审材料，不是"质量已通过"的证据。
- 数据来源与许可证：WMT24++（Apache-2.0）、NTREX-128（CC-BY-SA-4.0，派生文件同许可），
  见 `docs/eval/DATASETS.md`。
- 本目录文件由 `scripts/merge-eval-runs.py` 从两份原始报告确定性生成；
  `tools/quality-runner/tests/datasets.rs` 会校验它们与数据集逐条对齐。
