> **归档说明**：本文是外部输入文档的原样归档（2026-11-05 收到），标题沿用其自身的 `10 —` 编号。
> 归档只做排版（统一标题层级、去除对话系统的包裹符号），**不改动任何结论、数字与验收判据**。
> 阅读约定：
> - 文中的状态（"建议 / 待实施"）、性能与完成度均为**该文档撰写方的表述**，不是本仓库的自证；
> - 文中的 crate 名（`runtime-bergamot`、`translator-nmt`、`translator-platform`）与文件路径是**建议**，实施时可以选择不拆；
> - 文中的 ADR 标题是**候选**，并入 `docs/01` 前必须核对已占用编号（`ADR-001`～`ADR-023` 已占用，见 `01` §14）；
> - 本文档要求的 P3.0 证据（模型 revision、原生参考链路、真实译文、RSS 基线）**尚未全部产出**，进展与失败记录见 `11_…`（实施时创建）。

# 10 — P3 Linux 本地 NMT 开发建议与验收计划

> 状态：**建议 / 待实施**，不代表功能已经完成
> 日期：2026-11-05
> 适用项目：轻量跨平台混合翻译 SDK
> 当前主要目标：**Ubuntu 24.04 x86_64 / CPU 优先，先交付完整 Linux SDK；为 Windows、macOS、Android 预留接口，暂不承担其构建及性能验收**
> 依据：`01_翻译SDK完整开发设计`、`02_翻译资源候选与备选清单`、`07_P2.3-P2.5_租约预算与术语约束报告`、`08_P2完整技术审查与P3开发建议`、`09_P2-S稳定化报告`、`rust_env_setup.md`

## 0. 决策摘要

1. **P3 的主目标是第一条真实、可重复的离线整句翻译链路**：经公开 `Translator::translate()` API，走 `Local NMT` 插件，在 Ubuntu CPU 上加载经核验授权的 Marian/Firefox 语言对模型，返回真实译文，不再由 Mock 生成。
2. **首个参考运行时优先 Bergamot / Marian**；ncnn、ORT、Burn 保留为可替换后端，不在第一条真实翻译完成前同时攻克。Bergamot 的 Linux 原生构建、具体语言方向、模型文件格式和授权均为 **P3.0 的验证项**，不是既成事实。
3. **继续 UI 与核心隔离**：`translator-api` 依赖稳定契约；`runtime-bergamot` 是有特性开关的独立 crate；不得把 C++ 类型、Ubuntu 系统路径、Tokenizer 私有实现泄漏到公共 API。
4. **P3.1 与首个真实后端一起实现** P2-S 留下的 single-flight、模型实例租约、`ResolvedExecutionPlan`、资源 revision/内容摘要隔离，并补上依赖反向引用和关键崩溃注入。
5. **Linux 端实际低内存测试是强制验收项目**。区分权重文件大小、空闲 RSS、热模型 RSS、推理峰值 RSS、冷/热延迟和模型卸载后 RSS；预算是软准入，不承诺内存硬上限。
6. **其他平台暂不发布、暂不测试通过**。保留平台抽象和 build feature，不能把 Linux 测试结果写成跨平台结论。

### 0.1 已有基础（来自报告，不是本次源码复核）

- P0/P1 已建立 `translator-types`、`translator-plugin-api`、`translator-core`、`translator-api`，实现字典/规则与 Mock NMT 路由。
- P2 已有资源 Manifest 双版本、仓库安装/激活/回滚、协作式租约、记账式内存预算、诊断、术语约束和 `dictionary.sqlite.v2`。
- P2-S 报告记载 **244 passed / 0 failed / 9 ignored**，修复了跨进程租约/卸载竞态、PID 复用判断、缓存指纹回归测试；同时承认 Windows/macOS/Android 尚未验证。
- **仍然 pending**：真实 NMT、真实加载生命周期、完整执行计划、内容 revision、实际 RSS 指标、依赖反向引用、安装断电一致性及关键故障注入。详见第 8 节。

