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

## P12-Q 证据

| 文件 | 内容 |
|---|---|
| `outputs/word-v1.jsonl` | 100 词/短语（词典） |
| `outputs/sentence-v1-v4.jsonl` | 100 句验证（新保护+句级切分） |
| `outputs/paragraph3-{whole,sentence,group}.jsonl` | 50 段 A/B/C |
| `outputs/document-v1-v4.jsonl` | 10 篇 Markdown |
| `outputs/beam-cached-{1,2,4}.jsonl` | 40 条 beam 对照 |
| `outputs/critical-fixes-final.jsonl` | 旧 4 条 Critical 修复 |
| `reports/compare-p12-v4.{json,md}` | P11 vs P12 baseline/full |
| `reports/compare-sentence-v4.*` | 句子验证集 |
| `reports/compare-paragraphs.*` | A/B/C |
| `reports/compare-documents-v4.*` | 文档结构 |
| `reports/compare-beam-cached.*` | beam 质量/延迟 |
| `reports/p12-summary.md` | 汇总 |

> 人工评审仍未完成；P12 结论保持 Conditional。

## P13-Q 证据

| 文件 | 内容 |
|---|---|
| `outputs/sentence-v1-{int8,fp16,fp32}.jsonl` | 100 句模型精度变体 |
| `outputs/p13-fp32-full.jsonl` | fp32 320 条 |
| `outputs/paragraph3-best-final.jsonl` / `p13-paragraph-best-fp32.jsonl` | 段落 Best（int8/fp32） |
| `reports/compare-model-variants*.{json,md}` | 模型变体对照 |
| `reports/compare-model-320.*` | fp32 vs int8 320 条 |
| `reports/compare-paragraphs-best*.{json,md}` | 段落 Best |
| `reports/p13-review-packet.csv` | 44 条人工评审包（PENDING_HUMAN） |
| `reports/p13-clippy.log` / `p13-test-*.log` | 工具与测试证据 |

> 人工评审未完成；P13 结论保持 Conditional，fp32/Best 不自动成为全局默认。

## P14-Q 证据

| 文件 | 内容 |
|---|---|
| `outputs/p14-context-final.jsonl` | 前一句上下文实验（50 段） |
| `outputs/p14-best-final.jsonl` | Best 最终质量（50 段） |
| `reports/compare-selector-final.{json,md}` | Best 选择器 vs oracle（准确率/regret） |
| `reports/compare-context-final.*` | Context vs 逐句 |
| `reports/compare-best-final.*` | Best vs 逐句 |
| `reports/p14-candidate-review.csv` + `p14-candidate-key.json` | 50 段盲评 A/B 与密钥 |
| `reports/p14-test-*.log` / `p14-clippy.log` | 测试与静态检查 |

> 94 条人工评审（44 重点 + 50 盲评 A/B）PENDING_HUMAN；P14 结论 Conditional。
