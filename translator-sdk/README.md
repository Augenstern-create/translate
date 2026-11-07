# translator-sdk

轻量跨平台混合翻译 SDK。**Rust Library，不实现 UI**，预留桌面 / Android / CLI / 服务端适配接口。

设计文档见仓库根目录 `../docs/`：

- `01_翻译SDK完整开发设计.md` — 架构与公共 API
- `02_翻译资源候选与备选清单.md` — 词典 / 模型 / 运行时候选
- `03_P0骨架与验证报告.md` — P0 骨架与验证证据
- `04_P1_字典与离线查词报告.md` — P1 词典包格式、基准与限制清单
- `05_审查响应与P2实施计划.md` — 外部审查的逐条处置、ADR-008~014、P2 拆解与验收映射
- `06_P2.1-P2.2_资源协议与仓库报告.md` — 资源协议（v1/v2 双版本号）、安装/激活/回滚、偏差清单
- `07_P2.3-P2.5_租约预算与术语约束报告.md` — 资源租约、内存预算、术语约束与诊断、`dictionary.sqlite.v2` 实测
- `08_P2完整技术审查与P3开发建议.md` — 外部审查归档：四个 P3 前决策、P0/P1 风险表、C-01～C-12 测试矩阵、P2-S 方案
- `09_P2-S稳定化报告.md` — P2-S 稳定化：P0 收口（租约/卸载原子性、锁回收、术语默认值）+ 明确 pending 清单
- `10_P3_Linux本地NMT开发建议与验收计划.md` — 外部输入：P3 分阶段（P3.0～P3.4）目标、准入门槛与测试矩阵
- `11_P3.0_原生模型验证报告.md` — 真实模型 + Bergamot 原生链路的实测报告（译文/延迟/RSS/失败项）
- `12_P3.0技术审查与P3.1准入决策.md` — 外部审查归档：P3.0 有条件通过、三个 P0 门槛、P3.1-A～E 拆分与测试矩阵
- `13_P3.1-A报告.md` — 独立 worker 进程 + 帧协议 + mock worker（实现/测试/偏差三部分）
- `14_P3.1-A技术审查与P3.1-B实施建议.md` — 外部审查归档：P3.1-A 有条件通过、P3.1-B 实施约束与 B-T01…T12 验收矩阵
- `15_P3.1-B报告.md` — 真实 Bergamot 进 worker 进程（隔离快照 / 类型化配置 / 分阶段超时 / 实测基线）
- `17_P3完整实施与验收总报告.md` — 外部 P3 验收框架归档：C/D/E 待办、SP-01～12 与 LC-01～10 矩阵、发布红线
- `18_P3.1-CDE集成验收报告.md` — 严格文本保护（默认安全分段）+ 执行计划/single-flight/排队截止时间 + 真实 NMT 接入 `Translator`/CLI
- `19_P3.2报告.md` — **本轮**：反向依赖（LC-07）、安装崩溃注入与修复（LC-08）、残留回收 + PID 复用、计划摘要作键的缓存（LC-09）、空闲回收与内存准入
- `23_P3冻结审查与P4插件替换建议.md` — 外部审查归档：P3 冻结规范、P4-01～07 工作包、P4-G01～08 门禁
- `24_P3冻结与P4-02可行性门禁报告.md` — P3 冻结（`p3-frozen` + 自校验清单 + 阻断台账）+ 候选门禁（三个 ONNX 陷阱、B 类测量）
- `25_P4-03与P4-04_第二NMT后端与插件替换验证报告.md` — **本轮**：第二个真实 NMT 后端（ONNX Runtime）进 `Translator` 契约、退化抑制、同文本对照与门禁判定
- `26_P4-05至P4-07_回归对比与结论报告.md` — **本轮**：SP/LC 回归矩阵（含不适用项）、加载/卸载与长稳基准、A/B 分类与 `conditional-go` 裁定
- `27_P4完整技术审查与P5承接建议.md` — 外部审查归档：P4 裁定、R-01～R-08 修订项、P5 承接清单
- `28_P4审查响应与验收口径修订报告.md` — 验收台账（C9）、同文本 A/B 重测、499 字口径拆解、D-77～D-82
- `29_P5环境勘察与Windows验证方案对比报告.md` — P5 前置：本机硬件/Android 现状实测、Windows 三种验证路线对比与建议、D-83～D-85
- `30_P5-01_Android工具链与Windows验证矩阵报告.md` — NDK r30 + x86_64 AVD 打通 JNI 三段链（含 `EM_AARCH64` 负对照）、只读勘察脚本与 W1～W10 验证矩阵、D-86～D-91
- `31_Windows验证机部署清单.md` — 实体机实测后的清单 v2（**部分建议已被 `docs/32` 取代，见文首状态块**）
- `32_P5开发建议与实施计划（工作区与权限边界）.md` — 外部审查/计划稿归档：唯一可写工作区 `E:\augenstern\translate`、逐项审批、固定 Anaconda Python
- `45_P7.3_Windows复测报告.md` — 既有 NAN-WINDOWS 复测：344 tests、ORT 24 用例、缓存隔离、G06 N/A / G08 PASS
- `46_P7验收决策_G11准备与P8启动记录.md` — G12 方案①、G11 两步走与上传范围预检、ADR-063（JNI 多实例）
- `44_P7.1-P7.5_跨平台质量收口与发行候选报告.md` — **P7 收口**：G06/G08 闭合、J-07/J-10/J-15 Android 34 PASS（1 PENDING）、J-14 模拟器基线、Linux 391 tests、SBOM/工件哈希；G12/G11 仍 open，裁定 Conditional
- `43_P7.0_基线与测试矩阵报告.md` — **本轮**：P7.0 基线/权限/依赖只读核对、五条阻断项按台账纠偏（G06=取消在途请求、G08=abort vs kill -9、D-46=许可证正文未入库）、J-07/J-10/J-14/J-15 判据冻结、D-144～D-147
- `42_P7发行准备与跨平台质量收口开发建议.md` — 外部计划稿归档：P7.0～P7.5 分解、P7-01～P7-12 验收矩阵、权限硬约束
- `41_P6阶段总结与验收报告.md` — P6 收口——J 矩阵最终状态（11 PASS / 4 pending）、契约增量、裁定 Conditional、5 条阻断项未动
- `40_P6.1-P6.4_JNI绑定层实施报告.md` — **本轮**：`translator-jni` + Android Harness，模拟器 23/23 PASS（含并发关闭 389/11/0 崩溃）
- `39_P6.0_契约冻结与只读预检报告.md` — JNI 契约 v1 冻结（4 入口/字符串代际句柄/JSON DTO/UTF-16/NUL 拒绝）、只读预检（无 JNI crate、默认 feature 无 HTTP）、D-130～D-135
- `38_P6跨平台绑定与Android_JNI开发建议与验收计划.md` — 外部计划稿归档：P6.0～P6.5 分解、J-01～J-15 测试矩阵、权限边界
- `37_P5阶段总结与验收报告.md` — P5 收口——三分口径裁定 **Conditional**、平台证据表（九项能力 × 四平台）、工作包索引、未验证项清单
- `36_P5.5_Android模拟器验证报告.md` — **本轮**：真 SDK 跑在 x86_64 AVD（词典/资源/CLI/识别/翻译）、ARM64 仅编译合规（运行被模拟器证伪）、物理性能未验证
- `35_P5.4_Windows进程存活探测与跨进程资源验证报告.md` — Windows 存活探测（kernel32 裸 FFI，零新依赖）、lc08 与 cross_process 解 gate、Windows 333/0/60 vs Linux 375/0/60、D-115 结案（`ORT_CACHE_DIR`）
- `34_P5.2-P5.3_Windows构建回归与ORT实跑报告.md` — W1–W10 全绿（322 passed/0 failed/56 ignored；ORT 真实模型 24 passed；中文+空格路径构建通过）、7 个跨平台修复提交、D-108～D-117、D-92 实测确认 directml 静态包、D-115 待处置（工作区外 360.9 MB 缓存）
- `33_P5.1_工作区与环境只读核验报告.md` — P5.1 只读预检（Rust 1.99.1 / Anaconda 3.13.5 / VC 17.14 / 工作区空且 E: 133.7 GiB）、`dsh-win-env/v1` 采集脚本、D-101～D-107