## 1. 范围与非目标

### 1.1 本阶段必须实现

- Ubuntu x86_64 CPU 上的真实离线文本翻译，至少完成一个**已核实可用的方向**（例如 en→zh 或官方注册表实际可获得的其他语言对）；第二方向是扩展验收，不阻塞首条链路。
- 一次模型加载、多次请求复用、安全卸载；真实 Tokenizer/词表与模型一起绑定，避免文字输入配置不一致。
- `AutoLocalFirst`、`LocalOnly`、`ExplicitEngine` 与真实 NMT 整合；无网络授权时永不走 Remote LLM。
- 安装模型包 → 资源检查 → 执行计划解析 → NMT 实例获取 → 推理 → 输出与诊断 → 安全释放，形成可测试的闭环。
- Linux 本机延迟、内存、并发、取消/超时、异常恢复测试与基线报告。

### 1.2 明确不做

- 不实现 UI、划词、OCR、语音或 Android JNI；不在 P3 主线上开发 macOS/Windows 产物。
- 不自研或训练大型翻译模型；不在第一步实现多语言路径自动中转和全自动模型推荐。
- 不承诺 ncnn 能直接运行 Firefox 模型；仅在后续对照阶段验证转换与 seq2seq 解码兼容性。
- 不把线程中 `tokio::timeout` 返回视为原生 C++ 同步推理已被杀死；不做未经验证的强制取消。

## 2. 建议阶段划分与准入门槛

| 阶段 | 目标 | 强制交付 | 进入下一阶段的门槛 |
|---|---|---|---|
| **P3.0 模型和 Linux 原生基线** | 确认模型与参考运行时确实可用 | 锁定模型 revision/授权/校验和，原生 CLI 能真实翻译，记录冷/热基线 | 译文真实、断网可重复、参考命令可复现 |
| **P3.1 Rust FFI / 插件接入** | 经 `translator-api` 完成真实翻译 | `runtime-bergamot` adapter、线程模型、`LoadedModel`、构建 feature、错误映射 | 核心不依赖 Bergamot 私有类型；插件契约测试通过 |
| **P3.2 生命周期与一致性** | 模型并发、租约、执行计划、仓库安全 | single-flight、模型租约、执行计划指纹、反向依赖、故障注入 | 同一模型并发不重复加载；卸载不悬挂/不野指针 |
| **P3.3 Linux 性能与质量** | 验证低内存和实用译质 | release 基准、金样本、量化/线程配置对照、预算及回收结果 | 产出诚实报告：达标或明确失败及替代路线 |
| **P3.4 Linux SDK 发布候选** | 从资源包到 API/CLI 的完整交付 | Linux 安装说明、版本锁定、许可证、示例、CI、发行检查表 | 不联网可运行；原有 P0–P2-S 测试全绿 |

**实施优先级**：P3.0 是真实性验证；P3.1 才开始绑定 API；P3.2 解决资源安全；P3.3 决定是否切换到 ncnn；P3.4 输出 Linux-only 可交付物。必要时 P3.1 和 P3.2 的接口设计同步，但必须分别验收。

## 3. P3.0：模型与 Bergamot 参考链路

### 3.1 模型选择流程

1. 从 Mozilla 官方当前翻译模型注册表查找目标语言方向，核对**确切模型 id、质量档（tiny/base-memory/base）、依赖词表、文件格式、许可证与来源 revision**。若不存在合格方向，不强行使用"近似"模型。
2. 检查 Bergamot 原生实现可否加载该完整模型包；若原生参考链路不适配，评估适用的 Marian/OPUS-MT + 对应参考运行时。
3. 固定 `model_id + revision + tokenizer + runtime_version + sha256`；冻结一套最小输入样本：问候、数字、否定、专有名词、占位符、长句、空文本。
4. 在 Linux CLI 中离线执行翻译并记录输出、运行时间、RSS；**只有参考链路成功**才开发 Rust FFI。

