# 变更记录

版本号策略：P3 期间一律 `0.1.0`（**不发布**，`publish = false`）；每次阶段结束打 tag，
tag 名就是阶段名（`p0` `p1` `p2` `p2s` `p3.0` `p3.1a` `p3.1b` `p3.1cde` `p3.2` `p3.3` `p3.4` `p4.0` `p4.1` `p4.2` `p4.3` `p5.0` `p5.1` `p5.2` `p5.3` `p5.4` `p5.5` `p5.6` `p5.7` `p6.0` `p6.1` `p6.2` `p7.0`）。
真正的语义化版本从 P7（SDK 发行）开始。

每条都标注证据所在：能复现的测试或 `docs/NN` 报告。

## P7-00 — 基线与测试矩阵（tag `p7.0`，报告 `docs/43`，计划稿 `docs/42`）

不改代码、不跑 Android 测试。只做只读核对 + 判据冻结。

- 基线：tag `p6.2` / `1397657`；Rust 1.99.1；NDK 30.0.16248370；jni 0.21.1；ORT（Windows）= directml 静态包。
- 依赖：`cargo tree -p translator-api --no-default-features` 命中 `ort|onnx|reqwest|hyper|tauri|tokio` = **0**；
  `translator-api` **无反向依赖**（JNI 不在其依赖图）；`translator-jni` 只开 `dictionary/mocks/resource/rules`。
- **阻断项按台账纠偏**（旧报告措辞有误）：G12 权重许可未确认 / G11 CI 未真实 runner 运行 /
  **G06 = 不支持取消在途请求** / **G08 = 恢复测试用 `abort()` 而非外部 `kill -9`** / **D-46 = LICENSE 正文未入库**。
  其中 D-46 是补文件级、G06 才是功能缺口。
- 冻结 J-07/J-10/J-14/J-15 的注入方法与期望错误码（含 J-15c `.so` 重载标 PENDING：ART 类加载器限制）。
- 两处待批资源（360.9 MB ORT 旧缓存、3.1 GB arm64 镜像 + AVD）**未删未移未改**。

## P10.1-A3/A4 — 补充验证（报告 `docs/59A`）

- 10,000 次：实际完成 **4,000**，Private 稳定 412.9–413.1 MiB；未完成，NM-03 PENDING。
- A4-1：A/B 各 20 次并发 0 错误，A unload 后 B 重载 212.6 ms，pool=1。
- A4-2：独立副本 tokenizer 变化后 shared Arc=false、cache identity 改变；原模型未改。
- A4-3：100 MiB 拒绝 / 650 MiB 通过；估计 600 MiB vs 实测 Private 413.1 MiB。
- NM-09 BLOCKED（同进程无隔离）；NM-07 N/A（无取消点）。

## P10.1-A — Windows ORT 真实模型 Soak（报告 `docs/59`）

- 新增 `p10_soak` example（共享 ORT、内存采样、unload/reload）。
- 1,000 次：0 错误；冷加载 704.4 ms；p50 185.76 / p95 651.08 ms；RSS 424.7 / Private 413.1 MiB；
  卸载后 Private 20.2 MiB；两个共享实例 `Arc::ptr_eq=true`、pool=1。
- 10,000 次未完成（PENDING）；ORT 无请求级取消（N/A）；模型内容切换/崩溃注入 PENDING。
- 测试代码/log 仅在 Windows 工作区与 `tests/p10`；无新模型下载。

## P10.0 — 模型/运行时/环境预检（报告 `docs/58`）

- 基线 `18592bf`；Linux 419/0/60 复跑通过。
- Windows ORT 模型三文件（encoder/decoder/tokenizer）大小与 SHA-256 已记录；ORT 24 用例沿用 P4/P7 证据。
- Bergamot 模型/native 不在本机；Android 无真实 NMT runtime；ncnn 无实现。P10.1 仅 Windows ORT 可立即执行。
- 未下载模型、未改功能代码、未写工作区外；G11/G12 仍 open。

## P9.5 — P9 收口与支持矩阵（报告 `docs/56`）

- P9.5-A：JNI 可选 fastText LID（路径白名单）、Android 真实识别/lemma/en→zh 样本、直接 Arc 指纹；
  Android **47 PASS / 0 FAIL / 1 PENDING**。
- P9.5-B：P9.0～P9.5 汇总、资源锁/再分发状态、v3 兼容、MSRV 差异、最终支持矩阵。
- **工程能力 Conditional；语言资源质量未达标**：en→zh 2/10、人工评审 PENDING、全量 No-Go、Lindera PENDING。
- Linux 419/0/60（all 423/0/66）、Windows 372/0/60（all 376/0/66）；G11/G12 仍 open。