---

## 当前状态：P0～P3 完成并**已冻结**（`p3.4` / 分支 `p3-frozen`），**P3 = Conditional**：功能可验收，含权重的发行被 release-blockers（G12 等五项）阻断。P4 工作包全部交付（P4-01～P4-07，`p4.3`），裁定 **`conditional-go`**：插件替换可行 = go，替换 P3 后端 = **no-go**；**门禁闭合情况是 6 项通过 + G05/G06 部分满足**（未闭合项已进机器可查台账 `fixtures/p4/acceptance.json`，由 `check-release.py` 的 C9 盯）。**P6 已收口**（`p6.2`）：`bindings/translator-jni` 让完整 Rust SDK 经 JNI 在 Android x86_64 模拟器上真实可用（23/23 断言，含真词典/并发关闭）；Linux 379 / Windows 337 passed 无倒退；J-07/J-14/J-15 与 APK 载体、真实 Android NMT、ARM64 运行仍 pending。**P5**（`p5.7`）：裁定 **Conditional**——Windows 实体机 W1–W10 全绿（333 passed / 0 failed / 60 ignored，含 ORT 24 条真实模型、6 条跨进程不变量、5 个崩溃注入点），Linux 375/0/60 无回退；Android x86_64 模拟器上真 SDK 跑通（词典/资源/CLI），ARM64 仅编译合规、运行不可得、物理性能**未验证**；5 条发行阻断项仍 open。见 `docs/37`

- **P0**：workspace 结构、核心类型、插件契约、协调器与路由、缓存、门面 API、Mock 插件、CLI。
  **不接任何真实模型、不装任何 native 库**，整条链路即可运行 —— 这是设计文档规定的交付判据。
- **P1**：词典包格式 v1（`index.fst` + `entries.sqlite` + `manifest.json` + `LICENSE.txt`）、
  CC-CEDICT 打包工具、只读 mmap 查询、启发式语言识别、占位符保护。
  已用完整 CC-CEDICT（121,584 词条 / 197,593 键）验证，查询 p50 约 17 μs（release，本机）。
- **P1 规则**：声明式 JSON 规则包（`translator-rules`），构建期编译为带 sha256 的只读二进制，
  **强制金样本校验**（规则改坏了拒绝加载），支持 `nfkc` 与 `replace`（术语替换/缩写展开）两种动作、
  语言对门控、优先级、`contains`/`is_single_word` 条件。短输入开销约 287 ns/次。

- **P2.1**：资源协议 v2（`manifest_schema_version` / `payload_schema_version` 分离）、
  v1 清单只读兼容、兼容性判定（协议/payload/平台/依赖/kind）、未知版本明确拒收。
- **P2.2**：资源仓库 `translator-resource`——安装到 staging、逐文件 size+sha256 校验、
  原子落位、原子写状态、激活/回滚/安全清理、崩溃后清理遗留目录；安装新版本**不夺取激活位**，
  激活版本拒删。
- **P2.3**：资源**租约**（谁挂载谁登记；有有效租约的版本拒绝删除，跨进程可见）、
  **内存预算**（装配期准入，超限即 `OutOfBudget`；数值是自报下界，不是 RSS）、
  **跨进程状态锁**（`O_EXCL` 锁文件，超时给出持锁者与路径）。
- **P2.4**：输出侧**术语约束**（`TargetTermGuard`，只改裸文本，URL/代码/模板变量一字不动）+
  **结构化诊断** `translate_with_diagnostics()`（规则命中、受保护跨度、还原缺失、术语替换明细、是否缓存命中）。
- **P2.5**：`dictionary.sqlite.v2`（释义内联进 `entries.senses`）与 v2 打包工具。
  真实 CC-CEDICT 实测：查询 p50 **17.36 → 8.98 µs（−48%）**、库体积 **18.2 → 13.6 MB（−25%）**；v1 仍可读。
- **P2-S**（稳定化，不新增能力）：
  - **租约/卸载原子性**：`acquire_lease` 与 `uninstall`/`cleanup` 进同一个跨进程临界区并在锁内重读状态
    （修复"租约指向已删除版本"的真实缺陷，C-01）；
  - **锁与租约记进程身份**（pid + Linux 启动标识）：能识别 pid 复用，无法判定存活时按占用处理（C-04）；
  - **术语默认策略改为 `Warn`**：默认不再改写译文，`Enforce` 必须显式开启；术语规则可声明源文触发条件
    （`source_variants`），避免"译文里有这个词就认定源文该用某术语"（C-06）；
  - **三方对账** `resource audit [--prune]`：state / installed / staging / 租约对账，只报告，删孤儿要显式；
  - 跨进程测试 6 条（独立二进制 re-exec，不是线程模拟）。

- **P3.0**（原生验证，不改 Rust 代码）：锁定 Mozilla 官方 en→zh-Hans 权重
  （`model.enzh.intgemm.alphas.bin` v3.0，43.8 MB，sha256 与注册表一致）+ Bergamot commit `9271618`，
  在 Ubuntu x86_64 CPU 上通过原生 CLI 得到**真实译文**：
  `Hello world!` → `你好,世界!`。
  实测（本机、beam=1）：冷到首行 **605.9 ms**、热 p50 **12.82 ms** / p95 14.38 ms（100 次同进程）、
  峰值 RSS **678.5 MiB（2 线程）/ 413.8 MiB（1 线程）**。
  24 条固定样本 23 条通过；**模板变量 `{name}` 会被模型吞掉**——占位符保护必须在 P3.1 里前置。
  详见 `../docs/11_P3.0_原生模型验证报告.md`。