> 不在本文件臆造具体模型仓库路径、模型文件名或下载体积。P3.0 必须填写实测模型清单（第 11 节）后才可宣布选型完成。

### 3.2 Linux 构建要求

- 使用独立固定版本的 C++ 构建：`CMake` + `Ninja`，固定 Bergamot/Marian Git commit，固定子模块 revision；启用 CPU 最小功能集。
- `runtime-bergamot` 通过 `build.rs` 或明确的本地构建产物链接；**不要**运行时依赖 Python、PyTorch 或联网模型下载。
- 构建脚本不得写死 `/home/nan/...`、`/opt/rust/...`；这些仅属于当前开发机的环境文档。优先环境变量/构建配置，并用 `cargo:rerun-if-*` 管理重构建。
- 若使用 `bindgen`，限制 C ABI 边界，尽可能自建薄 C 接口包装 C++ 类，减少编译器/ABI 差异与所有权不确定性。
- 对 Linux 最小发布产物执行 `ldd`/运行库检查，确认没有意外的重型运行依赖。

### 3.3 P3.0 验收证据

- 原生 CLI 对至少 20 条固定句子返回真实可读译文，支持 `--offline` 或断网环境运行。
- 同一模型与配置连续 100 次推理无崩溃/错误，输出可重复或差异有解释。
- 提交 `models.lock`/Manifest（revision、checksum、license、runtime commit）以及无密钥复现命令。
- 记录冷启动、首次翻译、热翻译 p50/p95、进程峰值 RSS；这些是观测结果，不是平台承诺。

## 4. P3.1：Rust API 与 NMT 插件接入

### 4.1 依赖与 crate 边界

```text
translator-api
   └── translator-core + translator-plugin-api + translator-types
            ↑ 插件只实现契约
runtime-bergamot (feature = runtime-bergamot)
   └── bergamot-sys / thin C ABI
translator-resource
   └── model manifest / snapshot / lease
```

- `translator-api` 默认编译不要求 C++、模型文件或网络；启用 `runtime-bergamot` feature 后才链接原生 runtime。
- `TranslationEngine` 抽象业务翻译；`InferenceBackend` 抽象某种模型格式的执行；`LoadedModel` 抽象受控的已加载实例。**翻译能力和底层运行时不可混为一个类型。**
- 如果 C++ 句柄不是线程安全的，把真实实例封装在**专属 worker/actor** 中，由插件暴露 `Send + Sync` 的消息代理；禁止无依据地给裸指针 `unsafe impl Send/Sync`。

### 4.2 推荐内部接口草案（须在编码时根据现有 trait 适配）

```rust
// 内部模型标识，不能仅靠名字和版本代表内容
pub struct ModelRevision {
    pub package_id: String,
    pub version: String,
    pub content_digest: [u8; 32],
}

pub struct ResolvedExecutionPlan {
    pub engine_id: String,
    pub backend_id: String,
    pub model: ModelRevision,
    pub tokenizer_digest: [u8; 32],
    pub source_lang: String,
    pub target_lang: String,
    pub decode_profile_digest: [u8; 32],
    pub rules_digest: Option<[u8; 32]>,
    pub glossary_digest: Option<[u8; 32]>,
    pub privacy_scope: String,
}

// 示例：保留使用方稳定 API，执行计划归 core 实现
// translator.translate(request).await?;
```

此草案**不是已经编译的接口**。应从既有类型做最小增量扩展；公开结构体/枚举的破坏性修改必须写迁移说明。

### 4.3 FFI 错误与生命周期

