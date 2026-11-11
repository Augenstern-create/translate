# 开发指南

---

## 1. 环境

| 项目 | 要求 |
|---|---|
| Rust | stable ≥ 1.85（本仓库以 1.99 验证） |
| 平台 | Linux（首要验证）、Windows（交叉编译已通过，实机见 `docs/testing.md`） |
| 组件 | `rustfmt`、`clippy` |
| 网络 | 构建需要 crates.io；翻译需要可访问的后端（自部署或转发站） |

```bash
cd translator-sdk
cargo build            # 默认构建：无模型、无 native 依赖
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

---

## 2. 配置文件

默认读取 `./translate.toml`（可用 `--config` 或 `TRANSLATE_CONFIG` 覆盖）。

```toml
# ---------- 顶层：可选数据文件 ----------
glossary  = "glossary.json"     # 术语表
templates = "templates.json"    # 固定 UI 文案
dictionary = "resources/dict"   # 离线词典包目录
rules = "rules.bin"             # 确定性规则包（可选）

# ---------- 后端 ----------
[backend.remote]
enabled = true
base_url = "https://your-relay.example/v1"
model = "configured-model-id"
model_revision = "2026-11-01"        # 必填：缓存要能区分模型替换
api_key_env = "TRANSLATE_API_KEY"    # 只写变量名，不写密钥
timeout_ms = 30000
max_output_tokens = 2048
max_retries = 2
max_concurrent_requests = 4
disable_thinking = false
thinking_style = "none"              # none | chat_template | reasoning_effort | thinking_type
# extra_body = { top_p = 0.9 }       # 供应商特有字段

[backend.local]
enabled = false
base_url = "http://127.0.0.1:8080/v1"
model = "local-translation-model"
api_key_env = ""                     # 本机服务无需鉴权
model_revision = "llama.cpp-b6000"

# ---------- 行为 ----------
[translation]
mode = "precise"                     # precise | fast
term_policy = "warn"                 # off | warn | enforce
remote_fallback = "disabled"         # disabled | on-local-unavailable | on-local-failure
preferred_backend = "remote"         # 可选；不写时 CLI 用第一个启用的后端
max_input_chars = 20000
strict_integrity = true              # 产品必须为 true

[cache]
capacity = 256
```

**关于 `preferred_backend` 与远程授权：**

- 回环地址（`127.0.0.1` / `localhost`）视为**本地**后端，默认策略就能用；
- 公网地址视为**远程**后端：`AutoLocalFirst` 不会自动使用它，
  必须显式设置 `preferred_backend`，或把 `remote_fallback` 打开；
- CLI 在"只有一个启用后端"时会自动把它设为默认（用户写下 base_url 即视为授权）。

---

## 3. 命令行

```bash
cargo build -p translator-cli
CLI=./target/debug/translator-cli

