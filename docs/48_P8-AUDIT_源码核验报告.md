# 48 — P8-AUDIT 源码核验报告

- 日期：2026-11-07
- 阶段：**P8-AUDIT（只读源码核验，先于 P8 开发）**
- 审计 commit：`429cc9b`（`p7.0-8-g429cc9b`；工作区干净）
- 环境：Linux x86_64，`rustc 1.99.1` / `cargo 1.99.1`
- 关联：`docs/47`（P8～P11 计划稿，用户提供）、`docs/46`（P7 验收决策）、`docs/01` ADR-063
- 一句话：**10 个工作包已逐项落到源码、现有测试和日志；没有修改功能代码。P8.1（JNI 多实例）是明确缺口，P8.2/P8.3 是 Partial，P9/P10/P11 的大功能多数是 Confirmed missing。**

> 本报告不是实现报告，也不是发行批准。所有判定以 `audit/evidence-matrix.md` 的源码行/测试为准；旧报告只用于对照，不作为当前源码证据。

---

## 1. 方法与基线

- 只读 `git status` / `git log` / 源码 / `Cargo.toml` / 现有测试；
- 未改 JNI 注册表、未新增功能、未下载模型、未触发外部 CI；
- 复跑现有测试：`cargo test --workspace` → **391 passed / 0 failed / 60 ignored**；
- 专项：cache_fingerprint 5、routing 13、lifecycle 6、cancellation 4、J-07 5、JNI lib 7，全部 PASS；
- `0ed70db..429cc9b` 对 `translator-sdk` 功能源码没有改动（仅新增 `tools/ci-preflight.py` 与文档/证据），所以 P7 的 Android 34 PASS / 1 PENDING 仍对应当前 JNI 源码。

证据：`audit/evidence-matrix.md`、`audit/commands.log`、`audit/full-test.log`。

---

## 2. 判定总表

| 工作包 | 判定 | 关键事实 | 下一步 |
|---|---|---|---|
| **P8.1 JNI 多实例** | **Confirmed missing（修复）**；单实例 Verified | `Registry.generation` 全局自增；`nativeCall` 先比较全局 generation；创建 B 会让 A 的 handle 失效；无多实例测试 | **P8 P0**：先 M-01～M-06 失败测试，再 per-entry generation/closed |
| **P8.2 共享模型/worker** | **Partial** | 每个 `TranslatorBuilder::nmt_bergamot` 新建引擎/transport；single-flight 只在单 transport 内；无 `SharedModelPool`；手动共享 `Arc` 可行但无 SDK 保证/测试 | 先做 SM-01 加载次数实验，再决定池化或明确“进程级独占” |
| **P8.3 执行计划/缓存** | **Partial** | `ResolvedExecutionPlan` 存在且 NMT 引擎级 plan cache 已用；Coordinator 的 `CacheKey` 仍是 `model_id=None`、`decode_config=default`、`privacy_domain=default`、`resource_fingerprint=id@version` | 补 EP-01/03/04 测试；是否把 plan 接入 Coordinator 单独决策 |
| **P9.1 en→zh/Kaikki/JMdict** | **Confirmed missing** | `pack-dictionary` 只解析 CC-CEDICT；仓库只有 `dictionary.cc-cedict.zh-en`（zh→en）；无 Kaikki/JMdict 解析器 | 先做资源/许可/格式评估；不要拿 CEDICT 反向索引冒充 en→zh |
| **P9.2 LID/形态** | **Partial（LID）/ Confirmed missing（形态）** | `HeuristicIdentifier` 可用；无 fastText/Lindera 依赖；`translator-rules` 明确“词形未实现” | 先 LID 对比实验；形态按语言插件单独立项 |
| **P10.1 生命周期/低内存** | **Partial** | single-flight、TTL/unload、快照清理、worker 恢复有测试；缺 1,000/10,000 次 soak、多实例内存、Android 真实内存 | 先补长时间与多实例实验，不重写现有 Manager |
| **P10.2 Android 真实 NMT** | **Confirmed missing / Blocked** | JNI 默认不含 NMT；`ProcessTransport` 只把 Linux 标 Verified；ARM64 仅交叉编译 | 先选许可合适的轻量模型与 runtime，再谈 PoC |
| **P10.3 ncnn** | **Confirmed missing** | 无 ncnn crate/依赖/工具；仅注释与 Mock 字符串 | 独立 PoC，pnnx 成功 ≠ 译文正确 |
| **P11.1 真实 Remote** | **Confirmed missing** | 无 `translator-llm`/HTTP provider；只有 `MockRemoteEngine`；生产依赖树无 HTTP 客户端 | 先做 loopback Mock Server 合同；不接真实付费 API |
| **P11.2 三引擎路由** | **Partial** | `routing::select` 与 G06 取消已验证；无真实 Remote + 真实 Local NMT + 真实 Dictionary 的同一矩阵 | 等 P11.1/P10 有真实插件后跑 R-01～R-10 |

