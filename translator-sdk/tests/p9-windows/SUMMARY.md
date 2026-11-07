# P9.2/P9.3-A Windows 回归（commit `0f86676`）

| 项 | 结果 |
|---|---|
| fmt | PASS |
| clippy 默认 / 全 feature | PASS / PASS |
| `cargo test --workspace` | **363 passed / 0 failed / 60 ignored** |
| `cargo test --workspace --all-features` | **366 passed / 0 failed / 66 ignored** |
| ORT contract / real / facade | 13 / 6 / 5 passed |
| G08 | 1 passed |
| G06 | 0 tests（N/A，Linux 专属） |
| release gate | FAIL（仅 G12/G11） |

P9.2 的 fastText 模型对照与 P9.3-A lemmatizer 只在 Linux 运行；Windows 验证可选 feature 编译、回归与 ORT。
