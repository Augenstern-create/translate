# 迁移指南：从旧架构到在线翻译主线

**旧版本：** `16c55b9`（`main` / `archive/pre-online-refactor-20261111`）
**新版本：** `refactor/online-translation-v1`

---

## 1. 先看结论

| 你可能在用的东西 | 现在怎么办 |
|---|---|
| `Translator::builder().nmt_bergamot(...)` / `.nmt_ort(...)` | **删除**。改用 `.http_config_file("translate.toml")` 或 `.http_config_toml(...)` |
| `.composition(...)` / `.semantic_composition(...)` | **删除**。整句交给模型 |
| `.learning(...)` / `.learning_store()` / `.flush_learning()` | **删除**。持续学习线已归档 |
| `.resource_*` / `.memory_budget(...)` / `.limits(...)` | **删除**。资源租约系统已归档 |
| `.term_table_file(...)` / `.term_guard(...)` | 改用 `.glossary_file(...)`（新的术语表格式，见 §4） |
| `.strict_placeholder_restore(true)` | **删除**。现在默认就严格（失败即报错） |
| `.rules_file(...)` | **保留**，但规则包只允许确定性变换（NFKC、术语替换） |
| `.templates_file(...)` | **保留**，建议升级到 `schema_version = 2`（见 §5） |
| `.dictionary_package(...)` | **保留** |
| `.lemmatizer(...)` | **保留**（需要自己实现 `MorphologyAnalyzer`，见 §6） |
| `.language_identifier(...)` | **保留** |
| `.translation_mode(...)` / `.text_kind(...)` | **保留** |
| `.cache_capacity(...)` / `.strict_integrity(...)` / `.remote_fallback(...)` | **保留** |

---

## 2. 公开类型的变化

### 2.1 `EngineDescriptor` / `ProviderDescriptor` / `BackendDescriptor`

```rust
// 旧
EngineDescriptor { id: "mock.nmt", capabilities: &[Capability::Nmt], priority: 50,
                   plugin_api: API, license: "MIT" }

// 新（id/capabilities/license 变成可配置的 String/Vec）
EngineDescriptor::new("mock.nmt", [Capability::Nmt], 50, "MIT")
```

原因：HTTP 后端的 id 来自配置文件，无法用 `&'static str` 表达。

### 2.2 `TranslationErrorCode` 新增类别

新增：`AuthenticationFailed`、`RateLimited`、`BudgetExceeded`、`BackendUnavailable`、
`BackendTimeout`、`InvalidBackendResponse`、`ConfigurationError`、`DictionaryUnavailable`。

如果宿主对错误码做了**穷尽匹配**，需要补分支；旧码全部保留，语义不变。

### 2.3 删除的类型

`SemanticCompositionEngine`、`CompositionEngine`、`ChineseSyntaxTree`、`MeaningGraph`、
`TranslationPlanner`、`RequestPlan`、`PlanCache`、`MemoryBudget`、`LanguageEvidence`、
`UnitExecutionTrace` 等全部移除。它们只属于旧的手写句法翻译路线。

---

## 3. 新增的类型（需要主动使用的）

| 类型 | 用途 |
|---|---|
| `Glossary` | 术语表（约束 + 校验） |
| `TermPolicy` | `Off` / `Warn`（默认）/ `Enforce`（硬约束不满足即失败） |
| `TemplateScope` | 模板作用域（app / page） |
| `ProtectionPolicy` / `SpanPolicy` | 保护策略（标记/分段、是否保护路径与标识符） |
| `BackendsFile` / `BackendConfig` | 后端配置（TOML） |
| `TranslationJob` / `EngineOutput` | 自定义后端的新执行契约 |
| `ModelIdentity` | 参与缓存键的模型身份 |

自定义后端建议实现 `translate_job()`（能拿到术语约束、占位符 token、截止时间与取消令牌）；
不实现也能工作（默认退化成 `translate()`，但拿不到术语约束）。

---

## 4. 术语表格式迁移

旧格式（`translator-rules` 的 `TargetTermTable`）是**输出侧变体替换**，已删除。

新格式：

```json
{
  "schema_version": 1,
  "id": "glossary.software",
  "version": 3,
  "entries": [
    { "id": "t1", "source": "large language model", "target": "大语言模型", "strict": true }
  ]
}
```

迁移要点：