- **P3.1-A**（新增 crate `translator-runtime`，**mock worker，未接真实模型**）：
  把原生推理关进**独立进程**，因为 `panic = "abort"` 让 `catch_unwind` 失效、线程也拦不住 SIGSEGV/`abort()`。
  - `RuntimeTransport` trait + `ProcessTransport`：长度前缀帧（4 字节大端、上限 8 MiB、超限在**分配前**拒绝）、
    `request_id` + `worker epoch` + `model revision` 关联、请求超时、三级终止（shutdown 帧 → SIGTERM → SIGKILL）、
    无僵尸、受控重启（预算用尽即 `CapabilityUnavailable`）、**不静默重放失败请求**；
  - 加载前校验：四文件齐全 / 非符号链接 / 大小 / 流式 sha256，且**在 spawn 之前**做（缺文件不再触发 native `abort()`）；
  - 失败映射逐条对应 `docs/12` §3.2；worker 崩溃带**信号号**（`abort` → SIGABRT 6、截断权重 → SIGSEGV 11），
    因为被信号杀死的进程 `ExitStatus::code()` 是 `None`；
  - `nmt-worker --mock` 能真的 `abort()`、真的超时、真的吐坏帧（不是返回错误码假装）；
  - 实测（本机 release，mock）：IPC 往返 p50 **0.016 ms** / p95 0.019 ms（n=200），
    宿主 2.8 MiB + worker 2.1 MiB（**空壳**，不含权重），`shutdown` 5.1 ms；
  - **这些数字不能当内存预算**：预算口径仍是 P3.0 的 413.8 MiB（1 线程峰值）/ 678.5 MiB（2 线程峰值）。
  详见 `../docs/13_P3.1-A报告.md`。

- **P3.1-B**（真实模型进进程，`docs/15`）：`Hello world!` → `你好,世界!`，整条链路是
  **宿主 → nmt-worker(Rust 监督者) → bergamot(native)**，三层进程。
  - **隔离快照**：四文件先复制进私有只读目录（0o700/0o400），复制与 sha256 用同一次读，
    worker 在 spawn native 前**再核一遍**摘要；源文件之后被改也影响不到已加载的模型；
  - **类型化配置生成**：只接受 5 个有范围检查的参数，`shortlist` 是独立键（写进 `models` 会让 Marian `abort()`），
    调用方无法注入任意 YAML；
  - **分阶段超时**：Load 30 s / Translate 15 s / 其余 10 s，终止仍是 shutdown 帧 → SIGTERM → SIGKILL；
  - **native 不产生孤儿**：`PR_SET_PDEATHSIG`，宿主在加载期击杀 worker 时 native 一起死；
  - 实测（1 线程、beam=1）：快照 **262 ms**、加载总墙钟 **719 ms**、首句 **417 ms**、
    热 p50 **13.17 ms** / p95 14.09（n=100）、host 3.0 MiB + worker 2.7 MiB + native 341.9 MiB、
    native 峰值 **413.8 MiB**（与 P3.0 一致）、卸载 43 ms、重载 725 ms；
  - 崩溃隔离：缺权重让 Marian 真 `abort()`（SIGABRT）→ 宿主收到 `RuntimeInitFailed` 并存活；
    native 被外部 SIGKILL → `BackendFailed` + worker 退出 → 下一次请求才重启（不重放）。
  详见 `../docs/15_P3.1-B报告.md`。

- **P3.1-C/D/E**（`docs/18`，一次合并批次）：公共入口终于能跑真实模型，而且**结构不会被静默破坏**。
  - **严格文本保护**（`translator-core::protection`）：URL / 邮箱 / 反引号代码 / `{name}` 在送入真实模型前被摘出，
    译文按字节拼回；缺失 / 重复 / 未知标记 → `IntegrityFailure`，不返回"看起来正常"的译文。
    **默认策略是安全分段**：受保护跨度根本不进模型。原因不是偷懒——5 种候选标记 × 7 类句子实测
    没有一种能稳定穿过真实 Marian（最好的 `#Pn#` 也只过 6/7），"唯一无碰撞标记"在该后端上不成立。
    代价：一句 N 个跨度 = N+1 次 native 调用，且逐段丢失上下文。
  - **执行计划与摘要**（`translator-core::plan`）：`plan-v1` 覆盖 engine / runtime / runtime build / 模型五文件摘要 /
    语言对 / beam / 线程 / 保护策略 / 规则与术语 / 隐私域，`canonical()` 稳定编码 → sha256。
    摘要就是**未来的**缓存身份；金值被钉在单元测试里。真实 NMT 目前**仍无缓存**，所以摘要只用于诊断。
  - **生命周期**：single-flight 加载（并发 10 次只 spawn 一次）、排队绝对截止时间（到期 `Timeout{operation:"queue"}`）、
    在飞请求计数、卸载等待在飞请求（不杀进程、不死锁）、worker epoch 校验防止复用旧加载。
  - **真实引擎** `translator-nmt-bergamot`：`ENGINE_ID = nmt.bergamot.cli`，`supports()` 只认配置的语言对，
    错误原码透传、**没有任何自动远程回退分支**；经 `TranslatorBuilder::nmt_bergamot()`（feature `nmt-bergamot`）
    与 CLI 的 `--nmt`（feature `nmt`）接入。
  - 实测（1 线程、beam=1）：`Hello world!` → `你好,世界!`（首句含加载 ≈ 1.13 s，热态 9–15 ms）；
    `Hello {name}! See https://example.com/a for {name}.` → 两个跨度逐字节保留。
  - **默认依赖树里没有 native 与进程代码**：`cargo tree -p translator-api` 默认看不到
    `translator-nmt-bergamot`/`translator-runtime`；未编译 `nmt` 特性却传 `--nmt` 会得到
    `CapabilityUnavailable`，不会静默降级成 Mock。
  详见 `../docs/18_P3.1-CDE集成验收报告.md`。

- **P3.2**（资源安全与一致性，`docs/19`）：`docs/17` §4.2 的 **LC-01～10 全部有实现与测试**。
  - **反向依赖（LC-07）**：`uninstall` 拒绝删除"被其他已安装包清单点名需要"的版本
    （`ReversedDependency`，消息里列出依赖方）；`cleanup` 保留它并记入 `kept_dependents`；
    `audit` 报"依赖缺口"（判定要**记录在 + 目录在**，只看 state 会把外部误删当成功）。
  - **安装崩溃注入（LC-08）**：复制 / rename / 写状态前后共 5 个注入点，真独立进程 `abort()`；
    `recover()` 从"清半成品"扩大为"收敛到一致"（删孤儿目录、丢失效记录、激活版本退回可用版本）。
    持久化：文件 `fsync` + **父目录 fsync**（仅 Unix，非 Unix 显式跳过，不吹跨平台）。
  - **残留回收**：快照与 worker 配置目录按「pid + `/proc/<pid>/stat` starttime」判定，
    **PID 复用不会让旧目录永远留着**，判定不了就保留（不误删正在被 native 打开的文件）。
  - **缓存（LC-09 / ADR-038）**：真实 NMT 的结果缓存键 = `plan-v1` 摘要 + 目标语言 + 原文 sha256，
    只缓存成功结果，有界 FIFO；摘要**跨卸载保留**，因此"卸载后重查同一句"0 ms 命中、不重载模型。
    实测：改 `nonbreaking_prefix.en` 一行 → 计划摘要 `dcc88e56…` → `4eba81ed…`，而 `model_id` **不变**
    ——这正说明 `content_digest` 不能当缓存键。
  - **空闲回收与准入**：`evict_if_idle(ttl)` 有在飞请求时绝不卸载；NMT 内存按实测量级
    （稳态 341.9 MiB / 峰值 413.8 MiB）计入装配期预算，超限 `OutOfBudget`；
    `Translator::nmt_states()/nmt_cache_stats()/evict_idle_nmt()/unload_nmt()` 可观测。
  - 实测：默认 361 passed / 0 failed / 37 ignored；真实模型与忽略集 **41/41**。
  详见 `../docs/19_P3.2报告.md`。

