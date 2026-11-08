# 翻译质量自动对照

- 金样本：/tmp/budget2.jsonl
- 样本数：20
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| auto | 0.3663 | 0.3894 | 0.00% | 0 | n/a | n/a | n/a | n/a | 1.0 |
| sentence | 0.3918 | 0.4333 | 0.00% | 0 | n/a | n/a | n/a | n/a | 1.0 |
| best | 0.3909 | 0.4322 | 0.00% | 0 | n/a | n/a | n/a | n/a | 1.0 |

## 分类 chrF

| 类别 | auto | sentence | best |
|---|---:|---:|---:|
| budget-2 | 0.3663 | 0.3918 | 0.3909 |

## 相对基线的变化

- **sentence**：chrF +0.0255，BLEU +0.0439，错误 +0，新增致命 0，回归样本 5，改善样本 7
  - `budget-2-10` chrf_drop -0.2425：The file system stores metadata separately. The database query returned no results. → 基线 `文件系统单独存储元数据。 数据库查询没有返回结果。` / 本次 `文件系统单独存储元数据。 数据库查询返回没有结果。`
  - `budget-2-09` chrf_drop -0.0831：Run the command from the command line. The operating system manages hardware resources. → 基线 `从命令行运行命令。操作系统管理硬件资源。` / 本次 `从命令行中运行命令。 操作系统管理硬件资源。`
  - `budget-2-03` chrf_drop -0.0703：The system deadlocked because two processes waited for each other. The stack overflow was caused by infinite recursion. → 基线 `系统陷入僵局是因为两个过程互相等待。 堆叠溢出是由无限的重复引起的。` / 本次 `系统陷入僵局是因为两个过程相互等待。 堆叠溢出是由无限的循环引起的`
  - `budget-2-08` chrf_drop -0.0525：The integration test covers the entire pipeline. The regression test caught the bug before release. → 基线 `集成测试覆盖整个管道 回归测试在释放前就发现了错误` / 本次 `整编测试涵盖整个输油管。 回归测试在释放前 发现了错误`
  - `budget-2-02` chrf_drop -0.0329：Garbage collection frees unused memory automatically. A race condition occurs when two threads access shared data. → 基线 `垃圾回收自动释放未使用的内存。 当两条线访问共享数据时, 就会出现种族状况。` / 本次 `垃圾回收自动免去未使用的内存。 当两条线访问共享数据时,就会出现种族状况。`
- **best**：chrF +0.0246，BLEU +0.0428，错误 +0，新增致命 0，回归样本 3，改善样本 6
  - `budget-2-10` chrf_drop -0.2425：The file system stores metadata separately. The database query returned no results. → 基线 `文件系统单独存储元数据。 数据库查询没有返回结果。` / 本次 `文件系统单独存储元数据。 数据库查询返回没有结果。`
  - `budget-2-09` chrf_drop -0.0831：Run the command from the command line. The operating system manages hardware resources. → 基线 `从命令行运行命令。操作系统管理硬件资源。` / 本次 `从命令行中运行命令。 操作系统管理硬件资源。`
  - `budget-2-08` chrf_drop -0.0525：The integration test covers the entire pipeline. The regression test caught the bug before release. → 基线 `集成测试覆盖整个管道 回归测试在释放前就发现了错误` / 本次 `整编测试涵盖整个输油管。 回归测试在释放前 发现了错误`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
