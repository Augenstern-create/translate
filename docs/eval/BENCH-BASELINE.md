# 公开基准（bench/test.jsonl）首轮可比基线

> 数据集：`fixtures/eval/bench/test.jsonl`，**2 213 条**（WMT24++ 958 + NTREX-128 1 255），
> 全部带真实参考译文；领域 story 分布见 `fixtures/eval/bench/STATS.md`。
> 人工评审：**PENDING_HUMAN**（自动指标不替代人工验收）。

## 1. 结论先行

| 模型 | 机器判定通过 | chrF++ | BLEU-4 | p50 | p95 | 实测费用（账单差值） |
|---|---|---|---|---|---|---|
| `deepseek-v4.1-flash` | **2 210 / 2 213（99.86%）** | **30.72** | **36.36** | **2 258 ms** | **3 462 ms** | **$0.0898** |
| `glm-5.3-flash` | **2 211 / 2 213（99.91%）** | 30.12 | 35.21 | 3 564 ms | 7 714 ms | **$0.0908** |

- 质量：两个模型在参考译文上的 chrF++ 只差 **0.6**、BLEU-4 差 **1.15**，
  在这个规模上属于同一档；**deepseek-v4.1-flash 略高**。
- 延迟：deepseek 明显更快（p50 2.26 s vs 3.56 s，p95 3.46 s vs 7.71 s）。
- 费用：两者几乎相同（都约 **$0.09 / 2 213 条** ≈ **$0.041 / 千条**）。
  这与自建 dev 集上的结论（glm 便宜 3.6×）**不同**——因为这里的文本更长
  （p50 118 字符 vs 自建集约 40 字符），输出 token 主导成本，
  且两个模型在这批文本上的输出量接近（66 k vs 75 k）。
- 结论：**在长文本、带参考译文的公开基准上，deepseek-v4.1-flash 的性价比更好**
  （同等质量、更快、同价）。

## 2. 方法（可复现）

```bash
export STARHUB_KEY=...        # 只从环境变量读取，不写进任何文件

# 关闭"思考" + 并发 3 + 重试 4（详见下文"429 的教训"）
cargo run -q --release -p quality-runner -- \
  --dataset fixtures/eval/bench/test.jsonl \
  --config /tmp/eval-deepseek-fast.toml \
  --jobs 3 --out /tmp/bench-ds.json
```

配置要点：

| 项 | 值 | 原因 |
|---|---|---|
| `disable_thinking = true` + `thinking_style = "thinking_type"` | 开 | 该转发站的 deepseek 模型支持 `thinking: {"type":"disabled"}`；实测把 completion token 从 45 降到 6、延迟从 4.4 s 降到 2.0 s |
| `max_retries = 4` | 开 | 站点对单 key 有并发上限（实测 ~3），超出直接 429 |
| `[http] max_concurrent_requests = 3` | 开 | 与站点上限匹配；并发 8 时 26%（577/2 213）请求 429 失败 |
| `max_http_calls_per_request = 3` | 开 | 单请求的物理调用上限（S2 预算） |
| `strict_integrity = true` | 开 | 完整性/数字/语言检查不合格即失败（不缓存、不返回） |

指标口径：chrF++（字符 1–6gram + 词 1–2gram，β=2）与 BLEU-4（CJK 按字混排分词、有效阶），
均为**逐句计算后平均**；BLEU 不是 sacrebleu 的 13a tokenizer，不能与之直接比较。

## 3. 分领域

| 领域 | 条数 | deepseek 通过 | glm 通过 |
|---|---|---|---|
| news（NTREX + WMT24++） | 1 404 | 1 404 / 1 404 | 1 403 / 1 404 |
| social（WMT24++） | 492 | 489 / 492 | 491 / 492 |
| literary（WMT24++） | 206 | 206 / 206 | 206 / 206 |
| speech（WMT24++） | 111 | 111 / 111 | 111 / 111 |

失败全部集中在这两类：

1. **纯话题标签贴文**（`#TeaganAir #pilot #flying #pa28`）：模型整条保留英文标签，
   译文没有汉字 → 被 SDK 的"目标语言"检查判为致命。参考译文中标签是翻译过的
   （`#飞行员培训`），所以这确实是模型行为问题，不是评测误判。
2. **内容过滤**（`ntrex128-1947`，glm）：服务端返回 `content_filter`，
   SDK 报 `IntegrityFailure`。这是服务端策略，不是翻译质量问题。

## 4. "思考"开关：为什么要关

实测同一条请求（`The meeting starts at 3 pm.`）：

| 设置 | 延迟 | completion tokens | 其中 reasoning |
|---|---|---|---|
| 默认（思考开） | 4.41 s | 45 | 38 |
| `thinking: {"type":"disabled"}` | 2.06 s | 6 | 0 |
| `reasoning_effort: "none"` | 1.74 s | 6 | 0 |

在 20 条 bench/dev 样本上：思考开 p50 3 792 ms、输出 6 907 tokens；
关掉后 p50 2 187 ms、输出 517 tokens（**13× 更少**），机器判定通过数不变。

首次尝试的完整跑（思考开、并发 3）在 **55 分钟**内没跑完，已花 **$0.43**；
关闭思考后 **24–40 分钟**跑完，花费 **$0.09**。这就是本轮改用"关思考 + 并发 3"的原因。

## 5. 429 的教训（对后续跑分很重要）

- 站点对单 API key 有并发上限，实测约 **3**（8 并发时只有 3 个成功）。
- 第一次全量跑用 `--jobs 4` × 两个模型 = 8 并发，结果 **577 / 2 213（26%）**
  请求收到 HTTP 429；`max_retries = 1` 时预算不够，全部记成失败。
  这批数据**不能**当作质量结论（99.86% 与 26% 的差别不是模型差异）。
- 改法：`max_retries = 4` + `max_concurrent_requests = 3` + `--jobs 3`；
  重跑后 429 完全消失（deepseek 2 213 条只发了 2 214 次 HTTP 调用，即 1 次重试）。

## 6. 数据与复现

- 汇总：`docs/eval/bench-test-summary.md`（由 `scripts/summarize-eval.py` 生成）
- 完整 JSON（含每句译文）：本地 `/tmp/bench-ds.json`、`/tmp/bench-glm.json`（未入库，体积约 1 MB/份）
- 数据集与许可证：`docs/eval/DATASETS.md`
- 账单：转发站 `GET /v1/dashboard/billing/usage` 的调用前后差值（美分）：
  deepseek 64.7008 → 73.6792，glm 73.6792 → 82.7572。

## 7. 局限

- 这只是**一个**转发站、**一天**的价格与延迟；不能外推到官方 API。
- chrF++/BLEU 只反映"与参考译文的相似度"，不反映可读性、术语一致性、业务正确性；
  人工评审仍是 `PENDING_HUMAN`。
- 参考译文本身没有人工质量标注（见 `docs/eval/DATASETS.md` §8）。
- 数据集缺法律/医疗/专利/电商等领域，结论只覆盖 news / social / literary / speech。