$CLI config check            # 装配校验（会真正构造 Translator）
$CLI doctor                  # 环境自检：配置/密钥/数据文件
$CLI translate "Hello."      # 翻译
$CLI translate --file in.txt --json
$CLI lookup bank --limit 3   # 查词（需要词典包）
$CLI usage                   # 本次会话的用量与估算费用
$CLI serve --port 8765       # 本地翻译页面（仅监听 127.0.0.1）
```

`serve` 是一个内嵌的单页界面（无外部资源、无前端构建链），提供翻译与查词两个功能框，
并把引擎、耗时、保护策略、质量摘要显示在结果下方。

**安全边界（S1 已加固）**：

- 只绑定 `127.0.0.1`，没有"对外开放"选项；
- 校验 `Host`（只接受 `127.0.0.1:<port>` / `localhost:<port>`）与 `Origin`（只接受同源），
  防止任意网页向本机服务提交写请求；
- 每次启动用**操作系统 CSPRNG** 生成 256 bit CSRF 令牌（取不到随机数就拒绝启动），
  页面通过 `<meta>` 取得，POST 必须带 `x-csrf-token`；
- 页面响应带 CSP：`default-src 'none'`，脚本与样式只认每次响应随机生成的 nonce，
  `connect-src 'self'`，不使用 `unsafe-inline`；
- 请求行 ≤ 8 KiB、请求头 ≤ 16 KiB、请求体 ≤ 256 KiB；超限在读取前直接 413/431；
- 只接受 GET/HEAD/POST，POST 必须是 `application/json`；
- 页面渲染一律 `textContent`，不解析词典数据为 HTML。

退出码：`0` 成功、`1` 运行错误、`2` 用法错误。错误信息里给出错误码，
例如 `（AuthenticationFailed）`、`（DictionaryUnavailable）`。

---

## 4. 提示词

位于 `prompts/en_zh/`，通过 `include_str!` 内嵌，也可用 `prompt_dir` 从磁盘覆盖。

**改提示词必须改版本号**（例如 `precise_v2`），因为版本会进缓存键。
改完必须跑评测（见 `docs/testing.md`），不允许"看到一个坏例子就改全局提示词"。

---

## 5. 术语表

```json
{
  "schema_version": 1,
  "id": "glossary.software",
  "version": 3,
  "entries": [
    { "id": "t1", "source": "pull request", "target": "拉取请求", "strict": true },
    { "id": "t2", "source": "large language model", "target": "大语言模型", "domain": "ai" }
  ]
}
```

- `strict = true`：`term_policy = "enforce"` 时不满足即判本次翻译失败；
- 术语表版本参与缓存身份，改术语表等于让相关缓存失效；
- 相同源术语在同一 `domain` 下给出不同译文 → **加载期报错**（不允许随机选择）。

---

## 6. 固定 UI 文案模板

```json
{
  "schema_version": 2,
  "id": "templates.ui",
  "version": 2,
  "source": "en",
  "target": "zh-CN",
  "entries": [
    { "source": "Save", "target": "保存" },
    { "source": "Settings", "target": "偏好设置", "app": "com.acme.editor" },
    { "source": "Cancel", "target": "取消", "match": "normalized" }
  ]
}
```

- 默认 `match = "exact"`（逐字，仅去首尾空白）；
- `normalized` 会折叠空白、转小写、去句末标点——只有明确声明时才启用
  （`Really?` 与 `Really!` 语气不同）；
- `app` / `page` 作用域更具体者优先；
- 模板命中后**同样**要过完整性校验：模板里带 `{filename}` 就必须出现在译文里。

---

## 7. 本地模型（Phase 5）

本地模型走**同一套 HTTP 后端**，典型部署：

```text
SDK → HTTP → llama.cpp server / vLLM / TGI → 翻译模型
```

```bash
# 示例：llama.cpp 自带 OpenAI 兼容服务
llama-server -m ./models/your-translation-model.gguf --port 8080 --ctx-size 4096
```

然后配置：

```toml
[backend.local]
base_url = "http://127.0.0.1:8080/v1"
model = "your-translation-model"
api_key_env = ""
model_revision = "llama.cpp-b6000"
```

**准入标准**（不得只因为"能启动"就上线）：质量、数字准确率、术语遵循率、
延迟、内存占用、连续请求稳定性、许可证。当前状态：**尚无用真实权重跑出的对比数据**
（见 `docs/known-issues.md`）。

---

## 8. 安全

- 配置文件只出现 `api_key_env`（变量名），**永不**出现密钥；
- 错误信息、日志、`Debug` 输出都不包含密钥；
- 不把用户原文写进日志（诊断里的 `protected_spans` 只回给发起方）；
- 提交前跑一遍 CI 的秘密扫描（`.github/workflows/ci.yml`）。

---

## 9. 提交前检查单

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

改动了这些地方时，额外要求：

| 改动 | 额外要求 |
|---|---|
| 保护/还原逻辑 | 必须新增或更新 `protection` 用例（含标记丢失、重复、冲突） |
| 数字检查 | 必须覆盖"允许中英互转"和"严格标识必须保真"两侧 |
| 提示词/模型 | 必须跑 `fixtures/eval/smoke.jsonl`，并在报告里写明版本 |
| 公共 API | 更新 `docs/migration.md` 与 CHANGELOG |
