# Phase 0：冻结基线、归档分支与可回滚点

**执行日期：** 2026-11-11
**基准提交：** `16c55b9f7f883bfd29dbfd2201aa3429d3e98516`（`E0: freeze baseline and add independent quality research audit`）
**执行主机：** Linux，i5-8250U / 32 GiB，`rustc 1.99.1`，`cargo 1.99.1`

---

## 1. 分支与回滚点

| 分支 | 用途 | 起始提交 |
|---|---|---|
| `archive/pre-online-refactor-20261111` | 重构前完整快照（含全部旧规则/NMT/研究代码、历史文档、旧 fixtures） | `16c55b9` |
| `refactor/online-translation-v1` | 本轮重构工作分支 | `16c55b9` |
| `main` | 未改动，保留原主线 | `16c55b9` |

回滚方法：

```bash
git switch main                     # 回到重构前主线
git switch archive/pre-online-refactor-20261111   # 只读查看完整旧实现
```

**归档分支不是"另一个备份目录"**：旧实现（`semantic` / `composer` / `pipeline` / `context_sense` / `planner`、
`translator-nmt-*` / `translator-runtime` / `translator-resource` / `translator-learning` / `translator-jni` /
`p33-bench` / `migrate-rules`）在新主干删除后，**只能**在该分支检索，不会在新主干保留重复副本。

---

## 2. 重构前工作区成员与依赖关系

`cargo metadata --no-deps` 实测（`translator-sdk/Cargo.toml`）：

```
translator-types            (无内部依赖)
translator-plugin-api    -> translator-types
translator-core          -> translator-types, translator-plugin-api, translator-analysis
translator-analysis      -> translator-types
translator-api           -> translator-types, translator-plugin-api, translator-core,
                            translator-dictionary, translator-analysis, translator-rules,
                            translator-learning, translator-nmt-bergamot, translator-nmt-ort,
                            translator-resource, pack-dictionary
translator-dictionary    -> translator-types, translator-plugin-api, pack-dictionary
translator-learning      -> translator-types, translator-resource
translator-resource      (无内部依赖)
translator-runtime       -> translator-types
translator-nmt-bergamot  -> translator-types, translator-plugin-api, translator-runtime,
                            translator-core
translator-nmt-ort       -> translator-types, translator-plugin-api, translator-core
translator-rules         -> translator-types, translator-core
translator-cli           -> translator-types, translator-plugin-api, translator-api,
                            translator-resource
translator-jni           -> translator-types, translator-api, translator-analysis,
                            translator-resource
pack-dictionary          -> translator-dictionary
p33-bench                -> translator-api, translator-core, translator-dictionary,
                            translator-nmt-bergamot, translator-plugin-api, translator-runtime,
                            translator-types
migrate-rules            -> translator-rules
quality-runner           -> translator-types, translator-api, translator-analysis
```

关键事实（决定了清理顺序）：

1. `translator-api` 是旧依赖的**汇聚点**：它同时依赖 `translator-learning`、`translator-nmt-bergamot`、
   `translator-nmt-ort`、`translator-resource`。因此必须先重写 `translator-api` 的构建器，
   才能从 Workspace 移除这些 crate；反过来（先删 crate）会让 `translator-api` 与
   `translator-cli`、`translator-jni` 同时编译失败。
2. `translator-core` 对 `translator-analysis` 的依赖只有两处用途：语言识别启发式与（可选）形态分析。
   因此它可以在 P2 内被替换成 core 内置的轻量实现，`translator-analysis` 随即退出产品依赖。
3. `pack-dictionary` 被 `translator-dictionary` 与 `translator-api` 同时依赖（校验/打包用途），
   属于"工具 + 库"双重身份，清理时不能简单归档。

---

## 3. 重构前测试基线

命令与结果（Linux，`cargo test --workspace --no-fail-fast`）：

```
passed=550  failed=0  ignored=60
```

按 crate 的主要计数（节选）：