## P9.4 — SDK 集成与跨平台回归（报告 `docs/55`）

- `TranslatorBuilder::language_identifier`/`lemmatizer` 注入；默认启发式、原词精确优先、lemma 只在未命中时兜底。
- Android JNI 接入 v3 JMdict 样本包：`lookup はし` 多义、双实例、detect、关 A 不影响 B；44 PASS / 1 PENDING。
- EP-02：`rules_file`/`term_table_file` 内容 SHA-256 进入资源指纹；2 条合同测试。
- Linux 417/0/60（all 421/0/66）、Windows 370/0/60（all 374/0/66）；G11/G12 仍 open。

## P9.2 / P9.3-A — LID 与英语词形（报告 `docs/54`）

- 新增可选 `translator-analysis/fasttext`（`fasttext-pure-rs` 0.1.0，MIT，纯 Rust）：`FastTextIdentifier`，
  40 条同集对照 accuracy/Macro-F1 均 0.975/0.9749；冷加载 44.9 ms、热 p50 0.0041 ms、加载 RSS +2.8 MiB。
- 新增 `EnglishLemmatizer` + `MorphologyAnalyzer`：规则 + 例外词表，8 tests；未接入词典查询（P9.4）。
- 未下载 Lindera；许可证审查仅到 crate MIT/词典另审。Linux 410/0/60（all 413/0/66）、Windows 363/0/60（all 366/0/66）。

## P9.1-S — 定向覆盖与同音索引修复（报告 `docs/53`）

- 新增 `dictionary.sqlite.v3`：`postings(key, rank, entry_id)` 支持一键多词条；v1/v2 仍只读兼容。
  JMdict v3 的 はし 返回 6 条（橋/箸/端/梯/愛し/波斯），橋/箸/端 各自独立；DI-09～13 通过。
- 额外 6×5 MiB Kaikki 未压缩 JSONL 分段（31,457,280 B；总 59,766,682 B < 64 MiB）；目标词仍只覆盖 light/close（2/10），
  停止扩大下载，评估其他合法数据源。
- 生成 100 条人工评审样本（50 Kaikki + 50 JMdict）；状态 **PENDING_HUMAN**，不冒充语言审查。
- Linux 409/0/60（all 412/0/65）；Windows 362/0/60（all 365/0/65）；全量词库 **No-Go**。

## P9.1 — 多语言词库小样本（报告 `docs/52`）

- 在 64 MiB 预算内下载 Kaikki 16 MiB 前缀、JMdict 10.6 MiB、fastText 938 KB（合计 28,309,402 B）。
- 新增 `tools/p9/import_kaikki.py` / `import_jmdict.py`；`pack-dictionary` 增加 `--format jsonl` 与 `--license-file`，
  修复“同读音不同词头被合并”的 bug；词典插件 id 改为包级唯一（多包注册）。
- 生成 Kaikki en→zh 950 条、JMdict ja→en 999 条包；DI-04/05/06/07/08 通过，DI-01 因目标词覆盖不足为 PARTIAL；
  `p33-bench dictionary` 支持 `--term/--lang`。
- Linux 404/0/60（all 407/0/65）、Windows 357/0/60（all 360/0/65）；资源仍内部研发、
  `redistribution=pending-review`；全量导入需再次审批。

## P9.0 — 语言资源准入（报告 `docs/51`，计划 `docs/50`）

- 只读复核现有 CC-CEDICT 与源码；确认 Kaikki/JMdict/fastText/Lindera 均未取得/未接入。
- 新增 `fixtures/p9/resource-lock.json`（6 项，未取得数据哈希为 null、许可/再分发 `pending-review`）与
  `tools/check-language-resources.py`（`--check` / `--self-test`），并加入 `ci.yml` 的无下载校验步骤。
- 未下载第三方数据、未新增依赖、未改 G11/G12；P9.1 需用户批准下载/许可判断后启动。

## P8 — 多实例、共享资源与缓存一致性（报告 `docs/49`）

- **P8.1 JNI 多实例**：`Entry` 自带 generation/closed，`index` 单调不复用；创建 B 不再作废 A；
  `capabilities.binding.multi_instance=true`；ADR-063 从 Proposed → **Accepted**。
  Android Harness M-01～M-06 通过，总计 **40 PASS / 0 FAIL / 1 PENDING**；P6 J-01/J-03/J-06/J-15 无回退。
