# P8 Windows 结果汇总（commit `9c6a202`）

| 项 | 结果 |
|---|---|
| fmt | PASS |
| clippy 默认 / 全 feature | PASS / PASS |
| `cargo test --workspace` | **354 passed / 0 failed / 60 ignored** |
| `cargo test --workspace --all-features` | **357 passed / 0 failed / 65 ignored** |
| G06 cancellation | 0 tests（N/A：Linux 进程组语义） |
| G08 lc08 | 1 passed |
| ORT contract / real / facade | 13 / 6 / 5 passed |
| license audit | PASS（172 包） |
| release gate | FAIL（仅 G12/G11） |
| ORT 缓存 | `.build\ort-cache\dfbin`；`%LOCALAPPDATA%\ort.pyke.io` 时间戳未变 |

P8 新增 Rust lib 多实例测试（m01/m03/m04/m06）与 Bergamot/ORT shared 测试在 Windows 上均通过；
M-01～M-06 的 Android 专属路径只在 gms2 模拟器执行。
