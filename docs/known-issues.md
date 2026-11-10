# 已知问题清单（集中记录）

> 依据 `docs/57` §6.3：普通问题直接修，边缘问题集中记录；**不允许把已知 Critical 翻译错误长期藏在 C 级**。
> 本轮状态：P21-R 中文生成体系深化与规则统一（2026-11-09）。人工评审整体 **PENDING**，不得自动视为通过。

| ID | 模块 | 问题 | 级别 | 影响 | 状态 |
|----|------|------|------|------|------|
| Q-001 | Dictionary | ECDICT 子集对 Google 10k 覆盖 98.98%；缺失项主要是老网页词/成人站点/拼写变体（`sitemap`、`hentai`、`dont`） | C | 低 | Open（接受） |
| Q-002 | Dictionary | ECDICT 释义顺序与质量继承上游；P18-L 新增日常短语同样继承上游，部分旧/专业/不自然表达 | B | 中 | Open（人工词义评审 PENDING；p18-dictionary-review.csv） |
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
| Q-013 | Dictionary | P18-L 后包为 38,766 条，仍不是 ECDICT 全量 77 万；已集中补日常短语/搭配/表达 | C | 中 | Partially fixed（P18-L） |
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
| Q-025 | Word | 上下文选义已移入 Coordinator，上下文正文进入缓存身份；覆盖词仍少，不是通用 WSD | B | 中 | Partially fixed（P17-A） |
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
| Q-037 | Word | 100/100 词表检查是“输出包含参考义项”，P18 覆盖率也不是上下文选义准确率 | C | 中 | Open（口径已注明） |
| Q-038 | Document | `document_structure` 只检查标题/列表数量与代码块逐字节保留，不评价译文语义 | C | 低 | Open |
| Q-039 | API | `translate_with_diagnostics_cancellable()` 仍走单路径，Paragraph/Document 的取消不经过 planner | C | 低 | Fixed（P17-A：取消贯穿统一规划/逐单元执行） |
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
| Q-061 | Templates | 40 条完整模板之外，P19-G 已增加有限参数化组合规则；仍不是通用模板系统 | C | 中 | Partially fixed（P19-G） |
| Q-062 | Dictionary | 上下文选义仅覆盖 11 个词/少量明确搭配；不是通用 WSD，无上下文仍返回多义 | B | 中 | Open（P16 部分） |
| Q-063 | Stats | `executions` 是执行单元数，不是宿主逻辑请求数；段落/文章会按单元计数 | C | 低 | Open（口径已注明） |
| Q-064 | Stats | `failures` 只在顶层 API 包装计数；被文档恢复的段落失败不计入 | C | 低 | Open |
| Q-065 | Cache | 词典 miss 回退 NMT 时，下一次仍先尝试词典（回退缓存键与词典键不同） | C | 低 | Fixed（P17-A：回退键写回/读回） |
| Q-066 | Diagnostics | 模板路由的 `capability` 仍为 Nmt（route=rule），capability 语义略粗糙 | C | 低 | Open |
| Q-067 | API | `translate_cancellable`/JNI 仍走单路径，模板与上下文选义只在 options/planner 入口生效 | C | 低 | Fixed（P17-A：统一 run_options 路径） |
| Q-068 | Quality | NMT 减少 6.8% 主要来自 22 条 UI/基础模板，不代表复杂文本也能减少 | C | 中 | Open |
| Q-069 | Metrics | P16 修正 chrF/BLEU 短句 n-gram 记 0 的问题；P11～P15 历史报告中的短句分数偏低，P16 起的对照使用修正口径 | C | 低 | Fixed（历史报告保留） |

