# P9.1 小样本证据

- 原始下载：`.build/p9-data/`（已 gitignore；仅内部研发，不随 SDK 分发）
- 资源锁：`fixtures/p9/resource-lock.json`
- 报告：`docs/52_P9.1_多语言词库导入报告.md`

| 文件 | 内容 |
|---|---|
| `kaikki-stats.json` / `jmdict-stats.json` | 导入统计 |
| `kaikki-manifest.json` / `jmdict-manifest.json` | 生成包的 manifest（id/来源/revision/逐文件 sha256） |
| `kaikki-bench.json` / `jmdict-bench.json` | p33-bench 的初始化/查询 p50/p95/RSS |
| `quality.txt` | 实际 CLI 查询输出（含缺失词条） |
| `di05-corrupt-install.log` | 坏包被 ResourceStore 完整性校验拒绝 |
| `di06-repro.txt` | 两次构建逐文件 sha256 一致 |
| `lock-check.log` | P9 资源锁校验 |

生成命令（在 `translator-sdk/`）：

```bash
cargo build --release -p pack-dictionary -p p33-bench -p translator-cli
python3 tools/p9/import_kaikki.py .build/p9-data/kaikki-zh-16m.jsonl \
  .build/p9-data/normalized/kaikki-en-1000.jsonl --limit 1000 --stats .build/p9-data/kaikki-stats.json
python3 tools/p9/import_jmdict.py .build/p9-data/jmdict_e.xml \
  .build/p9-data/normalized/jmdict-ja-1000.jsonl --limit 1000 --stats .build/p9-data/jmdict-stats.json
./target/release/pack-dictionary ... --format jsonl ...
./target/release/p33-bench dictionary --dir .build/p9-data/packages/kaikki-en-zh --term light --lang en
```