- Native wrapper 负责把 C++ 异常转成显式错误码，不得让异常或 Rust panic 穿过 FFI。
- 对外返回文本统一使用长度明确的 UTF-8 buffer；由创建一侧负责释放；不跨边界裸传 Rust `String`/`Vec`。
- 加载、执行、卸载必须有唯一所有权；执行中不得释放 decoder/encoder/模型权重，Drop 只能触发安全关闭流程。
- `RequestTimeout` 表示"客户端等待期限已到"；同步 native 执行若不支持安全中断，应在受控 worker 中继续完成或进入可观察的 draining 状态，不能释放其正在使用的内存。
- 使用现有 `ResourceNotInstalled`、`RuntimeInitFailed`、`BackendFailed`、`OutOfBudget` 等错误类别，保留真实 native 错误细节到**脱敏诊断**，不记录原文。

### 4.4 翻译行为回归

- `AutoLocalFirst`：单词精确匹配依旧允许词典优先；整句优先本地 NMT。
- `LocalOnly`：缺模型时明确报错，不得静默走字典或 Remote。
- `RemoteOnly`：必须独立检查网络许可，不受 NMT 可用性影响。
- `translate_with_diagnostics()` 与 `translate()` 调用同一核心路径，保持同一译文；结构化诊断需标记实际模型 revision、是否命中缓存及术语警告。
- 占位符保护沿用 P2 的安全扫描器；**P3 才用真实模型决定是否升级为跨段标记或分段翻译**，改动前提供 A/B 金样本。

## 5. P3.2：Resource Manager 的真实模型生命周期

### 5.1 原则

`Package Installed` ≠ `Model Loaded` ≠ `Model InUse`；安装状态、模型内存状态和请求生命周期必须分别建模。

```text
Installed / Unloaded
        ↓ load (single-flight)
Loading  ──failure──> Failed (backoff)
        ↓
Ready / Idle  ←── inference finished ──  InUse
        │                                     ↑ lease
        └── TTL / pressure / explicit unload ───┘
                   仅当活跃模型租约为 0 才真正释放
```

### 5.2 要求

1. **single-flight**：同一 `ModelRevision + backend + device + precision` 的并发加载只启动一次；所有等待者观察到同一结果，失败有明确重试/冷却策略。避免全局锁跨越重量级 native load。
2. **双层租约**：磁盘资源包租约保护文件不被删除；内存模型实例租约保护 native handle 不被销毁。两者必须有一致的锁顺序和生命周期。
3. **revision 隔离**：包版本相同但 hash 不同，不能当作相同模型；缓存、加载复用和诊断均使用 content digest。对已安装目录的篡改按验证策略拒绝/报警。
4. **依赖反向引用**：卸载模型/tokenizer/词典前查询已安装其他包的固定版本依赖；存在引用时阻止删除或提供显式破坏性操作，不允许静默破坏。
5. **安装崩溃测试**：复制、校验、目录 rename、state 写入前后设置故障注入点；`kill -9` 后检查 `audit/recover` 的行为。Linux 需补齐必要的文件/父目录 `fsync` 或明确降级的耐久性保证。
6. **预算**：加载前估算权重 + 工作区 + KV Cache + native 额外内存；允许软预算拒绝加载；模型实际占用和峰值由独立平台指标源报告。若没有实测 profile，不得填假精确值。
7. **回收策略**：默认单模型常驻、推理并发 1（可配置）；TTL/LRU 先做可观测的简单实现，推理中不可回收，空闲释放后检查 RSS 变化。
8. **取消/超时**：对等待排队/加载的请求及时取消；不可安全中断的 native 推理按 draining 处理，避免虚假的"已停止执行"。

### 5.3 Linux 指标接口

```rust
pub trait ProcessMetricsProvider: Send + Sync {
    fn snapshot(&self) -> Result<ProcessMemorySnapshot, MetricsError>;
}
```

第一版 Linux 实现可使用 `/proc/self/status`、`/proc/self/smaps_rollup`（可读时）和 `getrusage` 等来源；至少报告 RSS、峰值 RSS、时间戳、采集方法。PSS/匿名与文件映射分量只有数据源可靠时才暴露。该 trait 放在 `translator-platform` 或独立指标适配 crate，不让 `translator-core` 直接解析 `/proc`。

## 6. P3.3：真实翻译质量、延迟与内存的基线

### 6.1 测试分层