**分类统计（按主判定）**：`Confirmed missing` **5**（P8.1 / P9.1 / P10.2 / P10.3 / P11.1）；`Partial` **5**（P8.2 / P8.3 / P9.2 / P10.1 / P11.2）——其中 P9.2 的形态部分单独为 Confirmed missing。没有整体 `Verified` 的工作包；横向能力（JNI v1、G06、G08、文本安全、平台门禁）有 Verified 证据。为避免“用数字掩盖差异”，以本表逐项为准。

---

## 3. 关键发现

### 3.1 P8.1 是明确、可复现的缺口

源码事实（`bindings/translator-jni/src/lib.rs`）：

- `Registry { generation, next_index, entries }`；
- `nativeCreate`：`reg.generation += 1`，handle = `"{generation}:{index}"`；
- `nativeCall`：`if reg.generation != gen { AlreadyClosed }`；
- `nativeClose`：同样按全局 generation 决定是否删除。

因此两个独立 `Translator` 不能同时存活；这不是“猜测”，是当前实现的结构性行为。ADR-063 仍是 **Proposed**，M-01～M-06 尚无实现/测试。

### 3.2 P8.2 的“共享”目前只是手动 Arc，不是 SDK 能力

- `TranslatorBuilder::nmt_bergamot` 每次新建 `BergamotNmtEngine`，各自持有 `ProcessTransport`、`load_gate`、`PlanCache`；
- 两个实例会各 spawn 一个 worker，各自占 native 稳态内存；
- 资源租约只覆盖词典/规则目录，NMT 模型目录没有租约；
- 宿主可手动把同一个 `Arc<dyn TranslationEngine>` 传给两个 builder，但没有按 `(runtime, model digest, tokenizer, device)` 去重的公共池，也没有 SM-01 加载次数测试。

### 3.3 P8.3 的 plan 已存在，但没有完整进 Coordinator 缓存

- `ResolvedExecutionPlan` 字段完整（engine/runtime/build/model_revision/五文件/语言/beam/threads/protection/rules/glossary/pipeline/privacy）；
- Bergamot 引擎级 `PlanCache` 用 plan digest；但构造 plan 时 `rules_version=None`、`glossary_digest=None`、`privacy_domain="local"` 固定；
- Coordinator 的 `CacheKey` 仍是 `model_id=None`、`decode_config="default"`、`privacy_domain="default"`，资源指纹只是 `id@version`；
- 现有测试只证明“id@version 变化 → 重新执行”，未证明“同 id@version 不同内容”在 Coordinator 层失效。

### 3.4 P9/P10/P11 的多数“大功能”没有实现证据

- 只有 CC-CEDICT zh→en 词库；无 en→zh、Kaikki、JMdict；
- 只有启发式 LID；无 fastText/Lindera；
- Android 只有 JNI + 词典 + Mock 路由；无真实 NMT；
- 无 ncnn；
- 无真实 Remote Provider；只有 `MockRemoteEngine`。

这四项都不能按旧报告当作“已实现未测”。

---

## 4. 建议的下一步（P8 入口）

1. **先写 P8.1 失败测试**（M-01～M-06），确认当前全局 generation 行为；保持 JNI 四入口/handle 格式/JSON 信封不变。
2. 再改 `Registry`/`Entry`：per-entry generation + closed；`index` 不复用；`nativeClose(A)` 只关 A；CI/Android 跑 M-01～M-06 + P6 J-01/J-03/J-06/J-15a/b 回归。
3. 通过后再把 `capabilities.binding.multi_instance=true` 打开，并把 ADR-063 从 Proposed 改 Accepted。
4. P8.2/P8.3 在 P8.1 之后用 SM/EP 合同逐条验证；若现有实现已等价（例如手动 Arc 共享），只补测试与文档，不新建重复抽象。
5. P9/P10/P11 按 `docs/47` 的 A0→A1→A2→B1→C1→C2→C3→D1→D2 顺序推进；P9.1 与 P11.1 可在 P8 审计后独立开始。

---

## 5. 本轮未做 / 明确边界

- 未修改任何功能代码；未新增测试；未改 JNI ABI；未改 `Cargo.toml` 依赖。
- 未下载/分发模型；未调用真实远程 API；未 push GitHub、未触发外部 CI。
- 未把旧 P3/P4/P7 报告当作当前源码证据；所有“已实现”判断都有源码/测试路径。
- P10.2 的 Blocked 条件：缺少许可合适、可在 x86_64 AVD 运行的轻量 NMT 模型与 runtime；不是“代码写好了没跑”。
