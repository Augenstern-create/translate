# S3 基线：真实转发站质量评测（dev 集）

> 状态：**自动规则检查通过，人工质量评审 = `PENDING_HUMAN`**。
> 本文只报告"机器可判定"的结论（必须保留的内容、数字/金额、术语约束、格式），
> **不**对"译得准不准、顺不顺"下结论——那必须由人读。

## 1. 方法

| 项 | 值 |
|---|---|
| 数据集 | `translator-sdk/fixtures/eval/dev.jsonl`（100 条，16 个类别）与 `smoke.jsonl`（30 条） |
| 语向 / 模式 | en → zh-CN，`precise` |
| 后端 | OpenAI 兼容转发站 `https://starhub.iepose.cn/v1`（真实网络） |
| 模型 | `deepseek-v4.1-flash`、`glm-5.3-flash` |
| 密钥 | 仅从环境变量 `STARHUB_KEY` 读取；未写入仓库、配置或日志 |
| 策略 | `TranslationPolicy::RemoteOnly`（评测对象是远程 LLM，必须显式授权） |
| 术语 | 用数据集里的 `glossary` 字段生成临时术语表，`TermPolicy::Enforce` |
| 运行 | `cargo run -p quality-runner -- --dataset fixtures/eval/dev.jsonl --config <toml> --glossary-from-dataset --out <json> --out-md <md>` |

判定规则（全部机器可判定）：

1. `must_preserve`：URL / 邮箱 / 模板变量 / 代码片段必须逐字出现；
2. `must_keep_numbers`：关键数字保留（先看字面，再由 SDK 语义判定，`$39.99` ↔ `39.99 美元` 视为等价）；
3. 用例声明了数字时，SDK 报出的 `number_missing` / `number_added` / `number_relation_mismatch`（Major）也算失败；
4. `glossary`：术语约束必须满足（`Enforce`：不满足即失败，**不做**译后替换）；
5. 空译文、保护标记残留、受保护内容缺失由 SDK 完整性门禁负责，运行器记录其结果。

## 2. 结果

| 数据集 | 模型 | 通过 | HTTP 调用 | 输入/输出 token | p50 | p95 | max |
|---|---|---|---|---|---|---|---|
| dev（100） | deepseek-v4.1-flash | **100 / 100** | 100 | 25,736 / 17,240 | 3,143 ms | 4,319 ms | 5,068 ms |
| dev（100） | glm-5.3-flash | **100 / 100** | 100 | 23,526 / 1,665 | 2,817 ms | 5,426 ms | 12,073 ms |
| smoke（30） | deepseek-v4.1-flash | 30 / 30 | 30 | 7,699 / 4,420 | 2,567 ms | 3,347 ms | — |
| smoke（30） | glm-5.3-flash | 30 / 30 | 30 | 7,033 / 413 | 2,699 ms | 5,066 ms | — |

按类别（dev，两个模型都是 100% 通过）：clause 7/7、conditional 6/6、dialogue 6/6、
glossary 7/7、money 6/6、negation 6/6、paragraph 6/6、passive 6/6、polysemy 6/6、
question 6/6、reference 7/7、statement 6/6、tech-doc 7/7、tense 6/6、ui 6/6、url 6/6。

### 2.1 实测费用（来自转发站账单端点，不是估算）

方法：调用前后各读一次 `GET /v1/dashboard/billing/usage` 的 `total_usage`（单位：美分），
取差值；两次读取之间没有其它客户端使用该密钥。这是**该站点的真实计费**，
但只代表这一个转发站、这一天的价格。

| 数据集 | 模型 | 账单增量 | 折算每 1000 次请求 | 混合每百万 token |
|---|---|---|---|---|
| dev 100 条 | deepseek-v4.1-flash | +0.010304 USD | **$0.103** | ≈ $0.240 |
| dev 100 条 | glm-5.3-flash | +0.002824 USD | **$0.028** | ≈ $0.112 |
| smoke 30 条 | deepseek-v4.1-flash | +0.002918 USD | $0.097 | — |
| smoke 30 条 | glm-5.3-flash | +0.000586 USD | $0.020 | — |

结论：**glm-5.3-flash 在这个转发站上约便宜 3.6 倍**（同样的 dev 集、同样 100% 通过机器判定）。
注意这与"输出 token 更少"是两件事——费用差异来自单价与输出计数口径的叠加。

