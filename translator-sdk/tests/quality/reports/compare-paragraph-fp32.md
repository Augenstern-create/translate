# 翻译质量自动对照

- 金样本：fixtures/quality/paragraph-v1.jsonl
- 样本数：50
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| int8 | 0.4205 | 0.4655 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| fp32 | 0.4185 | 0.4684 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |

## 分类 chrF

| 类别 | int8 | fp32 |
|---|---:|---:|
| paragraph-daily | 0.3537 | 0.3254 |
| paragraph-mixed | 0.3729 | 0.3702 |
| paragraph-technical | 0.5374 | 0.5576 |

## 相对基线的变化

- **fp32**：chrF -0.0020，BLEU +0.0029，错误 +0，新增致命 0，回归样本 12，改善样本 7
  - `paragraph-v1-0026` chrf_drop -0.1345：I am not sure about that. That sounds like a good plan. Let me think about it. → 基线 `我不确定这一点。 听起来是个不错的计划 让我考虑一下` / 本次 `我不确定这一点。 听起来是个不错的计划 让我想想`
  - `paragraph-v1-0016` chrf_drop -0.1157：Use HTTPS to protect user data. The DNS lookup failed. The regex matches email addresses. → 基线 `使用 HTTPS 保护用户数据。 DNS 查询失败。 regex 符合电子邮件地址。` / 本次 `使用 HTTPS 保护用户数据。 DNS 搜索失败。 Regex 符合电子邮件地址。`
  - `paragraph-v1-0021` chrf_drop -0.0991：I lost my wallet. Please call the police. Where can I buy a ticket? → 基线 `我钱包丢了,请报警,我在哪里买票?` / 本次 `我钱包丢了 麻烦你报警 哪里买到票?`
  - `paragraph-v1-0035` chrf_drop -0.0713：The government plans to build a new high-speed railway that will connect the capital with the southern coast. She had been working on the project for six months when she discovered that the original data was incomplete. The instructions say that you should unplug the device before cleaning it and wait until it has cooled down completely. → 基线 `政府计划修建一条新的高速铁路, 连接首都与南部海岸。 她六个月来一直在该项目上工作,当时她发现原始数据不完整。 指示上说,在清洗之前,你应该拔掉装置,然后等到它完全冷却。` / 本次 `政府计划建造一条新的高速铁路,连接首都和南部海岸。她已经为此项目工作了六个月,当时她发现原始数据不完整。 指示说,你应该在清理之前拔掉装置,等到它完全冷却之后再拆除。`
  - `paragraph-v1-0036` chrf_drop -0.0693：The conference will bring together researchers, engineers, and product managers to discuss the future of artificial intelligence. Even though the price was higher than we expected, we decided to buy the house because it was close to the school. The application crashed when the user opened a file that contained more than ten thousand records. → 基线 `这次会议将召集研究人员、工程师和产品经理讨论人工智能的未来。 尽管价格高于我们预期,但我们决定购买这所房子是因为离学校很近。 用户打开了包含一万多张记录的文件,应用程序就崩溃了。 软件的用户在网上搜索了一台包含一千万多张记录的软件。 使用软件的使用者们将使用软件,并使用软件。` / 本次 `这次会议将召集研究人员、工程师和产品管理人员讨论人造情报的未来。 虽然价格比我们预期的要高,但我们决定买下房子,因为房子离学校很近。 当用户打开包含超过一万个记录的文件时, 程序崩溃。`
  - `paragraph-v1-0028` chrf_drop -0.0681：The restaurant is always crowded. I prefer tea to coffee. She is good at cooking. → 基线 `餐厅总是挤满了人 我更喜欢喝茶而不是喝咖啡 她擅长烹饪` / 本次 `餐厅总是挤满了人 我宁愿喝茶,也不要喝咖啡 她很会做饭`
  - `paragraph-v1-0014` chrf_drop -0.0647：The SDK includes a command-line interface. The CPU usage stayed below 50%. The GPU accelerates matrix multiplication. → 基线 `SDK包括一个命令线界面。 CPU使用率保持在50%以下. GPU 加速矩阵乘法的加速。` / 本次 `SDK包括一个命令线界面。 CPU使用率维持在50%以下. GPU 加速矩阵乘法。`
  - `paragraph-v1-0019` chrf_drop -0.0458：I am looking for a pharmacy. Could you speak more slowly? I do not understand what you mean. → 基线 `我在找药房 你能说慢点吗? 我不明白你的意思。` / 本次 `我在找药房 你能说慢点吗? 我不明白你的意思`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
