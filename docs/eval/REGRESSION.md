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

## 2. 这条数据集的定位：离线评测，不进入硬门禁

`regression.jsonl` 是**模型质量评测数据**，不是 SDK 的规则来源：

| 字段 | 用途 |
|---|---|
| `severity` | `critical` / `major`（人工记录的问题严重度）、`reference`（参考译文自身错误） |
| `review_focus` | 人工评审关注点（未翻译片段、人名一致性、习语…）——只是标签，不参与机器判定 |
| `known_bad` | 人工记录的真实错误译文，供 `--replay` **离线**复核确定性核验结果 |
| `reference_issue` | 参考译文的错误说明（有值时该用例不判模型失败） |

**SDK 里没有针对这些句子的任何特殊判断**：不检测"未翻译短语"、不维护英文词表、
不按句子类型分支。语义正确性由人工/独立评测判断，不能因为"没有报错"就当作正确。

## 2.1 确定性与非确定性

| 类型 | 例子 | 谁负责 |
|---|---|---|
| 确定性（可硬门禁） | URL/代码/占位符丢失或重复、编号/日期/金额被改、术语未满足、空译文 | SDK（`docs/guardrails.md` G1–G3） |
| 非确定性（只记录/人工） | `muppets` 词义、`performances` 语境、`Ivory` 人名、`accompanying media` 漏译、`seat back` 信息精度 | 人工评审 / 模型选择 |

`--replay` 现在只打印**确定性**核验结果（例如受保护内容是否完整），不再对"是否检出
未翻译片段"给通过/失败结论。

## 3. 当前模型表现（2026-11-13 复跑，方向纠正后，7 条 × 2 模型）

> 说明：这两份报告是**方向纠正后**的复跑。模型输出每次会有差异，
> 本轮两条用例的输出比首次审阅时更好（440 已完整翻译、611 DeepSeek 修正），
> 所以"是否检出"要结合 `--replay`（用人工记录的真实坏译文验证检测器）一起看。

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
