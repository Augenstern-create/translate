# P8-AUDIT 证据矩阵

> **P8 后续（2026-11-07）**：本矩阵是 P8-AUDIT 时点（`429cc9b`）的只读结论；随后 P8 已实现，
> P8.1 从 Confirmed missing 变为已实现，P8.2/P8.3 已补共享池与缓存身份。当前结果见 `docs/49`。
> 本文件保留审计原貌，不按实现结果改写历史判定。

- 审计日期：2026-11-07
- 审计 commit：`429cc9b`（`p7.0-8-g429cc9b`，工作区干净）
- 环境：Linux x86_64；`rustc 1.99.1` / `cargo 1.99.1`
- 方法：只读 `git` / 源码 / 现有测试；**未修改任何功能代码、未新增测试、未下载模型**
- 判据：`Verified` / `Partial` / `Confirmed missing` / `Blocked` / `Deferred by design`
- 重要前提：`0ed70db..429cc9b` 只改了文档、CI 预检与证据文件；`translator-sdk` 功能源码未变（`git diff --stat` 仅新增 `tools/ci-preflight.py`）。因此 P7 的 391/0/60、Android 34/0/1 仍对应当前功能源码。

## 0. 基线复跑（本轮）

| 命令 | 结果 | 日志 |
|---|---|---|
| `cargo test --workspace` | **391 passed / 0 failed / 60 ignored** | `audit/commands.log` |
| `cargo test -p translator-core --test cache_fingerprint` | 5 passed | 同上 |
| `cargo test -p translator-core --test routing` | 13 passed | 同上 |
| `cargo test -p translator-runtime --test lifecycle` | 6 passed | 同上 |
| `cargo test -p translator-runtime --test cancellation` | 4 passed | 同上 |
| `cargo test -p translator-resource --test p71_j07_security` | 5 passed | 同上 |
| `cargo test -p translator-jni --lib` | 7 passed | 同上 |

---

## 1. 十个工作包的源码判定

### P8.1 JNI per-entry generation 多实例 — **Confirmed missing（修复）**

| 证据 | 位置 |
|---|---|
| 注册表只有全局 `generation`；`nativeCreate` 每次 `reg.generation += 1` | `bindings/translator-jni/src/lib.rs:67-70`, `:553-565` |
| `nativeCall` 先比较 `reg.generation != gen`，再查 index | `:609` |
| `nativeClose` 同样按全局 generation 决定是否删除 | `:672` |
| `capabilities` 目前只有 `platform/engines/runtimes`，无 `binding.multi_instance` | `:183-207` |
| 现有 JNI 单测只覆盖 handle 解析/信封/平台名，没有多实例 | `:700+`（7 个 lib tests） |
| Android Harness 的 J-07 为避免创建 h3 提前作废旧 h2，只能复用 h2 | `tests/android-harness/src/ai/ridge/translator/Harness.java`（J-07 段注释） |

**结论**：单实例 create/call/close 已有 P6/P7 证据（Verified）；**多实例共存是明确缺口**，不是猜测。ADR-063 已写但仍是 Proposed，M-01～M-06 尚无实现/测试。
**风险**：任何需要两个 Translator 并存的宿主都会遇到 `AlreadyClosed`；JNI 是 ABI 边界，改动必须先补合同测试。

### P8.2 多 Translator 共享模型/worker — **Partial**

| 证据 | 位置 |
|---|---|
| `TranslatorBuilder::nmt_bergamot` 每次都 `BergamotNmtEngine::new` 并注册一个新 `Arc` | `crates/translator-api/src/lib.rs:360-385` |
| `BergamotNmtEngine` 自带 `ProcessTransport`、`load_gate`、`PlanCache`；加载是**每个引擎实例内**的 single-flight | `crates/translator-nmt-bergamot/src/lib.rs:172-182`, `:220-250` |
| `ProcessTransport::load_from_manifest` 的 `load_gate` 只覆盖同一个 transport | `crates/translator-runtime/src/transport.rs:67-69`, `:223-225` |
| 资源租约只服务词典/规则目录（`_leases: Vec<ResourceLease>`），NMT 模型目录不登记租约 | `crates/translator-api/src/lib.rs:58-62`, `dictionary_from_store` 段 |
| 没有 `SharedModelPool`/按 content digest 去重的公共池；`PluginRegistry` 只持有 `Arc<dyn TranslationEngine>` | `crates/translator-plugin-api/src/lib.rs:99-120` |
| 宿主可手动把同一个 `Arc<BergamotNmtEngine>` 传给两个 builder（`builder.engine`），但没有“按 digest 自动复用”的 API/测试 | `crates/translator-api/src/lib.rs:385` |

