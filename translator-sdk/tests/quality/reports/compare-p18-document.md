# 翻译质量自动对照

- 金样本：fixtures/quality/document-v1.jsonl
- 样本数：10
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| p16-doc | 0.6466 | 0.6612 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| p18-doc | 0.6444 | 0.6590 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |

## 分类 chrF

| 类别 | p16-doc | p18-doc |
|---|---:|---:|
| document | 0.6466 | 0.6444 |

## 相对基线的变化

- **p18-doc**：chrF -0.0022，BLEU -0.0022，错误 +0，新增致命 0，回归样本 1，改善样本 1
  - `document-v1-0004` chrf_drop -0.1129：# Database migration

The migration may take 30 minutes. Do not stop the service during this time.

1. Back up the database.
2. Run the migration script.
3. Verify the schema.
 → 基线 `# 数据库迁移

移民可能需要30分钟,在此期间不要停止服务。

1. 备份数据库。
2. 运行迁移脚本。
3. 验证计划
` / 本次 `# 数据库迁移

移徙可能需要30分钟。 这段时间不要停止服役。

1. 备份数据库。
2. 运行迁移脚本。
3. 验证计划
`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
