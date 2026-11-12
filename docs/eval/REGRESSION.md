# 翻译质量回归集（regression.jsonl）

来源：2026-11-13 对 `bench/test.jsonl` 全量译文的人工语言审阅（news / literary / social / speech 分层抽样），
挑出**自动检查通过但语义有问题**的样本，以及两条**参考译文本身错误**的样本。

数据集：`translator-sdk/fixtures/eval/regression.jsonl`（7 条）。

## 1. 覆盖的三类真实错误

| 类别 | 用例 | 问题 | 自动化程度 |
|---|---|---|---|
| 未翻译的英文片段 | `regress-440` | DeepSeek 输出里留下整段英文（`geared to being a weapon`） | **机器可判定**（`no_residual_english`） |
| 人物/实体一致性 | `regress-112` | `Ivory`（人名）被 GLM 译成“象牙白” | 人工评审（无法可靠自动判定） |
| 依赖上下文的词义 | `regress-ntrex-1`（muppets）、`regress-ntrex-1195`（performances） | 习语/领域词义选错 | 人工评审 |
| 信息精度 | `regress-611` | `seat back` / `bolting` 被简化掉 | 人工评审 |
| 参考译文质量 | `regress-ntrex-1569`（incapacitated）、`regress-ntrex-1012`（democracy） | NTREX 参考译文本身译错 | **不计模型失败**，参考已按英文原意更正并在 `reference_issue` 记录原值 |

## 2. 新增的自动检查：未翻译英文片段

`translator-core/src/quality/heuristic.rs` 的 `residual_english_fragment`：

- 连续 **≥5 个纯英文单词**（URL/代码/带数字的 token 会打断连续段）；
- 其中 **≥2 个功能词**（the/of/to/and/being…）→ 排除品牌（`iPhone 15 Pro`）；
- 至少 **1 个小写内容词**（长度 ≥4）→ 排除专名标题（`The Lord of the Rings`）；
- 命中只报 **Major**（`untranslated`），不判致命：这是启发式，目的是"让人看见"，不是自动拒稿。

已知边界：它抓不住 **2–4 个词的残留**。实测复查时 DeepSeek 的新输出是
`……并在 accompanying media 中以无谓的效果加以展示。`——`accompanying media` 只有 2 个词，
低于阈值，**不会**被这条规则报出来；这类残留仍需人工。阈值取 5 是刻意的：
降到 3 会让 `The Great Gatsby`、`Academy Award for Best Picture` 这类专名标题误报。

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

- 机器判定只覆盖 `no_residual_english`；
- `severity = reference` 的用例不会判模型失败（它们是参考译文问题）；
- 其余用例的输出需要人读，报告里的 `severity` 字段标明严重级别。

## 5. 后续建议

1. 把这 7 条作为**发布前必看清单**，每次换模型/提示词都复跑并人工核对；
2. 从 NTREX 里继续挑 20–50 条参考译文可疑的样本做同样标注（本轮只标了 2 条已确认的）；
3. 实体一致性、上下文词义目前只能人工，考虑后续引入术语表/上下文窗口，但**不要**为具体句子写规则。
