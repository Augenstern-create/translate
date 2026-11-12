# 测试与评测

---

## 1. 分层

| 层级 | 位置 | 目的 | 是否自动 |
|---|---|---|---|
| 单元测试 | `crates/*/src/**` 的 `#[cfg(test)]` | 每个模块的确定性行为 | 是 |
| 集成测试 | `crates/*/tests/` | 真实 TCP 报文、完整流水线 | 是 |
| 评测（机器可判定） | `fixtures/eval/*.jsonl` + `quality-runner` | 保护/数字/术语是否被破坏 | 是 |
| 人工质量评审 | 人工阅读译文 | 准确、完整、流畅、术语、格式 | **否（PENDING_HUMAN）** |

**自动指标永远不能替代人工评审。** 任何"质量已达标"的结论都必须有人读过译文。

---

## 2. 自动测试

```bash
cd translator-sdk
cargo test --workspace                 # 全部
cargo test -p translator-core --lib    # 只跑核心单元测试
cargo test -p translator-http          # HTTP 后端（含真实本地 mock 服务端）
cargo test -p translator-api           # 端到端流水线（脚本化引擎）
```

当前基线（Linux，`refactor/online-translation-v1`）：

```text
passed = 395   failed = 0   ignored = 4
```

覆盖的关键场景：

**保护（`protection` + `translator-api/tests/pipeline.rs`）**

- 单个/多个/重复/相邻占位符
- URL（含 query、端口、数字）、邮箱、反引号代码、三反引号代码块
- HTML 标签、模板标签、`%s` / `%1$s` / `%.2f` 与 `15%` 共存
- 编号 `#10248`、日期 `2026-11-11`、版本号 `v2.1.3`
- 标记丢失 / 重复 / 半截 / 被模型改写 → 必须报错
- 语序调整允许、内容替换禁止
- 源文自带 `⟦P1⟧` 时自动换标记；四种标记全冲突时回退分段
- 模型吞标记 → 自动重试并回退分段 → 成功且内容不丢
- 模型对分段返回空串 → 不得静默成功
- 多行文本换行结构保持；中文/emoji 不劈开字节

**数字（`quality::number`）**

- 数量词允许中英互转（`three books` ↔ `三本书` / `3 本书`）
- 百分比支持 `15%` / `百分之十五`，`50%` 判不一致
- 金额校验数值与币种；币种丢失只报告（Major）
- 日期等价格式；改月份/日期致命
- 区间上下限被改致命
- 千分位与空格千分位等价

**后端（`translator-http`，真实本地 HTTP 服务）**

- 请求路径/鉴权头/模型字段/messages 结构
- 401/403 不重试；429/5xx 有界重试；重试耗尽报不可用
- 超时、取消（立即返回）、并发上限
- 截断输出（`finish_reason=length`）不得当成功
- 密钥缺失时不发请求
- 术语与占位符进入 system 消息，原文只出现一次

**终检（`translator-core` 白盒，S1-01）**

- 修复不破坏内容 → 采用（`repair=applied`）
- 修复会拆坏受保护版本号 → 退回原候选（`repair=rejected`）
- 修复会改动版本号等严格数字 → 不采用
- 候选与修复后都不通过 → `IntegrityFailure`
- 严格术语在**最终文本**上重新核验（Enforce 时失败）

**Web 界面安全（`apps/translator-cli/tests/serve_security.rs`，真实 socket）**

- 首页可访问、带随机 CSRF 令牌与安全响应头；页面不含 `innerHTML`
- 合法同源请求可完成翻译；健康检查不需要令牌
- 缺少/错误 CSRF → 403；跨站 Origin → 403；非法 Host → 403
- 重复的 Host / Origin / x-csrf-token / Content-Type → 400
- 重启服务后 CSRF 令牌变化；令牌为 256 bit（64 个十六进制字符）
- 首页带 CSP：`default-src 'none'`、脚本/样式只认 nonce、无 `unsafe-inline`；
  header 中的 nonce 必须与 `<script nonce>` / `<style nonce>` 一致
