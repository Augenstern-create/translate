# 外部评测数据（派生）

本目录由 `cargo run -p eval-import` 生成，**不是**原始发布物。源文件、哈希与许可证
见 `SOURCES.json`；生成规则与统计见 `STATS.md` 与 `docs/eval/DATASETS.md`。

## 许可证

| 文件 | 来源 | 许可证 |
|---|---|---|
| `test.jsonl`、`dev.jsonl`（`dataset_id=wmt24pp` 的行） | [WMT24++](https://huggingface.co/datasets/google/wmt24pp) | Apache-2.0 |
| `test.jsonl`、`dev.jsonl`（`dataset_id=ntrex128` 的行） | [NTREX-128](https://github.com/MicrosoftTranslator/NTREX) | CC-BY-SA-4.0 |

NTREX-128 采用 CC-BY-SA-4.0：**包含其内容的派生文件（本目录中 `ntrex128` 的行）
同样按 CC-BY-SA-4.0 提供**，署名 Microsoft Translator / NTREX-128 项目。

WMT24++ 采用 Apache-2.0，署名 Google Research / WMT24++ 项目。

代码本身（本仓库的 Rust 代码）仍按仓库根目录的 MIT/Apache-2.0 双许可提供。
数据与代码的许可证相互独立。
