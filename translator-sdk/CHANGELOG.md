# 变更记录

版本号策略：P3 期间一律 `0.1.0`（**不发布**，`publish = false`）；每次阶段结束打 tag，
tag 名就是阶段名（`p0` `p1` `p2` `p2s` `p3.0` `p3.1a` `p3.1b` `p3.1cde` `p3.2` `p3.3` `p3.4` `p4.0` `p4.1` `p4.2` `p4.3` `p5.0`）。
真正的语义化版本从 P7（SDK 发行）开始。

每条都标注证据所在：能复现的测试或 `docs/NN` 报告。

## P5 前置 — 环境勘察与 Windows 验证方案对比（tag `p5.0`，报告 `docs/29`）

无代码变更，只做勘察与结论（因此不改 `Cargo.lock`、不动任何 crate）。

### 勘察结论

- **本机**：ASUS X542URV 笔记本（i5-8250U 4C/8T / 31 GiB / 根分区在 931 G SATA SSD，
  实测 449 写 419 读 MB/s / 剩余 597 G / VT-x + KVM 可用 / qemu 8.2.2 + OVMF(secboot) + Samba 已装，
  缺 libvirt、virt-manager、swtpm、virtiofsd）。
- **Android 环境还在且能跑**：`~/Android/Sdk` 8.3 GB、emulator 37.2.12、platform-tools 37.0.1、
  AVD `gms2`（android-34 Play Store，x86_64，4 核 / 2 GB）；headless 启动后 **35 s 内 `boot_completed=1`**，
  `adb emu kill` 干净退出。**但只有 x86_64 镜像、没有 NDK** → P5 要的 ARM64 JNI/物理机基准仍缺（D-83）。
- **Windows 三条路**：本机 KVM（快照可回滚，适合破坏性测试）／实体机 SSH（唯一能给"物理机基准"）／
  CI `windows-latest`（成本最低，且能为 G11 供证）。报告给的是**组合**而不是三选一。
- **关键约束（D-84）**：`translator-runtime` 的平台门禁只认 `linux`，所以 Windows 上 **Bergamot worker 必然
  被拒**；而 ORT 后端没有平台判断、ONNX Runtime 支持 Windows。Windows 验证矩阵必须据此裁剪。

### 结论

建议顺序：**C（CI 加 windows job，立刻）→ A（本机 KVM 迭代 + 快照）→ B（实体机验收 + 物理机基准）**；
Android 侧先装 NDK 用 x86_64 AVD 打通 JNI 调用链，另备一台 ARM64 真机出基准。
路线选定后再落 ADR（"平台验证三层策略"）。

---

## P4 审查响应 — 验收口径修订与比较方法修正（tag `p4.3`，报告 `docs/28`）

### 新增

- `fixtures/p4/acceptance.json`（`sdk-p4-acceptance/v1`）：P4 的**有条件验收台账**——工作包交付、
  门禁闭合（G01～G08 各自 pass/partial 与未闭合项）、8 条承接项（含 `priority/owner/exit_evidence`）、
  以及对 `release-blockers.json` 的**只引用不改写**。
- `tools/p4-acceptance.py --check` + `check-release.py` 的 **C9**：把验收口径做成机器可查
  （pass 不得挂未闭合项；partial 的每项必须进台账并带退出证据；阻断项引用必须与台账一致且仍 open）。
- `crates/translator-nmt-bergamot/examples/bergamot_bench.rs`：与 `p4_bench` **逐字同文本**的 Bergamot 基线，
  内存按**进程树**合计（worker 是 Rust 进程，吃内存的是它拉起的 native）。
- `fixtures/nmt/baseline-bergamot-same-texts-linux-x86_64.json`、
  `fixtures/p4/explain-499-token-budget.json`、`fixtures/p4/acceptance.json`。

### 变更

- `p4_bench` 升 `p4-ort-bench/v2`：每档记录 `text_sha256` / `src_tokens` / `token_cap` /
  `tokenizer_sha256` / `decode_policy` / `benchmark_revision`；并修正两处**测量污染**
  （每档新建分词器使 PSS 虚高约 70 MiB；Bergamot 侧只读 `/proc/self` 少算约 280 MiB）。
- `OrtIdentity` 增加 `decode_policy`（`DECODE_POLICY` 常量）：同一权重换解码策略就是另一个身份，
  为 G05 的 `plan-v1` 扩展备料；`token_cap()` 公开，保证测量与被测代码同一公式。
- `docs/25` §5：按 `fixtures/release-blockers.json` 更正 G06/G08/D-46 的含义并留勘误；
  §2.4.1 补 499 字跨阶段口径拆解（token 预算，非性能退化）。
