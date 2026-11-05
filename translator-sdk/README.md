# translator-sdk

轻量跨平台混合翻译 SDK。**Rust Library，不实现 UI**，预留桌面 / Android / CLI / 服务端适配接口。

设计文档见仓库根目录 `../docs/`：

- `01_翻译SDK完整开发设计.md` — 架构与公共 API
- `02_翻译资源候选与备选清单.md` — 词典 / 模型 / 运行时候选
- `03_P0骨架与验证报告.md` — P0 骨架与验证证据
- `04_P1_字典与离线查词报告.md` — P1 词典包格式、基准与限制清单
- `05_审查响应与P2实施计划.md` — 外部审查的逐条处置、ADR-008~014、P2 拆解与验收映射

---

## 当前状态：P0 骨架 + P1（离线查词 + 最小规则引擎）已完成

- **P0**：workspace 结构、核心类型、插件契约、协调器与路由、缓存、门面 API、Mock 插件、CLI。
  **不接任何真实模型、不装任何 native 库**，整条链路即可运行 —— 这是设计文档规定的交付判据。
- **P1**：词典包格式 v1（`index.fst` + `entries.sqlite` + `manifest.json` + `LICENSE.txt`）、
  CC-CEDICT 打包工具、只读 mmap 查询、启发式语言识别、占位符保护。
  已用完整 CC-CEDICT（121,584 词条 / 197,593 键）验证，查询 p50 约 17 μs（release，本机）。
- **P1 规则**：声明式 JSON 规则包（`translator-rules`），构建期编译为带 sha256 的只读二进制，
  **强制金样本校验**（规则改坏了拒绝加载），支持 `nfkc` 与 `replace`（术语替换/缩写展开）两种动作、
  语言对门控、优先级、`contains`/`is_single_word` 条件。短输入开销约 287 ns/次。

词库与规则都是**可选 feature**：不开 `dictionary` 时 `translator-api` 依赖树里没有 `rusqlite`；
不开 `rules` 时没有 JSON 解析依赖（但 `TextRules` 契约始终可用，宿主可自己实现）。

## 目录结构

```text
translator-sdk/
├── crates/
│   ├── translator-types/       # 请求/结果/错误/语言标签/能力描述（零外部依赖）
│   ├── translator-plugin-api/  # Engine / Provider / Backend trait + 注册表（含 mocks）
│   ├── translator-core/        # 协调器、路由、缓存、文本预处理
│   ├── translator-analysis/    # 语言识别（当前为启发式，可换 fastText）
│   ├── translator-rules/       # 声明式规则包：编译、金样本校验、执行
│   ├── translator-dictionary/  # 词典包读取、FST+SQLite 查询、引擎实现
│   └── translator-api/         # 唯一对外入口 + Builder
├── apps/
│   └── translator-cli/         # 调试与演示命令行
├── tools/
│   └── pack-dictionary/        # CC-CEDICT → 资源包
├── fixtures/                   # 金样本 + 示例规则包（rules/minimal.json）
├── resources/                  # 打包产物（体积大，gitignore）
└── Cargo.toml                  # workspace
```

依赖方向严格单向：

```text
translator-types  ←  translator-plugin-api  ←  translator-core  ←  translator-api  ←  CLI
                                                   ↖ translator-analysis
                                                   ↖ translator-rules（实现 core 的 TextRules 契约）
translator-dictionary  ←  translator-api（仅 `dictionary` feature）
translator-rules       ←  translator-api（仅 `rules` feature）
```

`translator-types` 与 `translator-plugin-api` **不依赖任何** native runtime、HTTP 客户端或 SQLite。
`translator-dictionary` 是唯一引入 SQLite（`rusqlite` + `bundled`）的 crate，且只在启用
`dictionary` feature 时才被编进 `translator-api`。

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

### 挂载真实词典包

`resources/zh-en/` 不入库，需本地生成（约 3 秒）：

```bash
curl -L -o /tmp/cedict.gz https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz
gunzip -c /tmp/cedict.gz > /tmp/cedict

cargo run --release -p pack-dictionary -- /tmp/cedict resources/zh-en \
  --id dictionary.cc-cedict.zh-en --version 0.1.0 --language zh-Hans --target en

cargo run --release -p translator-cli -- --dict resources/zh-en lookup 计算机
cargo run --release -p translator-cli -- --dict resources/zh-en translate 世界 --explain
```

宿主侧最小用法（不挂 `dictionary_package` 就退回内置 Mock 词表）：

```toml
translator-api = { path = "crates/translator-api", features = ["dictionary"] }
```

```rust
let translator = translator_api::TranslatorBuilder::new()
    .dictionary_package("resources/zh-en")
    .build()?;
```

词典包格式、基准数据（含一次“基准数据造假”事故记录）与限制清单见
`../docs/04_P1_字典与离线查词报告.md`。

### 挂载规则包

规则包是 JSON（开发格式）或带 magic+sha256 的二进制（发布格式），**每条规则必须带金样本**，
加载时会真跑一遍，跑不过就拒绝加载：

```bash
cargo run --release -p translator-cli -- --rules fixtures/rules/minimal.json translate "ｃａｔ" --explain
# → 规则包：id rules.zh-en.minimal v1 / 规则数 3 / 生效 norm.nfkc / 处理后 cat
# → 引擎 mock.dictionary  译文 猫

cargo run --release -p translator-cli -- translate "ｃａｔ"
# → 引擎 mock.nmt（没有规则时全角输入查不到词条，只能退化）
```

宿主侧只需一行（`rules_file` 失败会在 `build()` 报错，不会静默降级成"没有规则"）：

```rust
let translator = translator_api::Translator::builder()
    .dictionary_package("resources/zh-en")
    .rules_file("fixtures/rules/minimal.json")
    .build()?;
```

规则格式与设计取舍（为什么不做正则、为什么规则 trace 不进 `TranslationResult`）见
`../docs/04_P1_字典与离线查词报告.md` §5。

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

## 尚未实现

- 除 CC-CEDICT 之外的词库（Kaikki / JMdict 各有解析器与许可审查）
- en→zh 方向的词库（CC-CEDICT 是汉英单向）
- 真正的语言识别（fastText `lid.176.ftz`），当前是零模型启发式
- 规则包的输出侧能力：术语**约束**（校验译文包含指定词）、分句、词形；正则类匹配按 §7.3 的风险评估后决定不做
- 词形分析（Lindera 等）
- 资源包管理（安装/卸载/回滚/租约/内存硬预算）
- Bergamot / ncnn / ONNX Runtime 适配
- C ABI / JNI 绑定
- 远程 LLM Provider 与凭据注入

各项的具体原因与去向见 `../docs/04_P1_字典与离线查词报告.md` §8、§9。

## 许可证

Apache-2.0 OR MIT（见 `Cargo.toml`）。第三方资源包另行登记，勿默认继承本仓库许可。
