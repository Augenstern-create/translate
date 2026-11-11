# 架构说明（v1：在线翻译主线）

**版本：** 1.0
**状态：** 已实施并合并到 `main`。重构过程报告与本地模型实测记录保存在 `research/traditional-engine` 分支的 `docs/refactor/` 下（不在产品主分支保留）。
**基准：** `refactor/online-translation-v1`

---

## 1. 一句话概括

**翻译模型负责语言转换，Rust SDK 负责可靠性、可控性、数据处理与跨平台接口。**

SDK 不试图自己"理解英语语法"，它做的是：

1. 把用户原文里**不能变**的东西（URL、邮箱、占位符、代码、重要数字）保护起来；
2. 把命中的术语约束、提示词版本、模型身份拼成一次**可复现**的请求；
3. 调用后端（远程转发站或本地模型服务，同一套 HTTP 代码）；
4. 严格还原受保护内容，跑完整性门禁；
5. 只有通过门禁的结果才写缓存、才返回给用户。

---

## 2. 模块地图

```text
crates/
  translator-types        公共类型：请求/结果/错误码/语言标签
  translator-plugin-api   插件契约：TranslationEngine / DictionaryProvider / 注册表
  translator-http         OpenAI 兼容 Chat Completions 后端（远程与本地同一实现）
  translator-core         流水线核心：
                           text        受保护跨度扫描
                           protection  统一保护计划（标记 + 分段）
                           quality/    完整性、数值语义、启发式诊断
                           terms       术语表（约束 + 校验）
                           templates   固定 UI 文案整句匹配
                           cache       缓存身份与 FIFO 缓存
                           routing     轻量后端路由
                           language    语言识别与形态分析的最小实现
                           rules       确定性文本变换接口
                           coordinator 固定顺序的流水线
  translator-dictionary   FST + SQLite 离线词典（查词，与句子翻译分开）
  translator-rules        声明式确定性规则包（NFKC、术语替换）
apps/
  translator-cli          命令行客户端（最小可用界面）
tools/
  pack-dictionary         词典打包
  quality-runner          评测运行器
prompts/
  en_zh/precise_v1.txt    提示词（受版本管理）
  en_zh/fast_v1.txt
fixtures/eval/            评测数据集（smoke / dev）
```

---

## 3. 主链路（唯一一条）

```text
TranslateRequest
   │
   ├─ 1 校验输入（非空、长度上限）
   ├─ 2 规范化 + 语言识别（显式指定优先）
   ├─ 3 确定性规则（可选）
   ├─ 4 整句模板匹配 ──命中──► 完整性校验 → 缓存 → 返回
   ├─ 5 保护计划（标记优先 / 分段回退）
   ├─ 6 命中术语（只取源文实际出现的）
   ├─ 7 路由 + 缓存身份
   ├─ 8 缓存查询 ──命中──► 返回（带 CacheHit 警告）
   ├─ 9 执行（一次 + 至多 content_retry 次内容重试）
   ├─ 10 还原受保护内容（严格：丢失/重复/未知/多吐一份 → 错误）
   ├─ 11 安全格式修复 + **统一终检**（对最终文本做完整性 + 术语校验；
   │      修复改动关键数字或校验失败时退回原候选）
   └─ 12 写缓存（只写通过终检的文本，且 result.text 就是被校验的那份）→ 返回
```

代码位置：`crates/translator-core/src/coordinator.rs` 的 `run_inner`。

**顺序是固定的。** 不允许再出现"先试语义组合、再试普通组合、最后才轮到模型"这种靠碰运气命中的分支结构。

---

## 4. 保护机制（`protection.rs`）

| 策略 | 做法 | 适用 |
|---|---|---|
| `Marker`（默认） | 受保护片段替换为 `⟦P1⟧`，整句翻译，译后严格还原 | 上下文完整，质量最好 |
| `Segment`（回退） | 受保护片段不进模型，只翻译普通片段再拼回 | 模型不可靠时兜底 |

