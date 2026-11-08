# Best 候选选择审计（离线参考）

- 样本：50
- **选择器准确率**：62.00%（选中候选 = 两候选中 chrF 更高者）
- 平均 regret：0.0224 chrF
- 选择分布：{'sentence': 35, 'whole': 15}

## 选错且损失最大的样本

| ID | 选中 | Oracle | whole | sentence | best | regret |
|---|---|---|---:|---:|---:|---:|
| paragraph-v1-0027 | sentence | whole | 0.5927 | 0.3453 | 0.3453 | 0.2474 |
| paragraph-v1-0007 | sentence | whole | 0.8958 | 0.7318 | 0.7318 | 0.1641 |
| paragraph-v1-0047 | whole | sentence | 0.2428 | 0.3607 | 0.2428 | 0.1179 |
| paragraph-v1-0009 | whole | sentence | 0.6132 | 0.7219 | 0.6132 | 0.1087 |
| paragraph-v1-0039 | sentence | whole | 0.3757 | 0.2871 | 0.2871 | 0.0886 |
| paragraph-v1-0008 | sentence | whole | 0.6219 | 0.5498 | 0.5498 | 0.0721 |
| paragraph-v1-0037 | sentence | whole | 0.4642 | 0.3953 | 0.3953 | 0.0689 |
| paragraph-v1-0001 | sentence | whole | 0.5976 | 0.4934 | 0.5366 | 0.061 |
| paragraph-v1-0046 | sentence | whole | 0.5028 | 0.4542 | 0.4542 | 0.0486 |
| paragraph-v1-0014 | sentence | whole | 0.7003 | 0.6571 | 0.6571 | 0.0432 |
| paragraph-v1-0029 | sentence | whole | 0.4937 | 0.4531 | 0.4531 | 0.0406 |
| paragraph-v1-0012 | sentence | whole | 0.4588 | 0.3744 | 0.4239 | 0.0348 |
| paragraph-v1-0045 | sentence | whole | 0.3496 | 0.3358 | 0.3358 | 0.0138 |
| paragraph-v1-0048 | sentence | whole | 0.248 | 0.2449 | 0.2449 | 0.0032 |
| paragraph-v1-0010 | sentence | whole | 0.5969 | 0.5356 | 0.594 | 0.0029 |
| paragraph-v1-0049 | sentence | whole | 0.3139 | 0.3112 | 0.3112 | 0.0027 |
| paragraph-v1-0028 | sentence | whole | 0.2087 | 0.207 | 0.207 | 0.0017 |
| paragraph-v1-0005 | sentence | whole | 0.2564 | 0.2491 | 0.3113 | 0.0 |
| paragraph-v1-0042 | sentence | whole | 0.4598 | 0.4598 | 0.4598 | 0.0 |

> 参考只用于审计；运行时选择器不读取参考。
