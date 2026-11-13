# SDK 五类确定性护栏（职责与调用路径）

本 SDK 是**翻译模型的可靠性护栏**，不是语义分析器：

- 模型负责：词义、句意、语法、自然表达；
- SDK 负责：输入合规化、确定性内容保护、用户术语约束、独立查词、固定 UI 文案、
  输出后的完整性核验、有界重试与诊断。

> 语义错误（多义词、习语、人名歧义、漏译短语）**不进入硬门禁**，只在离线评测
> （`docs/eval/`）里由参考译文指标与人工评审衡量。

## 1. 五类护栏 ↔ 模块 / API

| 护栏 | 模块 | 关键类型/函数 | 判定级别 |
|---|---|---|---|
| **G1 特殊结构保护** | `translator-core/src/text.rs`、`protection.rs` | `normalize()`、`ProtectionPlan::prepare()/restore()/spans()`、`quality::integrity::check_protected()`、`ProtectionPlan::verify_counts()` | 丢失/重复/改动 = **Critical** |
| **G2 数字与数值完整性** | `quality/number.rs` | `number::check() → NumberCheck{issues, …}`（编号/日期/时间/百分比/金额/区间/数量） | 编号·日期·金额·百分比·区间 = **Critical**；数量词/序数 = Major |
| **G3 术语表** | `terms.rs` | `Glossary::constraints_for()`（生成前）、`Glossary::verify()`（生成后）、`TermPolicy::{Off,Warn,Enforce}` | `Enforce` 下严格术语未满足 = **Critical** |
| **G4 独立查词** | `translator-dictionary` + `translator-api::LookupRequest` | `Translator::lookup()`、`DictionaryProvider` | 查词失败不影响翻译（两条独立路径） |
| **G5 固定 UI 文案** | `templates.rs` | `TemplateTable::lookup()`（精确/归一化 + app/page 作用域） | 命中直接返回，但仍走终检 |

## 2. 唯一主链（数据流）

```
用户文本
  → text::normalize                                (G1 输入合规化)
  → [G5] TemplateTable::lookup                     ── 命中 → 直接返回（仍过终检）
  → [G1] ProtectionPlan::prepare                   ── 生成 protected_spans
  → [G3] Glossary::constraints_for(source)         ── 生成 term_constraints
  → CacheKey（含保护/术语/模板/规则版本）
  → 缓存查询 ── 命中 → validate_cached()（重跑终检）→ 返回
  → [模型] 完整文本翻译（一次请求，不分词）
  → [G1] plan.restore()
  → [终检] quality::check_with_protection(plan, restored, target, mode)
        ├─ [G1] integrity::check_protected  + verify_counts
        ├─ [G2] number::check
        ├─ [G3] Glossary::verify
        └─ heuristic（否定/条件/重复/比例/语言：只报告）
  → 安全格式修复（仅确定性格式）→ **再跑一次终检**
  → 失败且预算允许 → 内容重试（retry_hint）→ 回到模型
  → 返回结果 + 诊断
```

**唯一入口**：`Coordinator::finalize_translation`（模型路径）与 `validate_final`
（终检）；宿主不会绕过它。

## 3. 统一核验清单（VerificationManifest）

不新增类型，直接复用已有对象——清单在**前处理**生成、模型**不可见**、终检**复用同一份**：

| 清单字段 | 实际载体 | 生成位置 | 消费位置 |
|---|---|---|---|
| `protected_spans` | `ProtectionPlan::spans()` | `ProtectionPlan::prepare` | `restore()`、`check_protected()`、`verify_counts()` |
| `numeric_constraints` | `QualityReport.numbers_in_source / strict_identifiers_in_source` | `number::check` 从**源文**提取 | 与译文数值比对 |
| `term_constraints` | `Glossary::constraints_for()` → `TranslationJob.glossary` | 协调器（前处理） | `Glossary::verify()`（终检） |
| `template_identity` | `TemplateEntry` 的 app/page/匹配模式 | `TemplateTable::lookup` | 命中即返回，诊断里记录 `route=template` |
| `resource_versions` | `CacheIdentity`（prompt/glossary/protection/规则/模板版本） | `CacheIdentity::new` + `with_*` | 缓存键、诊断 |
| `source_fingerprint` | `CacheKey.text_hash`（哈希，不存原文） | `CacheKey::new` | 缓存命中、磁盘缓存校验 |

## 4. 每条输出路径都过终检

