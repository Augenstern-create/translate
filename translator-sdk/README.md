# translator-sdk

英译中翻译 SDK（Rust）+ 命令行客户端。**不做 UI**，只提供库与 CLI。

> 架构：**翻译模型负责语言转换，SDK 负责可靠性、可控性、数据处理与跨平台接口。**
> 详见 [`../docs/architecture.md`](../docs/architecture.md)。

---

## 能做什么

- 英文 → 简体中文的句子/段落翻译，后端可换（自部署模型服务或 OpenAI 兼容转发站）
- 英文单词/短语查词（离线词典：FST 索引 + SQLite 释义）
- URL / 邮箱 / 模板变量 / 代码 / 标签 / 编号 / 日期 / 百分比的**严格保护**
- 术语表约束（生成前注入 + 生成后校验）
- 固定 UI 文案整句匹配（精确 / 归一化，带 app/page 作用域）
- 明确的失败、重试、取消、超时行为（每类错误都有独立错误码）
- 用量统计与成本估算（本地提醒，非计费依据）

## 不能做什么（有意为之）

- 不做手写英文语法翻译引擎
- 不做多模型投票 / 反复推理 / 自动润色
- 不内置模型权重，也不代管云端密钥

---

## 快速开始

```bash
cd translator-sdk

# 1) 配置：复制示例并填上你的后端
cp translate.example.toml translate.toml
export TRANSLATE_API_KEY=...          # 密钥只放环境变量

# 2) 自检
cargo run -p translator-cli -- config check
cargo run -p translator-cli -- doctor

# 3) 翻译 / 查词
cargo run -p translator-cli -- translate "The file at {path} was not found."
cargo run -p translator-cli -- translate --file input.txt --json
cargo run -p translator-cli -- lookup bank

# 4) 图形界面（本地网页，仅监听 127.0.0.1）
cargo run -p translator-cli -- serve --port 8765
# 浏览器打开 http://127.0.0.1:8765/
```

作为库使用：

```rust
use translator_api::Translator;
use translator_types::{LanguageTag, TranslateRequest};

let translator = Translator::builder()
    .http_config_file("translate.toml")?
    .glossary_file("glossary.json")?      // 可选
    .cache_capacity(512)
    .build()?;

let zh = LanguageTag::new("zh-CN")?;
let result = translator
    .translate(&TranslateRequest::new("Hello.", zh))
    .await?;
println!("{}", result.text);
```

---

## 工作区结构

```text
crates/translator-types        公共类型
crates/translator-plugin-api   插件契约
crates/translator-http         OpenAI 兼容后端（远程/本地同一实现）
crates/translator-core         流水线：保护/质量/术语/模板/缓存/路由/协调器
crates/translator-dictionary   离线词典
crates/translator-rules        确定性规则包
crates/translator-analysis     研究线（不参与产品构建）
apps/translator-cli            命令行客户端
tools/pack-dictionary          词典打包
tools/quality-runner           评测运行器
prompts/en_zh/*.txt            版本化提示词
fixtures/eval/*.jsonl          评测集（smoke / dev）
```

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

当前基线：`305 passed / 0 failed / 4 ignored`（详见 [`../docs/testing.md`](../docs/testing.md)）。

## 文档

| 文件 | 内容 |
|---|---|
| [`../docs/architecture.md`](../docs/architecture.md) | 模块地图、主链路、数据格式、缓存身份 |
| [`../docs/development.md`](../docs/development.md) | 配置、CLI、提示词、本地模型、安全 |
| [`../docs/testing.md`](../docs/testing.md) | 测试分层、评测集、验收门禁、人工评审清单 |
| [`../docs/known-issues.md`](../docs/known-issues.md) | 已知问题与 PENDING_HUMAN 清单 |
| [`../docs/migration.md`](../docs/migration.md) | 从旧架构迁移 |
| [`../docs/refactor/`](../docs/refactor/) | 本轮重构的基线与验收报告 |

## 许可

Apache-2.0 OR MIT。第三方声明见 `THIRD_PARTY_NOTICES.md`。