- `docs/26`：A/B 表改用同文本重测数字（延迟 **10.9～15.0×**、PSS **1.63×**，旧值 15.7～26.1×/1.7× 作废）；
  卸载措辞收紧为"释放部分内存、未回到基线"；`13/13` 拆为 PASS 11 / EXPECTED_UNSUPPORTED 1 / OPEN 1；
  §4.4 明确"工作包交付 ≠ 门禁闭合 ≠ 候选可用"。
- `timeout_ms` 能力披露（crate 文档 + README + 报告）：接受参数但不生效，`lc04` 是实现后会失败的钉子。

### 结论

- `docs/27` 的 R-01～R-08 全部落实；P4-G05/G06 的未闭合项全部进承接台账（不再只写在散文里）。
- 候选替换 P3 的判定不变：**no-go**；默认路径不变（两个 feature 默认关闭）。
- 五项发行阻断项仍 **open**，`--release-mode` 仍 FAIL（按设计）。

---

## P4-05 / P4-06 / P4-07 — 结构回归、对比基准、结论与回退（tag `p4.2`，报告 `docs/26`）

### 新增

- `crates/translator-nmt-ort/tests/p4_contract.rs`：SP-01～12 与 LC-01～10 的真实模型回归（13 条，`#[ignore]`）。
  判定口径与 Bergamot 那套一致（字节级结构比较）；**不适用项写成可执行事实**，不假装通过
  （`lc04_超时字段当前被忽略` 会在超时真实现时故意失败）。
- `examples/p4_soak.rs` + `fixtures/nmt/baseline-p4-ort-soak-linux-x86_64.json`：加载/卸载循环与持续翻译的
  RSS/PSS 漂移（进度打 stderr，不污染 JSON 基线）。
- `fixtures/p4/derived-tokenizer.json`：派生资产（`tokenizer.json`）的"生成脚本 + 产物"双 sha256。

### 修复

- **分词器静默丢字符**：`tokenizer.json` 的 `unk_id` 由 `None` 改为 `1`（`<unk>`）。
  原行为会让词表外字符（emoji/生僻字）被直接丢弃——只剩 emoji 的片段变空 token 序列报 `InvalidRequest`，
  含 emoji 的正常句子则悄悄少字符。`tools/p4/build-tokenizer.py` 已同步并写进注释。

### 变更

- `fixtures/nmt/baseline-p4-ort-linux-x86_64.json` 用新分词器重测（118 / 657 / 2763 ms；PSS 575.0 MiB）。

### 结论

- P4-05：SP/LC 适用项 **13/13 通过**；SP-08/09（标记模式）与 LC-04/05/07/08 **按设计不适用**，逐条记原因。
- P4-06：热路径 118 / 657 / 2763 ms；100 次持续翻译后 RSS **持平**在 598 MiB（不涨），卸载回落到 249 MiB；
  与 Bergamot 的差距 15.7～26.1×、PSS 1.7×——**B 类比较，不得归因于 runtime**。
- P4-07：整体 **`conditional-go`**。插件替换可行性 = `go`；"用它替换 P3 后端" = **`no-go`**
  （长句译文不可用：`model weights`→`模型重量`、单个 `!` 片段幻觉成脏话）。默认状态即回退（两个 feature 默认关闭）。

---

## P4-03 / P4-04 — 第二个真实 NMT 后端与插件替换验证（tag `p4.1`，报告 `docs/25`）

### 新增

- `crates/translator-nmt-ort`：`nmt.ort.opus-mt` 引擎（ONNX Runtime + opus-mt int8）。
  契约与 Bergamot 后端完全一致；`OrtIdentity` 给出三文件摘要 / runtime build / 线程 / token 上限。
- `translator-api`：feature `nmt-ort`（只编契约）/ `nmt-ort-runtime`（链接 ORT），
  `TranslatorBuilder::nmt_ort()`、`Translator::nmt_ort_engines()`、`Translator::unload_nmt_ort()`。
- `tools/p4/build-tokenizer.py`：`source.spm` + `vocab.json` → `tokenizer.json`，并在写文件**之前**用
  `Hello world! → [3828, 431, 50]` 自检 id 空间（门禁阶段就是在这里被骗过一次）。
- `examples/p4_bench.rs`（延迟/内存基线）、`examples/dump_io.rs`（打印 ORT 会话真实 I/O 名）。
- `fixtures/nmt/baseline-p4-ort-linux-x86_64.json`、`fixtures/p4/python-merged-parity.json`：同文本对照证据。
- 真实模型测试 11 项（`p4_ort.rs` 6 + `nmt_ort_facade.rs` 5），全部 `#[ignore]`，需 `P4_ORT_MODEL_DIR`。

