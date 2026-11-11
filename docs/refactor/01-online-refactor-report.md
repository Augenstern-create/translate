# 在线翻译主线重构：实施与验收报告

**日期：** 2026-11-11
**分支：** `refactor/online-translation-v1`
**归档基线：** `archive/pre-online-refactor-20261111` @ `16c55b9`
**执行环境：** Linux（i5-8250U / 32 GiB），rustc 1.99.1

---

## 1. 完成情况总览

| Phase | 内容 | 状态 |
|---|---|---|
| 0 | 冻结基线与归档分支、源码清单、依赖图、秘密扫描、Smoke 评测集 | ✅ 完成 |
| 1 | 新增 `translator-http`：真实 OpenAI 兼容后端 | ✅ 完成（含真实 TCP 测试与 CLI 实跑） |
| 2 | 新翻译主链：保护 / 质量 / 术语 / 模板 / 缓存 / 路由 / 协调器 | ✅ 完成 |
| 3 | 切断旧规则依赖（句法/语义/组合/管线/规划器全部退出产品构建） | ✅ 完成 |
| 4 | 工作区清理、文档重构、AGENTS.md、CI 重写 | ✅ 完成 |
| 5 | 本地模型接入路径 | ⚠️ 代码路径就绪（同一 HTTP 后端），**实测未做**（无模型权重 / 无 GPU 实测） |
| 6 | SDK 稳定 + 最小可用客户端 | ✅ CLI 可用（translate / lookup / config / doctor / usage） |

---

## 2. Phase 1：真实 HTTP 后端

新增 crate `crates/translator-http`：

```text
config.rs    后端配置（TOML，只存密钥环境变量名）
prompt.rs    版本化提示词 + 术语/占位符注入
protocol.rs  Chat Completions 请求构造与响应解析
client.rs    同步 ureq → Future 的线程桥接（超时/重试/并发/取消）
engine.rs    TranslationEngine 实现（含 model_identity）
usage.rs     token 用量与成本估算
errors.rs    错误分类（鉴权/限流/余额/不可用/超时/响应非法/配置）
```

设计选择与理由：

- **不引入 async runtime**：同步 HTTP 客户端在 worker 线程执行，用 `Waker` 桥接；
  宿主（含 `pollster`）不需要选运行时，执行器也不会被阻塞。
- **重试有界且共享截止时间**：默认 2 次额外尝试，只重试 429/5xx/传输错误；
  鉴权、余额、配置错误**不重试**（避免重复计费）。
- **截断即失败**：`finish_reason = length` 直接报错，不返回半句话。
- **回环地址 = 本地后端**：`127.0.0.1` 的自部署模型不会被"远程授权"拦住。

验证证据（真实 TCP + 真实 HTTP 报文，本地 mock 服务端）：

```text
translator-http: 40 单元测试 + 15 集成测试
- 请求路径/鉴权头/模型字段/messages 结构
- 401 不重试；429 重试后成功；5xx 重试耗尽报不可用
- 超时 → BackendTimeout；预取消 → 立即返回 Cancelled
- 并发闸门把并发压到 1；截断输出 → InvalidBackendResponse
- 密钥缺失 → 不发任何请求
```

**CLI 端到端实跑**（本机 mock 服务端 + 真实 HTTP）：

```console
$ translator-cli translate "Hello, world."
你好，世界。

$ translator-cli translate "Order #10248 has shipped." --json
{
  "text": "订单 #10248 已发货。",
  "engine_id": "http.local",
  "protection": "marker",
  "quality": { "fatal": false, "issues": 0, "summary": "模式=precise 数字 1/1 保留；无完整性问题" },
  "route": "local_nmt"
}

# 服务端实际收到的请求（摘自 mock 服务日志）
{"auth": "Bearer test-key", "user": "Order ⟦P1⟧ has shipped.", "system_has_placeholders": true}
{"auth": "Bearer test-key", "user": "This large language model is fast.", "system_has_terms": true}

$ TRANSLATE_E2E_KEY=bad translator-cli translate "Hello, world."
错误：翻译失败：[auth] 后端返回 HTTP 401：... （AuthenticationFailed）

$ translator-cli lookup hello          # 未安装词典
错误：查词失败：没有注册任何词典；请安装词典包或注册词典插件（DictionaryUnavailable）
```