关键不变量：

1. 受保护片段**逐字节**出现在输出里；
2. 每个标记恰好出现一次；未知/重复/半截标记 → `IntegrityFailure`；
3. 允许模型调整语序，不允许替换内容；
4. **源文本身含有标记形态时自动换一种标记**（四种候选），全冲突则回退分段；
5. 失败就报错，不返回"看起来正常"的半成品。

`ProtectionPlan` 同时提供 `restore()`（还原）与 `verify()`（只校验），
协调器在标记失败后会自动切到分段策略重试一次。

扫描器（`text.rs`）覆盖：URL、邮箱、模板变量（`{x}`/`{{x}}`/`${x}`）、
格式占位符（`%s`/`%1$s`/`%.2f`）、反引号代码、HTML/模板标签、编号（`#10248`）、
日期、版本号、百分比、十六进制；路径与标识符（`API_KEY`、`user_id`）可配置开启。

---

## 5. 数字与质量（`quality/`）

| 文件 | 职责 | 是否致命 |
|---|---|---|
| `integrity.rs` | 空输出、标记残留、受保护内容丢失 | **是** |
| `number.rs` | 编号、日期、百分比（严格）；金额、时间、数量、区间（分级） | 严格项是 |
| `heuristic.rs` | 否定丢失、重复句段、语言不符、比例异常 | 否（只报告） |
| `report.rs` | 结果结构与汇总 | — |

设计要点：

- **不强制所有数字原样出现**：`I have three books.` → `我有三本书。` 合法；
- 但 `#10248`、`15%`、`2026-11-11` 这类**确定性标识**必须一致；
- 中文数字可解析（十五、一百零五、两），用于等价判定；
- 区间关系（`between 10 and 20`）上下限被改 → 致命。

---

## 6. 术语（`terms.rs`）

```json
{
  "schema_version": 1,
  "id": "glossary.software",
  "version": 3,
  "domain": "software",
  "entries": [
    {
      "id": "term_001",
      "source": "large language model",
      "target": "大语言模型",
      "source_language": "en",
      "target_language": "zh-CN",
      "case_sensitive": false,
      "match_type": "phrase",
      "domain": "software",
      "priority": 100,
      "strict": true,
      "origin": "user"
    }
  ]
}
```

- 只在**生成前**把命中的术语送进提示词（不全表塞入，省费用、少干扰）；
- 生成后**校验**是否满足，而不是把译文里的词强行替换；
- `TermPolicy::Warn`（默认）只报告；`Enforce` 表示"`strict` 术语不满足则本次翻译失败"；
- 冲突规则：更长源短语 > 领域规则 > 用户规则 > priority；同级且目标不同 → 加载期报错。

---

## 7. 缓存身份（`cache.rs`）

```text
source_text_hash        source_language       target_language
backend_id              model_id              model_revision
prompt_version          glossary_version      protection_version
translation_mode        text_kind             strategy(+rules 版本)
context_fingerprint     privacy_scope         generation_config_hash
```

任何一项变化都会让缓存不命中。原则：

- 只缓存**通过完整性校验**的结果；
- 不同隐私域不共享缓存；
- 第一阶段只是进程内 FIFO 缓存（无持久化）；
- 缓存条目连带诊断一起存，避免"第一次报术语违规、第二次不报"。

---

## 8. 后端抽象（`translator-http`）

- 协议：OpenAI 兼容 `POST {base_url}/chat/completions`；
- 同步 HTTP 客户端（`ureq`）在**独立 worker 线程**里执行，用 `Waker` 桥接成 Future：
  宿主不需要 async 运行时，执行器也不会被阻塞；
- 传输层重试有界（默认 2 次），所有尝试共享同一个截止时间；
- 取消：Future 立即返回 `Cancelled`，worker 在两次重试之间检查令牌；
- 错误分类：鉴权 / 限流 / 余额 / 不可用 / 超时 / 响应非法 / 配置错误；
- 配置只存**环境变量名**，密钥永不进仓库。

