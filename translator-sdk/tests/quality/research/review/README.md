# E0 人工评审说明

## 文件

- `e0-diagnostic-v1-review.csv`：27 条人工种子，优先评审。
- `e0-probes-v2-review.csv`：200 条合成探针，用于路由/症状覆盖；不代表独立语料。

## 规则

- 所有行当前为 `PENDING_HUMAN`。
- `reference_zh`、`error_codes`、`severity`、`reviewer_notes` 必须由人工填写。
- `error_codes` 使用 docs/83 的 TOK/SYN/SENSE/ROLE/SCOPE/TRANSFER/REALIZE/SELECT/NMT-* / TERMINOLOGY / FLUENCY / UNKNOWN。
- 一个样本允许多个错误码。
- 歧义样本允许保留多个语义解释，不强迫唯一答案。
- 不得由模型自动填写后标记为人工评审。
- 评审完成后另存版本，不覆盖原始评审表。
- 合成探针只能用于工程/路由诊断，不能作为 QUALITY_PASS 的独立人工语料。
