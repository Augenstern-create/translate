# 翻译质量自动对照

- 金样本：fixtures/quality/paragraph-budget-v1.jsonl
- 样本数：40
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| auto | 0.4484 | 0.4892 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| best | 0.4449 | 0.4841 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |

## 分类 chrF

| 类别 | auto | best |
|---|---:|---:|
| budget-2 | 0.3979 | 0.3909 |
| budget-3 | 0.4989 | 0.4989 |

## 相对基线的变化

- **best**：chrF -0.0035，BLEU -0.0051，错误 +0，新增致命 0，回归样本 1，改善样本 0
  - `budget-2-06` chrf_drop -0.14：This project is open source and free to use. Please submit a pull request with your changes. → 基线 `该项目是开源,可自由使用。 请提交包含您更改的拉动请求。` / 本次 `此工程是打开源码, 并可以自由使用。 请随您更改提交拉动请求。`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