| 类别 | 测试样本 | 指标 |
|---|---|---|
| 基础正确性 | 问候、陈述、否定、数字、日期、时态、术语 | 可读性、语义保真、关键事实错误 |
| 语言/分词 | 中文、英文、标点、混合脚本、显式源语言 | 语言方向、Tokenizer/EOS 正确性 |
| 保护规则 | URL、电子邮件、`{name}`、反引号代码、多保护跨度 | 未丢失关键占位符；重要标记零破坏 |
| 长文本 | 20/100/500 字及超长输入 | 分句/截断、时延、峰值内存 |
| 模型实例 | 连续 100/1000 次、10 并发、切换语言方向 | 加载次数、线程数、崩溃/泄漏 |
| 中断和错误 | 缺权重、坏校验和、退出时取消、磁盘满、kill -9 | 明确错误、可恢复、无悬挂 |

量化模型与未量化参考链路必须用同一批文本、同一解码设置对照；若模型来源不同，只能比较端到端产品方案，不能将差异归因于推理框架。

### 6.2 推荐测量状态

1. **空 SDK**：不带字典、不带真实 runtime feature。
2. **字典 Ready**：真实词库挂载、模型未加载。
3. **native runtime 已初始化**：模型未装入。
4. **冷加载模型**：从 `load()` 到 Ready。
5. **热翻译**：同一模型连续执行，记录 p50/p95/p99。
6. **翻译峰值**：短句、长句、并发请求分别记录 RSS/峰值 RSS。
7. **空闲卸载后**：等待 TTL 或显式卸载，采集 RSS；解释 allocator、页缓存未必立刻下降。

**参考目标而非保证**：空 SDK/词典维持 P2 已定预算；Lite NMT 的额外 RAM 初始目标为桌面约 300–400 MB。如果真模型超出，记录失败并分析权重、工作空间和量化，不得为了"过线"省略峰值内存。

### 6.3 性能记录格式（建议）

```json
{
  "status": "measured_or_pending",
  "os": "Ubuntu 24.04",
  "arch": "x86_64",
  "cpu": "RECORD_ACTUAL_CPU",
  "runtime_commit": "REQUIRED",
  "model_revision": "REQUIRED",
  "model_sha256": "REQUIRED",
  "threads": 1,
  "beam_size": 1,
  "samples": "fixtures/nmt/golden-v1.jsonl",
  "cold_load_ms": null,
  "warm_latency_p50_ms": null,
  "warm_latency_p95_ms": null,
  "peak_rss_mib": null,
  "unload_after_rss_mib": null,
  "note": "null values mean not yet measured"
}
```

## 7. 兼容性与未来跨平台预留

| 边界 | P3 Linux 立即实现 | 未来平台预留（不在 P3 验收） |
|---|---|---|
| Public API | Rust library + CLI，无 OS 类型 | JNI/C ABI/UniFFI/HTTP 适配调用相同契约 |
| 文件与安装 | `PathBuf`、平台无关 manifest、原子升级能力 | Windows 文件映射/删除语义另测 |
| 进程活性与锁 | Linux 实现 + conservative fallback | Windows `LockFileEx` / macOS 进程探针未来实现 |
| 内存指标 | Linux `ProcessMetricsProvider` | Windows/macOS/Android 平台采集器后置 |
| 推理 | CPU Bergamot plugin | ncnn/ORT/Burn backend 另行适配验证 |
| 取消 | 排队/装载阶段可取消、执行阶段显式说明 | Android 生命周期与后台回收后置 |
| 发布 | Linux x86_64 发布、Linux CI | Windows/macOS/Android 仅保证架构可扩展，不能宣称构建通过 |

不要在 Linux 核心代码里硬编码 `LD_LIBRARY_PATH`、glibc 具体路径、主机路径、`/proc` 解析；这些属于构建配置或 Linux 平台适配。

## 8. P2-S 遗留问题及 P3 处置表

