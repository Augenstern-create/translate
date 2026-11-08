# 翻译质量自动对照

- 金样本：fixtures/quality/paragraph-v1.jsonl
- 样本数：50
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| sentence | 0.3987 | 0.4493 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| best | 0.4205 | 0.4655 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |

## 分类 chrF

| 类别 | sentence | best |
|---|---:|---:|
| paragraph-daily | 0.3473 | 0.3537 |
| paragraph-mixed | 0.3661 | 0.3729 |
| paragraph-technical | 0.4831 | 0.5374 |

## 相对基线的变化

- **best**：chrF +0.0218，BLEU +0.0162，错误 +0，新增致命 0，回归样本 2，改善样本 14
  - `paragraph-v1-0047` chrf_drop -0.1179：The train is late. He is learning Chinese. They are watching a movie. → 基线 `火车晚了 他在学习中文 他们在看电影` / 本次 `火车晚了,他正在学习中文,他们正在看电影`
  - `paragraph-v1-0009` chrf_drop -0.1087：A large language model can generate text. Natural language processing helps computers understand text. Cloud computing reduces the need for local servers. → 基线 `大语言模式可以生成文本。 自然语言处理有助于计算机理解文字。 云计算减少了对本地服务器的需求。` / 本次 `大型语言模式可以生成文本。 自然语言处理有助于计算机理解文本。 Cloud Cally Calpection 减少了对本地服务器的需求。`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