| Q-070 | Learning | P20-K 已实现学习记录、候选提取、审核与 staging 发布；尚未自动合并进正式词典/规则，也未完成长期数据积累 | B | 中 | Implemented foundation（P17-A） |
| Q-071 | Remote | 远程回退是 `Translator` 级显式授权；没有按请求预算、按服务商策略或调用次数上限 | B | 中 | Open（P21-M） |
| Q-072 | Learning | 敏感内容检查是标记式启发式，不是通用 DLP；无法保证识别所有密钥/个人信息 | B | 高 | Open（保持默认不收集） |
| Q-073 | Learning | `knowledge.json.v1` 只是候选资源，不会自动重建词典；需要独立打包/审核工具与回归 | C | 中 | Open（P18-L/P19-G） |
| Q-074 | Learning | 学习候选的人工审核、正反例测试与长期回归尚未建立；不能把自动候选当已验证知识 | B | 高 | PENDING_HUMAN |
| Q-075 | Metrics | 已报告可靠覆盖率/模型比例与兜底原因，但“错误直出率”仍需语义评审，自动 fatal 只能作下界 | C | 中 | Open（无自动语义 oracle） |
| Q-076 | Rules | 文档单句段落可复用模板/词典；P19-G 已加有限参数化组合，仍非通用参数化句法 | C | 中 | Partially fixed（P19-G） |

| Q-077 | Dictionary | P18 日常 10k 测试集是 **源内覆盖基准**（与构建补充同源），不是独立第三方语义基准 | B | 高 | Open（需外部人工验收集） |
| Q-078 | Quality | 100% 是“完整词头/短语可查询且有中文义项”，不是翻译准确率、上下文选义或句子质量 | B | 高 | Open（口径已注明） |
| Q-079 | Dictionary | 新增日常短语部分来自 ECDICT，可能含古旧、残片或不自然表达；50 条人工评审 PENDING_HUMAN | B | 高 | PENDING_HUMAN |
| Q-080 | Dictionary | 词典包增至 38,766 条 / 5.3 MB SQLite；前缀查询在 word 测试中约 1.4 ms/条，未做性能优化 | C | 低 | Open（符合当前阶段范围） |
| Q-081 | Data | Google 10k 词表无单独许可证声明，仅本地研究选词；正式发行前需替换/重审 | B | 中 | Open（发行前） |
| Q-082 | Regression | 320 句/P17 质量保持；P19-G 只对少量 book/light 等做规则级选义，完整短语上下文仍有限 | C | 中 | Partially fixed（P19-G） |

| Q-083 | Composition | P19-G 只支持少量已验证日常句式（祈使/陈述/疑问/否定/情态/进行/过去/be/条件）；复杂嵌套仍回退 NMT | B | 高 | Implemented first version（P19-G） |
| Q-084 | Composition | 组合词表仅 177 个高频词/27 条固定表达；未收录实词或无可靠词性时拒绝组合 | B | 中 | Open（按真实缺口扩充） |
| Q-085 | Composition | 没有完整依存句法/指代/时态体系；只验证已支持结构，不能外推为通用规则 MT | B | 高 | Open（范围边界） |
| Q-086 | Composition | 36 条正例/5 条反例为项目自编，规则译文人工评审 PENDING_HUMAN | B | 高 | PENDING_HUMAN |
| Q-087 | Composition | 组合只对 Sentence + AutoLocalFirst/DictionaryOnly 生效；LocalOnly/RemoteOnly 仍按原策略走模型 | C | 低 | Fixed（策略边界） |
| Q-088 | Quality | 320 集 chrF +0.0098、NMT 300→291，只是单一参考自动指标；不等同语义正确率 | C | 中 | Open（口径已注明） |
| Q-089 | Composition | 量词表只覆盖少量高频名词；无可靠量词时 `a/an` 结构拒绝组合而不是猜 | C | 低 | Open（有意保守） |
| Q-090 | Composition | 组合规则只使用词性/规则词表，未形成通用 SenseResolution 公开枚举；book/light 等按上下文选义 | C | 中 | Open（P19 第一版） |

