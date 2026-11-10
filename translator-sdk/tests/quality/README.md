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

## P15-Q 证据

| 文件 | 内容 |
|---|---|
| `outputs/p15-budget-auto-v2.jsonl` / `p15-budget-best.jsonl` | 40 段混合预算 Auto vs Best |
| `outputs/sentence-v1-{shiqing,m2m100-screen,nllb-screen}.jsonl` | 不同权重/架构筛选 |
| `reports/compare-budget-v2.*` | Auto 0.4484 vs Best 0.4449 |
| `reports/compare-model-different.*` | 不同权重模型对照 |
| `reports/p15-ai-preliminary-review.md` | 12 条 AI 辅助初步评审（非人工） |
| `reports/p15-test-*.log` / `p15-clippy.log` | 测试与静态检查 |

> 94 条人工评审仍 PENDING_HUMAN；AI 辅助评审不计入人工验收。

## P16-R 证据

| 文件 | 内容 |
|---|---|
| `outputs/p16-word.jsonl` | 100 词/短语，词典命中、NMT=0 |
| `outputs/p16-templates.jsonl` | 40 条完整模板，规则命中、NMT=0 |
| `outputs/p16-golden.jsonl` / `p16-golden-notemplates.jsonl` | 320 句模板 vs 无模板（NMT 322→300） |
| `reports/p16-*.summary.json` | RouteStats（dictionary/rule/nmt/fallback/cache） |
| `reports/compare-p16-golden.*` | chrF +0.0371、0 回归 |
| `reports/compare-p16-templates.*` | 40/40、chrF/BLEU 1.0 |
| `fixtures/templates/en-zh.core.json` | 40 条完整句子模板 |
| `fixtures/quality/template-cases-v1.jsonl` | 模板验收集 |

> 模板只做整句精确匹配；未命中回退 NMT。人工评审仍 PENDING_HUMAN。

## P18-L 证据

| 文件 | 内容 |
|---|---|
| `fixtures/quality/daily-knowledge-v1.jsonl` | 10,000 条日常知识集（word/phrase/collocation/expression/polysemy） |
| `fixtures/quality/daily-knowledge-v1.{construction,frozen}.jsonl` | 7,959 / 2,041 建设集与冻结集 |
| `fixtures/quality/daily-knowledge-v1.lock.json` | 源文件 sha256、选择规则、限制 |
| `fixtures/dictionary/en-zh.daily-supplement.jsonl` | 8,519 条日常短语/搭配/表达补充 |
| `outputs/p18-daily-baseline-lookup.jsonl` | 旧包 0.1.0 exact lookup 基线 |
| `outputs/p18-daily-after-lookup.jsonl` | 新包 0.2.0 exact lookup |
| `outputs/p18-word-forms-lookup.jsonl` | 20 个常见词形 |
| `reports/p18-daily-baseline.{json,md}` | 46.11% 总 / 45.86% 冻结 |
| `reports/p18-daily-after.{json,md}` | 100% 总 / 100% 冻结 |
| `reports/p18-daily-summary.json` | 建设前后、资源哈希、词形与限制 |
| `reports/p18-dictionary-review.csv` | 50 条人工释义抽样（PENDING_HUMAN） |

> 10k 集是**源内覆盖基准**，不是独立第三方语义基准；人工评审与外部验收集仍未完成。

## P19-G 证据

| 文件 | 内容 |
|---|---|
| `fixtures/rules/en-zh.composition.json` | 177 词/27 固定表达/8 短语动词/量词 |
| `fixtures/quality/composition-v1.jsonl` | 41 条独立组合集（36 正例、5 反例） |
| `outputs/p19-composition.jsonl` | 组合规则执行结果 |
| `outputs/p19-golden.jsonl` | 320 句，composition 开启 |
| `outputs/p19-document.jsonl` | 10 篇文档，composition 开启 |
| `reports/p19-composition-validation.{json,md}` | 36/36 正例、5/5 回退 |
| `reports/compare-p19-golden.*` | chrF 0.5074→0.5172，NMT 300→291 |
| `reports/compare-p19-document.*` | 文档结构 1.0 |
| `reports/p19-composition.summary.json` | rule/NMT 统计 |

> 组合规则只覆盖已验证日常简单句；人工评审仍 PENDING_HUMAN。

## P20-C 证据

| 文件 | 内容 |
|---|---|
| `fixtures/quality/semantic-adversarial-v1.jsonl` | 7 条语义对抗集（4 正例、3 反例） |
| `outputs/p19-semantic-baseline.jsonl` | P19 baseline：7/7 NMT |
| `outputs/p20-semantic.jsonl` | P20-C：4 Rule / 3 NMT |
| `outputs/p20-golden.jsonl` | 320 句，P19+P20 同时开启 |
| `outputs/p20-document.jsonl` | 10 篇文档，P19+P20 |
| `reports/p20-semantic-validation.{json,md}` | 4/4 正例、3/3 回退 |
| `reports/compare-p20-golden.*` | P19/P20 320 同分、零退化 |
| `reports/compare-p20-document.*` | 文档结构 1.0 |

> P20-C 语义层 opt-in；旧 36 正例仍由 P19 CompositionEngine 处理；人工评审 PENDING_HUMAN。

## P21-R 证据

| 文件 | 内容 |
|---|---|
| `fixtures/quality/p21-realization-v1.jsonl` | 13 条中文生成专项（NP/量词/时间/否定/情态/体貌/疑问） |
| `outputs/p21-realization.jsonl` | 20/20 规则，0 NMT |
| `fixtures/quality/p21-unseen-v1.jsonl` | 10 条未见组合 + 1 条否定/并列反例 |
| `outputs/p21-unseen.jsonl` | 14 Rule / 1 NMT |
| `fixtures/quality/p21-frozen-v1.jsonl` + `outputs/p21-frozen.jsonl` | 20 条后置集，20 Rule |
| `fixtures/quality/p21-matrix-v1.jsonl` + `outputs/p21-matrix.jsonl` | 100 条结构矩阵，100 Rule |
| `reports/p21-review-packet.csv` | 118 行统一人工评审包（PENDING_HUMAN） |
| `outputs/p21-p19-migration.jsonl` | P19 迁移路由：33 semantic / 4 P19 / 4 NMT（41 条） |
| `outputs/p21-golden.jsonl` | 320 句 P21 A/B |
| `outputs/p21-document.jsonl` | 10 篇文档 P21 |
| `reports/compare-p21-golden.*` | chrF 0.5172→0.5184、NMT 291、零回归 |
| `reports/compare-p21-document.*` | 文档结构 1.0 |

> 36/36 P19 正例已迁移；条件/比较/简单被动已进入语义路径；人工评审 PENDING_HUMAN。