- **P3.3**（Linux 性能、质量与低内存，`docs/20`）：`docs/17` §6.1 的九个场景都有**带口径**的数字。
  - 新增 `tools/p33-bench`：`empty` / `dictionary` / `worker` / `load` / `nmt` / `concurrency` /
    `unload` / `sustain` / `quality`，每个 JSON 都带 `caliber`（构建、是否含加载、缓存开关、
    线程/beam、分位算法）。分位数用**最近秩法**，不插值——人能手工复核。
  - **首次采 PSS**（`smaps_rollup`）并按 `PPid` 建进程树：native RSS 340.80 / **PSS 332.56** MiB（差 2.4%）；
    卸载后全树 PSS **340.30 → 3.07 MiB**，进程 3 → 2，残留快照 0。
  - 关键数字（release / 1 线程 / beam=1 / 缓存关闭 / 热态）：17 字 p50 **6.78 ms**、94 字 **25.05 ms**、
    499 字 **175.67 ms**、分段句（2 个跨度 = 3 次调用）**19.95 ms**、缓存命中 **≈0 ms**；
    空 SDK 初始化 0.01 ms / RSS 3.16 MiB；IPC 往返 p50 0.04 ms；加载 647 ms、首句 409 ms。
  - **测量暴露并修掉的真问题**：`native --cpu-threads 1` 不约束 OpenBLAS/OpenMP，线程超订让
    并发 10 的吞吐只有 15.0 rps、p50 139 ms。worker 现在把 `OPENBLAS_NUM_THREADS`/`OMP_NUM_THREADS`
    对齐到 native 线程数（**宿主显式设置则不覆盖**）：31.0 rps / p50 35 ms，499 字单句 340 → 176 ms。
  - **质量三层分离**（结构 / 事实 / 代理指标），24 条金样本：适用 22、结构 22/22、事实 22/22；
    但 `mix-01` 机器全绿而人读是错的（`参须` 应为 `参数须`）——**代理指标不能替代人读**。
  - **并发不扩展**：单 worker、单 in-flight，1/2/10 客户端吞吐 39.3 / 37.3 / 31.0 rps，并发 10 的 p95 1.18 s。
    这是限制，不是特性；FIFO 排队仍是待办。真冷盘（清 page cache）本轮未测（需要 root）。
  详见 `../docs/20_P3.3报告.md`。

- **P3.4**（发行候选，`docs/21`）：把"包里有什么、红线守不守、别人能不能复现"变成机器检查。
  - **许可清单**：`licenses/third-party.json`（53 个依赖，SPDX + checksum + `legacy_notation` 标记）、
    `licenses/model-assets.json`（权重/Bergamot 的坐标、哈希、许可状态与再分发结论）、
    `THIRD_PARTY_NOTICES.md`（发行物里的人可读声明）。
  - **发行红线** `tools/check-release.py` 七项：已跟踪文件无权重/大文件、许可清单与依赖树一致、
    模型坐标三方一致、打包清单不含权重、`.gitignore` 覆盖、红线文案存在、可选的本机权重哈希核对。
    CI 的 `release-redline` job 每次提交都跑。写死"权重再分发必须是 `prohibited-until-confirmed`"，
    谁改成 `allowed` 立刻红。
  - **CI 分流**：`.github/workflows/ci.yml`（默认门禁：fmt / clippy -D warnings / 双 feature 测试 /
    默认依赖树不得出现 runtime / rustdoc -D warnings / 性能冒烟 / 文档编号检查）
    与 `.github/workflows/real-model.yml`（自托管、手动触发、缺前置**快速失败**）。
    **本仓库没有远端 runner，这两个 workflow 没在 GitHub 上真跑过**（`docs/21` D-47）。
  - **平台门禁可测了**：`translator-runtime::platform` 把 `#[cfg]` 分支换成判定表，
    非 Linux（windows/macos/android/ios/freebsd/openbsd）明确 `CapabilityUnavailable`，
    错误信息带平台名与原因；CLI `--diagnose` 增加"平台"一行。
  - **打包**：全部 crate `publish = false`；实测只有 `translator-types` / `translator-resource`
    能 `cargo package`（其余 12 个因 path-only 内部依赖不可打包，属设计取舍）。
  - 实测：**365 passed / 0 failed / 41 ignored**，真实模型 **41 passed**，红线 7 项全 PASS；
    `LICENSE-MIT`/`LICENSE-APACHE` 正文后在 P7 补齐（D-46 已 closed）。
  - **P3 最终裁定：Conditional（有条件通过）**。功能门槛可验收；**含权重发行仍被阻断**（G12：
    权重许可未确认），G06/G08（P7 已闭合）、G11（真实 runner）也是条件项。
    见 `../docs/21_P3.4报告.md` §4。

词库与规则都是**可选 feature**：不开 `dictionary` 时 `translator-api` 依赖树里没有 `rusqlite`；
不开 `rules` 时没有 JSON 解析依赖（但 `TextRules` 契约始终可用，宿主可自己实现）。