---

## 3. Phase 2：新翻译主链

### 3.1 统一保护（`protection.rs`，重写）

- 两套保护路径合并为一套 `ProtectionPlan`；
- 默认标记策略 + 严格校验（丢失/重复/未知/半截/被改写 → 错误）；
- 标记与源文冲突时自动换标记（4 种候选），全冲突回退分段；
- 协调器在标记失败后自动切分段并重试一次；
- 分段策略下"模型对非空片段返回空串"直接报错（否则会拼出"看起来正常"的结果）。

### 3.2 质量门禁（`quality/` 拆分）

```text
integrity.rs  空输出 / 标记残留 / 受保护内容丢失        → 致命
number.rs     编号·日期·百分比（严格）；金额·时间·数量·区间（分级）
heuristic.rs  否定 / 重复 / 语言 / 比例                  → 只报告
report.rs     结果结构
```

数字语义能力：中文数字解析（十五、一百零五、两、一万二千）、百分比等价
（`15%` ↔ `百分之十五`）、日期等价格式、12/24 小时换算、区间上下限关系。

### 3.3 术语（`terms.rs`，重写）

- 生成前把**命中**的术语送进提示词（只发命中的）；
- 生成后校验，不做译后字符串替换；
- 排序：最长源短语 > 领域 > 用户 > priority；同级冲突加载期报错；
- `Warn`（默认）报告，`Enforce` 表示硬约束不满足即失败。

### 3.4 模板（`templates.rs`，扩展）

- `schema_version = 2`：`Exact`（默认）/ `Normalized` 两种匹配；
- `app` / `page` 作用域，更具体者优先；
- 同作用域重复声明报错；模板结果同样过完整性校验。

### 3.5 缓存（`cache.rs`，重构）

缓存键覆盖 15 个维度（文本、语言对、backend、model、model_revision、prompt、
glossary、protection、mode、text_kind、strategy+rules、context、privacy、
generation config）。任一维度变化都不命中。

### 3.6 协调器（`coordinator.rs`，重写）

1832 行 → 约 1200 行，但职责从"策略研究 + 产品路由 + 执行控制 + 结果处理"
收敛为"固定顺序调用 + 错误传播 + 完整性门禁 + 诊断"。

固定流水线（顺序写死，不允许靠碰运气命中）：

```text
校验 → 语言识别 → 规则 → 模板 → 保护 → 术语 → 路由/缓存身份 → 缓存查询
     → 执行（1 + content_retry）→ 还原 → 质量门禁 → 术语校验 → 缓存写入
```

---

## 4. Phase 3/4：切断旧依赖与清理

### 移除的 crate（归档分支保留）

```text
translator-nmt-bergamot   translator-nmt-ort   translator-runtime
translator-resource       translator-learning  translator-jni
p33-bench                 migrate-rules
```

`translator-core` 内部移除：`semantic` `composer` `pipeline` `context_sense`
`planner` `evidence` `plan` `plan_cache` `budget`（共约 13,000 行），
并把语言识别/词形分析的最小实现移入 `core::language`，使 `translator-analysis`
退出产品依赖。

### 移除的数据与工具

- `translator-sdk/tests/**`（旧 Android/NMT/Windows 验证脚手架）
- `fixtures/{quality,nmt,p3-freeze,p4,p5,p9,research,rules,templates,terms,golden}`
  （约 5.5 MB 旧规则评测数据）
- `tools/` 下与旧路线绑定的 Python 脚本与 `p3/p4/p5/p9/p10/quality` 目录
- 旧文档 82 篇（归档分支保留），主干只保留 5 篇 + 本报告

### CI

`.github/workflows/ci.yml` 重写为：fmt → clippy(-D warnings) → test → doc →
秘密扫描。删除 `real-model.yml`（绑定旧的 ORT/NMT 栈）。

