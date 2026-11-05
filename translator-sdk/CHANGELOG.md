# 变更记录

版本号策略：P3 期间一律 `0.1.0`（**不发布**，`publish = false`）；每次阶段结束打 tag，
tag 名就是阶段名（`p0` `p1` `p2` `p2s` `p3.0` `p3.1a` `p3.1b` `p3.1cde` `p3.2` `p3.3` `p3.4` `p4.0` `p4.1`）。
真正的语义化版本从 P7（SDK 发行）开始。

每条都标注证据所在：能复现的测试或 `docs/NN` 报告。

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
