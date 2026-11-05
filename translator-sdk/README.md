# translator-sdk

轻量跨平台混合翻译 SDK。**Rust Library，不实现 UI**，预留桌面 / Android / CLI / 服务端适配接口。

设计文档见仓库根目录 `../docs/`：

- `01_翻译SDK完整开发设计.md` — 架构与公共 API
- `02_翻译资源候选与备选清单.md` — 词典 / 模型 / 运行时候选
- `03_P0骨架与验证报告.md` — 当前实现状态与验证证据

---

## 当前状态：P0（架构骨架）已完成

已实现：workspace 结构、核心类型、插件契约、协调器与路由、缓存、门面 API、Mock 插件、CLI。
**不接任何真实模型、不装任何 native 库**，整条链路即可运行 —— 这是设计文档规定的交付判据。

## 目录结构

```text
translator-sdk/
├── crates/
│   ├── translator-types/       # 请求/结果/错误/语言标签/能力描述（零外部依赖）
│   ├── translator-plugin-api/  # Engine / Provider / Backend trait + 注册表（含 mocks）
│   ├── translator-core/        # 协调器、路由、缓存、文本预处理
│   └── translator-api/         # 唯一对外入口 + Builder
├── apps/
│   └── translator-cli/         # 调试与演示命令行
├── fixtures/                   # 金样本（P1 起使用）
└── Cargo.toml                  # workspace
```

依赖方向严格单向：

```text
translator-types  ←  translator-plugin-api  ←  translator-core  ←  translator-api  ←  CLI
```

`translator-types` 与 `translator-plugin-api` **不依赖任何** native runtime、HTTP 客户端或 SQLite。

## 快速开始

需要 Rust 工具链（本机装在 `/opt/rust`）：

```bash
source /opt/rust/env.sh
```

构建与测试：

```bash
cargo build
cargo test --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo fmt --all --check
```

运行 CLI：

```bash
cargo run -p translator-cli -- translate hello --to zh-Hans --explain
cargo run -p translator-cli -- translate "good morning my friend" --to zh-Hans --explain
cargo run -p translator-cli -- translate elephant --to zh-Hans        # 词典未命中 → 回退 NMT
cargo run -p translator-cli -- translate hello --to zh-Hans --policy remote   # 应被拒绝
cargo run -p translator-cli -- lookup cat --lang en
cargo run -p translator-cli -- detect "你好"
cargo run -p translator-cli -- capabilities
```

## 设计要点（来自设计文档，已在代码中体现）

| 原则 | 落地位置 |
|---|---|
| 本地优先，远程**默认关闭** | `TranslationPolicy::AutoLocalFirst`；`MockRemoteEngine` 默认无授权，测试覆盖 |
| 查词与整句翻译分开建模（ADR-003） | `TranslationEngine` vs `DictionaryProvider` 两套接口 |
| 插件可替换，公共 API 不变 | 全部依赖 trait，`Coordinator` 不认识具体引擎 |
| 兼容性显式声明，不靠字符串猜 | `InferenceBackend::supports_model()` 返回 `CompatibilityReport` |
| 占位符丢失必须告警 | `TranslationWarning::PlaceholderRestoreFailed`，有测试覆盖 |
| 未安装资源要给出安装建议 | `TranslationError::suggested_packages`，测试覆盖 |
| 缓存键包含全部影响维度 | `CacheKey`：文本哈希 / 语言对 / 引擎 / 术语表 / 规则版本 / 解码配置 / 隐私域 |

## 尚未实现（P1 起）

- 真实词典（CC-CEDICT / Kaikki / JMdict）与 FST+SQLite 存储
- 真正的语言识别（fastText `lid.176.ftz`）
- 规则 DSL 与规则包编译
- 资源包管理（Manifest、校验、租约、内存预算）
- Bergamot / ncnn / ONNX Runtime 适配
- C ABI / JNI 绑定
- 远程 LLM Provider 与凭据注入

## 许可证

Apache-2.0 OR MIT（见 `Cargo.toml`）。第三方资源包另行登记，勿默认继承本仓库许可。
