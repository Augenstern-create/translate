# 翻译质量回归集（regression.jsonl）

来源：2026-11-13 对 `bench/test.jsonl` 全量译文的人工语言审阅（news / literary / social / speech 分层抽样），
挑出**自动检查通过但语义有问题**的样本，以及两条**参考译文本身错误**的样本。

数据集：`translator-sdk/fixtures/eval/regression.jsonl`（7 条）。

## 1. 覆盖的三类真实错误

| 类别 | 用例 | 问题 | 自动化程度 |
|---|---|---|---|
| 未翻译的英文片段 | `regress-440` | DeepSeek 输出里留下整段英文（`geared to being a weapon`） | **机器可判定**（保护计划 + 原文对应；`--replay` 可复验） |
| 人物/实体一致性 | `regress-112` | `Ivory`（人名）被 GLM 译成“象牙白” | 人工评审（无法可靠自动判定） |
| 依赖上下文的词义 | `regress-ntrex-1`（muppets）、`regress-ntrex-1195`（performances） | 习语/领域词义选错 | 人工评审 |
| 信息精度 | `regress-611` | `seat back` / `bolting` 被简化掉 | 人工评审 |
| 参考译文质量 | `regress-ntrex-1569`（incapacitated）、`regress-ntrex-1012`（democracy） | NTREX 参考译文本身译错 | **不计模型失败**，参考已按英文原意更正并在 `reference_issue` 记录原值 |

## 2. 自动检查：基于"保护计划 + 原文对应"，不是词表

实现位置：`crates/translator-core/src/quality/heuristic.rs::untranslated_fragments`
（由 `quality/mod.rs::check_with_protection` 调用，因此**只在有保护计划时**运行）。

判定顺序（每一步都来自前处理信息，没有英文功能词表、没有词数阈值）：

1. **遮掉受保护内容**：URL / 代码 / 模板变量 / 路径 / 编号由 `ProtectionPlan` 给出，
   它们在译文里原样出现是**正确**的，先按出现位置替换成空格；
2. **识别标识符**：全大写缩写、含数字、CamelCase、含 `_`/`-` → 合理保留，跳过；
3. **连续普通英文词 ≥2 个，且在原文里以同样顺序出现** → 视为"照搬原文"的可疑片段；
4. **专名/标题保护**：该短语在原文中含大写开头的实词（`The Lord of the Rings`）
   或整词全大写 → 不当漏译。

命中只报 **Major**（`untranslated`），不判致命、不自动改译文：无法确认语义时
只记录疑似问题（与 S3 方向纠正的要求一致）。

### 与旧实现的差别

| | 旧（已删除） | 新 |
|---|---|---|
| 阈值 | 连续 ≥5 词 | **≥2 词** |
| 判据 | 英文功能词表 + 小写内容词 | 原文对应 + 保护计划 + 标识符形态 |
| 保护内容 | 未考虑（靠"不在词表里"蒙） | 显式遮掉 |
| 误报控制 | 词数门槛 | 标识符/专名/大小写形态 |

因此**2 个词的残留**（`accompanying media`）现在能报出来，而 `iPhone 15 Pro Max`、
`The Lord of the Rings`、`https://…`、`cargo build` 都不会误报。

### 正反验证（`--replay`，零费用、不调模型）

`quality-runner --replay` 用人工记录的**真实坏译文**（用例里的 `known_bad`）复跑终检，
验证"这段坏译文必须被报出来"：

```bash
cargo run -p quality-runner -- --dataset fixtures/eval/regression.jsonl --replay --out /tmp/replay.json
```

结果（`docs/eval/runs/regression-replay.json`，9 条变体全部符合预期）：

| 用例 / 变体 | 期望检出 | 实测 |
|---|---|---|
| `regress-440` / deepseek（含 `geared to being a weapon`、`accompanying media`） | 是 | ✅ 检出 3 个片段 |
| `regress-440` / glm（正确译文） | 否 | ✅ 未报 |
| `regress-112` / glm（象牙白）、`ntrex-1` / deepseek（木偶）、`611` / glm（装回去）等 | 否（人工判断项） | ✅ 未报（不误报） |

另外 `quality/mod.rs` 还有 3 条终检级测试：坏译文**必须**报、正确译文**不得**报、
URL/代码保留**不得**报。

## 3. 当前模型表现（2026-11-13 复跑，7 条 × 2 模型）

| 用例 | DeepSeek | GLM |
|---|---|---|
| 440 未翻译片段 | 仍残留 `accompanying media`（2 词，低于阈值） | ✅ 已完整翻译 |
| 112 人名 | ✅ `Ivory` 保留 | ⚠️ `象牙白（Ivory）` —— 补了原名，但正文仍用“象牙白” |
| ntrex-1 muppets | ❌ 仍译“木偶” | ✅ “傻瓜” |
| ntrex-1195 performances | ❌ 仍译“个人表演环节” | ❌ 仍译“个人的表演” |
| 611 seat back / bolting | ✅ “座椅靠背用螺栓固定好” | ❌ “把座椅装回去” |
| ntrex-1569 incapacitated | ✅ “丧失行为能力” | ✅ |
| ntrex-1012 democracy | ✅ “民主制度” | ✅ |

两份原始报告：`docs/eval/runs/regression-deepseek-v4.1-flash.json`、
`docs/eval/runs/regression-glm-5.3-flash.json`。

**结论（与人工审阅一致）**：两个模型各有所长——DeepSeek 在信息精度上更好，
GLM 在口语/习语上更好；没有任何一个模型在全部类别上占优。因此
"DeepSeek 作为默认 Precise 候选、GLM 作为备选"是**候选**判断，不是质量认证。

## 4. 怎么跑

```bash
export STARHUB_KEY=...            # 只从环境变量读
cargo run -q --release -p quality-runner -- \
  --dataset fixtures/eval/regression.jsonl \
  --config ../../docs/eval/starhub.example.toml \
  --jobs 2 --out /tmp/regress.json
```

- 机器判定只覆盖 `no_residual_english`（保护计划 + 原文对应）与 `forbidden_phrases`（人工确认的漏译片段，数据驱动）；
- `severity = reference` 的用例不会判模型失败（它们是参考译文问题）；
- 其余用例的输出需要人读，报告里的 `severity` 字段标明严重级别。

## 5. 后续建议

1. 把这 7 条作为**发布前必看清单**，每次换模型/提示词都复跑并人工核对；
2. 从 NTREX 里继续挑 20–50 条参考译文可疑的样本做同样标注（本轮只标了 2 条已确认的）；
3. 实体一致性、上下文词义目前只能人工，考虑后续引入术语表/上下文窗口，但**不要**为具体句子写规则。
