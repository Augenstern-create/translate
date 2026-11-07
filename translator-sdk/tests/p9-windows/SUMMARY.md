# P9.5 Windows 回归（commit `a9e4656`）

| 项 | 结果 |
|---|---|
| fmt | PASS |
| clippy 默认 / 全 feature | PASS / PASS |
| `cargo test --workspace` | **372 passed / 0 failed / 60 ignored** |
| `cargo test --workspace --all-features` | **376 passed / 0 failed / 66 ignored** |
| ORT contract / real / facade | 13 / 6 / 5 passed |
| G08 | 1 passed |
| G06 | 0 tests（N/A，Linux 专属） |
| release gate | FAIL（仅 G12/G11） |

P9.5-A 的 Rust LID/lemma/指纹测试在 Windows 通过；Android JNI fastText/lemma 只在 gms2 验证。
