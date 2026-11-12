# 翻译质量评审包

- 数据集：`fixtures/eval/dev.jsonl`
- 模式：Precise（mock=false）
- 机器可判定项：100 / 100 通过（失败 0）
- 延迟：p50 2817 ms / p95 5426 ms / max 12073 ms
- HTTP 调用 100 次，内容重试 0 次，token 23526/1665
- 估算费用：未配置单价（不做估算）
- 人工评审：**PENDING_HUMAN**

> 自动指标只覆盖"必须保留的内容"，不代表译文准确、自然。

| id | 类别 | 结果 | 原文 | 译文 | 机器判定 |
|---|---|---|---|---|---|
| dev-001 | statement | ✅ | The new office opens on the first Monday of next month. | 新办公室于下个月第一个星期一开业。 | Checks a future event expressed with a calendar reference. |
| dev-002 | statement | ✅ | Most of our customers renew their subscription without contacting support. | 我们的大多数客户无需联系支持团队即可续订。 | Checks a general claim in the simple present tense. |
| dev-003 | statement | ✅ | The museum's east wing is closed for renovation until further notice. | 博物馆东翼因装修关闭，恢复开放时间另行通知。 | Checks a status statement with an open-ended time phrase. |
| dev-004 | statement | ✅ | Our team moved the analytics dashboard to a faster server over the weekend. | 我们的团队在周末将分析仪表板迁移到了一台更快的服务器上。 | Checks a completed action with a time adverbial. |
| dev-005 | statement | ✅ | A short summary at the top of the report helps busy readers decide what to read next. | 报告顶部的简短摘要可以帮助忙碌的读者决定接下来阅读什么内容。 | Checks a longer subject phrase and a purpose clause. |
| dev-006 | statement | ✅ | The city plans to add two new bus lines before the end of the year. | 市政府计划在年底前新增两条公交线路。 | Checks a planned action with a quantity word in the object. |
| dev-007 | question | ✅ | Could you explain why the report was submitted after the deadline? | 您能解释一下为什么报告是在截止日期之后才提交的吗？ | Checks a polite question that contains an embedded passive clause. |
| dev-008 | question | ✅ | How many people will attend the workshop on Thursday afternoon? | 有多少人会参加周四下午的研讨会？ | Checks a how-many question with a time adverbial. |
| dev-009 | question | ✅ | Have you already sent the revised contract to the legal team? | 你已经把修改后的合同发给法务团队了吗? | Checks a present perfect question with already. |
| dev-010 | question | ✅ | What should we do if the customer asks for a refund after thirty days? | 如果客户在三十天后要求退款，我们应该怎么办？ | Checks a question with a conditional clause attached. |
| dev-011 | question | ✅ | Is the new version of the app compatible with older phones? | 新版本的应用程序兼容旧款手机吗？ | Checks a yes/no question with a long noun phrase. |
| dev-012 | question | ✅ | Why did the build fail only on the machine that runs the nightly tests? | 为什么构建只在运行夜间测试的机器上失败？ | Checks a why-question with a relative clause at the end. |
| dev-013 | negation | ✅ | The client did not approve the budget, so we cannot start the campaign. | 客户没有批准预算，所以我们无法启动该活动。 | Checks two negated clauses joined by a result connector. |
| dev-014 | negation | ✅ | I don't think the meeting will end before noon. | 我认为会议不会在中午之前结束。 | Checks negated opinion with a contraction and a time clause. |
| dev-015 | negation | ✅ | None of the test cases passed after the database migration. | 数据库迁移后，没有任何测试用例通过。 | Checks a negative quantifier as the subject. |
| dev-016 | negation | ✅ | The documentation does not explain how to reset a forgotten password. | 文档没有说明如何重置忘记的密码。 | Checks negation followed by an embedded how-to clause. |
| dev-017 | negation | ✅ | We never received the package, and the tracking page has not updated since Monday. | 我们一直没有收到包裹，自周一以来物流跟踪页面也没有更新。 | Checks two negative statements in one sentence. |
| dev-018 | negation | ✅ | There is no easy way to undo a delete once the file has left the recycle bin. | 一旦文件离开回收站，就没有简便的方法可以撤销删除操作了。 | Checks existential negation with a once-clause. |
| dev-019 | conditional | ✅ | If the server runs out of memory, it will restart automatically within a minute. | 如果服务器内存耗尽，它会在一分钟内自动重启。 | Checks a first conditional with a time expression in the result. |
| dev-020 | conditional | ✅ | Unless the client signs the contract by Friday, we will move the launch to April. | 除非客户在周五之前签署合同，否则我们将把上线时间推迟到四月。 | Checks unless as a negative condition. |
| dev-021 | conditional | ✅ | If you had saved the file earlier, you would not have lost the last paragraph. | 如果你之前保存了文件，就不会丢失最后一段了。 | Checks a third conditional with past perfect forms. |
| dev-022 | conditional | ✅ | Should the payment fail, the order will be placed on hold for twenty-four hours. | 如果付款失败，订单将被暂停二十四小时。 | Checks an inverted formal conditional with a passive result. |
| dev-023 | conditional | ✅ | The script only sends an alert when the disk usage stays above 90% for ten minutes. | 该脚本仅在磁盘使用率持续十分钟以上保持在 90% 以上时才发送警报。 | Checks a conditional reading of when with a numeric threshold. |
| dev-024 | conditional | ✅ | Even if the weather improves, the outdoor event will remain indoors because the stage is already set up. | 即使天气好转，户外活动仍将在室内举行，因为舞台已经搭建好了。 | Checks a concessive conditional plus a because-clause. |
| dev-025 | passive | ✅ | The old bridge was rebuilt by local engineers after the flood. | 洪水过后，当地工程师重建了这座旧桥。 | Checks a past passive with an explicit agent. |
| dev-026 | passive | ✅ | All customer records must be encrypted before they leave the data center. | 所有客户记录在离开数据中心之前必须经过加密。 | Checks a modal passive with a before-clause. |
| dev-027 | passive | ✅ | The decision was made by the committee without consulting the residents. | 该决定是委员会在未征求居民意见的情况下作出的。 | Checks a passive followed by a negative prepositional phrase. |
| dev-028 | passive | ✅ | The results will be announced on the company blog next Monday. | 结果将于下周一在公司博客上公布。 | Checks a future passive with a time and place. |
| dev-029 | passive | ✅ | Mistakes in the invoice were corrected and the new copy was sent by email. | 发票中的错误已更正，新副本已通过电子邮件发送。 | Checks two coordinated passive clauses. |
| dev-030 | passive | ✅ | The manuscript had been reviewed twice before the editor accepted it. | 手稿在被编辑接受之前已经审阅过两次了。 | Checks a past perfect passive with a frequency adverb. |
| dev-031 | tense | ✅ | By the time we arrived, the presentation had already ended. | 等我们到达时，演示已经结束了。 | Checks past perfect for an action completed before another past action. |
| dev-032 | tense | ✅ | She will have finished the migration by the end of the sprint. | 到本冲刺结束时，她将已完成迁移。 | Checks future perfect with a by-phrase. |
| dev-033 | tense | ✅ | They are still testing the feature that we shipped last week. | 他们还在测试我们上周发布的功能。 | Checks present progressive contrasted with a past action. |
| dev-034 | tense | ✅ | I have lived in this city for eight years and I still discover new streets. | 我在这座城市住了八年，仍然会发现新的街道。 | Checks present perfect with for and a present-tense follow-up. |
| dev-035 | tense | ✅ | The team was debugging the payment flow when the alert fired. | 团队正在调试支付流程时，警报触发了。 | Checks past progressive interrupted by a past simple event. |
| dev-036 | tense | ✅ | We had been waiting for approval since March when the request was finally rejected. | 当申请最终被拒绝时，我们从三月起就一直在等待批准。 | Checks past perfect progressive with a since-phrase. |
| dev-037 | clause | ✅ | The report that the auditor sent yesterday contains several errors that we need to fix. | 审计员昨天发送的报告中有几处错误需要我们修正。 | Checks two nested relative clauses in one sentence. |
| dev-038 | clause | ✅ | Although the interface looks simple, the underlying scheduler handles thousands of tasks every second. | 虽然界面看起来很简单，但底层的调度器每秒能处理数千个任务。 | Checks a concessive subordinate clause before the main clause. |
| dev-039 | clause | ✅ | What surprised everyone was how quickly the new hire learned the codebase. | 让大家惊讶的是这位新员工如此快速地熟悉了代码库。 | Checks a nominal subject clause and a nominal complement clause. |
| dev-040 | clause | ✅ | The company will reopen the position once the budget for next year is approved. | 公司将在明年预算获批后重新开放该职位。 | Checks a once-clause with an embedded passive. |
| dev-041 | clause | ✅ | Because the train was delayed, we missed the opening remarks and had to wait outside. | 因为火车晚点了，我们错过了开场致辞，只能在外面等候。 | Checks a causal clause with a coordinated result. |
| dev-042 | clause | ✅ | The book, which I borrowed from a friend last summer, is still sitting on my shelf. | 那本我去年夏天从朋友那里借来的书，现在还放在我的书架上。 | Checks a non-restrictive relative clause in the middle of a sentence. |
| dev-043 | clause | ✅ | Whoever finishes the checklist first should help the others with the remaining items. | 谁先完成核对清单，就应该帮助其他人处理剩余的项目。 | Checks a free relative clause as the subject. |
| dev-044 | reference | ✅ | Maria told her sister that she would pick up the tickets, but she forgot. | 玛丽亚告诉她姐姐她会去取票，但她忘了。 | Checks ambiguous pronoun reference in reported speech. |
| dev-045 | reference | ✅ | The laptop would not fit in the bag because it was too thick. | 这台笔记本电脑装不进包里，因为它太厚了。 | Checks that it refers to the laptop rather than the bag. |
| dev-046 | reference | ✅ | When the manager met the client, he apologized for the delay. | 经理会见客户时，为延误道歉了。 | Checks a pronoun whose antecedent depends on world knowledge. |
| dev-047 | reference | ✅ | Take the cable from the drawer and plug it into the port on the left. | 从抽屉里拿出线缆，插到左边的端口上。 | Checks an imperative with an object pronoun across clauses. |
| dev-048 | reference | ✅ | The old library and the new café stand next to each other, and the latter is always crowded. | 老图书馆和新咖啡馆相邻而立，后者总是人头攒动。 | Checks long-distance reference through the latter. |
| dev-049 | reference | ✅ | Our team reports to the platform group, and they own the release process. | 我们的团队向平台组汇报，发布流程由他们负责。 | Checks that they resolves to the platform group. |
| dev-050 | reference | ✅ | The instructions say to restart the router; this usually fixes the problem. | 说明中提到要重启路由器，这通常能解决问题。 | Checks demonstrative this referring to a whole action. |
| dev-051 | polysemy | ✅ | Please run the tests again before you merge the branch. | 请在合并分支之前再次运行测试。 | Checks run in its execute-software sense. |
| dev-052 | polysemy | ✅ | She withdrew some cash from the bank on her way to the market. | 她在去市场的路上从银行取了一些现金。 | Checks bank in its financial-institution sense. |
| dev-053 | polysemy | ✅ | The room was so light that we did not need to turn on the lamp. | 房间很亮，我们不需要开灯。 | Checks light as brightness rather than weight. |
| dev-054 | polysemy | ✅ | He is going to run for mayor in the next election. | 他打算在下次选举中竞选市长。 | Checks run for in its campaign sense. |
| dev-055 | polysemy | ✅ | The package is light enough to carry with one hand. | 这个包裹很轻，可以单手提起。 | Checks light as low weight rather than brightness. |
| dev-056 | polysemy | ✅ | This machine can run for hours on a single charge. | 这台机器充电一次可以运行数小时。 | Checks run in its operate-continuously sense. |
| dev-057 | ui | ✅ | Your changes could not be saved: %s. Check your connection and try again. | 无法保存您的更改：%s。请检查您的网络连接后重试。 | Checks a printf-style placeholder inside an error message. |
| dev-058 | ui | ✅ | Showing 1–20 of 348 results | 显示348个结果中的1–20个 | Checks a pagination label with a range and a total count. |
| dev-059 | ui | ✅ | Delete {count} selected items? This action cannot be undone. | 删除 {count} 个所选项目？此操作无法撤销。 | Checks a named placeholder in a confirmation dialog. |
| dev-060 | ui | ✅ | Signed in as {{ user.email }} | 已登录为 {{ user.email }} | Checks a template expression with double braces and a space. |
| dev-061 | ui | ✅ | Upload failed: the file exceeds the 25 MB limit. | 上传失败：文件超过了 25 MB 的限制。 | Checks a unit of measurement inside an error string. |
| dev-062 | ui | ✅ | The report for <b>{0}</b> is ready, and the download link expires in 24 hours. | <b>{0}</b>的报告已生成，下载链接将于24小时后过期。 | Checks an HTML tag together with a positional placeholder. |
| dev-063 | tech-doc | ✅ | Call the `parse_config` function before you load any plugins. | 在加载任何插件之前，先调用 `parse_config` 函数。 | Checks that an inline identifier keeps its backticks. |
| dev-064 | tech-doc | ✅ | Set the environment variable `LOG_LEVEL` to `debug` when you reproduce the issue. | 重现该问题时，请将环境变量 `LOG_LEVEL` 设置为 `debug`。 | Checks two inline code spans in one instruction. |
| dev-065 | tech-doc | ✅ | The worker retries failed jobs three times with exponential backoff. | 工作进程会对失败的作业进行三次重试,并采用指数退避策略。 | Checks technical prose with a frequency expression. |
| dev-066 | tech-doc | ✅ | To reset the cache, stop the service, delete the files under `/var/cache/app`, and start it again. | 要重置缓存，请停止服务，删除`/var/cache/app`下的文件，然后重新启动服务。 | Checks a POSIX file path inside a multi-step instruction. |
| dev-067 | tech-doc | ✅ | The endpoint accepts a JSON body and returns 201 on success. | 该端点接受 JSON 请求体，成功时返回 201。 | Checks an HTTP status code in reference documentation. |
| dev-068 | tech-doc | ✅ | On Windows the config file lives in `C:\Program Files\App\config.ini`. | 在 Windows 上，配置文件位于 `C:\Program Files\App\config.ini`。 | Checks a Windows path with spaces and backslashes. |
| dev-069 | tech-doc | ✅ | Run `pnpm build` and then serve the output from the `dist` directory. | 运行 `pnpm build`，然后从 `dist` 目录提供输出。 | Checks two command-line tokens in a short procedure. |
| dev-070 | dialogue | ✅ | "I'll send the updated schedule tonight," she said. "Please check your inbox tomorrow morning." | “我今晚会把更新后的日程表发给你，”她说。“明天早上请查看你的收件箱。” | Checks two quoted sentences with an embedded request. |
| dev-071 | dialogue | ✅ | "Did you finish the report?" he asked. "Not yet," I replied. | “你写完报告了吗？”他问。“还没有，”我回答道。 | Checks a question-and-answer exchange with reporting verbs. |
| dev-072 | dialogue | ✅ | The technician said, "Turn off the router, wait ten seconds, and turn it back on." | 技术人员说：“关闭路由器，等待十秒，然后重新打开。” | Checks quoted imperatives inside a reporting clause. |
| dev-073 | dialogue | ✅ | "We should leave now," Anna said, "or we'll miss the last train." | “我们该现在出发了，”安娜说，“否则就赶不上末班车了。” | Checks a quotation split by the reporting clause. |
| dev-074 | dialogue | ✅ | Customer: "My order arrived damaged." Agent: "I'm sorry about that. We can send a replacement today." | 客户：“我收到的订单商品损坏了。”客服：“很抱歉。我们可以今天给您补发一件替换商品。” | Checks a two-speaker support exchange with speaker labels. |
| dev-075 | dialogue | ✅ | "How long will the migration take?" the client asked, and the engineer answered, "About two hours." | “迁移需要多长时间？”客户问道，工程师回答说：“大约两个小时。” | Checks two quotations joined by a coordinating conjunction. |
| dev-076 | money | ✅ | The subscription renews at $12.50 per month after the free trial ends. | 免费试用结束后，订阅将以每月12.50美元的价格自动续订。 | Checks a monthly price with a decimal amount. 金额币种由 SDK 语义校验（中文写成“12.50 美元”是正确的），不做逐字匹配。 |
| dev-077 | money | ✅ | We saved 30% on the annual plan, which brought the total to $839.93. | 我们通过年付方案节省了 30%，总计金额为 $839.93。 | Checks a discount percentage next to a final total. 金额币种由 SDK 语义校验（中文写成“12.50 美元”是正确的），不做逐字匹配。 |
| dev-078 | money | ✅ | The invoice lists 3 items for a total of $247.00, including tax. | 发票上列出了3个商品项目，含税总计247.00美元。 | Checks an item count and a total amount in one sentence. 金额币种由 SDK 语义校验（中文写成“12.50 美元”是正确的），不做逐字匹配。 |
| dev-079 | money | ✅ | Refunds over $500 require approval from a finance manager. | 超过500美元的退款需要财务经理的批准。 | Checks a threshold amount without decimals. 金额币种由 SDK 语义校验（中文写成“12.50 美元”是正确的），不做逐字匹配。 |
| dev-080 | money | ✅ | The price rose from $19.99 to $24.99 without any notice. | 价格在没有通知的情况下从19.99美元涨到了24.99美元。 | Checks two prices in a before-and-after comparison. 金额币种由 SDK 语义校验（中文写成“12.50 美元”是正确的），不做逐字匹配。 |
| dev-081 | money | ✅ | The agency charged €4,500 for the redesign and £900 for the copywriting. | 该机构对重新设计收取了4,500欧元，文案撰写收取了900英镑。 | Checks two currencies with a thousands separator. 金额币种由 SDK 语义校验（中文写成“12.50 美元”是正确的），不做逐字匹配。 |
| dev-082 | url | ✅ | Read the migration guide at https://example.com/docs/migrate?v=2&step=3 before upgrading. | 升级前请阅读 https://example.com/docs/migrate?v=2&step=3 处的迁移指南。 | Checks a documentation URL with two query parameters. |
| dev-083 | url | ✅ | The tracking page is at https://example.org/orders/10248?ref=email and updates every hour. | 追踪页面位于 https://example.org/orders/10248?ref=email，每小时更新一次。 | Checks a URL that embeds an order number. |
| dev-084 | url | ✅ | If the link https://example.com/reset?token=abc123&user=42 expires, request a new one. | 如果链接https://example.com/reset?token=abc123&user=42过期，请申请新的链接。 | Checks a URL inside a conditional clause. |
| dev-085 | url | ✅ | Our status page at https://status.example.org reports outages and scheduled maintenance. | 我们的状态页面 https://status.example.org 报告服务中断和计划维护信息。 | Checks a bare domain used as a location. |
| dev-086 | url | ✅ | Download the installer from https://example.com/downloads/app-3.4.1.exe and verify the checksum. | 从 https://example.com/downloads/app-3.4.1.exe 下载安装程序并校验校验和。 | Checks a download URL that contains a version number. |
| dev-087 | url | ✅ | Post the announcement to https://example.net/blog/2025/03/launch?utm_source=newsletter. | 将公告发布到 https://example.net/blog/2025/03/launch?utm_source=newsletter。 | Checks a URL with a date path and a tracking parameter. |
| dev-088 | glossary | ✅ | A pull request should be reviewed by at least one other engineer. | 拉取请求应至少由一名其他工程师审查。 | Checks that pull request uses the required glossary target. |
| dev-089 | glossary | ✅ | The large language model was fine-tuned on internal support tickets. | 该大语言模型基于内部支持工单进行了微调。 | Checks that large language model follows the glossary in a passive clause. |
| dev-090 | glossary | ✅ | Every pull request that changes the public API needs a short design note. | 每个更改公共 API 的拉取请求都需要一份简短的设计说明。 | Checks that the glossary term survives inside a relative clause. |
| dev-091 | glossary | ✅ | Our large language model answers questions using the documents in the knowledge base. | 我们的大语言模型使用知识库中的文档来回答问题。 | Checks the glossary term as a possessive subject. |
| dev-092 | glossary | ✅ | Open a pull request against the main branch and request a review from the platform team. | 针对 main 分支创建一个拉取请求，并请求平台团队进行审查。 | Checks the glossary term in an imperative with two actions. |
| dev-093 | glossary | ✅ | The team evaluated the large language model on a small blind set before the release. | 团队在发布之前在一个小型盲测集上对该大语言模型进行了评估。 | Checks the glossary term in a past-tense evaluation sentence. |
| dev-094 | glossary | ✅ | A draft pull request is a good place to discuss a large language model integration early. | 草稿拉取请求是及早讨论大语言模型集成的好地方。 | Checks two glossary terms that appear in the same sentence. |
| dev-095 | paragraph | ✅ | The migration starts at 9 PM. Expect about twenty minutes of downtime. We will post an update when the service is back. | 迁移将于晚上9点开始，预计会有大约20分钟的停机时间。服务恢复后我们将发布更新。 | Checks a three-sentence status update with a clock time. |
| dev-096 | paragraph | ✅ | First, open the settings panel. Then select the language you want and save your choice. The interface updates immediately. | 首先，打开设置面板。然后选择你想要的语言并保存你的选择。界面会立即更新。 | Checks a three-sentence instruction sequence with ordinals. |
| dev-097 | paragraph | ✅ | Memory usage rose sharply after the deploy. The team traced the leak to an unclosed connection. A patch shipped the same evening. | 部署后内存使用量急剧上升。团队将内存泄漏追查到一个未关闭的连接。补丁于当天晚上发布。 | Checks a three-sentence incident summary with pronoun chains. |
| dev-098 | paragraph | ✅ | The client asked for a shorter report. We removed the appendix and kept the summary. The final version is eight pages long. | 客户要求提供更短的报告。我们删除了附录，保留了摘要。最终版本共八页。 | Checks a three-sentence narrative with a quantity word. |
| dev-099 | paragraph | ✅ | Rain is expected through the morning. The outdoor session will move to the main hall. Please check the sign near the entrance. | 预计整个上午都有雨。户外环节将移至大礼堂举行。请查看入口附近的告示。 | Checks a three-sentence notice with a passive clause. |
| dev-100 | paragraph | ✅ | The backup job failed twice last night. Logs show a timeout on the storage server. The on-call engineer is investigating now. | 昨晚备份任务失败了两次。日志显示存储服务器发生超时。值班工程师正在进行调查。 | Checks a three-sentence technical paragraph with a frequency adverb. |