- **P8.2 引擎共享**：`BergamotNmtEngine::shared` 与 `OrtNmtEngine::shared` 进程内池（只存 `Weak`），
  键覆盖模型文件 role+sha256 与配置；相同模型/配置的多个 `Translator` 共享 engine/worker 或 ORT 会话；7 条 shared 测试通过。
- **P8.3 缓存一致性**：`TranslationEngine::cache_identity` + `CacheKey.engine_cache_identity`；
  Bergamot 用 plan digest、ORT 用文件内容哈希、词典用 `content_digest`；RemoteOnly/本地缓存域隔离。
- Linux **401 passed / 0 failed / 60 ignored**（all-features 404 / 65 ignored）；Windows **354 / 0 / 60**（all-features 357 / 0 / 65）；
  fmt/clippy（默认+全 feature）通过。G12/G11 仍 open。

## P8-AUDIT（报告 `docs/48`，计划 `docs/47`）

- 只读核验当前 commit `429cc9b`：10 个工作包逐项落到源码/测试/判定，**未改功能代码**。
- 结论：P8.1 JNI 多实例 **Confirmed missing**（全局 generation 作废旧句柄）；P8.2 共享模型 / P8.3 计划缓存 **Partial**；
  P9（en→zh/Kaikki/JMdict、正式 LID/形态）、P10.2/P10.3（Android NMT、ncnn）、P11.1（真实 Remote）多数 **Confirmed missing**。
- 复跑：`cargo test --workspace` 391 passed / 0 failed / 60 ignored；专项 cache_fingerprint/routing/lifecycle/cancellation/J-07/JNI 全绿。
- P8.1 下一步：先写 M-01～M-06 失败合同，再按 ADR-063（Proposed）改 per-entry generation/closed。

## P7 验收与 P8 启动（报告 `docs/45` / `docs/46`）

- **P7 条件性验收通过**（Conditional / Internal Preview）；G12 方案①（暂不分发权重）、G11 两步走（仅本地准备）。
- **Windows P7.3 复测完成**：`344 passed / 0 failed / 60 ignored`（all-features 65 ignored）；ORT 24 用例通过；
  fmt/clippy 默认+全 feature 通过；G08 外部强制终止 1 passed；G06 在 Windows 为 N/A（Linux 进程组语义）。
- **G11 准备**：新增 `tools/ci-preflight.py`（无权重/凭据/大文件，详见 preflight 输出）并加入 `ci.yml`；
  未 push、未触发远程 CI，G11 保持 open。
- **P8 P0**：ADR-063（Proposed）——JNI 多实例改为 per-entry generation/closed，handle 格式与入口不变。

## P7-01…P7-05 — 质量收口与发行候选（报告 `docs/44`）

### 新增

- **G06 取消**：`CancelToken`；`Translator::translate_cancellable`；`ProcessTransport::translate_with_deadline_and_cancel`；
  worker 等待分片观察令牌，取消时终止整个进程组；JNI 新增 `nativeCall(handle,"cancel",{"request_id":...})`。
- **G08 外部 SIGKILL**：安装崩溃点改为写 marker 后等待外部 `SIGKILL`；测试断言 `signal=9`，recover 后旧版本逐文件校验。
- **J-10 边界**：JNI 文本 64 KiB / payload 256 KiB / lookup limit 1000 / JSON 深度与对象形状检查；控制字符明确拒绝。
- **J-07/J-15 Android**：坏包/穿越包、50 次句柄重建、并发 close、JNI 预取消；`PerfHarness` 采集 J-14。
- `tools/make-sbom.py`（CycloneDX 1.5，147 组件）；`tests/android-harness/p7/make_j07_repo.py`。

### 实测

- Linux：`cargo test --workspace` **391 passed / 0 failed / 60 ignored**；`--all-features` 391 / 0 / 65；
  fmt、clippy `-D warnings`、license audit（172 包）通过。
- Android `gms2`：**34 PASS / 0 FAIL / 1 PENDING（J-15c）**；J-14 35 次样本（create p50 0.209 ms，lookup p50 0.207 ms，
  RSS 113,964→116,632 KiB）。
- 阻断项：**G06/G08 关闭**（证据 `out/g06-contract.log`、`out/g08-external-kill.log`）；G12/G11 仍 open，
  `check-release.py --release-mode` 继续 FAIL；裁定 **Conditional / Internal Preview**。

## P6-01/P6-02 — JNI 绑定层与 P6 收口（tag `p6.1` / `p6.2`，报告 `docs/40` / `docs/41`）

### 新增