- **P4（进行中）**：
  - **P4-01 冻结**（`docs/24`）：`p3-frozen` 分支 + `fixtures/p3-freeze/manifest.json`（`tools/freeze-manifest.py --check`
    会因 tag 移动、`Cargo.lock`/基线 JSON 变化、台账条目被删而失败）+ `fixtures/release-blockers.json`
    （P4 时点：G12/G11/G06/G08/D-46 五项 open；P7 后 G06/G08/D-46 已闭合，G12/G11 仍 open）。
  - **P4-02 候选门禁**：`ort + opus-mt-en-zh`（Marian int8 ONNX，**许可 apache-2.0**），
    资产 111.0 MiB / 9 个文件全部锁定在 `fixtures/p4/candidate.lock.json`；能产出真实译文
    （`The cat is on the table.` → `猫在桌子上`）。
  - **三个坑**（写进锁定文件，避免 P4-03 重踩）：`source.spm` 的 piece id **不是**模型 id（模型 id 来自
    `vocab.json` 的 65,001 项联合词表，用错不报错、只输出通顺但错误的中文）；merged decoder 首步 encoder
    侧 K/V 必须给真实长度；带缓存步的 `present.encoder.*` 是 0 长度占位、不能回喂。
  - **诚实结论**：这是 **B 类比较**（不同模型）。本机 1 线程实测 17 字 118.6 ms / 94 字 647.6 ms、
    进程 909 MiB，**明显差于** Bergamot（6.78 / 25.05 ms、341 MiB）——不得归因于 runtime。
  - **P4-03 插件实现**（`docs/25`）：新 crate `translator-nmt-ort`（`nmt.ort.opus-mt`）。
    两层 feature：`nmt-ort` 只编契约、`nmt-ort-runtime` 连 ORT 一起编；**默认构建依赖图里 `ort` 出现 0 次**，
    默认 CLI 的 `ldd` 里没有 `libonnxruntime.so`。真实模型测试 6 项 + 门面测试 5 项全部通过；
    `tools/p4/build-tokenizer.py` 复现 `tokenizer.json` 并自检 id 空间（`Hello world!` → `[3828, 431, 50]`）。
  - **P4-04 替换验证**：只挂 ORT 后端（无词典/无规则/无 Bergamot）时 `Translator::translate()` 正常出译文，
    `engine_id = nmt.ort.opus-mt`，受保护跨度逐字节保留。
  - **退化抑制**（本后端自己的解码策略）：纯贪心会把 `Hello ` 这种碎片重复 97 次「你好」（同一模型在 Python
    参考实现里同样如此）。已加长度上界 + 无重复 3-gram + 三连停 + 尾部收敛，`degeneracy_count()` 可观测。
  - **同文本同预算对照**：Rust 插件 123 / 653 / 2752 ms，Python 参考 120.0 / 651.3 / 2652.3 ms
    —— 说明"实现没写歪"，**不**说明 runtime 优劣。内存 PSS：Rust 574.8 MiB vs Python 870 MiB。
  - **质量**：短句可用（`The cat is on the table.` → `猫在桌子上`），长句不可用
    （`model weights` → `模型重量`、`load time` → `负载时间`）。P4-G05（计划摘要）与 P4-G06（隔离/租约/崩溃恢复）
    只部分满足，见 `docs/25` §4。
  - **P4-05 结构回归**（`docs/26`）：SP-01～12 与 LC-01～10 的适用项 **13/13 通过**（字节级结构断言）；
    不适用的四条 LC（deadline / native 死亡 / 租约 / 安装期 kill -9）**逐条写明原因**，不假装通过。
    顺带修掉一个真缺陷：分词器 `unk_id=None` 会**静默丢弃**词表外字符（emoji），现已固定为 `<unk>`（ADR-049）。
  - **P4-06 长稳**：5 轮加载/卸载 + 100 次翻译（约 40 s）。卸载后 RSS 回落到 249 MiB，
    持续翻译第 20 次起 **持平在 598 MiB**（不涨而非慢涨）。
  - **P4-07 裁定：`conditional-go`**。插件替换可行性 = go；"用它替换 P3 后端" = **no-go**
    （**同文本**重测：延迟 10.9～15.0×、PSS 1.63×、长句不可用）。默认运行行为仍走 P3.4 路径
    （两个 feature 默认关闭）；要回退源码以冻结 tag `p3.4` 为准。
  - **P4 验收口径**（`docs/28`，`p4.3`）：工作包交付 ≠ 门禁闭合 ≠ 候选可用。台账
    `fixtures/p4/acceptance.json` 记录 8 个门禁（6 pass + G05/G06 partial）与 8 条承接项，
    `tools/p4-acceptance.py --check` 与 `check-release.py` 的 **C9** 会挡住"pass 却挂未闭合项"
    "承接项没有退出证据" "阻断项在验收文件里被顺手写闭" 三种写法。

## 目录结构

```text
translator-sdk/
├── crates/
│   ├── translator-types/       # 请求/结果/错误/语言标签/能力描述（零外部依赖）
│   ├── translator-plugin-api/  # Engine / Provider / Backend trait + 注册表（含 mocks）
│   ├── translator-core/        # 协调器、路由、缓存、文本预处理、内存预算、术语契约、诊断
│   ├── translator-analysis/    # 语言识别（当前为启发式，可换 fastText）
│   ├── translator-rules/       # 声明式规则包：编译、金样本校验、执行
│   ├── translator-dictionary/  # 词典包读取、FST+SQLite 查询、引擎实现
│   ├── translator-resource/    # 资源协议（v1/v2）+ 安装/激活/回滚仓库 + 租约 + 跨进程锁
│   ├── translator-runtime/     # P3.1-A：worker 进程边界（帧协议 / 生命周期 / 模型校验）+ mock worker
│   ├── translator-nmt-bergamot/# P3.1-E：真实本地 NMT 引擎（保护计划 + 进程传输 → TranslationEngine）
│   └── translator-api/         # 唯一对外入口 + Builder
├── apps/
│   └── translator-cli/         # 调试与演示命令行
├── tools/
│   ├── pack-dictionary/        # CC-CEDICT → 资源包（payload v1/v2）
│   ├── migrate-rules/          # 规则包离线迁移（pipeline_version 1 → 2）
│   ├── p33-bench/              # P3.3：性能/质量测量工具（九个场景，输出带口径的 JSON）
│   ├── license-audit.py        # P3.4：许可清单生成（--write）与策略校验（--check）
│   ├── check-release.py        # P3.4：发行红线七项检查（权重不入库 / 许可一致 / 打包清单 …）
│   ├── bench-gate.py           # P3.4：CI 的宽松性能上限（只抓数量级退化）
│   └── p3/                     # P3.0 原生参考链路：构建/取模型/基线采集脚本 + CLI 补丁（非 cargo 包）
├── fixtures/                   # 金样本 + 示例规则包（rules/minimal.json）+ 示例术语表（terms/zh-en.json）
│   └── nmt/                    # P3.0：NMT 金样本、模型锁定记录、配置模板、基线 JSON（权重不入库）
├── licenses/                   # P3.4：第三方依赖清单 + 模型/运行时资产清单（机器可读）
├── resources/                  # 打包产物（体积大，gitignore）
├── THIRD_PARTY_NOTICES.md      # P3.4：发行物里的人可读第三方声明
├── CHANGELOG.md                # 每阶段变更与证据位置（tag 名 = 阶段名）
└── Cargo.toml                  # workspace（所有 crate publish = false）
```

依赖方向严格单向：

```text
translator-types  ←  translator-plugin-api  ←  translator-core  ←  translator-api  ←  CLI
                                                   ↖ translator-analysis
                                                   ↖ translator-rules（实现 core 的 TextRules 契约）
translator-dictionary  ←  translator-api（仅 `dictionary` feature）
translator-rules       ←  translator-api（仅 `rules` feature；内含术语表编译）
translator-resource    ←  translator-api（仅 `resource` feature）
translator-nmt-bergamot ←  translator-api（仅 `nmt-bergamot` feature）+ CLI（仅 `nmt` feature）
translator-runtime     ←  translator-nmt-bergamot（唯一上层消费者）
```

`translator-types` 与 `translator-plugin-api` **不依赖任何** native runtime、HTTP 客户端或 SQLite。
`translator-dictionary` 是唯一引入 SQLite（`rusqlite` + `bundled`）的 crate，且只在启用
`dictionary` feature 时才被编进 `translator-api`。

## 快速开始

需要 Rust 工具链（本机装在 `/opt/rust`）：

```bash
source /opt/rust/env.sh
```

构建与测试：

```bash
cargo build
cargo test --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo fmt --all --check
```

运行 CLI：

```bash
cargo run -p translator-cli -- translate hello --to zh-Hans --explain
cargo run -p translator-cli -- translate "good morning my friend" --to zh-Hans --explain
cargo run -p translator-cli -- translate elephant --to zh-Hans        # 词典未命中 → 回退 NMT
cargo run -p translator-cli -- translate hello --to zh-Hans --policy remote   # 应被拒绝
cargo run -p translator-cli -- lookup cat --lang en
cargo run -p translator-cli -- detect "你好"
cargo run -p translator-cli -- capabilities
```

