# 翻译质量自动对照

- 金样本：fixtures/quality/beam-subset.jsonl
- 样本数：40
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| greedy | 0.4402 | 0.4658 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| beam2 | 0.4624 | 0.4885 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| beam4 | 0.4682 | 0.4974 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |

## 分类 chrF

| 类别 | greedy | beam2 | beam4 |
|---|---:|---:|---:|
| long | 0.3623 | 0.3794 | 0.3941 |
| numbers_units | 0.57 | 0.6007 | 0.5917 |

## 相对基线的变化

- **beam2**：chrF +0.0222，BLEU +0.0227，错误 +0，新增致命 0，回归样本 2，改善样本 10
  - `en-zh-0214` chrf_drop -0.1624：The software update fixes several security problems, improves battery life, and adds support for three new languages. → 基线 `软件更新解决了几个安全问题,改善了电池使用,增加了三种新语言的支持。` / 本次 `软件更新解决了若干安全问题,改善了电池使用,并为三种新语言增加了支持。`
  - `en-zh-0224` chrf_drop -0.0428：The conference will bring together researchers, engineers, and product managers to discuss the future of artificial intelligence. → 基线 `这次会议将召集研究人员、工程师和产品管理人员讨论人造情报的未来。` / 本次 `会议将召集研究人员、工程师和产品管理人员讨论人造情报的未来。`
- **beam4**：chrF +0.0280，BLEU +0.0316，错误 +0，新增致命 0，回归样本 3，改善样本 11
  - `en-zh-0295` chrf_drop -0.2347：The train departs at 07:15 and arrives at 09:40. → 基线 `火车于7: 15出发,9: 40抵达。` / 本次 `火车于7点15分出发,9点40分抵达`
  - `en-zh-0214` chrf_drop -0.0771：The software update fixes several security problems, improves battery life, and adds support for three new languages. → 基线 `软件更新解决了几个安全问题,改善了电池使用,增加了三种新语言的支持。` / 本次 `软件更新解决了若干安全问题,改善了电池寿命,增加了三种新语言的支持。`
  - `en-zh-0229` chrf_drop -0.0765：We need to confirm the delivery address, the contact number, and the preferred delivery time before we can process your order. → 基线 `我们需要确认发送地址 联系号码，优先交付时间,然后才能处理您的订单.` / 本次 `我们需要确认发送地址 联系号码，处理您的订单之前的首选交付时间.`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
