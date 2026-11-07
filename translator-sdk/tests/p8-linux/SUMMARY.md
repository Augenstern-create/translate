# P8 Linux 结果汇总（commit `3cecd27`）

| 检查 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | **401 passed / 0 failed / 60 ignored** |
| `cargo test --workspace --all-features` | **401 passed / 0 failed / 65 ignored** |
| `cargo test -p translator-jni --lib` | 10 passed（含 m01/m03/m04/m06） |
| `cargo test -p translator-api --features nmt-bergamot --test shared_engine` | 4 passed |
| `cargo test -p translator-core --test cache_fingerprint` | 6 passed |
| `cargo test -p translator-dictionary --lib` | 1 passed（content_digest） |

日志：`full-test.log`、`all-features.log`、`clippy-default.log`、`clippy-all.log`。
