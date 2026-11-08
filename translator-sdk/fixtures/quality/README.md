# 英中质量样本（T5）

| 文件 | 用途 | 来源 |
|---|---|---|
| `golden-v1.jsonl` | 320 条英中翻译金样本，11 个类别 | 项目自编；参考译文为人工撰写，`review_status` 全部为 `pending` |
| `google-10000-english-usa.txt` | 独立高频词覆盖目标（9,999 词） | `first20hours/google-10000-english`（Google Trillion Word Corpus 派生词表；仅用于本地覆盖度测量） |
| `ambiguous.json` | 10 个多义词的义项组要求 | 项目自编 |
| `phrases.json` | 技术/固定短语期望译法 | 由 `fixtures/dictionary/en-zh.supplement.jsonl` 派生 |

## 金样本字段

```json
{"id":"en-zh-0001","source":"...","reference":"...","category":"basic",
 "critical_constraints":["target_language_zh"],"review_status":"pending"}
```

- `critical_constraints` 只列**确定性**要求（数字、占位符、URL、否定），不把“与参考译文不同”当错误。
- `review_status` 必须为 `pending` 或 `reviewed`；未完成人工评审的样本在报告中标为 PENDING，不得自动当作通过。
- 参考译文不是唯一正确答案；chrF/BLEU 只作趋势指标，人工评审按语义/完整性/自然度/术语/格式分项。

## 重新生成词典/运行质量

```bash
python3 tools/quality/import_ecdict.py .build/quality-data/ecdict.csv .build/quality-data/ecdict-en-zh.jsonl \
  --wordlist fixtures/quality/google-10000-english-usa.txt \
  --supplement fixtures/dictionary/en-zh.supplement.jsonl \
  --max-entries 30200
cargo run -p pack-dictionary -- .build/quality-data/ecdict-en-zh.jsonl resources/en-zh-ecdict \
  --format jsonl --id dictionary.ecdict.en-zh --language en --target zh-Hans \
  --license MIT --license-file .build/quality-data/en-zh-LICENSE.txt
python3 tools/quality/run-dictionary-quality.py --runner target/release/quality-runner \
  --dict resources/en-zh-ecdict --out tests/quality/reports/dictionary.json
```

## P12-Q 新增样本

| 文件 | 数量 | 用途 |
|---|---:|---|
| `word-phrases-v1.jsonl` | 100 | 词/短语精确词典（90 条补充词 + 10 多义词），约束 `dictionary_contains` |
| `sentence-v1.jsonl` | 100 | 40 条新写 + 60 条 P11 留出验证句；否定/条件/数字/多义/长句 |
| `paragraph-v1.jsonl` | 50 | 3 句/段，来自已验证句对组合；A/B/C 策略对照 |
| `document-v1.jsonl` | 10 | Markdown 标题/段落/列表/引用/代码块/表格，约束 `document_structure` |
| `beam-subset.jsonl` | 40 | 25 长句 + 15 数字句，beam 1/2/4 延迟与质量对照 |
| `critical-fixes.jsonl` | 4 | P11 四条致命样本的修复回归 |

```bash
# Word/Phrase（词典）
target/release/quality-runner --text-kind word --policy auto --dict resources/en-zh-ecdict \
  --input fixtures/quality/word-phrases-v1.jsonl --output tests/quality/outputs/word-v1.jsonl

# Sentence（真实 ORT）
target/release/quality-runner --text-kind sentence --mode precise --policy auto \
  --dict resources/en-zh-ecdict --rules fixtures/rules/en-zh.quality.json \
  --terms fixtures/terms/en-zh.technical.json --term-policy enforce \
  --input fixtures/quality/sentence-v1.jsonl --output tests/quality/outputs/sentence-v1-final.jsonl

# Paragraph A/B/C
for s in whole sentence group; do
  target/release/quality-runner --text-kind paragraph --paragraph-strategy $s \
    --input fixtures/quality/paragraph-v1.jsonl --output tests/quality/outputs/paragraph3-$s.jsonl
done

# Beam
for b in 1 2 4; do
  target/release/quality-runner --text-kind sentence --beam $b \
    --input fixtures/quality/beam-subset.jsonl --output tests/quality/outputs/beam-cached-$b.jsonl
done
```
