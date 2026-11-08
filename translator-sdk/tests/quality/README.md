# T5 质量证据目录

| 路径 | 内容 | 是否手工编辑 |
|---|---|---|
| `outputs/baseline.jsonl` | 真实 ORT 基线（320 条） | 否，工具生成 |
| `outputs/fast.jsonl` | Fast 模式 | 否 |
| `outputs/rules.jsonl` | 默认规则包 | 否 |
| `outputs/terms.jsonl` | 术语 Warn | 否 |
| `outputs/full.jsonl` | 词典+规则+术语，Precise | 否 |
| `outputs/full-fast.jsonl` | 同上，Fast | 否 |
| `outputs/baseline-96.jsonl` / `baseline-256.jsonl` | 解码上界对照 | 否 |
| `outputs/terms-enforce.jsonl` / `full-enforce.jsonl` | 术语 Enforce | 否 |
| `outputs/rule-cases-*.jsonl` | 12 条规则专项 | 否 |
| `strict/outputs/full.jsonl` | 严格完整性门禁（4 条拒绝） | 否 |
| `reports/dictionary.json` | 词典覆盖/多义/短语 | 否 |
| `reports/compare-v1.{json,md}` | 配置对照与回归样本 | 否 |
| `reports/summary.md` | 汇总 | 否 |
| `reports/human-review-sample.csv` | 62 条人工评审抽样（空白评分） | **是（评审人填写）** |
| `reports/dictionary-review-sample.csv` | 60 条词典评审抽样 | **是** |

## 重新生成

```bash
cargo build -p quality-runner --features ort --release

python3 tools/quality/run-dictionary-quality.py \
  --runner target/release/quality-runner --dict resources/en-zh-ecdict \
  --out tests/quality/reports/dictionary.json

python3 tools/quality/run-translation-golden.py \
  --configs baseline,fast,rules,terms,full,full-fast,baseline-96,baseline-256,terms-enforce,full-enforce

python3 tools/quality/compare-translations.py \
  --golden fixtures/quality/golden-v1.jsonl \
  --baseline baseline=tests/quality/outputs/baseline.jsonl \
  $(for c in fast rules terms full full-fast baseline-96 baseline-256 terms-enforce full-enforce; do echo --run $c=tests/quality/outputs/$c.jsonl; done) \
  --out tests/quality/reports/compare-v1

python3 tools/quality/summarize-quality.py \
  --dictionary tests/quality/reports/dictionary.json \
  --compare tests/quality/reports/compare-v1.json \
  --terms-output tests/quality/outputs/terms.jsonl \
  --out tests/quality/reports/summary.md
```

> 人工评审列必须由评审人填写；空白不代表通过。`review_status` 未改为 `reviewed` 前，质量结论保持 PENDING。