- 超大 Content-Length → 413 且**不读取请求体**（有耗时断言）
- 非法 Content-Length → 400；chunked → 400；超大请求头 → 431
- 非 JSON Content-Type → 415；PUT → 405；非法 JSON → 400

**HTTP 预算 / 并发 / 队列（`crates/translator-http/tests/s2_limits.rs`）**

- 成功请求只消耗 1 次调用；传输重试各消耗 1 次；预算耗尽返回 `BudgetExceeded`
  且不再发请求（用服务端命中次数断言）
- 同一 `RequestBudget` 跨多次发送共享；32 线程竞争 10 次额度恰好成功 10 次
- 全局并发（2）在两个后端之间共享，峰值 ≤2；单后端上限（1）独立生效
- 失败路径结束后 `global.active() == 0`（无配额泄漏）
- 队列满立即 `QueueFull`；排队中取消/超时都不发到服务端（服务端命中数=1）
- `workers`/`queue_capacity` 与配置一致
- 401/403 不重试；500/502/503 重试后 `BackendUnavailable`；504/408 → `BackendTimeout`
- 429 遵守 `Retry-After: 1`（≥900ms 后再试）；非法响应体不触发传输重试
- 取消后不再重试（服务端命中 ≤2）
- 状态检查不消费结果：`Slot::is_done()` 连调 5 次后 Future 仍能拿到响应；
  worker 完成 / 观察线程检查 / Future 轮询并发时不丢响应
- 运行时销毁后观察线程与 worker 全部退出（`live_watcher_threads` / `live_worker_threads`
  回到基线，最多等 3 秒）
- 直接调用 `engine.translate()`（绕过协调器）也受 `1 + max_retries` 预算约束
- 证书/协议类确定性错误不重试；连接重置/被拒仍可重试
- Builder 装配的两个后端共享全局并发（峰值 ≤2）与队列上限（队列 1 时出现 `QueueFull`）
- 分段回退：答非所问的片段返回 `IntegrityFailure` 且不写缓存
- 段落策略：`Sentence` 三句 → 三次调用、拼接正确；预算 2 时第三句被拒
- 数字/时间/币种：`两点半`、`九点一刻`、`USD 39.99`、`39.99 EUR`、`0.5 BTC`、`₹500` 均判等价
- 响应中断：请求已发出后失败最多重试一次，且计入疑似计费
- 磁盘缓存：写盘后用新实例加载即命中（引擎只被调用一次）；损坏/版本不符的文件被忽略；
  条目自校验和被篡改时丢弃；语言标签按磁盘记录恢复（ja→ko 不会被写成 en→zh-CN）；
  命中缓存但丢失受保护内容的译文会被判失效并重译

**流水线（`translator-api`）**

- 缓存命中不重复调用模型；模式/术语表版本/规则版本变化必须失效
- 模板命中不调用模型；模板作用域生效
- 术语 `Warn` 报告 / `Enforce` 拒绝
- 预取消请求不调用模型
- 无后端 / 非法配置 / 非法术语表 / 非法模板 → 装配期报错

---

## 3. 评测集

| 文件 | 条数 | 用途 |
|---|---:|---|
| `fixtures/eval/smoke.jsonl` | 30 | 每次改动快速验证 |
| `fixtures/eval/dev.jsonl` | 100 | 提示词/模型/策略研发 |

字段说明见 `fixtures/eval/README.md`。要点：

- `must_preserve`：必须逐字出现在译文里的片段；
- `must_keep_numbers`：必须字面保留的关键数字；
- `glossary`：应满足的术语；
- `reference_zh`：**全部为 `null`** —— 人工参考译文尚未产生。

运行：

```bash
cargo run -p quality-runner -- \
  --dataset fixtures/eval/smoke.jsonl \
  --config translate.toml \
  --out /tmp/smoke-report.json
```

退出码：`0` 全部机器可判定项通过、`1` 有失败、`2` 用法/配置错误。
报告里的 `human_review` 字段固定为 `PENDING_HUMAN`。

**防止评测污染（§16.4）：**