| Q-091 | P20-C | 语义层只新增 4 类结构（并列祈使、否定 want-to、过去+时间、颜色定语+be 形容词），不是通用句法/语义解析 | B | 高 | Implemented first version（P20-C） |
| Q-092 | P20-C | MeaningFrame 仅覆盖谓词/施事/受事/补语/时间/极性/时体/疑问的最小子集；无完整论元角色、作用域和指代 | B | 高 | Open（范围边界） |
| Q-093 | P20-C | ChineseRealizer 未覆盖完整“的/地/得”、全部体貌、复杂修饰、全部量词和所有否定范围 | B | 高 | Open（下一阶段） |
| Q-094 | P20-C | CandidateSelector 只做确定性 rank；未使用中文 n-gram/LM，不能据此宣称自然度排序正确 | C | 中 | Open（有意保守） |
| Q-095 | P20-C/P21 | 语义层仍 opt-in；P19 正例已 36/36 迁移，但复杂句仍回退 NMT | C | 中 | Fixed（P21-R scope） |
| Q-096 | P20-C | 4 条正例与 3 条反例为项目自编，未盲评；不能代表真实日常分布 | B | 高 | PENDING_HUMAN |
| Q-097 | P20-C | P20 语义层未覆盖 P19 全部句型，两个 Composition 路径需同时配置才能兼容旧 36 正例 | C | 中 | Implemented（兼容路径） |
| Q-098 | P20-C | PP 附着、嵌套否定、未解决代词一律回退；回退是安全行为，但语义能力仍有限 | B | 高 | Open（按真实样本扩充） |

| Q-099 | P21-R | 36/36 P19 正例已走新 MeaningFrame/Realizer；短语动词/固定表达/条件/比较/简单被动已接入 | C | 中 | Fixed（P21-R scope） |
| Q-100 | P21-R | ChineseRealizer 已有 的/地/得基础、着/过、简单比较/被动，但复杂补语、相对从句、全部体貌/量词仍缺 | B | 高 | Open（后续） |
| Q-101 | P21-R | CandidateValidation 只检查谓语/论元/否定/时间/地点/疑问标记等硬约束，不能证明语义等价或自然度 | B | 高 | Open（有意保守） |
| Q-102 | P21-R | P21 19 条中文专项、8 条高级结构、14 条未见组合、20 条后置集、7 条 P20 对抗集均为项目自编；统一评审包 98 行仍 PENDING_HUMAN | B | 高 | PENDING_HUMAN |
| Q-103 | P21-R | 320 chrF +0.0017 仅来自 2 条样本改善；不能据此宣称整体质量显著提升 | C | 中 | Open（口径已注明） |
| Q-104 | P21-R | 时间表达只覆盖 yesterday/today/tomorrow/last week/this week 等少量形式；复杂时间未支持 | C | 中 | Open（按需扩充） |
| Q-105 | P21-R | 未实现完整英语句法/语义解析；PP 附着、嵌套否定、长距离指代仍保持安全回退 | B | 高 | Open（范围边界） |
| Q-106 | P21-R | P19 与 P21 两套组合路径仍需同时配置以兼容固定表达/短语动词/条件，尚未完全统一 | C | 中 | Open（渐进迁移） |


## 质量回归集合（必须持续跑）

- 全部 `fixtures/quality/golden-v1.jsonl`（320 条）都是回归样本。
- 重点 Critical/Major 样本：`en-zh-0002`（臆造）、`en-zh-0296`（十六进制数字丢位）、`en-zh-0309`（截断丢 11 PM）、`en-zh-0286`（丢 15%）、`en-zh-0151`（丢 50%）。
- 规则实验：`fixtures/quality/rules-cases.jsonl`（12 条），其中 `rule-0001` 是缩写展开退化样本，实验包不得默认加载。
- 任何新规则/新模型/新解码策略都必须与 `baseline` 在同一金样本上对照；新增 Critical 错误必须登记到本文件。
