# 质量/覆盖工具

## P18-L 日常知识集与词典

以下命令在 `translator-sdk/` 下执行；不联网。`ecdict.csv` 需来自本地已有快照
（本仓库不入库 66 MB 源文件）。

```bash
# 1) 从 ECDICT + Google 10k 构建 10k 日常知识集、80/20 split 与词典补充
python3 tools/quality/build_daily_knowledge.py

# 2) 生成 0.2.0 词典源
python3 tools/quality/import_ecdict.py \
  .build/quality-data/ecdict.csv .build/quality-data/ecdict-p18.jsonl \
  --wordlist fixtures/quality/google-10000-english-usa.txt \
  --max-frequency-rank 20000 --max-entries 60000 --include-phrases \
  --supplement fixtures/dictionary/en-zh.daily-supplement.jsonl \
  --stats .build/quality-data/ecdict-p18-stats.json

# 3) 打包为正式资源
./target/release/pack-dictionary .build/quality-data/ecdict-p18.jsonl resources/en-zh-ecdict \
  --format jsonl --id dictionary.ecdict.en-zh --version 0.2.0 \
  --language en --target zh-Hans --name ECDICT \
  --url https://github.com/skywind3000/ECDICT --license MIT \
  --license-file resources/en-zh-ecdict/LICENSE.txt \
  --revision snapshot-2026-11-09+p18-daily --payload-schema dictionary.sqlite.v3

# 4) exact lookup + 覆盖统计（不做前缀补全）
./target/release/quality-runner \
  --lookup-input fixtures/quality/daily-knowledge-v1.units.txt \
  --lookup-output tests/quality/outputs/p18-daily-after-lookup.jsonl \
  --lookup-exact --dict resources/en-zh-ecdict
python3 tools/quality/run_daily_coverage.py \
  --dataset fixtures/quality/daily-knowledge-v1.jsonl \
  --lookup-output tests/quality/outputs/p18-daily-after-lookup.jsonl \
  --out tests/quality/reports/p18-daily-after.json --label p18-after
```

基线（旧包 0.1.0）用同一命令，把 `--dict` 指向旧包备份；结果见
`tests/quality/reports/p18-daily-baseline.*`。

> 10k 集是源内覆盖基准，不是独立第三方语义基准。人工评审见
> `tests/quality/reports/p18-dictionary-review.csv`（`PENDING_HUMAN`）。
