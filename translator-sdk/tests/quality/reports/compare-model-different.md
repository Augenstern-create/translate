# 翻译质量自动对照

- 金样本：fixtures/quality/sentence-v1.jsonl
- 样本数：100
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| int8 | 0.4535 | 0.4621 | 0.00% | 0 | 1.0 | n/a | n/a | 1.0 | 1.0 |
| fp32 | 0.4724 | 0.4842 | 0.00% | 0 | 1.0 | n/a | n/a | 1.0 | 1.0 |
| shiqing | 0.4302 | 0.4420 | 0.00% | 0 | 1.0 | n/a | n/a | 1.0 | 1.0 |

## 分类 chrF

| 类别 | int8 | fp32 | shiqing |
|---|---:|---:|---:|
| conditional | 0.3608 | 0.3671 | 0.3334 |
| negation | 0.4137 | 0.49 | 0.437 |
| numbers_units | 0.623 | 0.6041 | 0.5561 |
| polysemy | 0.5038 | 0.5304 | 0.5298 |
| technical | 0.3429 | 0.3427 | 0.269 |

## 相对基线的变化

- **fp32**：chrF +0.0189，BLEU +0.0221，错误 +0，新增致命 0，回归样本 11，改善样本 17
  - `sentence-v1-0018` chrf_drop -0.5701：The job finished in 2 hours and 30 minutes. → 基线 `工作在2小时30分钟内完成。` / 本次 `工作在两小时三十分钟内完成`
  - `sentence-v1-0075` chrf_drop -0.3869：The flight takes about 12 hours and 45 minutes. → 基线 `飞行大约需要12小时45分钟。` / 本次 `飞行需要大约12小时45分钟。`
  - `sentence-v1-0066` chrf_drop -0.2229：We will ship the order once payment is confirmed. → 基线 `付款一经确认 我们就会发货` / 本次 `付款一经确认,我们即发货`
  - `sentence-v1-0039` chrf_drop -0.1925：The present was wrapped in blue paper. → 基线 `蓝色纸包裹着这个礼物` / 本次 `现今的面纱是蓝纸包裹的。`
  - `sentence-v1-0099` chrf_drop -0.149：We added a unit test for the parser. → 基线 `我们增加了一个单元测试 用于采集器。` / 本次 `我们加了一个分析器的单元测试`
  - `sentence-v1-0094` chrf_drop -0.1176：Use-after-free bugs are difficult to reproduce. → 基线 `使用后无菌虫很难复制。` / 本次 `无用虫难以繁殖。`
  - `sentence-v1-0001` chrf_drop -0.092：I did not expect the server to fail during the update. → 基线 `我没想到服务器在更新期间会失败` / 本次 `我没想到服务器在更新时会失败`
  - `sentence-v1-0032` chrf_drop -0.0891：He runs a small software company. → 基线 `他经营一家小型软件公司。` / 本次 `他经营一家小型软件公司`
- **shiqing**：chrF -0.0233，BLEU -0.0201，错误 +0，新增致命 0，回归样本 26，改善样本 22
  - `sentence-v1-0020` chrf_drop -0.6394：Please retry after 30 seconds. → 基线 `请在30秒后重试。` / 本次 `30秒后请重试。`
  - `sentence-v1-0086` chrf_drop -0.578：A memory leak can cause the application to crash. → 基线 `内存泄漏可能导致应用程序崩溃。` / 本次 `内存泄漏可导致程序崩溃。`
  - `sentence-v1-0018` chrf_drop -0.5701：The job finished in 2 hours and 30 minutes. → 基线 `工作在2小时30分钟内完成。` / 本次 `工作在两小时三十分钟内完成`
  - `sentence-v1-0058` chrf_drop -0.5214：Unless you hurry, you will miss the train. → 基线 `除非你快点 否则会错过火车的` / 本次 `赶不赶,火车会误点`
  - `sentence-v1-0070` chrf_drop -0.3678：Unless the config is valid, the application will not start. → 基线 `除非配置有效, 否则应用程序不会启动。` / 本次 `除非配置有效, 应用程序将不启动。`
  - `sentence-v1-0075` chrf_drop -0.2686：The flight takes about 12 hours and 45 minutes. → 基线 `飞行大约需要12小时45分钟。` / 本次 `飞行需要12小时45分钟。`
  - `sentence-v1-0091` chrf_drop -0.2555：The stack overflow was caused by infinite recursion. → 基线 `堆叠溢出是由无限的循环引起的` / 本次 `堆叠溢出是无限的循环造成的`
  - `sentence-v1-0024` chrf_drop -0.2452：The file is 4.5 GB, which exceeds the limit. → 基线 `文件为4.5GB, 超过限值。` / 本次 `超过极限的4.5GB`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
