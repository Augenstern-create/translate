# 81 — P21-R Chart Parser 与关系级验证实施报告

- 代码基线：`a5d205fa0830d3f0f22a3b9f834d54f36a431eb2`
- 范围：`translator-core::pipeline`
- 结论：**PARTIAL**。已建立真实跨度组合的 Chart Parser 与图驱动语义转换、关系级目标映射；复杂结构仍回退旧路径。
- 人工评审：**PENDING_HUMAN**

## 1. Chart Parser 实际算法

源码：`crates/translator-core/src/pipeline.rs:776`（ChartItem）、`:791`（Chart）、`:816`（ChartParser）。

采用 **bottom-up span chart**：

1. 对每个 token 产生词法类别（NP/N/V/ADJ/ADV/DET/NUM/AUX/NEG）；
2. 对每个 `start < mid < end` 组合相邻 chart item；
3. 由统一规则生成新 item，并记录 `rule_id`、children、span、head、lemma、pos、negated、past；
4. 直到无新 item；
5. 只接受覆盖 `0..words.len()` 的 Clause。

当前规则：

```text
NP → pronoun
NP → N
NP → DET NP
NP → NUM NP
NP → ADJ NP
VP → V
VP → V NP
VP → AUX VP
VP → NEG VP
VP → VP ADVP
Clause → NP VP
NP → NP CONJ NP
```

这不是 `parse_simple` 的包装：ChartItem 的 `children` 是真实 chart item id，`chart_item_to_graph()` 递归生成 SyntaxGraph。

限制（必须保留在报告中）：

- 当前 Chart 保存同跨度/同类别候选，二元组合枚举左右候选；最终按规则分数顺序尝试，尚未实现 packed forest 的概率/证据消歧；
- 尚未实现 Earley 预测/完成状态；PP 附着和基础 NP 并列已有限支持，完整并列/从句未完成；
- 因此不能声称“通用英语 Chart Parser 已完成”。

## 2. Chart → SyntaxGraph

`chart_item_to_graph()`：`:1154`。

- Clause → Subject/Predicate；
- VP → Head/Auxiliary/Negation/Object/Adverbial；
- NP → Determiner/Quantity/Modifier/Head；
- 每个节点保存 source span、lemma、pos、past；
- 关系边保存 head/dependent 节点 ID。

## 3. SemanticTransfer 的真实入口

新入口：

```rust
SemanticTransfer::transfer_graph(&SyntaxGraph) -> Option<MeaningGraph>
```

源码：`:1391`。

它通过 `VpFacts::collect()` 和 `NpFacts::collect()` 遍历 SyntaxGraph 的边：

- VP 的 Auxiliary/Negation/Object/Adverbial；
- NP 的 Determiner/Quantity/Modifier/Head；
- 被动角色由句法/词汇证据决定；
- 不读取 `SyntaxAnalysis.subject/object/negated/time`。

旧 `SyntaxAnalysis` 入口仍保留在 `:1400+`，仅作为 Chart 无法覆盖时的兼容回退；`analyze_single()` 在 `:2273` 先走 Chart，`:2293` 才回退。

## 4. 图变形测试

测试文件：`crates/translator-api/tests/p80_pipeline.rs`

已覆盖：

- `图变形不改变语义关系`：
  - 打乱 SyntaxGraph 节点顺序并重映射 ID；
  - 断言 MeaningGraph 的 Agent/Patient/Quantity/Scope 角色不变；
  - 交换 Subject/Object 边后，Agent/Patient 必须交换。
- 主测试：
  - 交换目标 NP 的 source 后，Relation Validator 拒绝；
  - 移除 Negation source 后拒绝；
  - 移除 Quantity source 后拒绝；
  - 移除条件子树后拒绝。

## 5. 关系级验证

`RelationMapping`：`:2087`；`validate()`：`:2101`。

- `ChineseNode` 现在有 `source: Option<MeaningNodeId>` 与 `role: Option<"Agent"/"Patient">`；
- Agent 检查目标 Noun 的 source == 源 Agent 且 role == Agent，同时目标 Verb source == 源 Event；
- Patient 检查目标 Noun source == 源 Patient 且 role == Patient；
- Quantity/Modifier/Time/Scope/Complement/Condition 分别检查对应目标节点或子树；
- 节点类型数量只作为附加条件，不再是唯一依据。

## 6. 条件句

- `analyze_conditional()` 在 `:2201`；
- 条件关系写入 MeaningGraph：`MeaningRelationKind::Condition`；
- ChineseSyntaxTree 增加 `condition: Option<Box<ChineseSyntaxTree>>`；
- `render()` 从条件子树和主树渲染，不再只拼接两个字符串；
- 验证器要求 Condition 关系存在时目标树必须有条件子树。

条件句目前仍使用分别解析后合并的受限实现；不是完整 Condition 作用域/指代系统。

## 7. 回归

