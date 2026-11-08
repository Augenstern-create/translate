# 翻译质量自动对照

- 金样本：fixtures/quality/paragraph-v1.jsonl
- 样本数：50
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| whole | 0.3917 | 0.4299 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| sentence | 0.3987 | 0.4493 | 0.00% | 0 | 1.0 | n/a | n/a | n/a | 1.0 |
| group | 0.3909 | 0.4294 | 0.00% | 1 | 0.6 | n/a | n/a | n/a | 1.0 |

## 分类 chrF

| 类别 | whole | sentence | group |
|---|---:|---:|---:|
| paragraph-daily | 0.2598 | 0.3473 | 0.3453 |
| paragraph-mixed | 0.3584 | 0.3661 | 0.3388 |
| paragraph-technical | 0.5425 | 0.4831 | 0.4965 |

## 相对基线的变化

- **sentence**：chrF +0.0070，BLEU +0.0194，错误 +0，新增致命 0，回归样本 20，改善样本 16
  - `paragraph-v1-0013` chrf_drop -0.251：We measured throughput and latency under load. The network bandwidth is the bottleneck. The API returns JSON responses. → 基线 `我们测量了载荷中的吞吐量和延缓度。 网络带宽是瓶颈。 API 返回 JSON 响应。` / 本次 `我们测量了载荷中的吞吐量和延缓度 网络带宽是瓶颈。 API回`
  - `paragraph-v1-0027` chrf_drop -0.2474：I have no idea. It depends on the weather. We had a great time yesterday. → 基线 `我不知道,这取决于天气 我们昨天玩得很开心` / 本次 `我不知道,我也不知道,我... 取决于天气 昨天我们玩得很开心`
  - `paragraph-v1-0007` chrf_drop -0.164：The file system stores metadata separately. The database query returned no results. Machine learning models require large amounts of data. → 基线 `文件系统单独存储元数据。 数据库查询没有返回结果。 机器学习模式需要大量数据。` / 本次 `文件系统单独存储元数据。 数据库查询返回没有结果。 机器学习模式需要大量数据。`
  - `paragraph-v1-0030` chrf_drop -0.1473：The library opens at nine in the morning and closes at eight in the evening, but during the summer it stays open until ten so that students can prepare for their exams. Although the weather forecast said it would be sunny, heavy rain started in the afternoon and we had to cancel the picnic. The new policy requires all employees to complete a security training course before they can access the internal network. → 基线 `图书馆上午9点开放,晚上8点关闭,但夏季开放到10点,学生们可以准备考试。 虽然天气预报说会晴天,但下午大雨开始,我们不得不取消野餐。 新政策要求所有员工在进入内部网络之前完成安全培训课程。` / 本次 `图书馆上午9点开放,晚上8点关闭,但夏季开放时间为10点,以便学生为考试做准备。 虽然天气预报说天气晴朗, 暴雨始于下午, 新政策要求所有雇员在进入内部网络之前完成安全培训课程。`
  - `paragraph-v1-0016` chrf_drop -0.1157：Use HTTPS to protect user data. The DNS lookup failed. The regex matches email addresses. → 基线 `使用 HTTPS 保护用户数据。 DNS 查询失败。 regex 符合电子邮件地址。` / 本次 `使用 HTTPS 保护用户数据。 DNS 搜索失败。 Regex符合电子邮件地址。`
  - `paragraph-v1-0001` chrf_drop -0.1042：A memory leak can cause the application to crash. The thread pool limits the number of concurrent tasks. Garbage collection frees unused memory automatically. → 基线 `内存泄漏可能导致应用程序崩溃。 线索集合限制同时任务的数量。 垃圾收集会自动释放未使用的内存。` / 本次 `内存泄漏可能导致应用程序崩溃。 线串集合限制同时执行的任务数量。 垃圾收集自动免去未使用的内存。`
  - `paragraph-v1-0039` chrf_drop -0.0886：You are welcome. I am fine. What is your name? → 基线 `不用客气,我很好,你叫什么名字?` / 本次 `欢迎您。 我没事 你叫什么名字? 名字吗?`
  - `paragraph-v1-0012` chrf_drop -0.0844：The runtime provides memory management and error handling. Garbage collection pauses can affect latency. The cache reduced the average response time. → 基线 `运行时间提供存储管理和错误处理。 垃圾收集暂停会影响延迟。 缓存会缩短平均反应时间。` / 本次 `运行时间提供存储管理和错误处理。 垃圾收集暂停可能会影响延缓。 缓冲器减少了平均反应时间。`
