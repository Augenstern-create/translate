# 翻译质量自动对照

- 金样本：fixtures/quality/sentence-v1.jsonl
- 样本数：100
- 人工评审：**PENDING**

| 配置 | chrF-zh ↑ | BLEU-zh（辅助） | 错误率 | 致命完整性 | 数字保真 | 占位符 | URL | 否定 | 目标语言 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| int8 | 0.4535 | 0.4621 | 0.00% | 0 | 1.0 | n/a | n/a | 1.0 | 1.0 |
| fp16 | 0.4648 | 0.4777 | 0.00% | 0 | 1.0 | n/a | n/a | 1.0 | 1.0 |
| fp32 | 0.4724 | 0.4842 | 0.00% | 0 | 1.0 | n/a | n/a | 1.0 | 1.0 |

## 分类 chrF

| 类别 | int8 | fp16 | fp32 |
|---|---:|---:|---:|
| conditional | 0.3608 | 0.3671 | 0.3671 |
| negation | 0.4137 | 0.4572 | 0.49 |
| numbers_units | 0.623 | 0.6041 | 0.6041 |
| polysemy | 0.5038 | 0.5304 | 0.5304 |
| technical | 0.3429 | 0.3427 | 0.3427 |

## 相对基线的变化

- **fp16**：chrF +0.0113，BLEU +0.0156，错误 +0，新增致命 0，回归样本 11，改善样本 16
  - `sentence-v1-0018` chrf_drop -0.5701：The job finished in 2 hours and 30 minutes. → 基线 `工作在2小时30分钟内完成。` / 本次 `工作在两小时三十分钟内完成`
  - `sentence-v1-0075` chrf_drop -0.3869：The flight takes about 12 hours and 45 minutes. → 基线 `飞行大约需要12小时45分钟。` / 本次 `飞行需要大约12小时45分钟。`
  - `sentence-v1-0066` chrf_drop -0.2229：We will ship the order once payment is confirmed. → 基线 `付款一经确认 我们就会发货` / 本次 `付款一经确认,我们即发货`
  - `sentence-v1-0039` chrf_drop -0.1925：The present was wrapped in blue paper. → 基线 `蓝色纸包裹着这个礼物` / 本次 `现今的面纱是蓝纸包裹的。`
  - `sentence-v1-0099` chrf_drop -0.149：We added a unit test for the parser. → 基线 `我们增加了一个单元测试 用于采集器。` / 本次 `我们加了一个分析器的单元测试`
  - `sentence-v1-0094` chrf_drop -0.1176：Use-after-free bugs are difficult to reproduce. → 基线 `使用后无菌虫很难复制。` / 本次 `无用虫难以繁殖。`
  - `sentence-v1-0001` chrf_drop -0.092：I did not expect the server to fail during the update. → 基线 `我没想到服务器在更新期间会失败` / 本次 `我没想到服务器在更新时会失败`
  - `sentence-v1-0032` chrf_drop -0.0891：He runs a small software company. → 基线 `他经营一家小型软件公司。` / 本次 `他经营一家小型软件公司`
- **fp32**：chrF +0.0189，BLEU +0.0221，错误 +0，新增致命 0，回归样本 11，改善样本 17
  - `sentence-v1-0018` chrf_drop -0.5701：The job finished in 2 hours and 30 minutes. → 基线 `工作在2小时30分钟内完成。` / 本次 `工作在两小时三十分钟内完成`
  - `sentence-v1-0075` chrf_drop -0.3869：The flight takes about 12 hours and 45 minutes. → 基线 `飞行大约需要12小时45分钟。` / 本次 `飞行需要大约12小时45分钟。`
  - `sentence-v1-0066` chrf_drop -0.2229：We will ship the order once payment is confirmed. → 基线 `付款一经确认 我们就会发货` / 本次 `付款一经确认,我们即发货`
  - `sentence-v1-0039` chrf_drop -0.1925：The present was wrapped in blue paper. → 基线 `蓝色纸包裹着这个礼物` / 本次 `现今的面纱是蓝纸包裹的。`
  - `sentence-v1-0099` chrf_drop -0.149：We added a unit test for the parser. → 基线 `我们增加了一个单元测试 用于采集器。` / 本次 `我们加了一个分析器的单元测试`
  - `sentence-v1-0094` chrf_drop -0.1176：Use-after-free bugs are difficult to reproduce. → 基线 `使用后无菌虫很难复制。` / 本次 `无用虫难以繁殖。`
  - `sentence-v1-0001` chrf_drop -0.092：I did not expect the server to fail during the update. → 基线 `我没想到服务器在更新期间会失败` / 本次 `我没想到服务器在更新时会失败`
  - `sentence-v1-0032` chrf_drop -0.0891：He runs a small software company. → 基线 `他经营一家小型软件公司。` / 本次 `他经营一家小型软件公司`

> 说明：chrF/BLEU 只反映与单一参考译文的接近程度，不能单独判定翻译正确；
> 数字/占位符/否定/目标语言是确定性检查。人工评审结果必须另行填写，不能自动标为通过。
