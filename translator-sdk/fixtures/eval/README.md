# 翻译评测集（en → zh-CN）

本目录存放英文到简体中文翻译系统的评测数据：

| 文件 | 行数 | 用途 |
| --- | --- | --- |
| `smoke.jsonl` | 30 | 每次改动后的快速回归（冒烟） |
| `dev.jsonl` | 100 | prompt 与模型开发阶段的调参、对比 |

## 格式

- UTF-8 编码，JSONL：每行一个 JSON 对象，以 `\n` 分隔。
- 文件内不写注释、不写 markdown 围栏、不允许尾随逗号。
- 每行对象的键固定为 10 个：`id`、`category`、`text`、`source_lang`、`target_lang`、`must_preserve`、`must_keep_numbers`、`glossary`、`notes`、`reference_zh`。

## 字段说明

- `id`：唯一标识，`smoke-001` / `dev-001` 形式，三位零填充。
- `category`：16 个类别之一，见「类别」一节。
- `text`：英文源串，保持原样（含占位符、URL、代码等）。
- `source_lang` / `target_lang`：固定为 `en` / `zh-CN`。
- `must_preserve`：必须在中文输出中逐字节原样出现的子串（URL、代码、占位符、标签、金额、日期等），无则为 `[]`。
- `must_keep_numbers`：必须原样保留的数字或标识符 token，无则为 `[]`。
- `glossary`：`{"source": "...", "target": "..."}` 术语对，`source` 必须逐字出现在 `text` 中；无则为 `[]`。
- `notes`：一句英文，说明该用例考察什么。
- `reference_zh`：**全部为 `null`**。人工参考译文尚未产出，评审状态为 `PENDING_HUMAN`；人工评审完成前不得填写或伪造参考译文。

## 类别

`statement`、`question`、`negation`、`conditional`、`passive`、`tense`、`clause`、`reference`、`polysemy`、`ui`、`tech-doc`、`dialogue`、`money`、`url`、`glossary`、`paragraph`。

## 使用规则

- `smoke.jsonl`：每次提交前后跑一遍，确认硬约束（占位符、数字、URL、术语）没有回归。
- `dev.jsonl`：用于 prompt 设计、模型选型与错误分析，允许反复查看。
- **不得**用 `dev.jsonl` 或任何盲测集调参；盲测集只用于最终评估，禁止针对其调整 prompt 或模型。
- 禁止修改 `text` 来迁就输出；约束校验以 `must_preserve` / `must_keep_numbers` 的子串匹配为准。
- 新增用例时同步更新本 README 的统计，并保持 `reference_zh` 为 `null`。

## 自检

用 `python3` 逐行 `json.loads` 校验：行数为 30 / 100、键集合一致、`id` 唯一、`reference_zh` 为 `null`，并断言每个 `must_preserve` / `must_keep_numbers` 条目都是本行 `text` 的子串。