### 2.2 定价表与实测不一致（不要用表算账）

`GET /api/pricing` 给出的倍率（`model_ratio` / `completion_ratio`）按 new-api 常见口径
（1 倍率 = $2/百万 token）推算，会**低估**：
deepseek 推算 $0.00782 vs 实测 $0.01030（+32%），
glm 推算 $0.00066 vs 实测 $0.00282（约 4.3 倍）。

因此本仓库**不硬编码供应商价格**，`quality-runner` 的 `estimated_cost_usd`
在未配置单价时为 `null`；要做费用对比就用上面的账单差值法。

### 2.3 必须说明的计量差异

`glm-5.3-flash` 报告的输出 token（30 条 smoke 仅 413）明显低于 `deepseek-v4.1-flash`
（4,420），但两者输出**字符数几乎相同**（639 vs 643）。差异来自服务端的计数口径
（很可能是把"思考/推理 token"计入或不计入 completion）。因此：

- **不要把 token 数当作跨厂商可比的成本指标**；
- 真实费用必须以转发站账单为准（本仓库不硬编码任何供应商价格，`estimated_cost_usd`
  在未配置单价时为 `null`）。

## 3. 本轮评测暴露并修掉的问题

| 问题 | 处理 |
|---|---|
| `must_preserve` 把金额写成字面 `$12.50`，但正确译文是 `12.50 美元` → 6 条 money 用例被误判失败 | 数据集修正：金额从 `must_preserve` 移到 `must_keep_numbers`（语义校验），并在 `notes` 说明理由 |
| SDK 把 `every second` / `finishes first` / `First, open ...` 当成序数词，要求译文出现"第二/第一" | `number` 模块新增序数词上下文判定：前面是 `every/each/per/a/an` 或紧邻词不是限定词时**不**当数字（8 条新断言） |
| 运行器只看 Critical 数字问题，币种丢失（Major）不会被发现 | 用例声明数字时，Major 的 `number_missing` / `number_added` / `number_relation_mismatch` 也算失败 |
| 运行器默认策略是"本地优先、绝不自动回退远程"，评测直接失败 | 显式 `RemoteOnly`；并在报告里注明这是评测用途 |
| 没有费用/用量维度 | 报告新增 `http_calls`、`content_retries`、`tokens_*`、`estimated_cost_usd`、`latency_p95_ms`，并可输出 Markdown 评审包 |

## 3.1 blind 集已建立（未运行、不得用于调参）

`translator-sdk/fixtures/eval/blind.jsonl`：96 条（16 个类别 × 6），与 dev/smoke 文本零重叠
（`tools/quality-runner/tests/datasets.rs` 里有断言）。它**没有**被自动化跑过，
也不会参与任何提示词/规则调整——按照约定，blind 集只用于人工评分。

`reference_zh` 仍全部为 `null`：参考译文需要人写，我没有代替人工撰写。

## 4. 人工评审材料

- `docs/eval/dev-deepseek-v4.1-flash.md`
- `docs/eval/dev-glm-5.3-flash.md`

每行包含：用例 id、类别、原文、译文、机器判定。评审建议关注：

1. 复杂从句的语序与指代（`clause`、`reference`、`paragraph`）；
2. 否定与条件是否被弱化（`negation`、`conditional`）；
3. 一词多义是否选对（`polysemy`）；
4. 数字/金额/时间的中文表达是否自然（`money`）；
5. 术语是否真的用了标准译法（`glossary`，当前 `Enforce` 会拦下不满足的译文）。

评分建议：准确性 / 完整性 / 流畅性各 1–5 分，术语（符合/不符合）、格式（保留/丢失）逐条记录，
最后给出"通过 / 需返工（列至少 3 条具体例子）"。

## 5. 仍未做（PENDING_HUMAN）

- `reference_zh` 仍为空：没有人工参考译文，因此**没有** BLEU/chrF 之类指标，也不应据此调参；
- blind 集尚未建立（blind 不参与调参）；
- 本地模型（Qwen2.5-1.5B）与远程模型的质量对比未做（本轮只跑远程）；
- 上述 Markdown 评审包**尚未有人阅读**。

在完成人工评审之前，本文不能作为"翻译质量合格"的证据，只能说明
"机器可判定的约束在真实服务上全部满足"。
