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
passed = 335   failed = 0   ignored = 4
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

## 6. 交叉编译检查（Windows）

```bash
rustup target add x86_64-pc-windows-gnu   # 只需一次
cargo check --workspace --target x86_64-pc-windows-gnu
```

这只能证明**代码可编译**，不能证明运行时行为（路径、编码、防火墙、代理）。
真实 Windows 验证仍是 `PENDING_HUMAN`。