- Blind Test 不参与提示词/模板编写；
- 不允许因为 blind 集某条错误就改提示词后继续宣称"独立测试"；
- 报告必须写明模型、提示词版本、术语表版本、运行日期；
- `PENDING_HUMAN` 不等于"通过"。

---

## 3.1 浏览器端到端检查（手工，可选）

CSP 与 XSS 只靠 Rust 测试无法完全证明——需要真实浏览器执行页面脚本。

```bash
# 1) 起一个 mock 模型 + 带恶意释义的词典包 + UI
python3 mock_openai.py &                       # 任意 OpenAI 兼容 mock
pack-dictionary dict.jsonl ./dictpkg --format jsonl --id dictionary.xss.test --version 1.0.0 \
  --language en --target zh-CN --license MIT   # 释义里放 <img src=x onerror=alert(1)>
translator-cli serve --port 18096 &

# 2) 用 CDP 驱动 headless chromium（Node 22 自带 WebSocket）
node browser_check.mjs http://127.0.0.1:18096/
```

判定标准（本轮实测）：令牌长度 64、译文正常返回、词典文本原样显示为**文本**、
`#dict img` 元素数为 0、页面标题未被改写、无 CSP 违规；
另外用"动态插入内联 `<script>`"做正向控制，确认被 CSP 拦截。

## 4. 验收门禁（自动部分）

以下任何一项不满足即视为不合格：

| 门禁 | 要求 |
|---|---|
| `cargo fmt --check` | 无差异 |
| `cargo clippy -- -D warnings` | 零警告 |
| `cargo test --workspace` | 0 失败 |
| 保护用例 | 100% 通过（含"多吐一份"检测） |
| 确定性数字/编号用例 | 100% 通过 |
| 空输出 | 不得作为成功返回 |
| 保护还原失败 | 不得写入缓存 |
| smoke 评测 | `failed = 0` |

---

## 5. 人工评审清单（PENDING_HUMAN）

当前**没有**执行过人工评审，因此以下条目全部为待办：

| 项目 | 状态 | 说明 |
|---|---|---|
| `fixtures/eval/*.jsonl` 的 `reference_zh` | PENDING_HUMAN | 需要人工翻译并复核 |
| 真实转发站译文抽样（≥50 条） | PENDING_HUMAN | 需要真实 base_url + 密钥 |
| 本地模型质量对比（Phase 5） | PENDING_HUMAN | 需要模型权重与实测延迟 |
| Windows 实机运行 | PENDING_HUMAN | 已做交叉编译检查，未在真实 Windows 上跑 |
| 术语表/模板业务验收 | PENDING_HUMAN | 需要业务方确认用词 |

人工评审记录格式建议：

```text
日期 / 模型 id / model_revision / 提示词版本 / 术语表版本
每条：准确性(1-5) 完整性(1-5) 流畅性(1-5) 术语(符合/不符合) 格式(保留/丢失)
结论：通过 / 需返工（列出至少 3 条具体例子）
```

---

## 6. Windows 验证现状

**已做（2026-11-12）**

| 项目 | 方式 | 结果 |
|---|---|---|
| 全工作区编译（含测试目标） | `cargo xwin check --workspace --all-targets --target x86_64-pc-windows-msvc` | 通过 |
| CLI 二进制链接 | `cargo xwin build -p translator-cli --target x86_64-pc-windows-msvc` | 生成 `translator-cli.exe` |
| 单元 + 集成测试 | Windows 实机 `cargo test --workspace --no-fail-fast` | **395 passed / 0 failed / 4 ignored**（S3 blind 集与费用测量后复跑） |

**Windows 端到端（2026-11-12 实测）**

| 场景 | 结果 |
|---|---|
| `translator-cli translate "Hello." --config ... --json` | `"text": "你好，世界。"`，`attempts=1` |
| 第二次运行（新进程） | `cached=true`、`http_calls=0`、`warnings=["CacheHit"]`（磁盘缓存） |
| `serve` 页面 | 200；`Content-Security-Policy` 含 per-response nonce，无 `unsafe-inline`；token 长度 64 |
| `POST /api/translate`（带令牌+Origin） | 200，`cached=true` |
| `POST /api/translate`（无令牌） | 403 |

