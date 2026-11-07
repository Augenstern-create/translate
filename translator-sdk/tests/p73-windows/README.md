# P7.3 Windows 复测证据

- 主机：NAN-WINDOWS（`ssh home`，Windows 11 25H2，Rust 1.99.1，Python 3.13.5）
- 工作区：`E:\augenstern\translate`
- 基线：`0ed70db`
- 原始日志：Windows `.build\reports\P7-*`；本目录为 UTF-8 转存，未改内容。

复现命令（在 Windows 工作区）：

```powershell
Set-Location E:\augenstern\translate\translator-sdk
. .\tools\p5\windows-build-env.ps1 -Root E:\augenstern\translate
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --all-features
cargo test -p translator-runtime --test cancellation -- --nocapture   # Windows: 0 tests (N/A)
cargo test -p translator-resource --test p32_lifecycle lc08 -- --nocapture
$env:P4_ORT_MODEL_DIR = "$PWD\..\.build\model-cache\ort-model"
cargo test -p translator-nmt-ort --features ort-runtime --test p4_contract -- --ignored
cargo test -p translator-nmt-ort --features ort-runtime --test p4_ort -- --ignored
cargo test -p translator-api --features nmt-ort-runtime --test nmt_ort_facade -- --ignored
D:\Anaconda3\python.exe tools\license-audit.py --check
D:\Anaconda3\python.exe tools\check-release.py --release-mode
cargo build --release -p translator-cli
```

汇总见 `p7-summary.txt`；报告见 `docs/45_P7.3_Windows复测报告.md`。