**结论**：单实例生命周期、TTL/unload、single-flight 已有证据；**跨实例共享/去重没有实现或测试**。手动共享 `Arc` 可行但不是 SDK 保证，也没有加载次数/租约合同。
**风险**：两个实例会各自 spawn worker、各自占 ~341 MiB 稳态 RSS；SM-01～SM-06 不能标 PASS。

### P8.3 ResolvedExecutionPlan / cache 一致性 — **Partial**

| 证据 | 位置 |
|---|---|
| `ResolvedExecutionPlan` 存在，字段含 engine/runtime/build/model_revision/五文件摘要/语言/beam/threads/protection/rules/glossary/pipeline/privacy | `crates/translator-core/src/plan.rs:23-45` |
| Bergamot 用 `plan.digest()` 作引擎级缓存键；plan 构造时 `rules_version=None`、`glossary_digest=None`、`privacy_domain="local"` 固定 | `crates/translator-nmt-bergamot/src/lib.rs:351-378` |
| 引擎级 `PlanCache` 只缓存成功结果，失败不缓存 | `crates/translator-core/src/plan_cache.rs:1-30` |
| Coordinator 的 `CacheKey` 仍有 `model_id: None`、`decode_config="default"`、`privacy_domain="default"`、`resource_fingerprint` | `crates/translator-core/src/cache.rs:31-48`, `:71-72` |
| Coordinator 只用 `engine_id + resource_fingerprint + rules_version` 组键，没有使用 `ResolvedExecutionPlan` | `crates/translator-core/src/coordinator.rs:370-378` |
| `resource_fingerprint` 只拼 `id@version`（词典/规则/术语），不是内容摘要 | `crates/translator-api/src/lib.rs:590-648` |
| `content_digest` 只取主模型 sha256 前 32 位 + package@version | `crates/translator-runtime/src/model.rs:180-190` |
| 现有测试覆盖“资源 id@version 变化 → 重新执行”，未覆盖“同 id@version 不同内容”在 Coordinator 层失效 | `crates/translator-core/tests/cache_fingerprint.rs` |
| Plan 单测覆盖字段变化改 digest；`p32_cache`（ignored，需真实模型）覆盖五文件变化 | `crates/translator-core/src/plan.rs:180+`; `crates/translator-nmt-bergamot/tests/p32_cache.rs` |

**结论**：NMT 引擎级 plan cache 是 Verified（有真实模型测试但标记 ignored）；**Coordinator 级 CacheKey 没有接入完整 plan**，EP-01/EP-03/EP-04 仍是缺口或只有部分证据；EP-05（失败不缓存）代码上成立，EP-06 资源 v1/v2 有既有测试。
**风险**：同一 engine_id 下换模型内容/解码/隐私域时，Coordinator 缓存可能命中旧结果；不能宣称“完整执行计划已进所有缓存”。

### P9.1 en→zh / Kaikki / JMdict 词库 — **Confirmed missing（导入器与 en→zh 包）**

| 证据 | 位置/事实 |
|---|---|
| `pack-dictionary` 只解析 CC-CEDICT 行格式（`parse_cedict` / `parse_line`），无 JSONL/XML 导入 | `tools/pack-dictionary/src/lib.rs:65-120` |
| 现有资源只有一个 `dictionary.cc-cedict.zh-en`（`language=zh-Hans`, `target_languages=["en"]`, 121,584 条） | `resources/zh-en/manifest.json` |
| 无 Kaikki/JMdict 解析器；仓库无对应 crate/工具 | 工作区 `tools/`、`Cargo.toml` members |
| `fixtures/golden/en_zh_basic.tsv` 只是测试语料，不是可安装词典包 | `fixtures/golden/en_zh_basic.tsv` |

**结论**：CC-CEDICT zh→en 已 Verified（P1/P7）；**en→zh 词典、Kaikki、JMdict 导入器均缺失**。DI-01～DI-06 无可执行实现。
**风险**：不能把 CEDICT 反向索引冒充 en→zh 词典；需要真实来源/许可/质量样本。

### P9.2 正式 LID / 词形分析 — **Partial（LID）/ Confirmed missing（形态）**