### 挂载真实词典包

`resources/zh-en/` 不入库，需本地生成（约 3 秒）：

```bash
curl -L -o /tmp/cedict.gz https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz
gunzip -c /tmp/cedict.gz > /tmp/cedict

cargo run --release -p pack-dictionary -- /tmp/cedict resources/zh-en \
  --id dictionary.cc-cedict.zh-en --version 0.1.0 --language zh-Hans --target en

cargo run --release -p translator-cli -- --dict resources/zh-en lookup 计算机
cargo run --release -p translator-cli -- --dict resources/zh-en translate 世界 --explain
```

宿主侧最小用法（不挂 `dictionary_package` 就退回内置 Mock 词表）：

```toml
translator-api = { path = "crates/translator-api", features = ["dictionary"] }
```

```rust
let translator = translator_api::TranslatorBuilder::new()
    .dictionary_package("resources/zh-en")
    .build()?;
```

词典包格式、基准数据（含一次“基准数据造假”事故记录）与限制清单见
`../docs/04_P1_字典与离线查词报告.md`。

### 挂载规则包

规则包是 JSON（开发格式）或带 magic+sha256 的二进制（发布格式），**每条规则必须带金样本**，
加载时会真跑一遍，跑不过就拒绝加载：

```bash
cargo run --release -p translator-cli -- --rules fixtures/rules/minimal.json translate "ｃａｔ" --explain
# → 规则包：id rules.zh-en.minimal v1 / 规则数 3 / 生效 norm.nfkc / 处理后 cat
# → 引擎 mock.dictionary  译文 猫

cargo run --release -p translator-cli -- translate "ｃａｔ"
# → 引擎 mock.nmt（没有规则时全角输入查不到词条，只能退化）
```

宿主侧只需一行（`rules_file` 失败会在 `build()` 报错，不会静默降级成"没有规则"）：

```rust
let translator = translator_api::Translator::builder()
    .dictionary_package("resources/zh-en")
    .rules_file("fixtures/rules/minimal.json")
    .build()?;
```

规则格式与设计取舍（为什么不做正则、为什么规则 trace 不进 `TranslationResult`）见
`../docs/04_P1_字典与离线查词报告.md` §5。

旧规则包（`pipeline_version: 1`）**不可执行**，但可以查看与迁移：

```bash
cargo run --release -p migrate-rules -- old.json --inspect     # 看元数据（不执行）
cargo run --release -p migrate-rules -- old.json --out new.json # 迁移（金样本不过则失败）
cargo run --release -p migrate-rules -- new.json --check        # 校验可执行
```

### 资源仓库：安装 / 激活 / 回滚

```bash
# 安装（v1 词典包无需重新打包）
cargo run --release -p translator-cli -- --store .resources resource install resources/zh-en
cargo run --release -p translator-cli -- --store .resources resource list
cargo run --release -p translator-cli -- --store .resources resource activate dictionary.cc-cedict.zh-en 0.1.0
cargo run --release -p translator-cli -- --store .resources resource rollback dictionary.cc-cedict.zh-en

# 直接从激活版本挂载词典（不用记路径）
cargo run --release -p translator-cli -- --store .resources --dict-id dictionary.cc-cedict.zh-en lookup 计算机
```

**安装新版本不会自动切换**激活版本，旧版本也不会被自动删除（P2.3 的租约只保护"正在用的"，
不负责自动回收）。

仓库自查（对账只报告，动手要显式）：

```bash
# state ↔ installed ↔ staging ↔ 租约 四方对账；有需要人介入的问题时退出码 1
cargo run --release -p translator-cli -- --store .resources resource audit
# 删掉"磁盘有目录、状态没记录"的孤儿（被租用的不会删）
cargo run --release -p translator-cli -- --store .resources resource audit --prune
```

### 租约：谁在用，谁登记

```bash
# 列出仓库里所有租约（"有效"= 持有进程还活着）
cargo run --release -p translator-cli -- --store .resources resource lease
# 回收已退出进程留下的租约
cargo run --release -p translator-cli -- --store .resources resource lease reclaim
# 有有效租约时删除会被拒绝，并在错误里列出持有者
cargo run --release -p translator-cli -- --store .resources resource uninstall <id> <version>
```

宿主侧挂载即持租约，`Translator` 存活期间该版本跨进程也删不掉：

```rust
let store = translator_resource::ResourceStore::open(".resources")?;
let translator = translator_api::Translator::builder()
    .dictionary_from_store(&store, "dictionary.cc-cedict.zh-en")  // 需要 feature `resource`
    .build()?;
```

诚实边界：租约是**协作式**的，绕过本 SDK 直接 `open()` 资源文件的程序检测不到；
无法判定进程存活时按"占用"处理（宁可要求人工 `reclaim`，不删正在被 mmap 的文件）。
存活判定用「pid + 进程启动标识」（Linux `/proc/<pid>/stat` 的 starttime），因此 **pid 被复用**
不会被误当成"还有人在用"；非 Linux 平台没有可移植 API，一律返回"无法判定"。

### 内存预算

```bash
cargo run --release -p translator-cli -- --dict resources/zh-en --memory-budget 1 memory   # 直接被拒
cargo run --release -p translator-cli -- --dict resources/zh-en --memory-budget 64 memory  # 显示占用
```

预算在 `build()` 装配期判定，超限返回 `OutOfBudget`。数字是各组件**自报的下界估计**
（索引大小 + 配置的页缓存 + 一个写死的连接开销），**不是 RSS**：
真实包实测 RSS 约 24 MB，而记账只有约 3 MB。所以它只能回答"肯定不够"，不能回答"够用"。

### 真实本地 NMT（Bergamot，可选 feature）

默认构建**不需要**任何模型或 native 库；要用真实模型必须显式开 feature 并给出路径：

```bash
export NMT_BERGAMOT_BIN=/path/to/bergamot          # native 可执行文件
export NMT_MODEL_DIR=/path/to/models               # 五文件模型目录
export NMT_WORKER_BIN=$PWD/target/debug/nmt-worker  # 需要先 cargo build -p translator-runtime --bin nmt-worker
export LD_LIBRARY_PATH=...                          # native 的依赖（Marian/OpenBLAS 等）

cargo run -q -p translator-cli --features nmt -- translate 'Hello world!' \
  --from en --to zh-Hans --policy local --diagnose --nmt
# → 译文 你好,世界! / 引擎 nmt.bergamot.cli / 模型 nmt.enzh.mozilla@3.0#4e5accc1… 
#   运行时 bergamot-cli / 保护 protect-seg-v1 / 计划摘要 dcc88e56fc890b9a
```

- `--nmt` 打开引擎；`--nmt-native/--nmt-model-dir/--nmt-worker/--nmt-snapshot-root/--nmt-protection/
  --nmt-threads/--nmt-beam/--nmt-source/--nmt-target` 覆盖对应配置（优先级高于同名环境变量）。
- **保护策略**默认 `segment`（受保护跨度不进模型）；`--nmt-protection marker` 是可选对照实验，
  失败会明确报 `IntegrityFailure`，不要把它当默认安全路径。
- 宿主侧等价写法：

```toml
translator-api = { path = "crates/translator-api", features = ["nmt-bergamot"] }
```

