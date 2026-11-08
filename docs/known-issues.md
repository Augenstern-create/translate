# 已知问题清单（集中记录）

> 依据 `docs/57` §6.3：普通问题直接修，边缘问题集中记录；**不允许把已知 Critical 翻译错误长期藏在 C 级**。
> 本轮状态：P12-Q 四级文本与质量开发（2026-11-08）。人工评审整体 **PENDING**，不得自动视为通过。

| ID | 模块 | 问题 | 级别 | 影响 | 状态 |
|----|------|------|------|------|------|
| Q-001 | Dictionary | ECDICT 子集对 Google 10k 覆盖 98.98%；缺失项主要是老网页词/成人站点/拼写变体（`sitemap`、`hentai`、`dont`） | C | 低 | Open（接受） |
| Q-002 | Dictionary | ECDICT 释义顺序与质量继承上游；部分旧/专业释义不够自然 | C | 中 | Open（人工词义评审 PENDING） |
| Q-003 | NMT | ORT opus-mt 模型存在臆造/追加内容（如 `Good morning.` → `早上好,我刚从伦敦回来`） | B | 高 | Open（待模型/解码改进） |
| Q-004 | NMT | 模型会丢/改关键数字；P12 已用数字保护/句级切分修复旧 4 条，其他数字仍可能出错 | B | 高 | Partially fixed（P12 320 条 fatal 0） |
| Q-005 | NMT | Beam 已实现（带 cache，beam2 5.4× / beam4 14.5× 延迟），40 条子集 chrF +0.022/+0.028；未默认 | C | 中 | Fixed（opt-in） |
| Q-006 | Planner | 句级切分只覆盖单一 `and/but`；其他长句结构仍整句翻译，超长文本仍可能截断 | C | 中 | Partially fixed（P12） |
| Q-007 | NMT | 当前兼容 ONNX 架构的候选只有 opus-mt 这一份；更强的 NLLB/M2M 需要通用架构支持，未评估 | B | 高 | Open（候选研究未完成） |
| Q-008 | Rules | 缩写展开实验包在 10 条对照上 chrF +0.0155 / BLEU −0.0414，且 `don't` 出现退化；默认包只保留无损 NFKC/弯引号 | C | 中 | Fixed（默认不加载实验包） |
| Q-009 | Terms | 术语表只能纠正“已知变体”（如 内存泄露→内存泄漏），不能修复任意错译；P11 已实测 7 处修正，P13 增加整段/整篇源文触发 | C | 中 | Partially fixed（聚合术语约束） |
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
| Q-025 | Word | 带上下文选义未实现：`context_id` 只进缓存身份；P13 的术语聚合不解决词义消歧 | B | 中 | Open |
| Q-026 | Paragraph | 句群策略（每 2 句）在长句组上会截断；50 段对照 group chrF 0.3909、完整率 0.50，低于逐句 0.3987/0.58 | B | 中 | Fixed（Auto 改为 1–2 句整段、≥3 句逐句；Group 仅实验） |
| Q-027 | Document | 仅支持纯文本/Markdown 的标题、段落、列表、引用、代码块；表格行原样保留不翻译 | C | 中 | Open（符合 docs/62 §7.1 范围） |
| Q-028 | Document | 标题/列表翻译质量受模型限制（如 `System status` → `系统系统状态`）；结构正确但语义不佳 | B | 中 | Open |
| Q-029 | NMT | Beam 已实现（带 cache）：beam2 约 5.4×、beam4 约 14.5× 贪心延迟；40 条子集 chrF +0.0222/+0.0280。未设为默认 | C | 中 | Open（opt-in） |
| Q-030 | Mode | Fast 仍使用同一 NMT 解码，只少语义启发式；无实时流式路径 | C | 低 | Open（合同明确） |
| Q-031 | Planner | 句级切分仅处理**单一** `and/but` 且第二段像谓语/主语从句；多连接词、so/because/while 等仍整句翻译 | C | 中 | Open |
| Q-032 | Quality | 数字保护可能改变语序（`below 50%` → `以下50%`）；已加范围词换位与标点修复，0286 仍略生硬 | B | 中 | Partially fixed |
| Q-033 | Document | 段落失败时保留原文并加 `SegmentFailed` 警告；只做了代码路径，缺真实故障注入测试 | C | 低 | Open |
| Q-034 | Planner | 句子切分的缩写表有限，未列出的缩写可能误切 | C | 低 | Open |
| Q-035 | Quality | 新增 100 词/100 句/50 段/10 篇的参考译文仍为项目自编，人工评审 PENDING | B | 高 | PENDING_HUMAN |
| Q-036 | Planner | Auto 分类基于句数/段落/Markdown/长度启发式，可能误判边界文本 | C | 低 | Open |
| Q-037 | Word | 100/100 词表检查是“输出包含参考义项”，不证明上下文中选对义项 | C | 中 | Open |
| Q-038 | Document | `document_structure` 只检查标题/列表数量与代码块逐字节保留，不评价译文语义 | C | 低 | Open |
| Q-039 | API | `translate_with_diagnostics_cancellable()` 仍走单路径，Paragraph/Document 的取消不经过 planner | C | 低 | Open |
| Q-040 | Planner | Auto 对 2 词短语（`good morning`）按 Word 处理；若词典未命中且无 NMT，错误码为 ResourceNotInstalled（已修） | C | 低 | Open |
| Q-041 | Planner | Auto 分类阈值（3 词/2 句/2 段）是经验值，边界文本可能误判 | C | 低 | Open |
| Q-042 | Document | 表格行不翻译，只保留原文；`document_structure` 检查不覆盖表格内容 | C | 低 | Open |
| Q-043 | Model | fp32 在 100 句独立集 chrF +0.0189，但 320 集 -0.0014、50 段 -0.0020；精度提升不是全局一致 | B | 中 | Open（不默认切换） |
| Q-044 | Model | fp32 峰值 RSS 约 1.13 GB（int8 0.58 GB）、冷加载 6.6s；fp16 更慢且内存 1.34 GB | C | 中 | Open（桌面可选） |
| Q-045 | Paragraph | Best 候选（整段+逐句）chrF +0.0218、覆盖 0.3848→0.4111，但延迟约 3×，完整率仍 0.60 | B | 中 | Open |
| Q-046 | Context | 已实现前一句上下文实验（P14），但多数段落回退；真正可靠的指代/篇章上下文仍未实现 | B | 中 | Partially fixed（实验） |
| Q-047 | Quality | 条件/遗漏/臆造仍是启发式；条件标记表放宽后可能漏报，需人工校准 | C | 中 | Open |
| Q-048 | Review | P13 评审包 44 条（Critical/句子/段落/文档）仍 PENDING_HUMAN | B | 高 | PENDING_HUMAN |
| Q-049 | Paragraph | Best 评分用句数/长度/quality 启发式，不偷看参考；可能选到“更长但更差”的候选 | C | 中 | Open |
| Q-050 | Model | fp32 320 集 35 条回归/37 条改善，类别差异大（否定/条件更好，basic/daily 更差） | B | 中 | Open |
| Q-051 | Model | 未找到不同权重的可运行 ONNX en→zh 候选：onnx-community/skillsafe/Xenova 均为同一 Helsinki opus-mt；liam168 无 ONNX 且许可未声明；NLLB/M2M 需通用架构 | B | 高 | Blocked（待导出/架构） |
| Q-052 | Selector | Best 选择器离线性准确率 62%、平均 regret 0.0224 chrF；简单规则未找到稳定更优方案 | B | 中 | Open（需人工 A/B） |
| Q-053 | Context | 前一句上下文实验 chrF +0.0133/覆盖 +0.0141，但 29/50 段发生 2 次回退（模型常把上下文与当前句合成一段），只作实验不默认 | B | 中 | Open（实验失败为主） |
| Q-054 | Review | P14 盲评 50 段候选 A/B 已生成，人工未评分 | B | 高 | PENDING_HUMAN |
| Q-055 | Model | 不同权重筛选：shiqing 100 句 0.4302、M2M 20 句 0.4277、NLLB 10 句 0.3392，均低于 base int8（0.4535/0.4679/0.3485） | B | 高 | Blocked（无更优候选） |
| Q-056 | Auto | P15 校准：混合 40 段 Auto 0.4484 > 强制 Best 0.4449，且 wall 26.7s vs 31.5s；2 句段落逐句 0.3918 > 整段 0.3663 | C | 中 | Fixed（Auto 2 句逐句、≥3 句 Best） |
| Q-057 | Review | 12 条 AI 辅助初步评审已生成（明确标记 AI_ASSISTED_PRELIMINARY），94 条人工评审仍未完成 | B | 高 | PENDING_HUMAN |
| Q-058 | Model | 本地 torch/optimum 导出的 shiqing ONNX 与 M2M/NLLB 缓存均在 `.build`（gitignore）；模型不入库 | C | 低 | Open（本地研究） |
| Q-059 | Model | NLLB/M2M 只做少量句子筛选，样本小、架构不同，不能作最终模型结论 | C | 中 | Open |
| Q-060 | Context | Context 仍是实验：29/50 段两次回退；真实上下文/指代未解决 | B | 中 | Open（等强模型） |


## 质量回归集合（必须持续跑）

- 全部 `fixtures/quality/golden-v1.jsonl`（320 条）都是回归样本。
- 重点 Critical/Major 样本：`en-zh-0002`（臆造）、`en-zh-0296`（十六进制数字丢位）、`en-zh-0309`（截断丢 11 PM）、`en-zh-0286`（丢 15%）、`en-zh-0151`（丢 50%）。
- 规则实验：`fixtures/quality/rules-cases.jsonl`（12 条），其中 `rule-0001` 是缩写展开退化样本，实验包不得默认加载。
- 任何新规则/新模型/新解码策略都必须与 `baseline` 在同一金样本上对照；新增 Critical 错误必须登记到本文件。
