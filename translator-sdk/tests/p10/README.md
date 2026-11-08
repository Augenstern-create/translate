# P10.1-A Windows ORT 证据

- 模型：`E:\augenstern\translate\.build\model-cache\ort-model`（哈希见 `docs/58`）
- `ort-1000.json` / `ort-1000.log`：1,000 次，0 errors，共享 Arc/pool=1，p50 185.76 / p95 651.08 ms。
- `ort-10000-partial.log`：10,000 次实验的 4,000 次部分日志（Private 412.9–413.1 MiB），**未完成，不计 PASS**。
- `shared.log`：A/B 各 20 次并发 + A unload；0 errors，B 重载 212.6 ms。
- `identity.log`：独立模型副本 tokenizer 变化 → 共享 Arc=false，cache identity 改变；原模型未改。
- `budget.log`：100 MiB OutOfBudget，650 MiB 通过；估计 600 MiB vs 实测 Private 413.1 MiB。