- `bindings/translator-jni`（cdylib + rlib）：契约 v1 四个导出符号；`"gen:index"` 代际句柄；
  锁内只查表 + `Arc::clone`、锁外调 SDK；`catch_unwind` 兜 panic；NUL 输入拒绝；`NewString(UTF-16)` 输出；
  `capabilities` 带 `runtimes[]` 如实报告未编译的 NMT；`mock_engines` 需显式开启。
- `tests/android-harness`（`javac → d8 → adb push → app_process`，**不用 Gradle**）：23 条断言，
  输出 `PASS/FAIL` + `SUMMARY`。
- 依赖 `jni 0.21`（MIT OR Apache-2.0）；`licenses/third-party.json` 重算为 **172 包**。

### 实测

- AVD `gms2`（android-34/x86_64）：**23/23 PASS** —— create/capabilities/close、中文+😀🚀、
  NUL 与空白拒绝、真词典 `你好→hello/hi`（cc-cedict）、多义不丢、空结果、
  重复关闭幂等、关闭后 `AlreadyClosed`、非法/过期句柄、资源清单、mock NMT 路由、
  `NetworkNotAuthorized`、坏 JSON/未知 method/缺字段。
- **并发关闭**：400 次 `lookup` 中主线程 60 ms 后 `close` → **389 ok / 11 `AlreadyClosed` / 0 崩溃**。
- J-11 `cargo tree -p translator-api` 命中 `jni` **0**；J-13 ARM64 cdylib：`AArch64` / `LOAD 0x4000` / 4 符号。
- J-12 Windows：`cargo test --workspace` **337 passed / 0 failed / 60 ignored**，clippy PASS。
- Linux：**379 passed / 0 failed / 60 ignored**；fmt / clippy / license-audit PASS；`--release-mode` 仍 **FAIL**。

### pending（未做，不算 PASS）

J-07 坏资源/路径穿越、J-14 模拟器 RSS/时延、J-15 装卸生命周期、APK/Instrumentation 载体、
真实 Android NMT、ARM64 运行。5 条发行阻断项仍 open。

## P6-00 — JNI 契约冻结与只读预检（tag `p6.0`，报告 `docs/39`，计划稿 `docs/38`）

不新增 crate、不改 `Cargo.lock`、不跑 Android 构建。只冻结契约 + 核对现状。

### 冻结的契约 v1（ADR-061）

- 4 个 native 入口：`nativeProtocolVersion` / `nativeCreate(configJson)` / `nativeCall(handle, method, payloadJson)` / `nativeClose(handle)`。
  方法级能力走 `nativeCall`（v1 四个 method：`capabilities` / `detect` / `lookup` / `resources`；`translate` 在 P6.2 加），**新增能力不改 ABI**。
- 句柄 = 字符串 `"generation:index"`（不是 `jlong`）：注册表查表 + 代际校验，过期返回 `AlreadyClosed`，不可能误当指针。
- 输出 `NewString(UTF-16)`，不用 `NewStringUTF`（Modified UTF-8 遇 Emoji 会与 Rust 的 UTF-8 冲突）。
- 输入含 `U+0000` → `InvalidRequest("文本含 NUL")`，不静默改写。
- 业务失败走结构化 JSON（沿用 12 个 `TranslationErrorCode`）；只有契约级错误抛 `IllegalArgumentException`；`panic` 在边界 `catch_unwind` → `BackendFailed`。

### 只读预检结论

- `rustc 1.99.1`、NDK `30.0.16248370`、`x86_64-linux-android` + `aarch64-linux-android` 均已就位；
  AVD `gms2`（android-34/x86_64）可作运行载体。
- `translator-api` 默认 feature 下 `cargo tree` 不命中 `http/tauri/reqwest/hyper` → P6.0 门禁 PASS。
- 仓库当前**没有任何 JNI crate / bindings 目录** → P6.1 需新建 `bindings/translator-jni`。
- 5 条发行阻断项仍 open，本阶段未动台账。

### 待批准（P6.1 开工前）

A 新建 `bindings/translator-jni`（改 workspace members + `Cargo.lock`）；B 新建 `tests/android-harness/`；
C 首次 Android 构建（启 AVD + `adb push` 到模拟器内）；D 若引入新依赖则增补 `licenses/`。

## P5-07 — Android 模拟器验证 + P5 收口（tag `p5.7`，报告 `docs/36` / `docs/37`）

### Android（`docs/36`）

- **真 SDK 跑在模拟器上**：交叉编译 `translator-cli`（x86_64 58.4 s / aarch64 43.0 s，含 `rusqlite` bundled
  的 SQLite C 编译），推到 AVD（android-34 / x86_64）后：
  `capabilities`（3 个 mock 引擎）、`detect 你好世界`（zh-Hans 0.60）、`translate`（路由到 mock.nmt）、
  `--dict` **真词典 lookup**（FST+SQLite，来源 cc-cedict）、`resource install/list/inspect/audit`
  三方一致、**卸载激活版本被正确拒绝**。