| 来源路径 | 终检位置 | 测试 |
|---|---|---|
| 模型正常翻译 | `finalize_translation` | `纯文本翻译走配置的引擎`、`受保护内容在诚实引擎下完整还原` |
| 内容重试 | 同上（重试后再跑） | `吞掉标记会触发一次重试并回退分段`、`分段回退与内容重试共享同一预算` |
| 保护策略回退（marker→分段） | 同上 | 同上 + `分段回退遇到答非所问的译文会失败而不是拼出乱码` |
| 固定 UI 模板 | 同一条终检 | `模板路径与模型路径的终检规则一致`、`模板译文也必须通过术语终检` |
| 内存缓存命中 | `validate_cached()`（重跑 `check_with_protection`） | `缓存命中返回与首次一致的终检文本`、`缓存译文必须重新通过完整性校验` |
| 磁盘缓存命中 | 同上（加载时另校验格式/版本/条目校验和） | `磁盘缓存在新实例之间生效`、`被篡改的缓存条目会被跳过` |
| 格式修复后 | 修复后再跑一次终检 | `finalize_tests`（Unchanged/Applied/Rejected 6 条） |

## 5. 本轮删除的错误方向补丁

| 删除项 | 位置 | 原因 |
|---|---|---|
| `untranslated_fragments` | `quality/heuristic.rs` | 以"保护遮罩 + 词形 + 原文匹配 + ≥2 词"判断漏译，属于新的独立启发式补丁 |
| `mask_protected` | 同上 | 仅服务上述检测 |
| `suspicious_phrases` / `is_identifier` / `phrase_is_proper_noun` | 同上 | 同上 |
| `check_with_protection` 中的调用 | `quality/mod.rs` | 不再在终检里引入该启发式 |
| `expected_checks` / `forbidden_phrases` 硬断言、`--replay` 的通过/失败期望 | `quality-runner` | 不得把"未检测到语义错误"记为"语义正确" |

保留：`regression.jsonl` 的 7 条真实样本（作为**离线评测数据**，带人工问题记录）；
`--replay` 保留为离线诊断（只打印确定性核验结果，不做通过/失败判定）。

## 6. 测试覆盖（正反）

| 类别 | 代表测试 |
|---|---|
| G1 丢失/重复/改动 | `受保护内容丢失致命`、`受保护内容完整时不报`、`sp21 模型多吐一份被发现`、`sp22 同一占位符出现两次`、`sp23 重叠不误报`、`金额币种丢失会拒绝返回`、`受保护内容换顺序不算破坏` |
| G2 金额成对 | `两笔金额的币种被交换会致命`、`人民币与日元不会互相顶替`、`金额校验数值与币种` |
| G2 数值 | `编号被改必须致命`、`百分比…被改`、`金额校验数值与币种`（含币种被换）、`区间上下限关系必须保持`、`数量词用中文写法是允许的`、`汇率与千分位写法不被误拒` |
| G3 术语 | `enforce 下硬约束未满足则失败`、`warn 下只报告`、`术语表版本变化不会复用缓存`、`术语表冲突在装配期报错` |
| G4 查词 | `查词与翻译是两条独立路径`、`没有词典时报词典不可用` |
| G5 模板 | `模板命中不调用模型`、`模板作用域生效`、`部分匹配不返回`、`模板也可以被显式关闭` |
| 缓存 | `缓存命中返回与首次一致的终检文本`、`磁盘缓存在新实例之间生效`、`命中前重新校验` |
| 预算/重试 | `预算耗尽后不再重试`、`分段回退与内容重试共享同一预算` |
| 模式 | `fast_模式同样执行确定性门禁`（重复受保护内容 → Fast 仍致命） |
| 反例（不误拒） | `数量词用中文写法是允许的`、`日期允许等价格式`、`上午与凌晨写法也等价`、`汇率与千分位写法不被误拒`、`普通文本里的_p1_不算残留` |

## 7. 护栏覆盖不到的语义问题（明确记录）

| 问题 | 例子（`regression.jsonl`） | 只能由 |
|---|---|---|
| 多义词误译 | `muppets` → “木偶” | 人工评审 / 模型选择 |
| 习语与语域 | `performances`（体育语境） | 人工评审 / 上下文输入 |
| 人名与普通名词混淆 | `Ivory` → “象牙白” | 人工评审（可考虑术语表显式约束） |
| 普通短语漏译 | `accompanying media` | 人工评审 |
| 信息精度损失 | `seat back` / `bolting` | 人工评审 |
| 参考译文本身错误 | NTREX 2 条（已标注） | 人工校对 |

这些**不**设为硬门禁；SDK 只保证"受保护内容、关键数字、明确术语"不被破坏，
并如实报告它能检测的问题。
