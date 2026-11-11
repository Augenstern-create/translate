# 迁移指南：从旧架构到在线翻译主线

**旧版本：** `16c55b9`
**新版本：** `main`（原 `refactor/online-translation-v1`）
**旧实现完整快照：** `research/traditional-engine`（研究用）与 `archive/pre-online-refactor-20261111`（只读归档）

---

## 1. 换掉了什么

旧架构的核心是**自己用规则生成译文**（句法分析 → 语义组合 → 中文生成）；
新架构把这件事交给翻译模型，SDK 只负责保护、约束、校验、缓存与错误语义。

旧实现（`semantic` / `composer` / `pipeline` / `context_sense` / `planner`、
`translator-nmt-*`、`translator-runtime`、`translator-resource`、`translator-learning`、
`translator-jni`、`translator-analysis`）**不再出现在产品主干**（其中英语词形还原的
最小能力已迁入 `translator-core::language`，查词默认可用），
只保留在 `research/traditional-engine` 分支。

---

## 2. 宿主代码怎么改

| 旧 API | 现在 |
|---|---|
| `.nmt_bergamot(...)` / `.nmt_ort(...)` | `.http_config_file("translate.toml")` |
| `.composition(...)` / `.semantic_composition(...)` | 删除（整句交给模型） |
| `.learning(...)` / `.learning_store()` | 删除（持续学习线已归档） |
| `.resource_*` / `.memory_budget(...)` / `.limits(...)` | 删除（资源租约系统已归档） |
| `.term_table_file(...)` / `.term_guard(...)` | `.glossary_file("glossary.json")` |
| `.strict_placeholder_restore(true)` | 删除（现在默认严格：失败即报错） |
| `.templates_file(...)` | 保留；建议升级到 `schema_version = 2` |
| `.rules_file(...)` | 保留（只允许确定性变换） |
| `.dictionary_package(...)` | 保留 |
| `.lemmatizer(...)` | 保留；**默认已内置**轻量英语词形还原，通常无需配置 |
| `.language_identifier(...)` | 保留（内置启发式识别器） |
| `.translation_mode(...)` / `.text_kind(...)` | 保留 |
| `.cache_capacity(...)` / `.strict_integrity(...)` / `.remote_fallback(...)` | 保留 |

最小可用改造：

```rust
let translator = Translator::builder()
    .http_config_file("translate.toml")?      // [backend.remote] / [backend.local]
    .glossary_file("glossary.json")?          // 可选
    .templates_file("templates.json")?        // 可选
    .dictionary_package("resources/dict")?    // 可选，用于查词
    .build()?;
```

---

## 3. 行为变化（需要重新验收的地方）

| 行为 | 旧 | 新 |
|---|---|---|
| 占位符还原失败 | 警告后返回缺内容的译文 | **报错**（`IntegrityFailure`），不写缓存 |
| 空输出 / 编号被改 | 部分路径静默返回 | 拒绝返回 |
| 术语 `Enforce` | 译后替换文本 | 硬约束不满足即失败（不改写译文） |
| 远程后端 | 注册即可用 | 回环地址=本地；公网地址需显式 `preferred_backend` 或 `remote_fallback` |
| 单词翻译 | 可能自动走词典 | `translate()` 永远走模型；查词用 `lookup()` |
| 句子翻译路径 | 模板→语义→组合→选义→模型 | 模板→模型（只有一条） |

公开错误码新增：`AuthenticationFailed`、`RateLimited`、`BudgetExceeded`、
`BackendUnavailable`、`BackendTimeout`、`InvalidBackendResponse`、
`ConfigurationError`、`DictionaryUnavailable`。旧码全部保留。

---

## 4. 数据格式

**术语表**（新，输出侧替换已废弃）：

```json
{"schema_version":1,"id":"glossary.software","version":3,"entries":[
  {"id":"t1","source":"pull request","target":"拉取请求","strict":true}]}
```

**固定 UI 文案模板**（推荐 `schema_version = 2`）：

```json
{"schema_version":2,"id":"templates.ui","version":2,"source":"en","target":"zh-CN",
 "entries":[{"source":"Save","target":"保存"},
            {"source":"Cancel","target":"取消","match":"normalized"}]}
```

旧 `pairs` 格式仍可加载，但按"归一化匹配"处理（历史兼容）。

---

## 5. 需要删除的依赖

```text
translator-nmt-bergamot  translator-nmt-ort  translator-runtime
translator-resource      translator-learning translator-jni
translator-analysis      p33-bench           migrate-rules
```

需要查词保留 `translator-dictionary`；需要确定性规则保留 `translator-rules`。

---

## 6. 想继续研究旧引擎怎么办

```bash
git switch research/traditional-engine
```

该分支保留完整旧实现与研究文档，可以独立演进，不影响产品主干。
只读快照在 `archive/pre-online-refactor-20261111`。