- **ARM64 只到编译合规**：ELF64 AArch64 / `LOAD` 对齐 `0x4000` / `NEEDED` 仅 libdl·libm·libc / SQLite 静态链接。
- **ARM64 运行被证伪**：装 `system-images;android-24;default;arm64-v8a`（3.1 GB）建 AVD 后启动 →
  `FATAL | QEMU2 emulator does not support arm64 CPU architecture`（新版模拟器只支持同架构镜像）。
- 物理设备性能（速度/耗电/温度/峰值内存）**未验证**；JNI 承载真 SDK（`translator-jni`）列入 P6。

### P5 收口（`docs/37`）

- 三分口径：工作包**完成** / 门禁**未闭合**（5 条阻断项仍 open，`--release-mode` 继续 FAIL）/ 候选**条件性可用**。
- 平台证据表：九项能力 × Linux / Windows / Android x86_64 / Android ARM64，凡未验证一律标 ❔，不因"能编译"算通过。
- 新增 **ADR-060**（Android = 模拟器功能 + 交叉编译合规；物理性能单列未验证）。

## P5-06 — P5.4 Windows 存活探测与跨进程资源验证（tag `p5.6`，报告 `docs/35`）

### 实现

- `translator-resource/src/leases.rs`：新增 `#[cfg(windows)] mod win_probe`，用 `kernel32` 四个函数
  （`OpenProcess` / `GetExitCodeProcess` / `GetProcessTimes` / `CloseHandle`），**零新依赖**；
  `ERROR_INVALID_PARAMETER(87)` → `Dead`、`ERROR_ACCESS_DENIED(5)` → `Unknown`、退出码 ≠ `STILL_ACTIVE` → `Dead`、
  `GetProcessTimes` 失败 → `Alive(None)`。创建时间（FILETIME）当启动标识，等价 Linux starttime。
- `process_start_id`(windows) 复用探测。

### 测试口径（解 gate 3 处 / 改断言 3 条 / 新增 3 条，无删除无跳过）

解 gate：`lc08`（5 个崩溃点）、`cross_process.rs` 全文件（6 条跨进程不变量）、`store::recover回收死进程的租约`。
改断言：`当前进程必然存活`、`不存在的pid视为已死`、`死进程的锁会被回收`（旧断言编码的是 probe 恒 Unknown 的旧语义）。
新增：已退出子进程判 Dead / 启动标识不一致视为不同进程 / 活着的子进程判 Alive。
**删掉一条自己写错的断言**：父子 start_id 必不同——Linux jiffies 10ms 粒度会撞（实测都是 19734338），
改为钉机制（`record_is_live(pid, start+1) == false`）。

### 结果

- Windows：`cargo test --workspace` **333 passed / 0 failed / 60 ignored**；`translator-resource` 80 passed
  （lib 30 + lifecycle 4 + store 40 + cross_process 6）；clippy `-D warnings` PASS。
- Linux：**375 / 0 / 60**，fmt + clippy 通过（无回退）。
- 差值 42 = 仍按平台 gate 的 Linux 专属用例（worker_process 17 / lifecycle 6 / Bergamot 配置 5 / transport 2 / 其余 unix）。

### D-115 实测闭环（2026-11-07）

`cargo clean -p ort-sys` → 设 `ORT_CACHE_DIR=$W\.build\ort-cache` → 重建：旧缓存**前后完全一致**
（5 文件/360.9 MB/mtime 未变），新缓存出现同样 5 文件/360.9 MB，链接路径改为工作区内，
`p4_ort` 6 passed + `p4_contract` 13 passed 用新缓存链接的二进制跑过。
新增 `tools/p5/windows-build-env.ps1`（点源即把 CARGO_HOME/CARGO_TARGET_DIR/TEMP/ORT_CACHE_DIR/HF_HOME 指向工作区）。
旧 360.9 MB 已可安全删除，待用户批准。

### D-115 结案（只读调查）

`ort-sys` 的 `internal::dirs::cache_dir()` **优先读 `ORT_CACHE_DIR`**，其次才是平台默认目录。
→ 后续 ORT 构建只需在构建进程内设 `$env:ORT_CACHE_DIR = '<workspace>\.build\ort-cache'`，
无需注册表/junction/系统环境变量。旧 `%LOCALAPPDATA%\ort.pyke.io`（360.9 MB）按用户决定原样保留，**本轮未跑任何 ORT 构建**。

