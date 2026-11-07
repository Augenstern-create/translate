# P7.3 Windows 结果汇总（NAN-WINDOWS，commit `0ed70db`）

| 项 | 结果 |
|---|---|
| 同步 | `3bbfbb9..0ed70db` fast-forward；`git status` 干净 |
| fmt | PASS（exit 0） |
| clippy 默认 | PASS |
| clippy 全 feature | PASS |
| `cargo test --workspace` | **344 passed / 0 failed / 60 ignored** |
| `cargo test --workspace --all-features` | **344 passed / 0 failed / 65 ignored** |
| G06 cancellation | 0 tests（N/A：目标 `#![cfg(target_os = "linux")]`） |
| G08 lc08 | **1 passed**（5 个崩溃点，Windows 用 TerminateProcess） |
| ORT contract / real / facade | 13 / 6 / 5 全 passed |
| license audit | PASS（172 包） |
| release gate | FAIL，仅 `['G12','G11']` |
| Windows CLI | 2,738,688 B；`BFD8DC517E07E77B1F5D5C911AC575EFA26A2EE7E8C5EA68618586098293A11A` |
| ORT 缓存 | `.build\ort-cache\dfbin`；`%LOCALAPPDATA%\ort.pyke.io` 时间戳未变（2026-11-06 17:23:54） |

说明：`p7-summary.txt` 是 Windows 脚本原始汇总；其中 `tests=0 all=0` 是 PowerShell 函数返回值的采集问题，
**以本表与 `P7-test-*.txt` 的 `test result` 行为准**。
