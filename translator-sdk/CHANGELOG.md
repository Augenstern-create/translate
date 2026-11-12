# 变更记录

## S3 外部评测集（2026-11-12）

- 新增 `tools/eval-import`：把 WMT24++（Apache-2.0）与 NTREX-128（CC-BY-SA-4.0）
  转成统一评测格式（`dataset_id`/`source`/`reference`/`domain`/`document_id`/
  `segment_id`/`split`），带去重、语言方向与"源=参考"过滤、文档级切分。
  共 2 954 条：dev 383、test 2 213、private/blind 358（不入库）。
- 新增 `scripts/fetch-eval-data.sh`：只下载允许再分发的数据，记录 URL/版本/许可证/
  日期/sha256 到 `fixtures/eval/external/SOURCES.json`；FLORES+/FLORES-200 gated、
  WMT26 无参考译文，均**未绕过限制**，只记录。
- `quality-runner` 新增 chrF++（字符 1–6gram + 词 1–2gram）与 BLEU-4
  （CJK 按字混排分词、有效阶），以及 `--compare` 多配置对照表。
- 原始语料与转换结果分离：`fixtures/eval/external/` 与 `fixtures/eval/private/` 均已 git 忽略。

## S3 评测基线（2026-11-12）

### blind 集与费用

- 新增 `fixtures/eval/blind.jsonl`（96 条，16 类，与 dev/smoke 零重叠），
  **未运行、不参与调参**，只用于人工评分。
- 新增 `tools/quality-runner/tests/datasets.rs`：数据集结构测试（字段/编号/重复/
  金额不得进 `must_preserve`/blind 不与 dev 重叠/类别齐全）。
- 实测费用（账单端点差值）：dev 100 条 deepseek-v4.1-flash +$0.010304、
  glm-5.3-flash +$0.002824（glm 约便宜 3.6×）；定价表倍率推算与实测不一致，
  已在 `docs/quality-baseline.md` 说明。

- `quality-runner`：新增 `--glossary-from-dataset`（数据集术语 → 临时术语表 + `Enforce`）、
  `--out-md`（人工评审包）、`latency_p95_ms` / `http_calls` / `content_retries` /
  `tokens_*` / `estimated_cost_usd` 字段；评测请求显式使用 `RemoteOnly`。
- 数字检查：英文序数词增加上下文判定（`every second` / `finishes first` / `First, ...`
  不再被当成"第几"），消除一类把正确译文判成漏数字的误报。
- 数据集：6 条 money 用例的金额从 `must_preserve`（逐字）移到 `must_keep_numbers`（语义），
  因为中文写 `12.50 美元` 是正确译文。
- 真实转发站基线：dev 100 条、smoke 30 条，两个模型均 100% 通过机器判定
  （见 `docs/quality-baseline.md`；人工评审仍为 `PENDING_HUMAN`）。

## 磁盘缓存复查修复（2026-11-12）

- **语言标签不再写死**：条目里持久化真实 `source`/`target`，加载时解析，
  非法标签的条目直接跳过（此前会一律报告 en→zh-CN）。
- **命中前重新校验**：缓存文本返回前重跑完整性与质量检查（丢失 URL/占位符、
  数字被改都会被判失效），失效条目立即删除并重译；
  另为每条目增加自校验和，文件被截断/手改/跨版本污染的条目在加载阶段丢弃。
- 磁盘缓存仍**默认关闭**，文档明确"开启后译文会落盘，可能含敏感内容"。
- 真实转发站冒烟：9 组真实请求全部一次成功（详见 `docs/testing.md`）。

## 遗留问题补齐（2026-11-12）

- **分段回退**：每个片段做保守合理性检查（目标语言缺失、长度比例异常），
  答非所问的片段返回 `IntegrityFailure`，不再拼成"看起来正常"的结果（K-03）。
- **数字/时间/币种**：支持 `半/两个半`、`两点半/九点一刻/差一刻十点`、
  ISO 币种代码前后缀与 `₹ ₽ ₩ ₺ ₿ Ξ`（K-04/K-05/K-06）。
- **响应中断**：请求已发出后的失败最多再试一次，避免重复计费（K-14）。
- **段落策略**：`Sentence` / `Group` 真正生效，共享预算与截止时间（K-18）。
- **磁盘缓存**：`[cache] file` / `builder.cache_file` 可选持久化，原子写入、
  损坏文件静默忽略、默认关闭（K-12）。
- **TLS 判定**：扩充确定性错误识别（K-39）。
- **Windows 实机**：CLI 翻译、磁盘缓存跨进程、`serve` CSP/CSRF 与 API 全部实测（K-22 部分完成）。

测试：**386 passed / 0 failed / 4 ignored**（Linux 与 Windows 一致）。