1. **把"变体列表"改成"源术语 → 目标写法"**：新机制在生成前约束，而不是生成后替换；
2. 旧条目里的 `variants` 无法直接复用——它们是"错误写法"，新机制不需要它们；
3. `strict = true` 的条目在 `term_policy = "enforce"` 下不满足会让本次翻译**失败**；
4. 版本号必须递增：它参与缓存身份。

---

## 5. 模板格式迁移

旧（`schema_version = 1`，`pairs`）：

```json
{ "schema_version": 1, "id": "t", "version": 1, "source": "en", "target": "zh-Hans",
  "pairs": [{ "source": "Good morning.", "target": "早上好。" }] }
```

新（`schema_version = 2`，`entries`，默认精确匹配）：

```json
{ "schema_version": 2, "id": "t", "version": 2, "source": "en", "target": "zh-CN",
  "entries": [{ "source": "Good morning.", "target": "早上好。" }] }
```

**兼容性说明：** 旧 `pairs` 仍可加载，但按**归一化匹配**（折叠空白 + 小写 + 去句末标点）
处理——这是它们被写出来时的语义，直接改成精确匹配会让已有模板全部失配。
建议逐步迁移到 v2 并显式声明 `"match": "normalized"`。

---

## 6. 语言识别与词形分析

`translator-core` 不再依赖 `translator-analysis`。产品侧提供：

```rust
pub trait LanguageIdentifier { fn identify(&self, text: &str, limit: usize) -> Vec<LanguageGuess>; fn name(&self) -> &'static str; }
pub trait MorphologyAnalyzer { fn lemmas(&self, word: &str) -> Vec<String>; fn name(&self) -> &'static str; }
```

内置 `HeuristicIdentifier`（Unicode 文字系统启发式，零模型）。
需要英语词形还原时，自己实现 `MorphologyAnalyzer` 并注入，例如：

```rust
struct MyLemmatizer;
impl MorphologyAnalyzer for MyLemmatizer {
    fn lemmas(&self, word: &str) -> Vec<String> { /* ... */ vec![] }
    fn name(&self) -> &'static str { "my-lemmatizer" }
}

Translator::builder().lemmatizer(std::sync::Arc::new(MyLemmatizer));
```

`translator-analysis` crate 仍在仓库中作为**研究线**，但不参与产品构建，
也不再提供产品可用的 trait 实现。

---

## 7. 行为变化（需要重新验收的地方）

| 行为 | 旧 | 新 |
|---|---|---|
| 占位符还原失败 | 默认只警告，返回缺内容的译文 | **默认报错**（`IntegrityFailure`），不写缓存 |
| 关键数字丢失 | 严格模式下报错 | 报错，并区分"严格标识"与"普通数量" |
| 输出为空 | 部分路径可能返回空串 | 拒绝返回 |
| 术语 `Enforce` | 译后替换文本 | 硬约束不满足即失败（不改写用户译文） |
| 远程后端 | 注册即可用 | 必须显式 `preferred_backend` 或 `remote_fallback`（回环地址除外） |
| 单词翻译 | 可能自动走词典 | `translate()` **永远**走模型；查词用 `lookup()` |
| 句子翻译路径 | 模板 → 语义组合 → 普通组合 → 上下文选义 → 模型 | 模板 → 模型（只有一条） |

---

## 8. 代码迁移示例

```rust
// 旧
let translator = Translator::builder()
    .nmt_ort(ort_config)
    .term_table_file("terms.json")?
    .strict_placeholder_restore(true)
    .learning(learning_config)
    .build()?;

// 新
let translator = Translator::builder()
    .http_config_file("translate.toml")?      // [backend.remote] / [backend.local]
    .glossary_file("glossary.json")?          // 可选
    .templates_file("templates.json")?        // 可选
    .dictionary_package("resources/dict")?    // 可选，查词用
    .term_policy(TermPolicy::Warn)
    .cache_capacity(512)
    .build()?;
```

---

## 9. 需要删除的依赖

宿主 `Cargo.toml` 里如果直接依赖了这些 crate，需要删除：

```text
translator-nmt-bergamot
translator-nmt-ort
translator-runtime
translator-resource
translator-learning
translator-jni
p33-bench
migrate-rules
```

需要查词的宿主保留 `translator-dictionary`；需要确定性规则的保留 `translator-rules`。

---

## 10. 回滚

```bash
git switch archive/pre-online-refactor-20261111   # 只读查看旧实现
git switch main                                   # 旧主线
```

旧实现、旧文档、旧 fixtures 全部保存在归档分支，新主干不再保留副本。