### 变更

- `ORT_STEADY_BYTES` **420 → 600 MiB**：门禁阶段是照 Python 宿主 RSS 折算的估计，Rust 进程内实测
  PSS 574.8 / RSS 578.1 MiB；准入估计偏低会让宿主高估可用内存。
- 解码加入退化抑制（长度上界 + 无重复 3-gram + 三连停 + 尾部收敛），触发次数经 `degeneracy_count()` 暴露；
  **不**新增 `TranslationWarning` 变体（会破坏宿主穷尽匹配）。
- `Translator` 在语言对不符时**自己**报错，不再只依赖 `Coordinator` 的 `supports()` 预检。
- `tools/freeze-manifest.py --check`：`Cargo.lock` 改为按**冻结提交**比对，工作区漂移只打印 `DRIFT`
  （原实现按 HEAD 比，第一次正常新增依赖就会误报）。
- `tools/license-audit.py`：白名单新增 `CDLA-Permissive-2.0`（`ort-sys` 的 build-dependency `ureq`
  需要根证书数据，只在构建期用于下载 ORT 预编译包）；清单 53 → 152 个包。

### 结论

- P4-G01/G02/G03/G04/G07/G08 通过；**P4-G05（计划摘要）/ P4-G06（隔离、租约、崩溃恢复）部分通过**。
- 与 P3 基线（Bergamot）的距离：延迟 15.7～26.1×、PSS 1.7×；**B 类比较，不得归因于 runtime**。
- 质量：短句可用、长句不可用（`model weights` → `模型重量`）。这是**插件替换可行**，不是"可以替换 P3 后端"。

---

## P4-01 / P4-02 — P3 冻结与候选门禁（tag `p4.0`，报告 `docs/24`）

### 新增

- `tools/freeze-manifest.py` + `fixtures/p3-freeze/manifest.json`：冻结清单与 `--check` 自校验
  （tag 移动、`Cargo.lock`/基线 JSON 变化、模型哈希不符、台账条目被删都会失败）。
- `fixtures/release-blockers.json`：G12 / G11 / G06 / G08 / D-46 五项开放台账（含关闭条件与现有证据）。
- `fixtures/p4/candidate.lock.json`：第二后端候选（`ort + opus-mt-en-zh`）的坐标、sha256、ONNX 签名、
  分词桥接规则、已验证译文、实测延迟/内存与门禁结论。

### 变更

- `licenses/model-assets.json`：许可状态改为显式状态机 `pending → reviewed_allowed / reviewed_denied`，
  解除禁发必须同时填审核人/时间/适用范围/证据。
- `tools/check-release.py`：C3 增加状态机一致性校验；新增 **C8 发行阻断台账** 与 **`--release-mode`**
  （开发模式不阻断 CI，发行模式遇 open 阻断项直接 FAIL）。

### 结论

- P3 冻结完成；P4-02 门禁 = **go-with-caveats**（真实译文、许可明确、资产锁定；但性能与内存明显差于基线，
  且本轮是 Python PoC，Rust 插件未实现 → P4-G01/G02 未满足）。

---

## P3.4 — Linux SDK 发布候选（tag `p3.4`，报告 `docs/21`）

### 新增

- `licenses/third-party.json`：53 个 Cargo 依赖的许可清单（SPDX 表达式 + checksum），随发行物一起给出。
- `licenses/model-assets.json`：模型权重与 Bergamot 二进制的坐标 / 哈希 / **许可状态与再分发结论**。
- `THIRD_PARTY_NOTICES.md`：人可读的第三方声明，含"为什么按路径启动能避开 MPL-2.0 传染"的说明。
- `tools/license-audit.py`：清单生成与策略校验（`--write` / `--check`），把非 SPDX 的 `MIT/Apache-2.0`
  按 OR 解释并**显式标记**，不悄悄归一化。
- `tools/check-release.py`：发行红线 7 项检查（权重不入库、许可清单一致、模型坐标三方一致、
  打包清单不含权重、`.gitignore` 覆盖、红线文案存在、可选的本机权重哈希核对）。
- `tools/bench-gate.py` + `fixtures/nmt/gate-ci.json`：CI 的**宽松上限**性能门禁（只抓数量级退化）。
- `crates/translator-runtime/src/platform.rs`：平台门禁从 `#[cfg]` 分支改成可单测的判定表；
  非 Linux 明确返回 `CapabilityUnavailable`，并首次有测试覆盖这条路径。
- `.github/workflows/ci.yml`（默认门禁 + `release-redline` 两个 job）
  与 `.github/workflows/real-model.yml`（自托管、手动触发、缺前置就快速失败）。