`TranslationEngine` 是唯一的后端契约。需要术语约束的后端实现
`translate_job(&TranslationJob)`（默认实现会退化成 `translate()`）。

### 8.1 执行层：预算 / 并发 / 队列（S2）

```text
协调器（每次逻辑翻译创建一个 RequestBudget）
   │  TranslationJob { budget, deadline, cancel }
   ▼
HttpRuntime（每个 Translator 一个，所有后端共享）
   ├── Pool        固定 worker（= 全局并发上限） + 有界队列 + 单一取消观察线程
   └── global      Limiter
   ▼
HttpTransport（每个后端一个）
   ├── backend limiter   该后端并发上限
   ├── agent             连接池 / TLS / 单次尝试超时
   └── meter             用量统计
```

执行顺序（每一步都检查取消与整体截止时间）：

```text
取消 → 截止时间 → 全局配额 → 后端配额 → 再检查取消/截止时间 → 预算扣减 → 发出请求
```

**预算扣减在取得配额之后**：排队超时、等待配额失败、未发送即取消都不消耗
物理调用额度（这些路径本来就不会产生费用）。

- **预算**：`max_http_calls_per_request`（默认 4）限制**物理调用**总数，
  分段回退、内容重试、传输重试共享同一份额度；超出返回 `BudgetExceeded`，
  且不会改用别的后端绕过。
- **全局并发**：`[http] max_concurrent_requests`（默认 4）是**所有后端共享**的
  上限；每个后端另有 `max_concurrent_requests`。两者都通过 RAII Permit 归还，
  任何错误/取消/超时路径都不会泄漏配额。
- **有界队列**：`[http] max_queue_size`（默认 16）。队列满立即返回 `QueueFull`，
  不阻塞、不无限排队；线程总数恒定为 `workers + 1`（worker + 一个取消观察线程）。
  观察线程只持有 `Weak`，最后一个运行时释放后自行退出，不会永久占用线程。
- **装配路径**：`Translator::builder()` 的所有 HTTP 入口（TOML / 配置文件 /
  `http_backend(engine)`）都只收集 `BackendConfig`，在 `build()` 时用**同一个**
  `HttpRuntime` 构造，因此 `http_global_concurrency` / `http_queue_size` 对
  多后端一律生效。
- **取消**：排队中立即取消（不消耗预算）；已取得配额未发送则停止；
  已发出的请求由 worker 等待网络超时结束，但 Future 立即返回
  `Cancelled` 且不再重试/切换后端（见 `docs/known-issues.md` K-15）。
- **Retry-After**：429/503 带 `Retry-After` 时遵守（上限 120 秒，且不超过截止时间）。
- **请求已发出后的失败**：读取响应体时连接中断说明请求一定已到达服务端，
  这类错误最多再试一次（避免重复计费），并始终计入
  `potentially_billable_unknown_calls`。
- **重试分类**：只有 `RateLimited`（429）与 `Unavailable`（连接被拒/重置、DNS 临时失败、5xx）
  会重试；TLS 证书/握手错误、非法 URI、HTTP 协议错误、鉴权、余额、超时、
  未知传输错误一律不重试——确定性错误重发只会浪费配额，超时重发还有重复计费风险。

---

## 9. 明确不做的事

- 不做通用手写英文语法翻译引擎；
- 不做"多模型投票 / 反复推理 / 自动润色"（成本不可控）；
- 不为 Android 新增 JNI；
- 不建立大规模模型资源租约系统；
- 不把自动指标当人工质量验收。

旧实现（`semantic` / `composer` / `pipeline` / `context_sense` / `planner` /
`translator-nmt-*` / `translator-runtime` / `translator-resource` / `translator-learning` /
`translator-jni`）完整保存在分支 `archive/pre-online-refactor-20261111`。
