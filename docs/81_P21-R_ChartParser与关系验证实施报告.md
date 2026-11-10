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
```

这不是 `parse_simple` 的包装：ChartItem 的 `children` 是真实 chart item id，`chart_item_to_graph()` 递归生成 SyntaxGraph。

限制（必须保留在报告中）：

- 当前 Chart 以 `(start,end,category)` 去重，**先到候选获胜**，尚未实现 packed forest 的多候选保留；
- 尚未实现 Earley 预测/完成状态，也未实现完整 PP 附着和并列；
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
- 默认测试：544 / 0 / 60；
- 全特性：557 / 0 / 67；
- P19 36/36；P21 专项 20/20；高级 8/8；矩阵 100/100；后置 20/20；独立 20/20；
- 文档结构 1.0；
- 人工评审 118 行 `PENDING_HUMAN`。

## 8. 未完成 / 不得宣布完成

- Chart 尚无 packed forest 多候选与概率/证据消歧；
- 无完整 Earley、PP 附着、并列、关系从句；
- `analyze_single` 仍保留旧线性解析回退；
- `SemanticTransfer::transfer(SyntaxAnalysis)` 仍在兼容路径；
- 条件句是受限合并，不是完整作用域图；
- 人工评审未执行。

因此本次只能判定为 **PARTIAL：算法骨架已替换，覆盖范围仍受限**。