**真实转发站冒烟（2026-11-12）**

通过 OpenAI 兼容转发站（`https://starhub.iepose.cn/v1`，模型 `deepseek-v4.1-flash`）
用真实密钥跑通主链；密钥只从环境变量 `STARHUB_KEY` 读取，未写入仓库或日志。

| 输入 | 输出 | attempts | HTTP 调用 | 保护 | 质量 |
|---|---|---|---|---|---|
| `Hello, world.` | 你好，世界。 | 1 | 1 | marker | 0 issue |
| `Order #10248 has shipped.` | 订单 #10248 已发货。 | 1 | 1 | marker | 0 issue |
| `Visit https://example.com/docs for details.` | 访问 https://example.com/docs 了解详情。 | 1 | 1 | marker | 0 issue |
| `You cannot access the account unless the owner approves it.` | 除非所有者批准，否则你无法访问该账户。 | 1 | 1 | marker | 0 issue |
| `The price is USD 39.99, a 15% discount, valid until 2026-12-31 at 2:30 pm.` | 价格为 39.99 美元，可享 15% 折扣，有效期至 2026-12-31 下午 2:30。 | 1 | 1 | marker | 0 issue |
| `If the build fails, ...` | 如果构建失败，管道会停止，并立即通知团队。 | 1 | 1 | marker | 0 issue |
| 长句（38 词，单句） | 部署流水线会验证每一次提交……（完整） | 1 | 1 | marker | 0 issue |
| Fast 模式 | 服务器暂时不可用，请稍后再试。 | 1 | 1 | marker | 0 issue |
| 错误密钥 | `AuthenticationFailed`（HTTP 401），**未重试** | 1 | 1 | — | — |

同一句第二次请求（新进程、开启磁盘缓存）：`cached=true`、`http_calls=0`、
`warnings=["CacheHit"]`，缓存文件 588 字节。

**S3 评测基线（2026-11-12）**

`quality-runner` 新增：`--glossary-from-dataset`（用数据集术语对生成临时术语表并按 `Enforce` 跑）、
`--out-md`（人工评审包）、以及 `http_calls` / `content_retries` / `tokens_*` /
`estimated_cost_usd` / `latency_p95_ms` 等报告字段。评测统一用 `TranslationPolicy::RemoteOnly`
显式授权远程后端。

结果（真实转发站，详见 `docs/quality-baseline.md`）：

| 数据集 | 模型 | 通过 | p50 | p95 |
|---|---|---|---|---|
| dev 100 | deepseek-v4.1-flash | 100/100 | 3,143 ms | 4,319 ms |
| dev 100 | glm-5.3-flash | 100/100 | 2,817 ms | 5,426 ms |
| smoke 30 | deepseek-v4.1-flash | 30/30 | 2,567 ms | 3,347 ms |
| smoke 30 | glm-5.3-flash | 30/30 | 2,699 ms | 5,066 ms |

费用（真实账单端点差值）：dev 100 条 deepseek-v4.1-flash +$0.010304、glm-5.3-flash +$0.002824。

数据集结构测试（`tools/quality-runner/tests/datasets.rs`，4 条）：
字段完整、id/文本不重复、金额不得进 `must_preserve`、blind 集与 dev/smoke 零重叠、
blind 覆盖全部 16 个类别。blind 集 96 条已建立但**未运行**，只用于人工评分。

人工评审状态：`PENDING_HUMAN`（评审包见 `docs/eval/`）。

**仍未做（S4）**

- 在该机器上手工运行 CLI 与 Web UI（TLS 证书、系统代理、中文路径、防火墙提示）；
- 词典 mmap 在大文件上的行为、长时间连续运行；
- 真实转发站（需要密钥）。

因此"Windows 构建与测试"已通过实机验证，但"Windows 端到端功能验证"仍需在 S4 补齐，
不能据此宣称 Windows 版本可直接交付。
