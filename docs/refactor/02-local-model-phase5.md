# Phase 5：本地模型接入实测

**日期：** 2026-11-11
**执行环境：** Linux，i5-8250U（4 核 8 线程），32 GiB RAM，**无 GPU 参与**
**结论：** 本地模型经**同一套 HTTP 后端**完成真实翻译；机器可判定项 29/30 通过；
人工质量评审仍为 `PENDING_HUMAN`。

---

## 1. 部署形态

```text
translator-cli / SDK → HTTP(127.0.0.1:18082) → llama.cpp server → Qwen2.5-1.5B-Instruct(Q4_K_M)
```

与远程转发站**完全相同**的代码路径（`translator-http`），只是 `base_url` 指向回环地址。
按路由规则，回环地址被视为**本地**后端，不需要"远程回传授权"。

| 组件 | 版本/规格 |
|---|---|
| llama.cpp | b12283（官方预编译 `llama-b12283-bin-ubuntu-x64`） |
| 模型 | `Qwen2.5-1.5B-Instruct` GGUF，Q4_K_M，1.06 GB |
| 服务参数 | `-c 4096 -t 4 -b 256 --host 127.0.0.1 --port 18082` |
| 常驻内存（RSS） | **1869 MB** |
| SDK 配置 | `model_revision = "gguf-q4_k_m-b12283"`、`temperature = 0`、`max_output_tokens = 512` |

复现命令：

```bash
llama-server -m qwen2.5-1.5b-instruct-q4_k_m.gguf --host 127.0.0.1 --port 18082 -c 4096 -t 4
# translate.toml
#   [backend.local] base_url="http://127.0.0.1:18082/v1"  api_key_env=""  model_revision="gguf-q4_k_m-b12283"
quality-runner --dataset fixtures/eval/smoke.jsonl --config translate.toml --out report.json
```

---

## 2. 结果

### 2.1 Smoke 评测（30 条，机器可判定项）

| 指标 | 值 |
|---|---|
| 通过 | **29 / 30** |
| 失败 | 1（术语未遵循） |
| 延迟 p50 | **732 ms** |
| 延迟 max | **2639 ms** |
| 人工质量评审 | `PENDING_HUMAN` |

按类别：statement / question / negation / conditional / passive / tense / clause /
reference / polysemy / ui / tech-doc / dialogue / **url** / paragraph 全部通过；
`glossary` 类别 1 条失败（见 §3）。

完整报告：[`evidence/smoke-local-qwen15b.json`](evidence/smoke-local-qwen15b.json)。

**保护机制实测**（真实模型 + 真实 HTTP）：

- URL 原样保留：`访问 https://example.com/docs?page=2&lang=en 获取完整指南。`
- 模板变量原样保留：`欢迎回来。{username}您的草稿已保存。`
- 日期本地化但数值不变：`合同续签将于2025年3月14日到期。`
- 金额币种本地化：`年度计划每用户费用为39.99美元。`
- 否定保持：`打印机不支持双面打印。`

### 2.2 术语约束在生成前生效（同一条输入，两次运行）

```console
# 不加载术语表
$ translator-cli translate "A large language model can summarize long documents."
大型语言模型可以总结长文档。

# 加载 glossary（large language model → 大语言模型，strict）
$ translator-cli translate "A large language model can summarize long documents."
大语言模型可以总结长文档。
```

这正是 §6.1 想要的效果：**术语在生成之前进入约束**，而不是译后替换。
（smoke-025 的失败也证明：不注入约束时 1.5B 模型会选另一个同义词。）

---

## 3. 失败项分析

| 用例 | 现象 | 性质 |
|---|---|---|
| `smoke-025`（glossary） | 无术语表时输出「大型语言模型」而不是约束要求的「大语言模型」 | **真实术语未遵循**。`term_policy = "enforce"` 下会直接判失败（已验证注入后模型会照做） |

评测过程中还发现并修正了**评测集自身的两处过严约束**（不是模型问题）：

- 金额 `$39.99` 要求逐字出现 → 自然中文写成 `39.99美元` 被误判。
  已改为语义判定（数值 + 币种），由 SDK 的 `quality::number` 负责；
- 时间 `2:00 AM` 要求逐字出现 → `凌晨2点` 被误判。
  已改为数值判定，并顺手补上 SDK 的上午/凌晨 12 小时制等价识别。

---

## 4. 与远程基线的对比：**未做**

按要求，"本地模型是否值得进入默认主链"必须与远程基线（转发站上的 DeepSeek）
在**同一数据集**上对比质量、数字准确率、术语遵循率、延迟。当前缺少：

- 可用的 `base_url` 与密钥（`PENDING_HUMAN`）；
- 人工评分（`reference_zh` 仍为空）。

因此本轮**不给出**"本地 vs 远程谁更好"的结论，也不把本地模型设为默认后端。

---

## 5. 结论与限制

**达成（Phase 5 的准入要求）：**

- 本地模型可以经统一接口完成真实翻译（可复现命令与参数已记录）；
- 延迟与内存有实测数字（p50 732 ms / RSS 1.87 GB，CPU 推理）；
- 保护/数字/术语机制在真实模型上被验证有效（含 URL、变量、日期、金额、否定）。

**未达成 / 限制：**

- 未与远程基线对比质量（缺密钥与人工评分）；
- 只有 CPU 推理数据，RTX 2050 的 GPU 路径未测；
- 1.5B 模型在术语选择上不稳定（需要术语表约束兜底）；
- 上下文长度 4096，长文档需要分块（当前 SDK 不做自动分块）。