- **group**：chrF -0.0008，BLEU -0.0005，错误 +0，新增致命 1，回归样本 18，改善样本 14
  - `paragraph-v1-0013` chrf_drop -0.3535：We measured throughput and latency under load. The network bandwidth is the bottleneck. The API returns JSON responses. → 基线 `我们测量了载荷中的吞吐量和延缓度。 网络带宽是瓶颈。 API 返回 JSON 响应。` / 本次 `我们测量了负载中的输送量和延缓度 网络带宽就是瓶颈 API回`
  - `paragraph-v1-0040` chrf_drop -0.2376：My name is Li Ming. Nice to meet you. See you tomorrow. → 基线 `我叫李明,很高兴认识你,明天见` / 本次 `我叫李明,幸会 明天见`
  - `paragraph-v1-0027` chrf_drop -0.1641：I have no idea. It depends on the weather. We had a great time yesterday. → 基线 `我不知道,这取决于天气 我们昨天玩得很开心` / 本次 `我不知道,这取决于天气 昨天我们玩得很开心`
  - `paragraph-v1-0030` chrf_drop -0.1177：The library opens at nine in the morning and closes at eight in the evening, but during the summer it stays open until ten so that students can prepare for their exams. Although the weather forecast said it would be sunny, heavy rain started in the afternoon and we had to cancel the picnic. The new policy requires all employees to complete a security training course before they can access the internal network. → 基线 `图书馆上午9点开放,晚上8点关闭,但夏季开放到10点,学生们可以准备考试。 虽然天气预报说会晴天,但下午大雨开始,我们不得不取消野餐。 新政策要求所有员工在进入内部网络之前完成安全培训课程。` / 本次 `图书馆上午9点开放,晚上8点关闭,但夏季开放到10点,学生们可以准备考试。 尽管天气预报说会晴天,但下午大雨开始,我们不得不取消野餐。 学校的校舍和校舍都已经停了下来。 图书馆的开业时间是上午9点钟,晚上八点关闭。 暑假,图书馆开放到十点,让学生们为考试做准备。 尽管气象预报说,它会晴朗,但大雨始于下午, 新政策要求所有雇员在进入内部网络之前完成安全培训课程。`
  - `paragraph-v1-0016` chrf_drop -0.1157：Use HTTPS to protect user data. The DNS lookup failed. The regex matches email addresses. → 基线 `使用 HTTPS 保护用户数据。 DNS 查询失败。 regex 符合电子邮件地址。` / 本次 `使用 HTTPS 保护用户数据。 DNS 搜索失败。 Regex符合电子邮件地址。`
  - `paragraph-v1-0043` chrf_drop -0.1065：They live in a small town. This is my friend. That is a good idea. → 基线 `他们住在小镇上 这是我的朋友 好主意` / 本次 `他们住在小镇上,这是我朋友 好主意,我还是想说`
  - `paragraph-v1-0001` chrf_drop -0.1019：A memory leak can cause the application to crash. The thread pool limits the number of concurrent tasks. Garbage collection frees unused memory automatically. → 基线 `内存泄漏可能导致应用程序崩溃。 线索集合限制同时任务的数量。 垃圾收集会自动释放未使用的内存。` / 本次 `内存泄漏可能导致应用程序崩溃。 线索库限制同时执行的任务数量。 垃圾收集自动免去未使用的内存。`
  - `paragraph-v1-0010` chrf_drop -0.0978：The distributed system must tolerate node failures. The load balancer distributes requests across servers. Each microservice has its own database. → 基线 `分布式系统必须能够容忍节点故障。 负载平衡器可以将请求分布在服务器上。 每个微服务都有自己的数据库。` / 本次 `分布式系统必须能够容忍节点故障。 负载平衡器可以将请求分布在服务器上。 每个微观服务机构都有自己的数据库。`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
