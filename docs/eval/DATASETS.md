# 外部评测数据集：来源、许可证与规模

本文记录**外部语料**如何进入本项目的评测体系。自建用例（`smoke.jsonl` / `dev.jsonl` /
`blind.jsonl`）保持原样，不被覆盖；外部数据单独放在 `fixtures/eval/bench/`（公开部分）
与 `fixtures/eval/private/`（私有盲测，不入库）。

## 1. 数据来源与许可证清单

| dataset_id | 数据 | 版本 / 快照 | 许可证 | 可再分发 | 用途 |
|---|---|---|---|---|---|
| `wmt24pp` | [WMT24++](https://huggingface.co/datasets/google/wmt24pp)（`en-zh_CN.jsonl`） | HF snapshot 2026-07-30 | **Apache-2.0** | 是 | 公开基准 |
| `ntrex128` | [NTREX-128](https://github.com/MicrosoftTranslator/NTREX)（`newstest2019-src.eng.txt` + `newstest2019-ref.zho-CN.txt` + `DOCUMENT_IDS.tsv`） | main @ 2026-11-12 | **CC-BY-SA-4.0** | 是（需署名 + 相同方式共享） | 开发集 + 公开基准 + 私有盲测（按文档切分） |

### 已调查但**未导入**

| 数据 | 结论 | 依据 |
|---|---|---|
| [FLORES+](https://huggingface.co/datasets/openlanguagedata/flores_plus) | HuggingFace gated，未登录/未接受条款返回 401 | 实测 `HTTP 401 Access restricted`；按项目规则不绕过 |
| [FLORES-200](https://huggingface.co/datasets/facebook/flores) | 同上（`gated: auto`），许可证 CC-BY-SA-4.0 但下载需账号 | 实测 `HTTP 401`，`resolve/main/...parquet` 被拒 |
| [WMT26 General MT](https://data.statmt.org/wmt26/) | 目前只公开 **blind set**（`wmt26_genmt_blindset.jsonl`，仅源文、无参考译文） | 目录列表；无参考译文无法计算 chrF/BLEU，带参考的测试集通常赛后发布 |
| Tatoeba Challenge | 可用（Tatoeba 句子 CC-BY 2.0 FR），但众包句子领域/质量不可控 | 第一版不纳入基准，仅记录为潜在补充 |

> 结论：**没有绕过任何访问限制**。受限数据只记录在案，不下载、不入库。

## 2. 实际可用样本

导入工具：`cargo run -p eval-import -- --external fixtures/eval/external --out fixtures/eval/bench`

| 划分 | 条数 | 说明 |
|---|---|---|
| `bench/dev.jsonl` | **383** | 开发集（NTREX 文档级切分），可提交 |
| `bench/test.jsonl` | **2 213** | 公开基准（WMT24++ 958 + NTREX 1 255） |
| `private/blind.jsonl` | **358** | 私有盲测（NTREX 文档级切分），**不入库** |
| 合计 | **2 954** | 约 3 000 条的第一版目标已达成 |

### 领域分布

| 领域 | 条数 | 来源 |
|---|---|---|
| news | 2 145 | NTREX 1 996 + WMT24++ 149 |
| social | 492 | WMT24++ |
| literary | 206 | WMT24++ |
| speech | 111 | WMT24++ |

### 长度与难度（源文字符数）

| 指标 | 值 |
|---|---|
| p50 | 118 |
| p90 | 282 |
| max | 967 |
| short（≤60） | 577 |
| medium（61–160） | 1 441 |
| long（161–400） | 802 |
| very-long（>400） | 134 |
| 多句段落（≥2 句末标点） | 703 |
| 文档数 | 293 |

详细统计见 `translator-sdk/fixtures/eval/bench/STATS.md`（导入时自动生成）。

## 3. 划分与防污染

- **开发集**：`bench/dev.jsonl`（383 条，NTREX 文档级）。
- **公开基准**：`bench/test.jsonl`（2 213 条）。一旦用于对外报告，就**不再**用于调提示词。
- **私有盲测**：`private/blind.jsonl`（358 条），已在 `.gitignore` 中排除；只在本地人工评审时使用。
- NTREX 的切分按 `sha256(document_id) % 100`：0–14 → dev，15–29 → blind，其余 → test，
  **同一文档不会跨 split**（`eval-import` 单测覆盖）。
- 跨数据集按归一化源文去重（WMT24++ 与 NTREX 之间也去重）。
- 自建 `smoke/dev/blind` 继续保留，与外部数据分开统计（`dataset_id = handcrafted`）。

## 4. 过滤规则（可复现）

`eval-import` 的丢弃计数会写进 `bench/SOURCES.json` 的 `converted.dropped`：

| 原因 | 条数（本版） |
|---|---|
| WMT24++ canary 行 | 1 |
| WMT24++ 标记为 `bad_source` | 37 |
| 源文与参考归一化后相同 | 2 |
| 源文重复 | 1 |
| 参考译文不含汉字（中英错位） | 0 |
| 空源文 / 空参考 | 0 |
| 源文没有拉丁字母 | 0 |

## 5. 元数据字段

每条样本：

```json
{
  "id": "wmt24pp-1",
  "dataset_id": "wmt24pp",
  "source": "...",
  "reference": "...",
  "domain": "social",
  "document_id": "...",
  "segment_id": 42,
  "split": "test",
  "source_lang": "en",
  "target_lang": "zh-CN",
  "script": "simplified",
  "license": "Apache-2.0",
  "char_len": 122,
  "difficulty": "medium",
  "multi_sentence": true
}
```

## 6. 自动指标

`quality-runner` 在有 `reference` 时自动计算：

- **chrF++**：字符 1–6 gram + 词 1–2 gram，β=2，逐句计算后取平均（本项目的主指标）；
- **BLEU-4**：CJK 按字、其余按空白切分，有效阶 + 简洁惩罚，
  **不是** sacrebleu 的 13a tokenizer，不能与 sacrebleu 直接比较；
- **COMET**：需要 PyTorch + 权重下载，属于可选扩展，本仓库不内置。

口径细节见 `tools/quality-runner/src/metrics.rs` 的模块文档与 9 条单测。

## 7. 统一对照接口

```bash
export STARHUB_KEY=...   # DeepSeek / GLM 走同一转发站
cargo run -p quality-runner -- \
  --dataset fixtures/eval/bench/dev.jsonl \
  --compare ../../docs/eval/starhub.example.toml,../../docs/eval/glm.example.toml \
  --out /tmp/bench.json
```

- 每个配置写一份 `<out>-<label>.json`，另出一份 `<out>-compare.md` 对照表；
- 自部署模型（llama.cpp / vLLM 等 OpenAI 兼容服务）只要把 `base_url` 指向本地即可，
  其余字段不变——这就是"DeepSeek / GLM / 自部署模型统一对照接口"。

首轮 20 条真实对照：`docs/eval/bench-dev20-compare.md`。

## 8. 仍然缺的领域（据实记录）

| 缺口 | 现状 | 影响 |
|---|---|---|
| 法律 / 合同 | 无 | 无法评估条款类长句、情态动词强度 |
| 医疗 / 药品说明 | 无 | 无法评估剂量、禁忌等高风险表述 |
| 专利 / 技术标准 | 无 | 术语密度不足（当前术语仅来自自建集） |
| 电商 / 商品描述 | 部分（`social` 略近似） | 缺 SKU、规格、单位混排 |
| 代码 / 混合排版 | 无（自建集有少量） | 缺 Markdown/HTML 混排真实样本 |
| 繁体中文 | 无 | WMT24++ 有 `en-zh_TW.jsonl`，本版未导入 |
| 交互式多轮 | 无 | 无上下文指代评测 |
| 人工参考译文的**质量标注** | 无 | 只有参考译文，没有"参考译文是否可接受"的标注 |

## 9. 复现步骤

```bash
cd translator-sdk
bash scripts/fetch-eval-data.sh          # 下载原始数据 + 记录 sha256/许可证/日期
cargo run -p eval-import -- --external fixtures/eval/external --out fixtures/eval/bench
cargo test -p eval-import -p quality-runner
```

原始数据在 `fixtures/eval/external/`（git 忽略），转换结果在 `fixtures/eval/bench/`，
清单 `fixtures/eval/bench/SOURCES.json` 记录 URL、版本、许可证、下载日期与 sha256。
