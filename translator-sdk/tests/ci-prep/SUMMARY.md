# G11 本地等价 CI 检查（2026-11-07）

不联网、不上传，只验证 `.github/workflows/ci.yml` 的命令在本机 Linux 可复现。

| 步骤 | 命令 | 结果 |
|---|---|---|
| 上传预检 | `python3 translator-sdk/tools/ci-preflight.py --check` | PASS（无权重/凭据/大文件） |
| 格式 | `cargo fmt --all -- --check` | PASS |
| Clippy 全 feature | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| 默认测试 | `cargo test --workspace --locked` | 391 passed / 0 failed / 60 ignored |
| 全 feature 测试 | `cargo test --workspace --all-features --locked` | 391 passed / 0 failed / 65 ignored |
| 依赖树 | 默认 `translator-api` 无 `translator-runtime`；`translator-runtime` 无 `translator-nmt-bergamot` | PASS |
| 文档 | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features` | PASS |
| 性能冒烟 | `p33-bench empty/dictionary/worker` + `tools/bench-gate.py --case all` | PASS（3 case） |
| 文档编号 | 45 个 `NN_` 文件，无缺前缀/重复 | PASS |
| 许可 | `license-audit.py --check` | PASS（172 包） |
| 发行门禁 | `check-release.py --release-mode` | FAIL（仅 G12/G11） |

原始日志：`clippy-all.log`、`doc.log`、`structure.log`、`bench.log`。
**未执行 `git push` 或远程 CI；G11 仍 open。**