## S2 审查收尾（2026-11-12）

- **Builder 装配路径**：`http_backend()` 与 TOML/配置文件入口改为只收集
  `BackendConfig`，在 `build()` 时用**同一个** `HttpRuntime` 构造所有后端；
  `http_global_concurrency` / `http_queue_size` 对所有入口生效
  （此前每个后端各建一个运行时，全局并发名不副实）。
- **预算扣除时机**：改为取得全局+后端配额、并再次确认未取消/未超时之后才扣；
  排队超时、等待配额失败、未发送即取消不再消耗物理调用额度。
- **观察线程生命周期**：watcher 只持有 `Weak`，最后一个运行时释放后退出；
  新增 `live_watcher_threads()` / `live_worker_threads()` 供诊断与测试。
- **`Slot::is_done()` 不再消费结果**：改为只读检查，修复"观察线程把响应取走、
  Future 永远拿不到结果"的竞态。
- **重试分类收紧**：`Transport` 不再是可重试类别；TLS 证书/握手、非法 URI、
  HTTP 协议错误一律不重试；连接被拒/重置、DNS 临时失败、5xx 仍有限重试。
- **直连引擎入口**：`HttpTranslationEngine::translate()` 在没有协调器预算时
  使用 `1 + max_retries` 的本地预算，避免形成"无预算调用通道"。

测试：新增 4 个 HTTP 回归用例 + 3 个 Builder 共享并发/队列用例 + 3 个错误分类单测；
总计 **372 passed / 0 failed / 4 ignored**。

## 稳定化 S2：HTTP 预算、并发、队列与成本统计（2026-11-12）

### 新增

- **请求预算** `RequestBudget`：一次逻辑翻译共享的物理 HTTP 调用额度（默认 4），
  分段回退、内容重试、传输重试都计入；耗尽返回 `BudgetExceeded`，不会改后端绕过。
- **真正的全局并发**：`[http] max_concurrent_requests`（默认 4）现在由同一
  `Translator` 的所有后端共享（`HttpRuntime` + `Limiter`），后端上限独立生效。
- **有界队列与固定 worker**：`[http] max_queue_size`（默认 16），满则立即
  `QueueFull`；线程数恒定为 `workers + 1`（取消由单一观察线程处理）。
- **统一截止时间**：排队、配额等待、连接、读响应、重试退避共享同一 deadline；
  排队中的取消/超时不会发出网络请求。
- **错误分类**：新增 `QueueFull`、`BudgetExceededLocal`；429/503 支持 `Retry-After`
  （上限 120s，且不超过截止时间）；401/403/400/404 不重试。
- **用量统计**：区分 logical_requests / http_calls_started / completed /
  transport_retries / content_retries / failed_calls / cancelled_requests /
  `potentially_billable_unknown_calls`（已发出但无响应，可能仍计费）。
- 诊断新增 `http_calls`；CLI 的 `--json` 与 Web UI 结果都会显示。

### 配置

```toml
[http]
max_concurrent_requests = 4      # 全局（所有后端共享）
max_queue_size = 16              # 有界队列

[translation]
max_http_calls_per_request = 4   # 一次逻辑请求的物理调用上限
```

旧配置保持兼容；缺省值保证"不会无限并发/无限重试"。

### 测试

新增 20 个 HTTP 限流/预算/队列用例、3 个协调器预算用例、3 个 limiter 单测；
总计 **361 passed / 0 failed / 4 ignored**。

## 稳定化 S1：翻译完整性与安全（2026-11-11）

### 修复

- **统一终检**：格式修复与术语校验移动到质量校验之前，并且都在**最终文本**上执行；
  修复后不通过则退回原候选（`repair=rejected`），修复改动编号/版本/日期/百分比时一律不采用。
  模板路径与模型路径共用同一套终检（模板此前会绕过术语约束）。
- **保护内容按次数严格校验**：模型多吐一份受保护内容会被判为
  `UnexpectedDuplicate`；合法重叠（代码片段里包含同一变量）不误报。
- **Web UI XSS**：词典渲染改用 `createElement` + `textContent`，页面不再出现 `innerHTML`。
- **本地 HTTP 加固**：Host/Origin 校验、每次启动随机 CSRF 令牌、
  请求行/头/体上限（413 在读取请求体之前拒绝）、方法与 Content-Type 校验、读写超时。
- 诊断新增 `repair` 字段（`unchanged` / `applied` / `rejected`）。

### 收尾（安全加固）

- CSRF 令牌改用操作系统 CSPRNG（`getrandom`）生成 256 bit；取不到随机数时拒绝启动。
- `Host` / `Origin` / `x-csrf-token` / `Content-Type` 重复一律 400。
- 增加 CSP：`default-src 'none'`、脚本/样式只认每次响应随机生成的 nonce、
  `connect-src 'self'`，不使用 `unsafe-inline`；内联 style 属性改为 CSS 类。

