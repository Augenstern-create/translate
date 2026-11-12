## 评测汇总

| 报告 | 通过 | chrF++ | BLEU-4 | p50 | p95 | max | HTTP 调用 | 输入/输出 token | 估算费用 |
|---|---|---|---|---|---|---|---|---|---|
| bench-ds.json | 2210/2213 | 30.72 | 36.36 | 2258 ms | 3462 ms | 9682 ms | 2214 | 532007/66330 | 未配置单价 |
| bench-glm.json | 2211/2213 | 30.12 | 35.21 | 3564 ms | 7714 ms | 18697 ms | 2215 | 537875/75080 | 未配置单价 |

### 按类别

| 类别 | bench-ds（通过/失败） | bench-glm（通过/失败） |
|---|---|---|
| literary | 206/0 | 206/0 |
| news | 1404/0 | 1403/1 |
| social | 489/3 | 491/1 |
| speech | 111/0 | 111/0 |

### 失败用例（bench-ds.json，共 3 条）

| 类别 | 失败数 |
|---|---|
| social | 3 |

前 20 条：

| id | 类别 | 失败原因 | 译文（截断） |
|---|---|---|---|
| wmt24pp-711 | social | SDK 判定致命问题 target_language_mismatch：目标语言为中文，但译文没有足够的汉字 | #TeaganAir #pilot #flying #pa28 #PilotTraining |
| wmt24pp-733 | social | SDK 判定致命问题 target_language_mismatch：目标语言为中文，但译文没有足够的汉字 | #PilotTraining #Flying #TeaganAir #PA28 #Pilot #FlightSchool |
| wmt24pp-748 | social | SDK 判定致命问题 target_language_mismatch：目标语言为中文，但译文没有足够的汉字 | #TeaganAir #FlightTraining #Flying #PilotTraining #FlightSch |

### 失败用例（bench-glm.json，共 2 条）

| 类别 | 失败数 |
|---|---|
| social | 1 |
| news | 1 |

前 20 条：

| id | 类别 | 失败原因 | 译文（截断） |
|---|---|---|---|
| wmt24pp-740 | social | SDK 判定致命问题 target_language_mismatch：目标语言为中文，但译文没有足够的汉字 | #TeaganAir #PilotTraining #FlightSchool #Stalls #PA28 #Piper |
| ntrex128-1947 | news | 翻译失败：[invalid_response] 后端以 content_filter 拒绝了本次翻译（IntegrityFailure） |  |

> 人工评审状态：**PENDING_HUMAN**。自动指标与规则检查不替代人工验收。