| 证据 | 位置 |
|---|---|
| `HeuristicIdentifier` 基于 Unicode 文字系统，返回多候选；明确自述“不是产品级 LID” | `crates/translator-analysis/src/lib.rs:31-108` |
| `translator-analysis` 只依赖 `translator-types`，无 fastText/Lindera 依赖 | `crates/translator-analysis/Cargo.toml` |
| `translator-core::text::guess_language` 直接调用 `translator_analysis::guess`，没有注入 `LanguageIdentifier` 的公共配置 | `crates/translator-core/src/text.rs:73-81` |
| 规则包明确写“未实现：分句、词形（需要 Lindera 与词典授权）” | `crates/translator-rules/src/lib.rs:19` |
| 没有 `lemma`/`stem`/morphology 实现或依赖 | 全仓 `grep -rni "lemma|stem|morpholog"` 无实现命中 |

**结论**：启发式 LID Verified（LID-01 部分可测）；fastText/正式 LID **未接入**；形态分析 **Confirmed missing**。
**风险**：短文本多候选语义已有，但不能宣称 LID-02 混淆矩阵/F1；日语词形检索未实现。

### P10.1 Bergamot/ORT 生命周期与低内存 — **Partial**

| 证据 | 位置 |
|---|---|
| worker 生命周期：`ProcessTransport` 的 spawn/respawn/terminate/快照清理、`active_requests`、deadline | `crates/translator-runtime/src/transport.rs`, `worker.rs` |
| LC-01 并发 10 次加载只 spawn 一次；LC-02 在飞 unload；LC-06 10 轮加载/卸载 | `crates/translator-runtime/tests/lifecycle.rs`（6 passed） |
| Bergamot TTL 空闲回收、显式 unload、`LoadState`、缓存统计 | `crates/translator-nmt-bergamot/src/lib.rs:237-335` |
| ORT `load/unload` 幂等、drop 会话释放 arena | `crates/translator-nmt-ort/src/lib.rs:258-282` |
| ORT 10 轮加载/卸载 RSS 断言 | `crates/translator-nmt-ort/tests/p4_contract.rs:298-320` |
| 缺少 1,000/10,000 次长时间 soak、跨实例内存实验、真实 Android 内存 | 仓库无对应测试 |
| 内存统计有 `process_rss_bytes`、worker/native RSS；`MemoryReport` 是预算下界，不是 RSS | `crates/translator-runtime/src/model.rs:441+`; `crates/translator-core/src/budget.rs:249` |

**结论**：单实例生命周期/卸载/错误恢复 Verified；**长时间、多实例、跨 runtime 内存在 P10 口径下仍是 Partial**。
**风险**：不能以 413.8 MiB 历史值推断当前版本或 Android；需要新的同机基线。

### P10.2 Android 真实小型 NMT — **Confirmed missing / Blocked**

| 证据 | 位置 |
|---|---|
| `translator-jni` 默认 features 为空；`nmt-bergamot`/`nmt-ort` 只是可选透传 | `bindings/translator-jni/Cargo.toml:8-14` |
| JNI `capabilities` 用 `cfg!(feature=...)` 报告 `not_compiled` | `bindings/translator-jni/src/lib.rs:199-204` |
| `ProcessTransport` 平台门禁只把 Linux 标为 Verified；Android 返回 `CapabilityUnavailable` | `crates/translator-runtime/src/platform.rs:18-70` |
| Android 证据只有 x86_64 AVD 的 JNI + 词典 + Mock 路由（无真实 NMT） | `docs/44` §2.2、`docs/45` 未涉及 |
| ARM64 仍只交叉编译；无 ARM64 运行宿主 | `docs/37`/`docs/44` |

**结论**：Android 真实 NMT **没有实现/构建证据**；P10.2 的 AND-NMT-01～08 全部无法执行。不是“已实现但未测”。
**风险**：需要先选许可合适的轻量模型与可移植 runtime；不能把 x86_64 Mock 或 Linux Bergamot 结果外推。

### P10.3 ncnn 转换与同机对照 — **Confirmed missing**

| 证据 | 位置 |
|---|---|
| 无 ncnn crate/依赖/工具；`Cargo.toml` 无 ncnn | 工作区 `Cargo.toml`/`Cargo.lock` |
| 仅注释提到 ncnn（“将来 ncnn/ORT 可以是另一种实现”） | `crates/translator-runtime/src/transport.rs:3` |
| `MockBackend` 用 `"ncnn-param"` 字符串模拟架构，不是 ncnn 实现 | `crates/translator-plugin-api/src/mocks.rs:466` |

**结论**：ncnn PoC **未开始**；NC-01～NC-05 无实现。
**风险**：pnnx 转换成功不等于译文正确；需要独立 PoC 与 Go/No-Go。

### P11.1 真实 Remote Provider — **Confirmed missing**

