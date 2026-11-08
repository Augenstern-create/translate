# 翻译质量自动对照

- 金样本：fixtures/quality/paragraph-v1.jsonl
- 样本数：50
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| sentence | 0.3987 | 0.4493 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| context | 0.4120 | 0.4607 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |

## 分类 chrF

| 类别 | sentence | context |
|---|---:|---:|
| paragraph-daily | 0.3473 | 0.3473 |
| paragraph-mixed | 0.3661 | 0.361 |
| paragraph-technical | 0.4831 | 0.5314 |

## 相对基线的变化

- **context**：chrF +0.0133，BLEU +0.0114，错误 +0，新增致命 0，回归样本 3，改善样本 10
  - `paragraph-v1-0030` chrf_drop -0.2086：The library opens at nine in the morning and closes at eight in the evening, but during the summer it stays open until ten so that students can prepare for their exams. Although the weather forecast said it would be sunny, heavy rain started in the afternoon and we had to cancel the picnic. The new policy requires all employees to complete a security training course before they can access the internal network. → 基线 `图书馆上午9点开放,晚上8点关闭,但夏季开放时间为10点,以便学生为考试做准备。 虽然天气预报说天气晴朗, 暴雨始于下午, 新政策要求所有雇员在进入内部网络之前完成安全培训课程。` / 本次 `图书馆上午9点开放,晚上8点关闭,但夏季开放时间为10点,以便学生为考试做准备。 尽管气象预报说,它会晴朗,但大雨始于下午, 即便如此,在野餐中,他们仍需要接受安全培训。`
  - `paragraph-v1-0014` chrf_drop -0.1057：The SDK includes a command-line interface. The CPU usage stayed below 50%. The GPU accelerates matrix multiplication. → 基线 `SDK包括一个命令线界面。 CPU使用率保持在50%以下. GPU 加速矩阵乘法的加速。` / 本次 `SDK包括一个命令线界面。 CPU 使用率保持在下面50%. GPU 加速矩阵乘法的加速。`
  - `paragraph-v1-0004` chrf_drop -0.0809：The source code is available on GitHub. This project is open source and free to use. Please submit a pull request with your changes. → 基线 `源代码可在 GitHub 上查阅。 该项目是开放源码,可自由使用。 请提交包含您更改的拉动请求。` / 本次 `源代码可在 GitHub 上查阅。 此工程为开源, 且可自由使用。 请随您更改提交拉动请求。`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