| 遗留项（依据 `09`） | P3 处理时机 | 阻塞什么 |
|---|---|---|
| `ResolvedExecutionPlan`、`content_digest`、single-flight | **P3.1/3.2 必须** | 可替换模型的缓存一致性与并发加载 |
| 模型实例租约、TTL/LRU | **P3.2 必须** | 安全卸载与空闲内存回收 |
| 依赖反向引用 | **P3.2 必须** | 模型/Tokenizer 删除安全 |
| 真实安装崩溃注入、目录持久化 | **P3.2 必须做 Linux 版** | Linux 包安装与恢复可信度 |
| 加载前内容校验 | **P3.2 必须定义策略并测试** | 权重被改写后运行不可信 |
| `ProcessMetricsProvider` | **P3.3 必须做 Linux 版** | 低内存结论真实性 |
| 术语表 `glossary_id` 关联 | P3 中记录现状，先保证单张快照 | 多表组合等后续升级 |
| Windows/macOS/Android 实证 | 以后独立阶段 | 不阻塞 Linux P3，禁止宣称通过 |
| 规则到真实 NMT 的保护跨度升级 | P3.1/3.3 A/B 验证后决策 | 占位符译文正确性 |

## 9. P3 统一测试与失败处理

### 9.1 必过的契约测试

- [ ] **无 Bergamot feature**：`translator-api` + Mock 测试全部正常，默认依赖不引 C++。
- [ ] **真实本地翻译**：至少一个方向由公开 `Translator::translate()` 输出真实译文，CLI 明确实际 engine/model id。
- [ ] **网络权限**：无网络、未授权远程，不发生 HTTP 请求。
- [ ] **同一模型 10 并发请求**：仅一次 native 加载；同 revision 共享，跨 revision 不共享。
- [ ] **资源活跃保护**：推理时尝试删除模型包和释放句柄，都被安全阻止或排队回收。
- [ ] **错误路径**：损坏模型、缺 tokenizer、版本不兼容、预算不足、超时/取消均有明确错误而非 panic。
- [ ] **缓存隔离**：更改模型 digest、Tokenizer、解码设置、术语/规则版本或隐私域，不会复用旧译文。
- [ ] **安装故障注入**：`kill -9` 后仓库可 `audit/recover`，不误删已安装激活版本。
- [ ] **FFI 安全**：异常/panic 不穿越 ABI；重复加载卸载不崩溃，无明显泄漏。
- [ ] **Linux 性能**：冷/热延迟、RSS、峰值和卸载后记录齐全，运行环境可复现。

### 9.2 失败时的调整顺序

1. 参考模型无法加载/译文错误：先核对权重与 tokenizer/语言方向、官方参考 CLI；不得用编造译文替代。
2. Rust FFI 不稳定：先缩小 C ABI 边界、改为单 worker 串行，禁止盲目 `unsafe impl Sync`。
3. 内存超预算：先检查量化档、线程数/beam、模型驻留和工作区；其次尝试更小模型；**最后**才转换 ncnn 作为替代研究。
4. Bergamot 确实无法完成所需语言方向/构建：依据资源备选清单评估 OPUS-MT + CTranslate2/ONNX Runtime；**不修改 `translator-api`**。
5. 对照后端必须记录转换成本、模型质量、内存峰值和维护风险，不能仅按 binary 大小决策。

## 10. 实施文件建议与验收产物

### 10.1 推荐新增/修改位置

```text
translator-sdk/
├── crates/
│   ├── runtime-bergamot/               # Linux feature-gated native adapter
│   ├── translator-nmt/                 # model-neutral NMT orchestration
│   ├── translator-platform/            # Linux process metrics / liveness adapter
│   ├── translator-resource/            # model lease + digest + deps + install safety
│   ├── translator-core/                # execution plan/cache/timeout/coordinator
│   └── translator-api/                 # stable builder/facade
├── apps/translator-cli/               # nmt probe / benchmark / resource diagnostics
├── fixtures/nmt/                      # offline sample sentences + expected constraints
├── tools/                              # model pack validation / optional asset importer
└── docs/
    ├── 10_P3_Linux本地NMT开发建议与验收计划.md
    ├── 11_P3.0_原生模型验证报告.md
    ├── 12_P3.1_真实NMT插件报告.md
    └── 13_P3_性能与发布验收报告.md
```

