# 已知问题清单（集中记录）

> 依据 `docs/57` §6.3：普通问题直接修，边缘问题集中记录；**不允许把已知 Critical 翻译错误长期藏在 C 级**。
> 本轮状态：P11 质量优先连续开发（2026-11-08）。人工评审整体 **PENDING**，不得自动视为通过。

| ID | 模块 | 问题 | 级别 | 影响 | 状态 |
|----|------|------|------|------|------|
| Q-001 | Dictionary | ECDICT 子集对 Google 10k 覆盖 98.98%；缺失项主要是老网页词/成人站点/拼写变体（`sitemap`、`hentai`、`dont`） | C | 低 | Open（接受） |
| Q-002 | Dictionary | ECDICT 释义顺序与质量继承上游；部分旧/专业释义不够自然 | C | 中 | Open（人工词义评审 PENDING） |
| Q-003 | NMT | ORT opus-mt 模型存在臆造/追加内容（如 `Good morning.` → `早上好,我刚从伦敦回来`） | B | 高 | Open（待模型/解码改进） |
| Q-004 | NMT | 模型会丢/改关键数字（`0x80070005` → `0x8007005`、`50%` 丢失、`11 PM` 句被截断） | B | 高 | Open（严格门禁已能拦截） |
| Q-005 | NMT | 只实现 greedy 解码；beam search / beam width 未实现，无法做 T3 的 beam 对照 | B | 中 | Open（功能缺口） |
| Q-006 | NMT | 长句未做句级切分；仅靠 token 上限，超长文本可能截断 | B | 中 | Open |
| Q-007 | NMT | 当前兼容 ONNX 架构的候选只有 opus-mt 这一份；更强的 NLLB/M2M 需要通用架构支持，未评估 | B | 高 | Open（候选研究未完成） |
| Q-008 | Rules | 缩写展开实验包在 10 条对照上 chrF +0.0155 / BLEU −0.0414，且 `don't` 出现退化；默认包只保留无损 NFKC/弯引号 | C | 中 | Fixed（默认不加载实验包） |
| Q-009 | Terms | 术语表只能纠正“已知变体”（如 内存泄露→内存泄漏），不能修复任意错译；本轮真实模型未触发违规，故无质量增益证据 | C | 中 | Open（契约测试通过） |
| Q-010 | Quality | 否定/目标语言/重复句段是启发式，可能误报；只有数字/占位符/空输出是致命门禁 | C | 中 | Open（已记录口径） |
| Q-011 | Quality | 中文数字（“五十”）不满足数字保真检查；参考译文与测试样本统一用阿拉伯数字规避 | C | 低 | Open（文档化） |
| Q-012 | Dictionary | 大小写回退依赖 `lower(headword)` 表达式索引；旧包（无索引）首次回退会扫表 | C | 低 | Open（重建包即解决） |
| Q-013 | Dictionary | 当前包只收 30,200 条（高频/考试/技术短语），不是 ECDICT 全量 77 万 | C | 中 | Open（可扩大） |
| Q-014 | Data | ECDICT 仓库声明 MIT，但上游释义来源未逐条审计；当前仅本地非商业研究，不随发行物分发 | B | 中 | Open（发行前重审） |
| Q-015 | Model | 本地 tokenizer.json 为适配 tokenizers 0.23 把 `normalizer` 的 null charsmap 置空；模型文件未改，但属研究性补丁 | B | 中 | Open（发行前替换为正式导出） |
| Q-016 | Quality | 人工评审只生成抽样模板（翻译 62 条 + 词典 60 条），未完成 320 条翻译评审 | B | 高 | PENDING_HUMAN |
| Q-017 | Quality | chrF/BLEU 只对照单一参考译文，不能单独判定正确率；无语义评估模型 | C | 中 | Open（接受） |
| Q-018 | Mode | Fast 只保证数字/占位符/空输出；否定/重复/目标语言启发式只在 Precise 跑 | C | 低 | Fixed（合同明确） |
| Q-019 | Mode | 模式目前只影响协调器检查与缓存身份，不影响 NMT 解码配置；未来 Fast 若改解码必须扩展 engine cache identity | C | 低 | Open（设计约束） |
| Q-020 | Rules | 规则包没有“变体是 preferred 前缀”的编译期检查；术语表有同类风险，本轮人工排查过 | C | 低 | Open |
| Q-021 | P10 | Windows ORT 连续 10k soak 仍 PENDING（1,500/4,000），不冒充通过 | C | 低 | Open（暂停） |
| Q-022 | Android | 真实 Android NMT / ARM64 运行 / ncnn / macOS 仍暂停或 BLOCKED | C | 低 | Open（暂停） |
| Q-023 | Tooling | `quality-runner` ORT 特性需要本地模型目录，不进入默认 CI；模型不入库 | C | 低 | Open（设计选择） |
| Q-024 | Quality | 长句/多义词的语义错误没有自动检测，只能靠金样本 chrF 与人工评审 | C | 中 | Open |

## 质量回归集合（必须持续跑）

- 全部 `fixtures/quality/golden-v1.jsonl`（320 条）都是回归样本。
- 重点 Critical/Major 样本：`en-zh-0002`（臆造）、`en-zh-0296`（十六进制数字丢位）、`en-zh-0309`（截断丢 11 PM）、`en-zh-0286`（丢 15%）、`en-zh-0151`（丢 50%）。
- 规则实验：`fixtures/quality/rules-cases.jsonl`（12 条），其中 `rule-0001` 是缩写展开退化样本，实验包不得默认加载。
- 任何新规则/新模型/新解码策略都必须与 `baseline` 在同一金样本上对照；新增 Critical 错误必须登记到本文件。