```rust
let mut config = translator_api::nmt_bergamot::BergamotNmtConfig::new(
    "/path/to/nmt-worker", "/path/to/bergamot", "/path/to/models",
);
config.runtime_build = "9271618".into();
let translator = translator_api::Translator::builder()
    .nmt_bergamot(config)
    .build()?;
```

真实模型测试不在默认门禁里（需要权重与环境变量）：

```bash
cargo test --workspace --all-features -- --ignored --test-threads=1   # 35 条
```

### 术语约束与诊断

```bash
# 默认 warn：只报告违规位置，一个字都不改（第三方术语表不该悄悄改用户译文）
cargo run --release -p translator-cli -- --terms fixtures/terms/zh-en.json \
  translate "计算器 computer 与 AI planet earth" --to en --from zh-Hans --diagnose
# enforce：显式开启才改写（只改裸文本，URL/代码/{变量} 不动）
cargo run --release -p translator-cli -- --terms fixtures/terms/zh-en.json --term-policy enforce \
  translate "计算器 computer 与 AI planet earth" --to en --from zh-Hans
# 源文触发：源文没有「计算器」时，computer 这条规则被跳过（不会误改正常词）
cargo run --release -p translator-cli -- --terms fixtures/terms/zh-en.json --term-policy enforce \
  translate "这台 computer 与 AI planet earth" --to en --from zh-Hans
```

```rust
let out = translator.translate_with_diagnostics(&req).await?;
println!("{}", out.diagnostics.summary());
for v in out.diagnostics.term.iter().flat_map(|t| t.violations.iter()) {
    println!("{v}");   // 「LLM」应为「large language model」（偏移 12）
}
```

术语表格式见 `fixtures/terms/zh-en.json`（含 `source_variants` 示例）；契约是 `TargetTermGuard`，
宿主可自己实现，不必开 `rules` feature。**字典 `lookup()` 与词典直出不受术语约束改写**（ADR-020）：
查词返回资源原样，最多给警告。详见 `../docs/07_P2.3-P2.5_租约预算与术语约束报告.md` §2.4–§2.5
与 `../docs/09_P2-S稳定化报告.md` §1.4。

### 词典 payload：v1 还是 v2

```bash
# 新包默认 v2（释义内联，查询约快一倍、库小 25%）
cargo run --release -p pack-dictionary -- /tmp/cedict /tmp/pkg --payload-schema dictionary.sqlite.v2
# 复现历史包
cargo run --release -p pack-dictionary -- /tmp/cedict /tmp/pkg --payload-schema dictionary.sqlite.v1
```

读取侧同时支持两种格式，打开时会校验清单声明与库文件 `PRAGMA user_version` 一致，
不一致直接拒绝（防止"改参数忘了同步清单"导致释义为空）。

## 设计要点（来自设计文档，已在代码中体现）

| 原则 | 落地位置 |
|---|---|
| 本地优先，远程**默认关闭** | `TranslationPolicy::AutoLocalFirst`；`MockRemoteEngine` 默认无授权，测试覆盖 |
| 查词与整句翻译分开建模（ADR-003） | `TranslationEngine` vs `DictionaryProvider` 两套接口 |
| 插件可替换，公共 API 不变 | 全部依赖 trait，`Coordinator` 不认识具体引擎 |
| 兼容性显式声明，不靠字符串猜 | `InferenceBackend::supports_model()` 返回 `CompatibilityReport` |
| 占位符丢失必须告警 | `TranslationWarning::PlaceholderRestoreFailed`，有测试覆盖 |
| 未安装资源要给出安装建议 | `TranslationError::suggested_packages`，测试覆盖 |
| 缓存键包含全部影响维度 | `CacheKey`：文本哈希 / 语言对 / 引擎 / 术语表 / 规则版本 / 解码配置 / 隐私域 / 资源指纹 |
| 结构化诊断不污染主结果（ADR-008/015） | `translate()` 返回类型不变；诊断走 `translate_with_diagnostics()` |
| 术语约束不得破坏占位符（ADR-009/015） | `TargetTermTable` 复用 `text::protected_spans`，有 3 个集成测试 |
| 预算不得宣传为硬上限（ADR-013） | 只做装配期准入 + 下界估计，文档与 CLI 输出都标注"不是 RSS" |
| 术语默认只报告不改写（ADR-015 增补） | `TermPolicy::default() == Warn`；`Enforce` 需显式开启，测试锁死默认值 |
| 查词不受术语改写（ADR-020） | `lookup()` 返回资源原样；输出侧约束只作用于整句译文 |
| 租约与卸载同临界区（ADR-022） | `acquire_lease`/`uninstall`/`cleanup` 共享 Mutex + `state.lock`，锁内重读状态 |
| 跨进程行为用独立进程验证 | `crates/translator-resource/tests/cross_process.rs`（线程无法构造锁/僵死/PID 复用） |

### P3.0：原生参考链路（真实模型，不是 SDK）

```bash
# 1) 构建 Bergamot（约 6 分钟；钉 commit 9271618）
./tools/p3/build-bergamot.sh /tmp/p3

# 2) 按 fixtures/nmt/models.lock.json 下载并逐个校验 sha256（约 40 MB）
./tools/p3/prepare-model.sh /tmp/p3/models

# 3) 一句话试试真实翻译
printf 'Hello world!' | /tmp/p3/bergamot-translator/build/app/bergamot \
  --model-config-paths /tmp/p3/models/config.bergamot.yml --cpu-threads 1
# → 你好,世界!

# 4) 基线：24 条金样本 + 冷启动 + 100 次热翻译 + RSS
python3 tools/p3/nmt_baseline.py --bergamot /tmp/p3/bergamot-translator/build/app/bergamot \
  --config /tmp/p3/models/config.bergamot.yml --samples fixtures/nmt/golden-v1.jsonl \
  --threads 2 --warm-iters 100 --out /tmp/p3/baseline.json
```

**这条链路只用于验证"模型确实可用"，`translator-api` 还没有接入真实模型**（P3.1）。
模型权重不入库（43 MB + 许可未确认），复现靠 `models.lock.json` 里的坐标与 sha256。

### P3.1-A：worker 进程边界（mock，不接真实模型）

```bash
cargo test -p translator-runtime           # 20 单元 + 17 集成（跑真实子进程）

# 可复现证据：IPC 分位数、宿主/worker RSS、崩溃隔离、受控重启、shutdown 延迟
cargo build -p translator-runtime --release --bins
cargo run -p translator-runtime --release --example p3_1a_evidence
```

**这里没有任何真实翻译**：mock worker 的译文是 `[目标语言] 原文`。
（P3.1-B 之后 `--bergamot` 已经是真路径，见下一节。）

### P3.1-B：真实 Bergamot（需要本机有权重与 native 二进制，均不入库）

```bash
export NMT_BERGAMOT_BIN=/tmp/p3/bergamot-translator/build/app/bergamot
export NMT_MODEL_DIR=/tmp/p3/models
export LD_LIBRARY_PATH=/tmp/p3/sysroot/usr/lib/x86_64-linux-gnu:/tmp/p3/sysroot/usr/lib/x86_64-linux-gnu/openblas-pthread

# 12 条真实验收（默认 #[ignore]，因为权重不在仓库里）
cargo test -p translator-runtime --test real_bergamot -- --ignored --test-threads=1

# 实测基线（可写 JSON；不加 NMT_BASELINE_OUT 就只打印）
cargo build -p translator-runtime --release --bins
NMT_BASELINE_OUT=fixtures/nmt/baseline-p3.1b-linux-x86_64-1thread.json \
  cargo run -p translator-runtime --release --example p3_1b_evidence
```

