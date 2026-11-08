# P10.1-A Windows ORT 证据

- 模型：`E:\augenstern\translate\.build\model-cache\ort-model`（哈希见 `docs/58`）
- `ort-1000.json` / `ort-1000.log`：1,000 次，0 errors，共享 Arc/pool=1，p50 185.76 / p95 651.08 ms。
- `ort-10000-partial.log`：10,000 次实验的 4,000 次部分日志（Private 412.9–413.1 MiB），**未完成，不计 PASS**。
- `shared.log`：A/B 各 20 次并发 + A unload；0 errors，B 重载 212.6 ms。
- `identity.log`：独立模型副本 tokenizer 变化 → 共享 Arc=false，cache identity 改变；原模型未改。
- `budget.log`：100 MiB OutOfBudget，650 MiB 通过；估计 600 MiB vs 实测 Private 413.1 MiB。
- `crash.log`：独立子进程 access violation（exit 3221225477/0xC0000005）与正常子进程；只证明子进程实验，不是 ORT 进程内隔离。
- `p10-ort-10000.checkpoint.jsonl`（Windows `.build/reports`）：连续 10k 每 500 次 checkpoint；本仓库只保留摘录/最终日志。
- `ort-10000-checkpoint.jsonl`：本轮连续 10k 运行至 1,500 的每 500 次 checkpoint（0 errors，PID 31244）；未完成。
- `ort-10000.log`：同一运行的原始控制台日志。