| 证据 | 位置 |
|---|---|
| 工作区没有 `translator-llm`/HTTP provider crate | `Cargo.toml` members |
| 唯一远程实现是 `MockRemoteEngine`，只检查 `network_consent` 并返回 `NetworkNotAuthorized` | `crates/translator-plugin-api/src/mocks.rs:182-240` |
| 生产依赖树无 HTTP 客户端（`translator-api --no-default-features` 无 reqwest/hyper/ureq） | `cargo tree`（P7 C2） |
| JNI/CLI 只透传 `RemoteOnly` 策略，没有真实 endpoint/credential 配置 | `apps/translator-cli/src/main.rs`, `bindings/translator-jni/src/lib.rs` |

**结论**：真实 Remote Provider **未实现**；RM-01（未授权拒绝）有 Mock 合同，RM-02～07 无实现。P11.1 不是“已有但没测”。
**风险**：不要用 Mock 译文冒充真实 HTTP；loopback Mock Server 只能验证协议，不能证明供应商兼容。

### P11.2 三引擎自动路由与取消 — **Partial**

| 证据 | 位置 |
|---|---|
| `routing::select` 实现 DictionaryOnly/LocalOnly/RemoteOnly/AutoLocalFirst；AutoLocalFirst 不自动回退远程 | `crates/translator-core/src/routing.rs:69-170` |
| 词典未命中回退 NMT 有测试；RemoteOnly 未授权拒绝有测试 | `crates/translator-core/tests/routing.rs`（13 passed） |
| 取消合同 G06 已有 Linux 4 条测试；JNI 预取消有 Android G06-JNI | `crates/translator-runtime/tests/cancellation.rs`; `docs/44` |
| 缺少真实 Remote + 真实 Local NMT + 真实 Dictionary 的同一矩阵 R-01～R-10 | 无三引擎集成测试 |

**结论**：路由/取消核心 Verified；**三引擎真实闭环（尤其 Remote）Partial/Blocked**。
**风险**：R-05 只能用 Mock；R-06 的 HTTP 取消未实现；不能把路由单测当三引擎验收。

---

## 2. 横向能力

| 能力 | 判定 | 证据 |
|---|---|---|
| JNI v1 四入口/handle 格式/JSON 信封 | **Verified** | `bindings/translator-jni/src/lib.rs`；7 lib tests + Android 34 PASS |
| G06 取消（排队/在飞/进程组） | **Verified（Linux）**；Windows N/A | `crates/translator-runtime/tests/cancellation.rs` 4 passed；`docs/45` G06 0 tests |
| G08 外部 kill 恢复 | **Verified（Linux signal=9；Windows TerminateProcess）** | `crates/translator-resource/tests/p32_lifecycle.rs`；`docs/45` 1 passed |
| 文本保护/还原/术语 | **Verified** | `translator-core/src/protection.rs`、`text.rs`；P3/P4/P7 tests |
| 平台门禁 | **Verified** | `translator-runtime/src/platform.rs` 单测；Windows 传输层 N/A |
| 资源坏包/穿越 | **Verified** | `p71_j07_security.rs` 5 passed |
| 单实例 NMT 生命周期 | **Verified（Linux）** | `lifecycle.rs` 6 passed；Bergamot/ORT 需真实模型用例 |
| 多实例/共享模型 | **Confirmed missing / Partial** | 见 P8.1/P8.2 |
| 真实 Remote | **Confirmed missing** | 见 P11.1 |

---

## 3. 结论与下一轮入口

1. **P8-AUDIT 完成**：10 个工作包已逐项落到源码/测试/判定；没有修改功能代码。
2. **最高优先级仍是 P8.1**：当前全局 generation 会让第二个 Translator 作废第一个句柄；先写 M-01～M-06 失败测试，再改注册表。ADR-063 仍是 Proposed，不能提前标 Accepted。
3. **P8.2/P8.3 是 Partial**：单实例生命周期和 NMT 引擎级 plan cache 已存在；跨实例共享池与 Coordinator 级完整 plan/cache 是缺口或仅部分证据。
4. **P9/P10/P11 的“大功能”多数是 Confirmed missing**：en→zh/Kaikki/JMdict、正式 LID/形态、Android 真实 NMT、ncnn、真实 Remote 都没有实现证据。不要按旧报告把它们当成已完成。
5. **不要在 P8-AUDIT 阶段改 JNI 注册表**：先补失败测试与 ADR 评审，再按 P8.1 步骤实施。

证据命令与退出码：`audit/commands.log`。源码路径均相对 `translator-sdk/`。