- 所有 crate `publish = false`：仓库 URL 仍是占位符，且内部依赖是 path-only，
  不允许出现"某个 crate 被误发到 crates.io"的情况。

### 变更

- `translator-runtime` 的平台检查统一走 `platform::ensure_supported()`；
  错误信息现在会带上**具体平台名**。

### 明确没有做

- 权重许可结论：**仍然是"未确认"**，因此不打包权重、不宣称可分发的离线模型包（`docs/17` P3-G12 仍阻断）。
- `LICENSE-MIT` / `LICENSE-APACHE` 正文未入库（只声明 SPDX），已在 `docs/21` 记为待办。

---

## P3.3 — Linux 性能、质量与低内存（tag `p3.3`，报告 `docs/20`）

### 新增

- `tools/p33-bench`：九个测量场景（`empty` / `dictionary` / `worker` / `load` / `nmt` /
  `concurrency` / `unload` / `sustain` / `quality`），全部输出带 `caliber`（口径）字段的 JSON。
- `fixtures/nmt/baseline-p33-*.json`：上述场景的基线（权重不入库，只有数字）。
- 质量评分器：结构（字节级）/ 事实（数字串）/ 代理指标**三层分离**，不合成"质量分"。
- 首次采集 **PSS**（`/proc/<pid>/smaps_rollup`）与进程树内存，替代"RSS 相加"。

### 修复（由测量暴露）

- worker 启动 native 时设置 `OPENBLAS_NUM_THREADS` / `OMP_NUM_THREADS`（宿主显式设置则不覆盖）：
  并发 10 的吞吐 15.0 → 31.0 rps，p50 139 → 35 ms；499 字单句 p50 340 → 176 ms。
- 排队等待的固定 1 ms 轮询改成 20 µs→500 µs 指数退避（超时语义不变）。
- `PlanCache` 容量为 0（关闭）时不再累加 miss 计数。

---

## P3.2 — 资源生命周期与一致性（tag `p3.2`，报告 `docs/19`）

### 新增

- 安装/升级/卸载的**崩溃注入**（5 个 crash point，`TRANSLATOR_TEST_CRASH_POINT`），
  以及 `recover()` 对孤儿目录 / 失效记录 / 回退激活版本的清理。
- 反向依赖保护：`dependents_of()`、卸载拒绝、`cleanup()` 保留、`audit()` 报告 `broken_dependencies`。
- `PlanCache`（按 `plan_digest + target + text_sha256` 的 FIFO 缓存，0 = 关闭，只缓存成功）。
- 快照/worker 目录的 **owner 标记 + 存活判定**（含 PID 复用场景），宿主与 worker 双向清扫。
- `BergamotNmtEngine` 的加载状态、缓存统计、空闲卸载（`idle_for` / `evict_if_idle`），
  并透传到 `translator-api` 与 CLI 的 `--diagnose`。

---

## P3.1 — C/D/E 集成（tag `p3.1cde`，报告 `docs/18`）

- 结构保护（`protect-seg-v1` 分段为默认、`protect-mark-v1` 可选），执行计划与 `plan-v1` 摘要。
- 真实 NMT 接进 `Translator::translate()` 与 CLI（feature `nmt-bergamot` / `nmt`），默认依赖树不含 native。
- single-flight 加载、排队 deadline、unload 与在途请求的协调。

## P3.1-B — 真实 Bergamot worker（tag `p3.1b`，报告 `docs/15`）

- 三层进程链路（宿主 → worker → bergamot），真实译文、崩溃隔离、受控重启。

## P3.1-A — Worker 隔离与 IPC（tag `p3.1a`，报告 `docs/13`）

- `translator-runtime`：长度前缀 JSON 帧、协议版本校验、8 MiB 上限、超时与终止升级。

## P3.0 — 原生参考链路（tag `p3.0`，报告 `docs/11`）

- 锁定 Mozilla en→zh-Hans v3.0 权重与 Bergamot commit `9271618`，拿到真实译文与首份性能基线。

## P2 — 资源仓库、租约与术语（tag `p2`，报告 `docs/07` `docs/09`）

- 资源仓库（安装/激活/回滚/状态对账）、租约、内存预算、术语约束、诊断。

## P1 — 离线词典（tag `p1`，报告 `docs/04`）

- `translator-dictionary`：FST 索引 + SQLite 释义，词典包 v1/v2，`pack-dictionary` 打包工具。

## P0 — 骨架（tag `p0`，报告 `docs/03`）

- 11 个 crate 的分层骨架、插件契约、Mock 引擎、CLI 雏形。