约定：默认 `cargo build` **不需要**任何 C++ 依赖（native 是外部二进制），
`--all-features` 也不会把 Bergamot 拖进编译。

## 尚未实现

按"会不会阻断发行"排序，不是按工作量。

### 阻断发行（P3 = Conditional 的原因）

- **模型权重再分发许可结论**：Remote Settings 记录里没有 license 字段，`mozilla/translations`
  代码是 MPL-2.0，但权重的再分发条款要模型卡/法务确认。当前结论是"不打包、不缓存、不分发"，
  所以**不存在**"开箱即用的离线翻译包"这种交付物。
- **`LICENSE-MIT` / `LICENSE-APACHE` 正文未入库**（历史项；P7 已补正文并关闭 D-46）。
- **CI 未在真实 runner 上跑过**：workflow 与本地等价命令都在，但本仓库没有远端（D-47）。
- **真实模型 job 未运行**：需要一台有权重与 native 二进制的自托管机器（D-50）。

### 功能缺口

- **取消在途请求**：有排队绝对截止时间（`Timeout{operation:"queue"}`）与卸载等待，
  但没有"取消一个已经在 native 里跑的请求"（G06）。
- **FIFO 排队**：当前并发恒为 1，排队靠 `try_lock` 竞争、**非公平**；实测并发 10 的 p95 达 1.18 s。
  修法是票据队列 + 条件变量，属于并发语义重新设计（`docs/20` D-42）。
- **真 `kill -9` 恢复测试**：现在用 `abort()` 模拟"进程在任意点死掉"（D-53），
  对"留下半成品目录"等价，对信号/时序不等价。
- **断电一致性加固**：文件与父目录 fsync 已做（仅 Unix），但电源级掉电仍未验证。
- **真冷盘加载**：清 page cache 需要 root，本轮所有加载数字都是"页缓存较热"口径（D-40）。

### 归后续阶段

- 第二后端的**缓存**（`plan-v1` 摘要未覆盖 ORT，P4-G05 部分）、**worker 隔离**（仍跑在宿主进程内，D-60）、
  **取消/超时**（`timeout_ms` 被忽略，D-63）、**Marker 保护模式**（显式拒绝，D-61）—— P4 遗留项
- **词表外字符保真**（emoji 等退化成下划线，D-71）与**片段级幻觉**（单个 `!` 也能被译成整句脏话）—— 候选模型能力问题
- **`timeout_ms` 对 ORT 后端不生效**（接受参数但不取消，D-63/R-08）：宿主只能在外层超时丢弃结果，
  而"宿主不再等"不等于"底层停止计算"。`lc04` 这条测试就是钉住这个现状的，实现 deadline 后它会故意失败
- 小时级长稳与并发压力（本轮只做了 5+100 次，D-74）—— P5 台架
- ncnn 分支的可行性（本轮只验了 ONNX Runtime；按 `docs/23` §4，ncnn 不通过也不阻塞 P4 结束）
- Windows / macOS / Android 的构建与真机测量（当前非 Linux 一律 `CapabilityUnavailable`）—— **P5**
- 远程 LLM Provider、凭据注入与隐私开关 —— **P6**
- 多 worker / 多 native 的容量规划（单实例 PSS 约 337 MiB）；内存**硬**上限与库内
  `ProcessMetricsProvider`（观测代码目前只在 `p33-bench` 里）—— P4/P6
- 真正的语言识别（fastText `lid.176.ftz`），当前是零模型启发式
- 除 CC-CEDICT 之外的词库（Kaikki / JMdict 各有解析器与许可审查）、en→zh 词库
- 术语表与 `glossary_id` 联动（多术语表选择属宿主策略）
- 规则包的其余能力：分句、词形；正则类匹配按 §7.3 的风险评估后决定不做
- 可读性的大规模人工评分（当前只做了 22 条抽样、单人打分，`docs/20` §5）

各项的具体原因与去向见 `../docs/04_P1_字典与离线查词报告.md` §8、§9
与 `../docs/21_P3.4报告.md` §4.1。

## P7 发行准备状态（2026-11-07，详见 `../docs/44`）

| 平台 / 形态 | 状态 |
|---|---|
| Linux x86_64（SDK / CLI / Bergamot worker） | 391 tests passed；fmt/clippy/license audit 通过 |
| Windows x86_64 | **P7.3 已复测**（`docs/45`）：344 passed / 0 failed / 60 ignored；ORT 24 passed；fmt/clippy 通过 |
| Android x86_64 模拟器（JNI v1） | 34 PASS / 0 FAIL / 1 PENDING（`.so` 重载受 ART 限制） |
| Android ARM64 | 仅交叉编译，运行未验证 |
| 真实 Android NMT | 未编译进 JNI 产物 |
| 模型权重 | **不分发**（G12 许可未确认） |

发行阻断台账：`G06`（取消在途请求）与 `G08`（外部 `kill -9` 恢复测试）已闭合；
`G12`（权重许可）与 `G11`（真实 CI runner）仍 open。`tools/check-release.py --release-mode`
因此继续 FAIL，当前裁定为 **Conditional / Internal Preview**。

## 无权重发行模式（当前唯一的合规形态）

SDK 本身**不含任何模型权重**，这是有意为之，不是"还没打包"：

```bash
# 1) 自行获取权重：按 lock 文件里的坐标下载，并逐个核对 sha256
#    （Remote Settings 的 translations-models-v2，5 个文件 / 49.5 MiB）
# 2) 自行构建 native（约 6 分钟；钉 commit 9271618）
tools/p3/build-bergamot.sh
# 3) 校验"我现在用的是哪版权重"
NMT_MODEL_DIR=/path/to/models python3 tools/check-release.py --verbose
```

- 权重与 Bergamot 二进制的许可状态、坐标、哈希：`licenses/model-assets.json`
- 第三方依赖许可：`licenses/third-party.json` + `THIRD_PARTY_NOTICES.md`
- 红线检查（CI 每次提交都跑）：`python3 tools/check-release.py`

**责任边界**：使用者自行获取权重并自行承担权利责任；SDK 不代下载、不代分发、不缓存上传。
Bergamot 以**独立进程**按路径启动（不链接、不打包），因此 MPL-2.0 的文件级 copyleft 不外溢到本 SDK。

## 许可证

- 本项目源码：`Apache-2.0 OR MIT`（见各 `Cargo.toml` 的 `license` 字段；正文见仓库根 `LICENSE-APACHE` / `LICENSE-MIT`）。
- 第三方 Cargo 依赖：见 `THIRD_PARTY_NOTICES.md` 与 `licenses/third-party.json`（172 个包，全部通过许可策略）。
- 模型权重：**许可未确认**，不随本仓库/发行物分发 → `licenses/model-assets.json`。
- 词典数据：本项目自建，来源见 `../docs/02`；资源包不默认继承本仓库许可。