这是建议布局，已有 crates 和工具保持原样；不要仅为目录整齐就拆分运行良好的代码。

### 10.2 每个里程碑报告必须包含

1. **实际实现**：具体 crate、入口、API 变更、native 版本、模型 revision、代码位置。
2. **复现证据**：完整命令、测试汇总、真实模型输出样例、性能采集方法、环境与配置。
3. **偏差与未实现**：所有 pending、未通过项、风险，区分"未测 / 未实现 / 失败"。
4. **替换验证**：禁用 real-runtime 后仍可用 Mock；替换词库/规则不改 API；后续可装第二 runtime。

## 11. P3.0 开工清单（可直接复制到 Issue）

- [ ] 选定首个语言对与模型质量档，并锁定模型来源、版本、tokenizer、许可证、SHA-256。
- [ ] 固定 Bergamot/Marian 参考代码 commit，打通 Ubuntu release 构建。
- [ ] 原生 CLI 在完全断网条件下翻译至少 20 条真实句子，并提交输出与错误样本。
- [ ] 使用 shell 脚本采集 cold load / warm latency / RSS；记录 CPU、线程数及批处理/beam 配置。
- [ ] 查明 native 句柄是否支持多线程共享，写出 FFI worker/actor 选择理由。
- [ ] 确定模型包 Manifest 中 runtime compatibility 的真实字段，不允许保留 `TBD` 后发布。
- [ ] 打开 Linux 环境 CI（默认 feature 的全部 P0–P2-S 测试）；原生模型测试可使用单独 job 和可下载但已固定 hash 的测试资源。
- [ ] 完成 P3.0 评审再进入 P3.1，不用先适配其他平台。

## 12. 推荐 ADR（合并前查重）

> 不在这里直接分配正式 ADR 编号，避免与 `docs/01` 已有 ADR-001～023 冲突。进入实施时核对最新编号并合并。

- **Linux-first Release, Platform-neutral Core**：Linux 先交付，其他平台仅预留边界；不得用 Linux 通过测试冒充跨平台通过。
- **Reference Translator Before FFI**：原生参考链路未成功前不实现 Rust native adapter。
- **Model Revision Is Content Identity**：模型实例与缓存使用内容 digest，不只用 `id@version`。
- **Native Runtime Worker Ownership**：不验证线程安全前，原生对象由受控 worker 独占。
- **Soft Memory Budget + Measured RSS**：预算记账与系统真实内存指标分开，低内存结论基于实测。
- **Deferred Multi-platform Bindings**：JNI/C ABI 等适配在 Linux SDK 闭环后实施。

---

## 13. 参考与资料边界

本文件依据**用户上传的项目设计及 P0–P2-S 实施报告**提出建议。此前报告中出现的性能、完成状态仅作为其自报测试结果；本次没有独立运行 `translator-sdk/` 源码，也没有验证外部模型下载或模型注册表。P3.0 必须重新核实以下上游资料与具体 revision：

- Bergamot：https://github.com/browsermt/bergamot-translator
- Mozilla Translations：https://github.com/mozilla/translations
- Firefox 模型注册表：https://mozilla.github.io/translations/firefox-models/
- Marian/OPUS-MT：https://huggingface.co/Helsinki-NLP
- ncnn（备选）：https://github.com/Tencent/ncnn

**最终验收判据**：Ubuntu/Linux 上有可复现的、无需联网的真实本地翻译；在不启用该后端时核心 SDK 仍可独立编译测试；模型资源的加载、替换、回收、错误处理和缓存一致性具备测试证据；其他平台保持清晰的扩展接口但不宣称已支持。
