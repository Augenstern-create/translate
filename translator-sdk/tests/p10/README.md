# P10.1-A Windows ORT 证据

- 模型：`E:\augenstern\translate\.build\model-cache\ort-model`（三文件，哈希见 `docs/58`）
- 命令：`p10_soak -- 1000`（release、intra-op=1、无缓存、10 条混合文本）
- `ort-1000.json`：冷加载 704.4 ms、p50 185.76 / p95 651.08 ms、0 errors、共享 Arc/pool=1、
  RSS/Private Bytes 曲线、unload/reload。
- `ort-1000.log`：原始控制台日志（UTF-8 转存）。
- 10,000 次：**未完成**；报告 `docs/59` 标 PENDING，不伪造结果。
