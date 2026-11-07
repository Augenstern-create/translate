# P9.1 Windows 回归（commit `85a232c`）

| 项 | 结果 |
|---|---|
| fmt | PASS |
| clippy 默认 / 全 feature | PASS / PASS |
| `cargo test --workspace` | **357 passed / 0 failed / 60 ignored** |
| `cargo test --workspace --all-features` | **360 passed / 0 failed / 65 ignored** |
| ORT contract / real / facade | 13 / 6 / 5 passed（facade 用 `--test-threads=1` 验证） |
| G08 | 1 passed |
| G06 | 0 tests（N/A，Linux 专属） |
| release gate | FAIL（仅 G12/G11） |

P9.1 的词库导入/查询样本只在 Linux 生成；Windows 本轮验证的是代码回归与 ORT 共享/卸载隔离。
原始日志在 Windows `.build\reports\`，本目录为 UTF-8 转存。
