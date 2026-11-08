# P15 AI 辅助初步评审（非人工验收）

> 状态：**AI_ASSISTED_PRELIMINARY**。这不是人工审核，不能计入 94 条 PENDING_HUMAN。
> 目的：为人工评审提供错误类型和优先样本；评分只代表当前 AI 对译文的初步判断。

评分 1–5；错误级别 Critical/Major/Minor/Acceptable。

| ID | 类型 | 原文 | 译文 | 语义 | 完整 | 自然 | 术语 | 上下文 | 格式 | 最差错误 | 说明 |
|---|---|---|---:|---:|---:|---:|---:|---:|---|---|
| en-zh-0286 | Critical | The discount is 15% for orders over $100. | 折扣是15%超过100美元的订单 | 3 | 4 | 2 | 4 | 3 | 5 | Major | 数字/术语保留，但“条件→对象→折扣”的语序不自然，可能误读 |
| en-zh-0151 | Critical | The CPU usage stayed below 50%. | CPU使用率保持在50%以下. | 5 | 5 | 4 | 5 | 5 | 4 | Minor | 语义正确；句号是 ASCII |
| en-zh-0296 | Critical | The error code is 0x80070005. | 错误代码是0x80070005. | 5 | 5 | 4 | 5 | 5 | 5 | Minor | 代码完整；标点 ASCII |
| en-zh-0309 | Critical | The event starts at 8 PM and ends at 11 PM. | 活动于下午8时开始，11点结束. | 5 | 5 | 4 | 4 | 5 | 4 | Minor | 时间关系正确；“下午8时”略生硬 |
| sentence-v1-0015 | Sentence | The build fails when a test times out. | 测试结束的时候, 建筑就会失败。 | 2 | 3 | 2 | 2 | 2 | 4 | Major | build 取“建筑”而非“构建”；when 条件关系弱 |
| sentence-v1-0025 | Sentence | The bank approved the loan after reviewing the documents. | 银行在审查文件后批准了贷款。 | 5 | 5 | 5 | 5 | 5 | 5 | Acceptable | 正确自然 |
| sentence-v1-0027 | Sentence | The spring in the mattress broke last night. | 昨晚床垫里的春天坏了 | 2 | 3 | 2 | 2 | 2 | 4 | Major | spring 多义词取“春天”，应为弹簧 |
| sentence-v1-0029 | Sentence | She will charge the battery before the trip. | 她会在行程前充电 | 4 | 3 | 3 | 3 | 3 | 5 | Minor | 漏“电池”；charge 译充电可接受 |
| paragraph-v1-0027 | Paragraph | I have no idea. It depends on the weather. We had a great time yesterday. | 我不知道,我也不知道,我... 取决于天气 昨天我们玩得很开心 | 3 | 3 | 1 | 3 | 3 | 3 | Major | 重复/省略号，句间不连贯；Best 选中此候选 |
| paragraph-v1-0007 | Paragraph | The file system stores metadata separately. The database query returned no results. Machine learning models require large amounts of data. | 文件系统单独存储元数据。数据库查询返回没有结果。机器学习模式需要大量数据。 | 4 | 5 | 3 | 3 | 5 | 5 | Major | models 应“模型”而非“模式”；“返回没有结果”生硬 |
| document-v1-0001 | Document | # System status ... | # 系统系统状态 ... | 4 | 5 | 3 | 3 | 5 | 5 | Major | 标题重复“系统系统”；结构/代码块正确 |
| document-v1-0002 | Document | # Release notes ... | # 单元说明 ... | 3 | 4 | 3 | 3 | 4 | 5 | Major | Release notes 应为“发布说明”；API 文件应为 API 文档 |

## 初步结论

- 数字/占位符/结构已明显改善；主要错误转向**词义、条件关系、标题/术语和句间连贯**。
- 0286 是“完整性正确但语义关系差”的典型；不能用数字门禁通过来判定质量。
- paragraph-v1-0027 是 Best 选择器选到较差候选的典型，应优先人工复核。
- 以上不是人工评审，94 条 PENDING_HUMAN 不变。
