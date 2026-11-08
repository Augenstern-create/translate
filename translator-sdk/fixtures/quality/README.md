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