## P5-05 — Windows 构建回归与 ORT 实跑（tag `p5.5`，报告 `docs/34`）

在工作区 `E:\augenstern\translate` 内按 `docs/32` 的权限边界完成 W1–W10。

### 结果

- `cargo fmt --check` / `clippy -D warnings`（默认 + all-features）**PASS**；
  `cargo test --workspace` **322 passed / 0 failed / 56 ignored**（all-features 322/0/61）；
  Linux 同口径 372/60——差值 -50 全部是**按平台语义 gate** 的 Linux 专属用例（见 `docs/34` §2.1）。
- **ORT 真实模型 24 passed**（contract 13 + real 6 + facade 5），模型 110 MB 传入工作区，`P4_ORT_MODEL_DIR` 注入。
- **W10 中文+空格路径**：`E:\augenstern\translate\.build\path test 中文\repo` 下 build/test 均 EXIT 0。
- W9 Python 工具（Anaconda 3.13.5 绝对路径）：check-release PASS（9 项/1 SKIP）、license-audit PASS、
  p4-acceptance PASS、freeze-manifest PASS（需先建本地分支 `p3-frozen`，bundle 只带 remote-tracking）。

### 为通过而改的代码（7 个提交，均先在 Linux 验证）

fmt 修复（D-108，两平台同时失败→不是平台问题）、3 处仅 Windows 的 clippy（D-109）、Bergamot 配置夹具 gate（D-110）、
transport gate（D-111）、worker/lifecycle 整目标 gate（D-112，理由：/proc 判活恒 false 会**假通过**）、
lc08 显式 ignore（D-113）、工具脚本两处编码修复（D-114，`subprocess` cp936 解码 + stdout cp936 编码）。

### 新增事实

- **D-92 实测确认**：默认特征链接 `directml` **静态**包（`onnxruntime.lib` 343.3 MB + `DirectML.dll` 17.7 MB），
  产物**没有** `onnxruntime.dll`；与 Linux 的 `none`（CPU 静态）变体不可并排比性能（D-116）。
- **D-115 待处置**：`ort-sys` 把 360.9 MB 缓存写到工作区外 `C:\Users\25026\AppData\Local\ort.pyke.io`。
  这是工具默认行为、非我指定路径，但违反 ADR-057；**未擅自删除**，等用户决定（保留/删除/先查重定向变量）。
- **D-117 / ADR-058**：跨平台验证用 `cfg` gate 表达"不可达"，两平台用例数必须同时给出并解释差值。

## P5-04 — 权限边界与 P5.1 只读核验（tag `p5.4`，报告 `docs/33`，审查稿 `docs/32`）

不改任何 crate。新增两个只读工具与一份证据；采纳"唯一可写工作区"约束。

### 新规则（ADR-057，来自 `docs/32`）

- 验证机上**只有 `E:\augenstern\translate` 默认可写**；工作区外一律只读，任何写入（含 `git --global`、
  注册表、PATH、Defender、服务、持久环境变量、工具安装位置）须**逐项先获批**，且与是否需要管理员无关。
- Python 固定 `D:\Anaconda3\python.exe`；Rust 不由本会话安装/切换（你已升到 1.99.1）；代理只允许进程级临时设置。
- 由此作废：`D:\dev`/`C:\dev` 落盘、全局 `core.autocrlf`、Defender 排除、`D:\python` 作默认解释器
  （`docs/31` 文首已加"部分取代"状态块，原始实测记录不改写）。

### 交付

- `tools/p5/windows-env-current.ps1` + `windows-env.sh`：`dsh-win-env/v1` 只读采集（workspace / toolchain /
  python / msvc / git / ort / network / write_risk），Linux 侧一条命令取回并校验。
- `docs/33`：P5.1 报告（三段式）+ 证据 `fixtures/p5/windows-env-current.json`（4,210 B，工作区内同名一份）。

### 实测（2026-11-06）

- 工作区 `E:\augenstern\translate`：**存在、空、无 `.git`**，所在盘剩 **133.7 GiB** → 需导入仓库（P5.2 前置）。
- **Rust = 1.99.1**（2026-10-13，host msvc，LLVM 23.1.1）与 Linux 完全同版 → `docs/31` 的 R13 WARN 消失，
  `cargo fmt --check` 的比对口径随之**收紧**（若有差异即代码问题）。