---

## 5. 验收证据（自动部分）

```text
cargo fmt --all -- --check                      通过（无差异）
cargo clippy --workspace --all-targets          0 warning
cargo test --workspace                          302 passed / 0 failed / 4 ignored
cargo xwin check --workspace                    Windows(MSVC) 全工作区 check 通过
cargo xwin build -p translator-cli              Windows .exe 链接成功（7.3 MB）
```

Windows 交叉编译说明：本机没有 `x86_64-w64-mingw32-gcc`（gnu 目标需要它，
且安装它属于需要授权的系统操作），因此改用 `cargo-xwin` + **MSVC 目标**：
rustls/ring 与 bundled SQLite 都成功编译并链接成真实 PE 文件。
这证明**代码可编译、可链接**，但仍未在实机运行（见 §6）。

测试分布：

| 目标 | 数量 |
|---|---:|
| `translator-core` 单元测试 | 131 |
| `translator-http` 单元 + 集成 | 40 + 15 |
| `translator-api` 集成（流水线 + 配置/数据） | 30 + 16 |
| 其它（types/plugin-api/dictionary/rules/analysis/pack） | 70 |

关键验收项的对应测试：

| 验收项 | 测试 |
|---|---|
| 受保护内容不得静默丢失 | `pipeline::保护彻底失败时报完整性错误且不写缓存`、`protection::sp08~sp16` |
| 关键数字不得静默改变 | `pipeline::关闭数字保护后编号被改会致命`、`quality::number::*` |
| 未通过校验不得入缓存 | `pipeline::保护彻底失败时报完整性错误且不写缓存` |
| 失败/重试/取消/超时可解释 | `translator-http` 15 个集成测试 |
| 术语在生成前约束、生成后校验 | `pipeline::术语约束会送进执行任务`、`enforce/warn` 用例 |
| 缓存身份完整 | `cache::任一维度变化都不得命中`、`pipeline::模式/术语版本/规则版本` |
| 查词与翻译分离 | `pipeline::查词与翻译是两条独立路径` |

---

## 6. 未完成 / 需要人工的部分（PENDING_HUMAN）

| 项目 | 原因 | 影响 |
|---|---|---|
| 真实转发站批量评测（≥50 条） | 没有 base_url 与密钥 | 无法给出真实质量结论 |
| `fixtures/eval` 参考译文与人工评分 | 需要人工翻译/复核 | 质量结论缺失 |
| 本地模型质量/延迟/内存对比（Phase 5） | 无模型权重、未在 RTX 2050 上实测 | 本地后端是否可用未知 |
| Windows 实机运行 | 仅交叉编译（MSVC 目标）通过 | 路径/编码/代理行为未验证 |
| 词典释义人工校对 | 依赖 ECDICT 上游质量 | 查词质量未知 |
| 图形界面 | 本轮最小可用界面是 CLI | 非技术用户仍需命令行 |

**这些不阻塞架构验收，但阻塞"可交付给最终用户"的判断。**

---

## 7. 遗留风险

1. **标记可靠性依赖模型**：旧实测（`docs/18`）最好 6/7，当前模型未复测。
   已有分段回退与严格校验兜底，但连贯性会下降（K-02）。
2. **分段回退下"模型答非所问"无法自动识别**（K-03）：只能靠启发式与人工。
3. **重试可能造成重复计费**：由服务端额度兜底（K-14）。
4. **`paragraph_strategy` 目前不改变执行方式**（K-18）：整段翻译优先。
5. **成本估算只是本地计数**（K-24）：硬额度必须在转发站执行。

---

## 8. 结论

架构目标（唯一主链、可换后端、可测量质量、成本可控、保护不可绕过）**已达成**，
并用 302 个自动化测试 + 真实 HTTP 端到端实跑 + Windows 交叉编译固定下来。

产品质量目标（"译文是否准确自然"）**尚未验收**：缺真实后端评测与人工评审，
状态为 `PENDING_HUMAN`。按方案要求，不以自动指标替代人工验收。
