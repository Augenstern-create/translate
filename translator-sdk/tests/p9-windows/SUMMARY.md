# P9.1-S Windows 回归（commit `d70f53e`）

| 项 | 结果 |
|---|---|
| fmt | PASS |
| clippy 默认 / 全 feature | PASS / PASS |
| `cargo test --workspace` | **362 passed / 0 failed / 60 ignored** |
| `cargo test --workspace --all-features` | **365 passed / 0 failed / 65 ignored** |
| ORT contract / real / facade | 13 / 6 / 5 passed（facade `--test-threads=1`） |
| G08 | 1 passed |
| G06 | 0 tests（N/A，Linux 专属） |
| release gate | FAIL（仅 G12/G11） |

P9.1-S 的词库 v3/定向样本只在 Linux 生成；Windows 验证 v3 代码回归、多来源/多值单测与 ORT。