- Anaconda Python **3.13.5**；VC 工具集 17.14.36429.23；SDK 10.0.26100.0 + 10.0.18362.0；`cl`/`link` 不在 PATH（正常）。
- ORT 本地仍是 **1.29.0**（CUDA13，12 个文件，pdb 合计约 900 MB），只读保留。
- 网络更正（D-103）：`cdn.pyke.io` **不是"不通"而是"极慢/不稳"**——HEAD 直连 12 s 超时、有界采样 20 s 仅 4,483 B
  （≈224 B/s），经代理 200；包体实为 **32.4 MB**（此前估"100–200 MB"偏高 3–6 倍）。

### 修复（真机才暴露，D-107）

`$args` 是 PowerShell 自动变量（参数同名会让 `cargo -V`/`git --version`/`python --version` 静默无参执行）；
`$py` 与 `$PY` 因变量名大小写不敏感而互相覆盖；脚本非 ASCII（中文注释、`§`）经 `ssh` + `-Command -` 传输时按 ACP 936
解码会导致**整脚本静默不执行**。三条都写进脚本注释，脚本现为全 ASCII。

## P5-03 — 实体机实测与部署清单重写（tag `p5.3`，报告 `docs/31` v2）

不改任何 crate。`windows-recon.ps1` 修两处真机缺陷并增采三项；`docs/31` 从"通用清单"改写为"这台机器的实测版"。

### 实测（`ssh home` → NAN-WINDOWS，2026-11-06）

- **判定 `R1`～`R15`：PASS 12 / FAIL 0 / WARN 1 / INFO 2**（唯一 WARN 是 rustc 1.97.0 vs Linux 1.99.1）。
- 机器：Win11 Pro 25H2 build 26200 / i7-1260P 12C16T / 15.7 GiB / C 38.6 GiB 空闲、D 120.2、E 133.7。
- 工具链都在 **D 盘**：VS Community 2022（VC 17.14 + SDK 10.0.26100）在 `D:\VisualStudio`、Git 2.51 在 `D:\Git`、
  Python 有 4 个（裸 `python` 是 MSYS2 3.12.11，`py -3` 是 `D:\python` 3.13.7）、cmake/node/msys2 mingw 齐全。
- **W1 已闭合**：`cargo new --bin smoke && cargo build` → 40.1 s，`smoke.exe` 129,536 B
  （`rustc 1.97.0` + D 盘 VS 的 `link.exe` + SDK 26100 这条链通）。
- **网络矩阵**：`static.rust-lang.org` 与 `index.crates.io` 直连 200；**`cdn.pyke.io` 直连 20 s 超时、经
  `127.0.0.1:7890` 200**。本机只设了 `ALL_PROXY`（无 `http_proxy/https_proxy`），而 `ort-sys` 用
  `ureq::Proxy::try_from_env()` 认 `ALL_PROXY`（源码可查），所以 ORT 下载大概率可用，仍建议显式设。
- **额外发现**：`D:\Programs\onnx` 有一套 ORT **1.29.0 CUDA 13** 的 dll/lib/include，且该 lib 目录在 PATH 上
  → 与 W6 要用的 1.31 DirectML 版同名 DLL 有"串味"风险；可作为离线备份方案但需改 `api-*` feature（决策 D）。

### 修复

- `windows-recon.ps1`：`tools.curl` 取到的是 PowerShell 5.1 的 `curl` 别名（返回 .NET 502）→ 改用 `curl.exe`；
  `rustup show active-toolchain` 在该 rustup 上不存在 → 失败时退回 `rustup show` 解析 `name:`；
  新增 `env`（含 `ALL_PROXY`/`CARGO_HOME`）、`tools.python_path`、`tools.python_launcher`。
- 真机跑通同时**关闭了 `docs/30` D-89 的一半欠账**：脚本确实在 **PowerShell 5.1** 上执行，37 个字段全部有值。

### 待你审核后才动

`git config --global core.autocrlf false` + `core.longpaths true`；仓库落 `D:\dev\translate-workspace`（bundle 传输）；
Defender 排除仓库与 `~/.cargo`（机器级改动，需批准）；W6 的 ORT 变体选择（决策 D）。见 `docs/31` §7。

## P5-02 — Windows 验证机部署清单与机器判定（tag `p5.2`，报告 `docs/31`）

不改任何 crate（`Cargo.lock` 不变）。新增 `tools/p5/windows-preflight.py`；`windows-recon.ps1` 增采
`rust.host` / `rust.components` / `rust.default_toolchain` / `vs.vc_tools_version` / `vs.vc_tools_path`。

### 交付

