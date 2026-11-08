# 英中质量汇总（自动指标）

- 人工评审：**PENDING_HUMAN**（未完成，不得当作通过）
- 词典：resources/en-zh-ecdict
- 金样本：fixtures/quality/golden-v1.jsonl（320 条）

## 1. 词典

- 高频词覆盖：**9898/10000 = 98.98%**
- 平均义项数（命中词）：1.69，多义项词：4908
- 多义词检查：**10/10**
- 技术/固定短语：**179/179**
- 项目补充条目：**179/179**

| 多义词 | 义项数 | 结果 |
|---|---:|---|
| bank | 2 | PASS |
| spring | 4 | PASS |
| charge | 3 | PASS |
| run | 6 | PASS |
| match | 4 | PASS |
| right | 6 | PASS |
| book | 3 | PASS |
| present | 4 | PASS |
| light | 5 | PASS |
| close | 6 | PASS |

## 2. 翻译（相对基线的自动对照）

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| p11 | 0.4577 | 0.4546 | 0.00% | 4 | 0.8983 | 1.0 | 1.0 | 1.0 | 1.0 |
| p12b | 0.4632 | 0.4607 | 0.00% | 0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 |
| p12f | 0.4659 | 0.4641 | 0.00% | 0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 |

### 分类 chrF

| 类别 | p11 | p12b | p12f |
|---|---:|---:|---:|
| basic | 0.4511 | 0.4521 | 0.4521 |
| conditional | 0.3629 | 0.3629 | 0.3629 |
| daily | 0.4028 | 0.4033 | 0.4033 |
| format | 0.6953 | 0.6964 | 0.6964 |
| long | 0.3562 | 0.3641 | 0.3641 |
| negation | 0.4407 | 0.4407 | 0.4407 |
| numbers_units | 0.5409 | 0.5599 | 0.5608 |
| polysemy | 0.3661 | 0.3661 | 0.3661 |
| proper_noun | 0.6458 | 0.6458 | 0.6458 |
| software | 0.4712 | 0.4764 | 0.4764 |
| technical | 0.4781 | 0.4934 | 0.5106 |

### 相对基线变化

- **p12b**：chrF +0.0055，BLEU +0.0061，错误 +0，新增致命 0，回归样本 2，改善样本 8
- **p12f**：chrF +0.0082，BLEU +0.0095，错误 +0，新增致命 0，回归样本 2，改善样本 15

## 3. 术语约束（Warn 诊断；Enforce 配置会实际改写）

发现 **8** 处术语不一致：

| ID | 源文 | 译文（Warn） | 违规 |
|---|---|---|---|
| en-zh-0113 | Garbage collection frees unused memory automatically. | 垃圾回收自动免去未使用的内存。 | 「垃圾收集」应为「垃圾回收」（偏移 0） |
| en-zh-0117 | Always check for buffer overflow when handling user input. | 处理用户输入时总是检查缓冲区溢出。 | 「缓冲溢出」应为「缓冲区溢出」（偏移 33） |
| en-zh-0121 | This project is open source and free to use. | 该项目是开源,可自由使用。 | 「开放源码」应为「开源」（偏移 12） |
| en-zh-0124 | We added a unit test for the parser. | 我们增加了一个单元测试 用于采集器。 | 「单位测试」应为「单元测试」（偏移 21） |
| en-zh-0139 | The load balancer distributes requests across servers. | 负载均衡器将要求分布于各服务器。 | 「负载平衡器」应为「负载均衡器」（偏移 0） |
| en-zh-0141 | The virtual machine runs a separate operating system. | 虚拟机运行一个单独的操作系统。 | 「虚拟机器」应为「虚拟机」（偏移 0） |
| en-zh-0145 | Garbage collection pauses can affect latency. | 垃圾回收暂停可能会影响延缓。 | 「垃圾收集」应为「垃圾回收」（偏移 0） |
| en-zh-0286 | The discount is 15% for orders over $100. | 折扣是15%超过100美元的订单 | 「命令」应为「订单」（偏移 30） |

## 4. 口径与限制

- chrF/BLEU 只衡量与单一参考译文的接近程度，不等同于翻译正确率；不得单独作为验收结论。
- 数字/占位符/URL/否定是确定性检查；否定与目标语言仍是启发式，可能误报。
- 人工评审必须按 `tests/quality/reports/human-review-sample.csv` 填写；当前为 PENDING。