- 320：chrF `0.5172 → 0.5196`、BLEU `0.5192 → 0.5210`、NMT 289、Rule 33、0 回归、3 改善；
- 默认测试：549 / 0 / 60；
- 全特性：562 / 0 / 67；
- P19 36/36；P21 专项 20/20；高级 8/8；矩阵 100/100；后置 20/20；独立 20/20；
- 文档结构 1.0；
- 人工评审 118 行 `PENDING_HUMAN`。

## 8. 未完成 / 不得宣布完成

- Chart 尚无 packed forest 多候选与概率/证据消歧；
- 无完整 Earley、完整 PP 附着、复杂嵌套并列、复杂关系从句；
- `analyze_single` 仍保留旧线性解析回退；
- `SemanticTransfer::transfer(SyntaxAnalysis)` 仍在兼容路径；
- 条件句是受限合并，不是完整作用域图；
- 人工评审未执行。

因此本次只能判定为 **PARTIAL：算法骨架已替换，覆盖范围仍受限**。

## 8.1 旧路径隔离诊断

`PipelineOutput` 增加：

```rust
route: ParserRoute::{Chart, LegacyLinear}
candidates: usize
candidate_score: u32
legacy_reason: Option<String>
```

`candidate_score` 由 MeaningGraph 关系数和 SyntaxGraph 边数构成，用于多候选排序；它仍是启发式分数，不是概率模型。

- Chart 成功时 `legacy_reason = None`；
- 旧路径回退时记录 `chart-unsupported`；
- 该字段只提供隔离诊断，不把旧路径宣称为已退场。

## 9. 复审问题整改记录

| 复审问题 | 本次处置 | 验证 |
|---|---|---|
| `NP → DET NP` 方向错误 | 调用改为 left=DET, right=NP | `限定词短语由_chart组合`，`The dog saw the cat.` 直接 Chart 解析并断言 Determiner/Subject/Object |
| 同跨度只保留一个候选 | `by_span` 改为 `Vec<usize>`；组合器枚举左右候选；`analyze_single` 逐个候选进行语义转换和验证 | `chart_保留跨度候选`；完整 packed forest/概率消歧仍未完成 |
| 多个修饰被覆盖 | `NpFacts.modifiers: Vec`，目标 NP 逐个生成 Modifier | `She bought two beautiful new books.` 断言 2 条 Modifier 关系 |
| 多个状语被覆盖 | `VpFacts.adverbials: Vec`，逐个生成 Time 关系/目标节点 | 多个 Time 关系保留 |
| 基础并列 | 增加 `np.coord3` 三元组合、`coordination` 关系、目标“和”连接 | `The dog and the cat saw the man.` 走 Chart，输出包含“和” |
| 关系从句 | 增加 `Rel`、`NP REL VP → NP`、`Relative` 事件、目标“V+O+的+N” | `The man who saw the dog ran.` 走 Chart，输出包含“看见狗” |
| 对象并列 | 增加 `V + NP CONJ NP` 的 VP/Clause 组合、对象 Coordination 目标“猫和男人” | `The dog saw the cat and the man.` 走 Chart；`I bought two books and three pens.` 也走 Chart |
| 从句并列 | 增加 `CLAUSE CONJ CLAUSE → CLAUSE`、子句拆分、ClauseCoordination 与目标“，”连接 | `The dog ran and the cat slept.` 走 Chart，输出 `狗跑了，猫睡觉了。` |
| want/否定作用域 | 增加 `V INF VP → VP`、Complement 关系和补语 Patient 关系 | `I don't want to go.`、`I want to buy a book.` 走 Chart，否定绑定 WANT |
| 条件否定 | 条件主句复用 Chart 路径 | `If it rains, I don't go.` 走 Chart，输出 `如果下雨，我不去。` |
| PP 附着 | 增加 P/Pp、`Pp → P NP`、`NP → NP PP`、`VP → VP PP`；VP PP 支持 Location/Instrument，NP PP 支持 PpModifier 目标修饰 | `She went to school.` 走 Chart；`The dog saw the man with a book.` 保留多个 PP 附着候选 |
| 路由不透明 | `PipelineOutput.route: ParserRoute::{Chart,LegacyLinear}`；Chart 成功样本断言 `ParserRoute::Chart` | p80 主样本、限定词样本 |
| Validator 类型兜底 | 节点映射改为严格 `source == idx && kind == expected`，不再按类型兜底 | p80 主样本与反例 |
| 只验证角色标签 | Agent/Patient 同时检查目标 NP 结构位置（subject/object，或被动反向）与 source/role | 保持 source/role 不变、只交换目标 NP 位置的测试必须拒绝 |
| 条件句合并后不重新验证 | 合并后调整 condition source 偏移，重新执行 `validate()`，失败则拒绝 | 条件样本 `validation.valid` 断言；移除条件子树必须拒绝 |

`analyze_single` 会把多个通过验证的候选按文本和语义关系签名比较；若不同候选产生不同语义，返回 `None` 回退，而不是让规则顺序决定。`PipelineOutput.candidates` 记录参与验证的候选数。

仍需明确：候选按规则分数排序后逐个尝试，**不是概率/证据驱动的最终消歧**；多候选存储不等于通用 Chart 消歧完成。当前总体仍为 **PARTIAL**。