| 测试目标 | passed |
|---|---|
| `translator-core`（lib 单元测试） | 见下 |
| `translator-analysis` | 8 |
| `translator-api`：`mock_e2e` | 9 |
| `translator-api`：`p17_architecture` | 13 |
| `translator-api`：`p19_composition` | 9 |
| `translator-api`：`p20_semantic` | 7 |
| `translator-api`：`p24_terms_diagnostics` | 13 |
| `translator-api`：`p80_pipeline` | 8 |
| `translator-api`：`text_four_levels` | 11 |
| `translator-api`：`templates_e2e` | 6 |
| `translator-dictionary` 相关 | 见 `cargo test` 全量日志 |
| `pack-dictionary` | 9 |

完整日志：`/tmp/baseline-test.log`（本地）；结论以"0 失败"为准。

**解释：** 这个基线的 550 个测试里有相当一部分是旧规则/语义/组合引擎的专项回归
（`p19_*` / `p20_*` / `p21_*` / `p80_*` 等）。它们会随 Phase 3/4 一起归档，
**不代表**新主链的质量指标。新主链的验收基线见 `docs/testing.md`。

---

## 4. 秘密信息与体积检查

```bash
grep -rInE "(sk-[A-Za-z0-9]{16,}|api[_-]?key\s*=\s*\"[^\"]{8,}|Bearer [A-Za-z0-9._-]{20,})" \
  --include="*.rs" --include="*.toml" --include="*.json" --include="*.md" --include="*.sh" .
```

结果：**0 命中**。仓库内没有硬编码的上游密钥。

体积检查（`git ls-files` 中最大的 10 个文件）：

| 大小 | 文件 | 处置 |
|---|---|---|
| 3.1M | `tests/quality/outputs/p18-daily-after-lookup.jsonl` | 归档（旧规则评测产物） |
| 2.3M | `fixtures/quality/daily-knowledge-v1.jsonl` | 归档（旧规则评测集） |
| 2.2M | `tests/quality/outputs/p18-daily-baseline-lookup.jsonl` | 归档 |
| 1.9M | `fixtures/quality/daily-knowledge-v1.construction.jsonl` | 归档 |
| 1.6M | `tests/quality/reports/compare-v1.json` | 归档 |
| 1.0M | `tests/quality/reports/p18-daily-baseline.json` | 归档 |
| 800K | `fixtures/dictionary/en-zh.daily-supplement.jsonl` | 保留（词典数据） |
| 544K | `tests/quality/outputs/p21-golden.jsonl` | 归档 |
| 540K | `tests/quality/outputs/p20-golden.jsonl` | 归档 |
| 512K | `tests/quality/outputs/p19-golden.jsonl` | 归档 |

`translator-sdk/target/` 34 GiB，已在 `.gitignore` 中，不入库。

---

## 5. 评测集骨架

新建 `translator-sdk/fixtures/eval/`：

| 文件 | 条数 | 用途 |
|---|---:|---|
| `smoke.jsonl` | 30 | 每次改动快速验证（保护/数字/术语/基础语义） |
| `dev.jsonl` | 100 | 提示词、模型与策略研发 |

字段与使用规则见 `translator-sdk/fixtures/eval/README.md`。

**重要：** 两份数据集的 `reference_zh` 全部为 `null`。
它们是**输入**集合 + 机器可判定的完整性期望（`must_preserve` / `must_keep_numbers` / `glossary`），
**不是**人工验收过的参考译文。凡涉及"译文是否准确/自然"的结论，状态一律为 `PENDING_HUMAN`。

---

## 6. Phase 0 验收对照

| 验收项 | 状态 | 证据 |
|---|---|---|
| 原始代码可完整恢复 | ✅ | `archive/pre-online-refactor-20261111` @ `16c55b9` |
| 旧研究结果可追溯 | ✅ | 归档分支保留全部历史文档与 fixtures |
| 未对主分支做破坏性删除 | ✅ | `main` 仍指向 `16c55b9` |
| 已有测试通过情况已记录 | ✅ | 550 passed / 0 failed / 60 ignored |
| 秘密信息检查 | ✅ | 扫描 0 命中 |
| 最小 Smoke 评测集 | ✅ | `fixtures/eval/smoke.jsonl`（30 条） |
| 依赖关系与源码清单 | ✅ | 本文 §2 |

**遗留 / PENDING_HUMAN：**

- 评测集的参考译文与人工评分尚未产生（PENDING_HUMAN）。
- Windows 侧的构建与测试在本阶段只做了交叉编译检查，未在真实 Windows 机器上运行（PENDING_HUMAN）。
