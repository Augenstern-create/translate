# P9.4 Windows 回归（commit `b2b3c47`）

| 项 | 结果 |
|---|---|
| fmt | PASS |
| clippy 默认 / 全 feature | PASS / PASS |
| `cargo test --workspace` | **370 passed / 0 failed / 60 ignored** |
| `cargo test --workspace --all-features` | **374 passed / 0 failed / 66 ignored** |
| ORT contract / real / facade | 13 / 6 / 5 passed |
| G08 | 1 passed |
| G06 | 0 tests（N/A，Linux 专属） |
| release gate | FAIL（仅 G12/G11） |

P9.4 的 Rust LID/lemma/EP-02 测试在 Windows 通过；Android JNI v3 只在 gms2 模拟器验证。
