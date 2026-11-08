# 翻译质量自动对照

- 金样本：fixtures/quality/golden-v1.jsonl
- 样本数：320
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| int8 | 0.4659 | 0.4641 | 0.00% | 0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 |
| fp32 | 0.4645 | 0.4656 | 0.00% | 0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 |

## 分类 chrF

| 类别 | int8 | fp32 |
|---|---:|---:|
| basic | 0.4521 | 0.4408 |
| conditional | 0.3629 | 0.3761 |
| daily | 0.4033 | 0.3728 |
| format | 0.6964 | 0.7 |
| long | 0.3641 | 0.3569 |
| negation | 0.4407 | 0.4911 |
| numbers_units | 0.5608 | 0.5589 |
| polysemy | 0.3661 | 0.3649 |
| proper_noun | 0.6458 | 0.6629 |
| software | 0.4764 | 0.4614 |
| technical | 0.5106 | 0.5112 |

## 相对基线的变化

- **fp32**：chrF -0.0014，BLEU +0.0015，错误 +0，新增致命 0，回归样本 35，改善样本 37
  - `en-zh-0070` chrf_drop -0.7168：The store is close to the station. → 基线 `商店离车站很近` / 本次 `店在车站附近`
  - `en-zh-0100` chrf_drop -0.6899：Let me think about it. → 基线 `让我考虑一下` / 本次 `让我想想`
  - `en-zh-0264` chrf_drop -0.4353：This action cannot be undone. → 基线 `此动作无法撤销。` / 本次 `此动作无法取消。`
  - `en-zh-0268` chrf_drop -0.4263：Your changes have been saved. → 基线 `您的更改已保存。` / 本次 `您的更改已被保存。`
  - `en-zh-0025` chrf_drop -0.398：The food is delicious. → 基线 `食物很好吃 好吃吗?` / 本次 `食物是美味的。`
  - `en-zh-0285` chrf_drop -0.3869：The flight takes about 12 hours and 45 minutes. → 基线 `飞行大约需要12小时45分钟。` / 本次 `飞行需要大约12小时45分钟。`
  - `en-zh-0221` chrf_drop -0.3498：The government plans to build a new high-speed railway that will connect the capital with the southern coast. → 基线 `政府计划修建一条新的高速铁路, 连接首都与南部海岸。` / 本次 `政府计划建造一条新的高速铁路,`
  - `en-zh-0279` chrf_drop -0.3493：Reboot the device to apply the changes. → 基线 `重新启动设备以应用更改。` / 本次 `重新启动设备以应用修改。`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