- `docs/31`：必装 6 件（Git + `core.autocrlf=false`、VS 2022 Build Tools 的 MSVC v143 + Win11 SDK、
  rustup 钉 1.99.1 + rustfmt/clippy、Python 3.12、OpenSSH Server 自启 + 防火墙、长路径注册表）、
  3 个非安装前提（≥25 GiB、NTFS、仓库别放 OneDrive）、**明确不要装**的 7 类（GNU 工具链 / CMake /
  OpenSSL / 完整 VS / WSL-Docker / Node-Java-Android SDK / 安全卫士类软件）。
- `tools/p5/windows-preflight.py`：把部署要求编成 `R1`～`R15` 的 PASS/FAIL/WARN/INFO，读
  `dsh-win-recon/v1`、输出 `dsh-win-preflight/v1`，`FAIL` 时退出码 1（可进 CI）。已用构造的
  PASS/FAIL/错误输入三份样本验过。
- 两个"为什么"写进清单：`link.exe` **不在** PATH 是正常的（用 vswhere 查
  `Microsoft.VisualStudio.Component.VC.Tools.x86.x64`）；`libsqlite3-sys`（`rusqlite` 的 `bundled`）
  现场编 C，所以 C 编译器是硬依赖。

### 本轮读代码得到的关键发现

- **D-92**：`ort-sys 2.0.0-rc.15` 的预编译索引里，`x86_64-unknown-linux-gnu` 有 `none` 行（即 P4 用的
  静态 CPU 版），而 **`x86_64-pc-windows-msvc` 没有 `none` 行**（只有 directml/webgpu/openvino/nvrtx/cuda13）；
  `resolve_dist()` 在无精确匹配时取"第一个候选" → 默认特征会解析到 **directml** 包。
  推论：Windows 首次构建要访问 `cdn.pyke.io`、产物旁多出 `onnxruntime.dll`，
  且**它的 ORT 性能数字与 Linux 不是同一 runtime 变体，不能并排算倍数**（ADR-052 口径）。
  未实测，真机结果优先。
- **D-93/D-94/D-95**：判定脚本先写后验；`docs/31` 占用了原计划的对比报告编号（对比报告顺延 `docs/32`）；
  Rust 钉 1.99.1 是为了让 `cargo fmt --check` 的差异只反映代码而不是工具链。

## P5-01 — Android 工具链就位与 Windows 验证矩阵（tag `p5.1`，报告 `docs/30`）

不改任何 crate（`Cargo.lock` 不变），只加验证脚手架：`tools/p5/windows-recon.ps1`、`tools/p5/windows-recon.sh`。

### 已完成

- **NDK r30（`ndk;30.0.16248370`，2.3 GB）** 装好；`rustup target add x86_64-linux-android`
  （`aarch64-linux-android` 原已有）。
- **JNI 三段链在 AVD 上打通**：零依赖 Rust `cdylib`（手写 `Java_HelloJNI_*` 符号）→ `d8` 出 dex →
  `app_process` 在 android-34 x86_64 AVD 上 `System.loadLibrary` 成功并取回返回值：
  `SMOKE arch=x86_64 abi_fn=1 add=42 / SMOKE OK`。
- **ARM64 产物合规**：符号正确、`LOAD` 段 16 KiB 对齐（`0x4000`）、`NEEDED` 只有 `libdl/libc`。
- **负对照（最重要的一条）**：同一份源码的 `aarch64` 产物在 x86_64 设备上被拒——
  `dlopen failed: ... is for EM_AARCH64 (183) instead of EM_X86_64 (62)`。
  即：现有 AVD 在 ARM64 上的证据力为 **0**，`docs/29` D-83 从推断升级为实测。
- **Windows 侧只读勘察**：`dsh-win-recon/v1` JSON（OS/build/提权/`LongPathsEnabled`/代码页/CPU/卷/VS/SDK/
  rust target/sshd 状态/Defender/WSL/IPv4 等），脚本全 ASCII、纯 getter；Linux 侧一条命令拉取，
  按 `utf-8→cp936→cp1252` 解码兜底（中文版 Windows 控制台吐 CP936）。已用假 `ssh` + CP936 载荷验证解析链。
- **W1～W10 验证矩阵**（`docs/30` §1.5）：含 **W7 = Windows 上 Bergamot 必须返回
  `CapabilityUnavailable`**（预期不支持，不是失败）。

### 未完成（外部依赖）

- Windows 实体机：**SSH 参数未到**，W1～W10 全为空表；`windows-recon.ps1` 只做过 pwsh 7.4.6 AST 解析，
  未在 PowerShell 5.1 真机执行（D-89）。
- ARM64：无真机 → "物理机基准"这一格仍空（D-87）。
- CI 未动（用户只选 B 路线），G11 继续 open（D-90）。

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