### 测试

新增 6 个终检白盒用例、3 个保护计数用例、8 个 serve 纯函数用例、
17 个真实 socket 安全用例；总计 **335 passed / 0 failed / 4 ignored**。
另用 headless 浏览器（CDP）实测：CSP 生效、词典恶意释义不被执行、翻译与查词正常。

## v1.0 在线翻译主线重构（2026-11-11）

本次重构把项目从"手写规则/语义翻译引擎"改回**一条可交付的主链**：
模型负责翻译，SDK 负责保护、约束、校验、缓存与错误语义。

### 新增

- `translator-http`：OpenAI 兼容 Chat Completions 后端（远程转发站与本地模型服务同一实现）
  - 同步 HTTP 客户端在 worker 线程执行，用 `Waker` 桥接成 Future（不强制宿主引入 async runtime）
  - 有界重试（共享同一截止时间）、并发闸门、协作式取消、用量与成本估算
  - 错误分类：鉴权 / 限流 / 余额 / 不可用 / 超时 / 响应非法 / 配置错误
  - 回环地址视为**本地**后端，公网地址默认需要显式授权
- 统一保护 `ProtectionPlan`：标记优先（4 种候选，自动避开与源文冲突的形态）+ 分段回退
  - 严格还原：丢失 / 重复 / 未知 / 半截 / 被改写 → `IntegrityFailure`
  - 覆盖 URL、邮箱、模板变量、格式占位符、反引号代码、HTML/模板标签、编号、日期、
    版本号、百分比、十六进制；路径与标识符可配置开启
- `quality/` 拆分：`integrity` / `number` / `heuristic` / `report`
  - 数字语义：中文数字解析、百分比等价、日期等价格式、12/24 小时换算、区间上下限关系
  - 严格项（编号/日期/百分比/受保护内容）致命，启发式项只报告
- 术语表 `Glossary`：源文触发约束（只发命中的）、优先级与冲突规则、译后校验
  - `TermPolicy::Enforce` 语义改为"硬约束不满足即失败"，不再做译后字符串替换
- 模板作用域（app/page）与 `Exact`/`Normalized` 两种匹配方式（`schema_version = 2`）
- 缓存身份重构：模型、部署版本、提示词版本、术语表版本、保护版本、模式、上下文、
  隐私域、生成参数全部进键
- 轻量后端路由 `BackendRouter` 取代 1400 行 `planner`
- CLI 重写：`translate` / `lookup` / `config check|show` / `doctor` / `usage`，默认真翻译
- `quality-runner` 重写为评测运行器（读 `fixtures/eval`，输出可复现 JSON 报告）
- 新增评测集：`fixtures/eval/smoke.jsonl`（30）、`dev.jsonl`（100）

### 移除（归档到 `archive/pre-online-refactor-20261111`）

- crate：`translator-nmt-bergamot`、`translator-nmt-ort`、`translator-runtime`、
  `translator-resource`、`translator-learning`、`translator-jni`、`p33-bench`、`migrate-rules`
- core 模块：`semantic`、`composer`、`pipeline`、`context_sense`、`planner`、`evidence`、
  `plan`、`plan_cache`、`budget`（约 13,000 行）
- `translator-analysis` 从产品主干移除（词形还原的最小能力迁入 `translator-core::language`）
- 旧文档 82 篇、旧 fixtures（约 5.5 MB）、旧 Android/NMT 验证脚手架、旧 Python 工具链

### 行为变化

- 占位符还原失败默认报错（不再"警告后返回缺内容的译文"）
- 空输出、编号被改、受保护内容丢失一律拒绝返回，且不写缓存
- `translate()` 不再自动走词典；查词请用 `lookup()`
- 句子翻译只有一条路径：模板 → 模型（不再有语义/组合/上下文选义分支）
- 远程后端默认需要显式 `preferred_backend` 或 `remote_fallback`（回环地址除外）

### 验收

- `cargo fmt --check` / `cargo clippy -D warnings` / `cargo test --workspace` 全绿
  （305 passed / 0 failed / 4 ignored）
- Windows（MSVC）交叉编译：`cargo xwin check --workspace` + `translator-cli.exe` 链接成功
- 端到端实跑：本地 llama.cpp + Qwen2.5-1.5B-Instruct 完成真实翻译
- 人工质量评审：**PENDING_HUMAN**（详见 `docs/testing.md`）

## 之前的历史

旧架构（P0–P21-R：词典、Bergamot/ORT 本地 NMT、手写语义组合、JNI 等）的变更记录
见归档分支 `archive/pre-online-refactor-20261111` 的同名文件。
