# Windows 验证机部署清单（依据实体机实测重写）

- 日期：2026-11-06（v2，实体机实测后修订）
- 状态：**待你审核**。本文只含勘察结论与待批动作，**尚未在 D 盘写任何东西、未改任何系统设置**
- 上一版：`docs/31` v1（通用清单，`p5.2`）——本版用实测数据替换了其中的"要求"，并把"你要装什么"变成"这台机器已经有什么、还差什么"
- 实体机：`ssh home`（`192.168.1.6:22`，主机名 **NAN-WINDOWS**，账号 `WORKGROUP\25026`，密钥免密，会话带管理员令牌）
- 关联：`docs/30`（W1～W10 验证矩阵）、`docs/29`（路线对比）、`docs/01` ADR-054/055/056

---

## 0. 一句话结论

**这台机器比我原清单假设的强得多：8 项必装里 6 项已经就绪，工具链不是缺、而是"装在 D 盘 + 版本比我基线新/旧的混合体"。**
判定结果 **PASS（R1～R15：PASS 12 / FAIL 0 / WARN 1 / INFO 2）**，唯一 WARN 是 rustc 版本（1.97.0 vs Linux 侧 1.99.1）。
真正要补的只有 4 件小事：**仓库落地到 `D:\dev`、关掉 `core.autocrlf`、指定 Python 解释器、给 ORT 下载走代理**；
其中 2 件需要你批准（Defender 排除、在 D 盘写 ~9 GB 的 `target/`）。

---

## 1. 实测环境（2026-11-06，全部机器采集，非手抄）

| 类别 | 实测值 | 来源 |
|---|---|---|
| 主机 | **NAN-WINDOWS**，`WORKGROUP\25026`，`192.168.1.6/24 (WLAN)`；另有 `10.222.222.1/24 (NodeBabyLink)` | recon |
| 系统 | **Windows 11 Pro 25H2，build 26200**，x64；`ProductName` 注册表仍写 "Windows 10 Pro"（Win11 已知现象，以 build 为准） | recon |
| 区域/编码 | culture `zh-SG`，system locale `zh-CN`，**ACP 936（GBK）** | recon |
| Shell | `powershell` **5.1.26100.9711**（勘察脚本就是在这个 5.1 上跑通的）；`pwsh` 存在（Store 别名） | recon |
| CPU | **i7-1260P**，12C/16T，2.1 GHz 基频 | recon |
| 内存 | **15.7 GiB**（采集时可用 5.8 GiB） | recon |
| 磁盘 | `C:` 299.4 GiB / **38.6 空闲**；`D:` 326 GiB / **120.2 空闲**；`E:` 326 GiB / 133.7 空闲；均 NTFS | recon |
| 长路径 | `LongPathsEnabled = 1` ✅ | recon |
| GPU | **RTX 2050 4 GB**（drv 32.0.16.1088）+ Intel Iris Xe | recon |
| Visual Studio | **Community 2022 17.14.36429.23，装在 `D:\VisualStudio\2022\Community`**；VC 工具集 `17.14` 就绪 | recon（vswhere） |
| Windows SDK | Lib `10.0.18362.0`、**`10.0.26100.0`** | recon |
| Rust | `cargo/rustc` **1.97.0**，`host = x86_64-pc-windows-msvc`；`rustup home = C:\Users\25026\.rustup`；工具链 `stable-x86_64-pc-windows-msvc`（active/default）；target 只有 msvc；组件含 **rustfmt / clippy / rust-src** | recon |
| Python | **裸 `python` = `D:\msys64\mingw64\bin\python.exe` 3.12.11（MSYS2/minGW 版）**；`py -0p`：`D:\python\python.exe` **3.13.7（默认）** + uv 管理的 CPython 3.12.13；另有 `D:\Anaconda3` 3.13.5 | recon |
| Git | **2.51.0.windows.1**，位于 `D:\Git`；`core.autocrlf=true`（**系统级**）、`core.symlinks=false`、`user.name/email` 已设、`safe.directory=*` | recon |
| 其它工具 | curl **8.21.0**、cmake 4.1.1、node v24.19.0、bsdtar 3.8.9；**`D:\msys64\mingw64\bin` 在 PATH 前部（含 `gcc.exe`/`cc.exe`）** | recon |
| sshd | `Running` / `Automatic`，监听 `0.0.0.0:22` 与 `:::22`，客户端 OpenSSH_for_Windows 9.8p2 | recon |
| 代理 | **`ALL_PROXY=http://127.0.0.1:7890`（本机在听）**；`http_proxy`/`https_proxy` **为空**；git 全局 `http.proxy=127.0.0.1:7890` | recon + 探针 |
| Defender | 实时保护开；排除项仅 `D:\Programs`、`E:\game` | recon |
| 其它运行时 | WSL 存在、Docker 存在、winget 存在、choco 无 | recon |
| ONNX Runtime | **`D:\Programs\onnx` 有 ORT 1.29.0（Windows x64 **CUDA 13 GPU** 版：`onnxruntime.dll` + `.lib` + `include`）**，且 `D:\Programs\onnx\lib` 在 PATH 上 | 探针 |

### 1.1 网络可达性（这条决定了 W6 能不能跑）

| 目标 | 直连 | 经 `127.0.0.1:7890` | 结论 |
|---|---|---|---|
| `static.rust-lang.org` | **200**（1.53 s） | 200（1.15 s） | 直连可用 |
| `index.crates.io` | **200**（0.53 s） | 200（0.72 s） | 直连可用 |
| `cdn.pyke.io`（ort 预编译包） | **超时（20 s 无响应）** | **200**（3.73 s） | **必须走代理** |

代理细节值得单独记一笔：这台机器只设了 `ALL_PROXY`，没设 `http_proxy/https_proxy`。
好处是 `ort-sys` 的下载器是 `ureq 3.x` + `Proxy::try_from_env()`（源码可查，`build/download/mod.rs:43`），
它认 `ALL_PROXY`，所以**大概率不用额外配置**；坏处是任何只认 `http_proxy/https_proxy` 的链路（cargo 的部分路径、
某些 Python 库）会"直连超时"，而症状看起来像"网络坏了"。这一点已写成风险 R-6。

### 1.2 工具链自检（唯一一次"动手"，在 `%TEMP%` 里，已删除）

```
$ cargo new --bin smoke && cargo build
BUILD_SECONDS=40.1
SMOKE_EXE=129536 bytes
```

含义：`rustc 1.97.0 + link.exe（来自 D 盘的 VS 2022）+ SDK 10.0.26100` 这条链**是通的**——
这是 W1 的核心，已经不需要"装完再试"。40.1 s 编一个 hello-world（Linux 上同类 1 s 级）说明
**Defender 实时扫描 + 首次运行开销很大**，这直接影响后面全量构建要等多久（见 P1-1）。

### 1.3 ORT 在 Windows 上的"变体"问题（沿用 D-92，本机又添一条证据）

`ort 2.0.0-rc.15` 的预编译索引里，Linux 有 `none` 行（P4 用的静态 CPU 版），而 **`x86_64-pc-windows-msvc`
没有 `none` 行**，只有 `directml / webgpu / openvino / nvrtx / cuda13`；默认 feature 下 `resolve_dist()`
会取"第一个候选" → **DirectML 变体**。这是读 `ort-sys` 的 `dist.tsv` + `resolve.rs` 得出的结论（未实测，
真机结果优先）。本机额外发现：`D:\Programs\onnx` 已有一套 **ORT 1.29.0 CUDA 13 GPU 版**（dll+lib+include），
它**可以**通过 `ORT_LIB_LOCATION` 被链接，但 API 版本比 `ort` 默认要的 1.31 旧，需要改 `api-*` feature，
且 CUDA 版会引入 GPU 路径——所以它是备份方案，不是首选（见决策 D）。

---

## 2. 判定结果（`tools/p5/windows-preflight.py`，原始输出）

```
R1   PASS  Microsoft Windows 11 Pro build 26200
R2   PASS  OS 架构 64-bit（必须 x64）
R3   PASS  12th Gen Intel(R) Core(TM) i7-1260P 12C/16T
R4   PASS  15.7 GiB
R5   PASS  C: NTFS 剩 38.6 GiB
R6   PASS  rustc 1.97.0 (2d8144b78 2026-07-07)（host x86_64-pc-windows-msvc）
R7   PASS  rustfmt + clippy 已装
R8   PASS  Python 3.12.11
R9   PASS  git version 2.51.0.windows.1
R10  PASS  Running/Automatic，监听 :::22, 0.0.0.0:22
R11  PASS  LongPathsEnabled=1
R12  PASS  MSVC toolset 17.14.36429.23（SDK Lib: 10.0.18362.0, 10.0.26100.0）
R13  WARN  1.97.0 (2d8144b78 2026-07-07)（Linux 侧 1.99.1）
R14  INFO  当前会话是否提权：True
R15  INFO  Defender 排除路径 2 条

结论：PASS（PASS 12 / FAIL 0 / WARN 1 / INFO 2）
```

`R13` 之外全绿；`R13` 是"口径差异"不是缺陷（见决策 A）。`R15` 提示的不是"没排除"，而是
**排除的是 `D:\Programs`、`E:\game`，仓库将来如果落在 `D:\dev` 并不在被排除范围里**。

---

## 3. 与通用清单（v1）的对照：8 项里 6 项已满足

| v1 项 | v1 要求 | 本机实测 | 状态 |
|---|---|---|---|
| B1 Git | ≥2.40 | 2.51.0（D:\Git） | ✅ 但有坑：`core.autocrlf=true` |
| B2 VS Build Tools | MSVC v143 + Win11 SDK | Community 2022，VC 17.14 + SDK 26100（在 D 盘） | ✅ 已满足 |
| B3 Rust | 钉 1.99.1 | **1.97.0**（stable，msvc，rustfmt/clippy 齐） | ⚠️ 版本差（决策 A） |
| B4 Python | ≥3.9 | 有 4 个：MSYS2 3.12.11（裸 python）/ 3.13.7（py 默认）/ uv 3.12.13 / Anaconda 3.13.5 | ✅ 但要**指定**解释器 |
| B5 OpenSSH | Running + 自启 + 22 | 全满足（含 IPv6） | ✅ 免密已通 |
| B6 长路径 | `LongPathsEnabled=1` | =1 | ✅（但 git 的 `core.longpaths` 未设） |
| B7 目录 | `C:\dev\...`、别放 OneDrive | 尚无 dev 目录；D 盘 120 GB 空闲 | ❌ 待做（决策 C） |
| B8 磁盘 | ≥25 GiB | C 38.6 / D 120.2 | ✅ 建议把 `target/` 放 D |

---

## 4. 还需要做什么

### P0（必须，做完才能开始 W 矩阵；都是"写文件"级别，不改系统）

| # | 动作 | 谁做 | 具体命令 | 风险 |
|---|---|---|---|---|
| P0-1 | 关掉 CRLF 转换 | **需要你确认→我执行** | `git config --global core.autocrlf false`<br>`git config --global core.longpaths true` | 低。只改 git 配置；不设的话工作树是 CRLF，`fixtures/**` 的 sha256 类校验会假失败 |
| P0-2 | 仓库落地 `D:\dev\translate-workspace` | 我（在 Linux 侧打包、scp、Windows 侧 clone） | Linux：`git bundle create /tmp/tw.bundle --all`（约 5.5 MB）→ `scp` → Windows：`git clone C:\Users\25026\tw.bundle D:\dev\translate-workspace` | 低。D 盘写 ~30 MB（后续 `target/` 再 ~9 GB） |
| P0-3 | 明确 Python 解释器 | 我（在命令里写死） | 统一用 **`D:\python\python.exe`**（3.13.7）或 `py -3`；**不要用裸 `python`**（那是 MSYS2 的 3.12.11，路径/编码行为不同） | 低。纯约定 |
| P0-4 | ORT 下载走代理（仅 W6 需要） | 我（构建时临时设 env） | `$env:http_proxy='http://127.0.0.1:7890'; $env:https_proxy=$env:http_proxy` 后再 `cargo build --features nmt-ort-runtime` | 低。`cdn.pyke.io` 直连超时，`ALL_PROXY` 大概率够用，显式设一遍最稳 |

### P1（建议，会显著影响结论质量或耗时）

| # | 动作 | 谁做 | 命令 | 影响 |
|---|---|---|---|---|
| P1-1 | Defender 排除仓库与 cargo 目录 | **需你批准**（我在 ssh 会话里有管理员令牌，但这是机器级改动） | `Add-MpPreference -ExclusionPath "D:\dev\translate-workspace","C:\Users\25026\.cargo"` | hello-world 都要 40 s；全量 `cargo test` + `clippy` 两套 feature 至少几十次编译，不排除会非常痛 |
| P1-2 | 别动系统 PATH | 我（不动） | 验证时一律用绝对路径调 `D:\python\python.exe`、`D:\Git\cmd\git.exe` | `D:\msys64\mingw64\bin` 里的 `gcc.exe`/`cc.exe`/`python.exe` 在 PATH 前部，改动它会影响你其它工程；**不动最安全** |
| P1-3 | 远端长稳/基准前先关掉 GPU 加速干扰 | 我（记录） | — | 本机是 RTX 2050 + Iris Xe 双显卡，ORT 若走 CUDA 会与 Linux 基线不可比（决策 D） |

### P2（可选）

| # | 动作 | 说明 |
|---|---|---|
| P2-1 | 传 ORT 模型 110 MB（`encoder 51 MB` + `decoder 58 MB` + `tokenizer 1.7 MB`） | 只有传了才能验 W6 的真实翻译；不传只验"能构建" |
| P2-2 | 清理 `E:`/`D:` 上与本项目无关的占用 | 不需要；只是记录 D 有 120 GB、E 有 133 GB |
| P2-3 | `rustup component add rust-src` | **已有**，无需做 |

---

## 5. 按这台机器重排的 W1～W10

| # | 项 | 前置 | 预期结果 | 备注（本机特有） |
|---|---|---|---|---|
| W1 | 工具链 / 链接 | 无 | **已通过**（`cargo build` hello 40.1 s，exe 129,536 B） | 已完成，不需要重跑 |
| W2 | `cargo fmt --check` | P0-2 | PASS，或出现**与代码无关**的格式差异 | rustfmt 1.97.0 vs 1.99.1；若差异出现，按工具链差异记（决策 A） |
| W3 | `cargo clippy -- -D warnings`（默认 + all-features） | P0-2 | PASS | 两套 feature 各一遍，耗时最长 |
| W4 | `cargo test`（默认 feature） | P0-2 | 与 Linux 同量级的通过/忽略数 | Linux 基线 `371 passed / 60 ignored` |
| W5 | 字典 + `rusqlite`（bundled，需 `cl.exe` 编 SQLite C 源） | P0-2 | PASS | **这是本机第一个真正的 C 编译测试**；MSVC 在 D 盘、`cl` 不在 PATH 属正常 |
| W6 | ORT 后端（真实模型） | P0-4 + P2-1 | 能构建；真实翻译取决于模型是否传过去 | 两处本机特有风险：① `ort` 默认解析到 **DirectML 1.31** 包（本文件 D-92 / §1.3）；② PATH 上有 **ORT 1.29 CUDA** 的同名 `onnxruntime.dll`（风险 R-4） |
| W7 | Bergamot 平台门禁 | P0-2 | **`CapabilityUnavailable`** | 预期不支持，不是失败（`docs/29` D-84） |
| W8 | CLI mock 路径 | P0-2 | 输出与 Linux 逐字一致 | 注意 ACP 936：中文输出比对要用 `chcp 65001` 或比对字节 |
| W9 | Python 工具（`check-release` / `p4-acceptance` / `freeze-manifest` / `license-audit`） | P0-2、P0-3 | 预期可跑 | **必须指定解释器**；`freeze-manifest` 依赖 `git` 与 tag（bundle 传输会带 tag） |
| W10 | 含空格/中文的路径 | P0-2 + 副本 | 不炸 | 本机 ACP 936，中文路径风险比英文机器高，值得实测 |

---

## 6. 已识别的环境风险（都会写进 `docs/32` 的对比报告）

| # | 风险 | 依据 | 对策 |
|---|---|---|---|
| R-1 | **`core.autocrlf=true`（系统级）** | recon：`git config --system` 含该项，全仓库无 `.gitattributes` | P0-1 改成 `false`；已 clone 的要重新检出 |
| R-2 | Python 有 4 个，裸 `python` 是 MSYS2/minGW 版 | recon：`where python` → `D:\msys64\mingw64\bin\python.exe` | P0-3 一律显式解释器 |
| R-3 | `D:\msys64\mingw64\bin`（含 `gcc.exe`、`cc.exe`）在 PATH 前部 | recon：PATH 第 30 项附近 | 不动 PATH；W5 观察 `cc` crate 是否误选 gcc（msvc target 下应选 `cl.exe`，实测确认） |
| R-4 | PATH 上有 **ORT 1.29 CUDA** 的 `onnxruntime.dll` 目录（`D:\Programs\onnx\lib`） | 探针 | W6 时临时从 PATH 摘掉该目录，或验证 exe 目录里的 1.31 DLL 优先；否则会出现"版本串味"的诡异崩溃 |
| R-5 | `cdn.pyke.io` **直连超时**，仅代理可达 | 探针（直连 20 s 超时 / 代理 200） | P0-4 显式设 `http_proxy/https_proxy` |
| R-6 | 只有 `ALL_PROXY`，无 `http_proxy/https_proxy` | recon `env` | 见 P0-4；`ort-sys` 用 `ureq::Proxy::try_from_env()` 认 `ALL_PROXY`，但 cargo 的部分路径不认，显式设置最稳 |
| R-7 | ACP 936 + 中文错误信息 | recon（`acp=936`, locale zh-CN） | 报告里引用原始输出时保留字节；解析错误码而非错误文本 |
| R-8 | Defender 使 hello-world 编译达 40.1 s | §1.2 | P1-1（待批） |
| R-9 | 采集时可用内存仅 5.8 GiB | recon | 链接 ORT 时注意；W6 前关掉无关大内存程序（或我加 `-j` 限制并行） |

---

## 7. 等你审核的四个决策

**决策 A — Rust 版本：保留 1.97.0，还是装 1.99.1？**
- A1（建议）：保留 1.97.0。理由：工具链已验证可用、无需再下 ~700 MB、`docs/26` 里所有基线数字都不受影响；
  代价是 `cargo fmt --check` 可能与 Linux 出现纯工具链差异，报告里写成"工具链差异"即可（R13 已 WARN 记录）。
- A2：`rustup toolchain install 1.99.1 --profile minimal --component rustfmt clippy` 后再验。
  好处是 W2 比对干净；代价是 700 MB + ~5 分钟，且多一个工具链要维护。

**决策 B — 是否允许我加两条 Defender 排除？**（`D:\dev\translate-workspace`、`C:\Users\25026\.cargo`）
- B1：允许（建议）。只影响扫描，不改安全策略实质；可随时用 `Remove-MpPreference` 撤回。
- B2：不允许。那我把并行度调低、接受更长的构建时间，并在报告里注明"未排除 Defender"。

**决策 C — 仓库落地位置与传输方式**
- C1（建议）：`D:\dev\translate-workspace`，用 `git bundle --all` 传（5.5 MB，带全部 tag），Windows 侧 `git clone` 出来。
- C2：`E:\dev\...`（空闲 133 GB）。D 盘也够，选 D 是因为你现有工具多在 D。
- C3：你自己放好，只告诉我路径（那我需要确认它是 ASCII 路径、且不在 OneDrive 同步目录下）。

**决策 D — W6（ORT）怎么做**
- D1：让它按默认行为下 **DirectML 1.31** 包（走代理）。最接近"用户默认体验"，但**与 Linux 的 CPU-only 静态版不是同一 runtime 变体，性能不可并排比**（D-92 / §1.3）。
- D2：用本机已有的 `D:\Programs\onnx`（**1.29 CUDA**）走 `ORT_LIB_LOCATION`。省下载，但 API 版本比 `ort` 期望的 1.31 旧，需要改 `Cargo.toml` 的 `api-*` feature，属于代码改动，且 CUDA 版会引入 GPU 路径。
- D3：W6 先只验"能构建、能加载"，不跑真实翻译，等模型传上来再说。
- 我的建议：**D1 + D3 组合**——先按默认（D1）构建并记录它到底解析到哪个包，模型传上来后再跑真实翻译；D2 只作为"网络彻底不通"时的备份方案。

---

## 8. 本轮复现命令（实际跑过的）

原始证据已入库（可复算，不用信本文的文字）：
`translator-sdk/fixtures/p5/win-recon-nan-windows.json`（采集原样，37 字段）与
`translator-sdk/fixtures/p5/win-preflight-nan-windows.json`（判定结果，`dsh-win-preflight/v1`）。
复算：`python3 tools/p5/windows-preflight.py fixtures/p5/win-recon-nan-windows.json`。

```bash
# 连通性
ssh -o BatchMode=yes home "hostname & whoami"           # NAN-WINDOWS / nan-windows\25026

# 采集 + 判定
cd translator-sdk
tools/p5/windows-recon.sh home -o /tmp/win-recon.json  # 37 个字段，utf-8 解码成功
python3 tools/p5/windows-preflight.py /tmp/win-recon.json   # PASS 12 / FAIL 0 / WARN 1 / INFO 2

# 环境探针（只读）
#   git config --global/--system --list
#   where.exe python / py -0p / python -c "import sys;print(sys.executable)"
#   rustup show
#   curl.exe --noproxy '*' -sS -o NUL -w "%{http_code}" https://index.crates.io/config.json   # 200
#   curl.exe --noproxy '*' ... https://cdn.pyke.io/0/pyke:ort-rs/ms@1.31.0/x86_64-pc-windows-msvc+directml.tar.lzma2
#                                       # 直连超时；去掉 --noproxy 后 200
#   Get-ChildItem D:\ ; Test-Path D:\Programs\onnx ; Get-Content D:\Programs\onnx\VERSION_NUMBER  # 1.29.0

# 工具链自检（%TEMP%，跑完即删）
#   cargo new --bin smoke; cargo build   -> 40.1 s, target\debug\smoke.exe 129536 B
```

---

## 9. 偏差登记

**编号沿用说明**：`D-92`（ORT 在 Windows 无纯 CPU 预编译行）仍然有效，见 §1.3。v1 的 `D-93`（判定脚本先写后验）
本轮已被实测取代——`windows-recon.ps1` 确实在真机 **PowerShell 5.1** 上跑通、37 个字段全部有值，欠账关闭；
只剩"preflight 自身只有构造样本验证"并入 `D-96`。v1 的 `D-94`（`docs/31` 占用对比报告编号）已完成，
对比报告即 `docs/32`；v1 的 `D-95`（Rust 钉 1.99.1）被本轮 `D-97` 取代。本轮新增 `D-96`～`D-100`，**未覆盖任何旧编号**。

- **D-96：勘察脚本在真机上暴露两处缺陷，已修复并重跑。**
  ① `tools.curl` 取到的是 PowerShell 5.1 的 `curl` 别名（`Invoke-WebRequest`），返回 `.NET 502` 而不是版本号——
  改为 `curl.exe`；② `rustup show active-toolchain` 在这台的 rustup 上不存在（报 `invalid value ... for '[+toolchain]'`），
  改为失败时退回 `rustup show` 解析 `name:`。顺带新增 `env`（代理/CARGO_HOME 等）、`python_path`、`python_launcher` 三块，
  因为"裸 `python` 是谁"这件事差点被漏掉。修复后重跑，字段全部有值（就是 §1 的数据）。
- **D-97：清单从"钉 1.99.1"改为"优先沿用已装版本"。**
  原因：这台是 1.97.0 且 rustfmt/clippy 齐全，工具链已验证可用；强制换版本只为一个 `fmt` 比对口径不划算。
  代价：`cargo fmt --check` 可能产生与代码无关的差异，**必须**在报告里按工具链差异标注，不能算代码问题（决策 A 请你拍板）。
- **D-98：v1 里"必须新建 `C:\dev`"的位置假设作废。**
  原因：这台机器的 VS、Git、Python、cmake、onnx 全在 D 盘，`C:` 只剩 38.6 GiB；把仓库与 `target/` 放 `D:\dev` 更合理。
  影响：后续所有命令里的路径都按 `D:\dev\translate-workspace` 写（决策 C 请你确认）。
- **D-99：v1 的"必装 6 件"在本机全部降级为"已就绪/仅需约定"。**
  这是这份修订版的意义：**不要把"清单"当成"还没做"的等价物**。真正待办的只有 P0-1/P0-2/P0-3/P0-4 四条写文件级动作，
  其中两条（P0-1 的 git 全局配置、P0-2 的 D 盘落盘）我按约定**等你批准后才动**。
- **D-100：`docs/31` 现在是"这台机器的实测版"，通用装机指引移到附录 A。**
  原因：再过一台新机器时，本文的"实测"部分不适用，但"装什么、为什么"仍然有用；两者混在一起会让下一个人误读
  （把本机现状当通用要求，或把通用要求当本机现状）。

---

## 附录 A：换一台新机器时的通用必装（本机**不需要**再执行）

| # | 组件 | 命令要点 | 为什么 |
|---|---|---|---|
| A1 | Git ≥2.40 | `winget install --id Git.Git -e`；随后 `git config --global core.autocrlf false`、`core.longpaths true` | `freeze-manifest.py` 用 `git`；本仓库无 `.gitattributes`，CRLF 会改字节 |
| A2 | VS 2022 Build Tools | `--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended` | `link.exe`（msvc target）+ `cl.exe`（`rusqlite` 的 `bundled`） |
| A3 | Rust | `rustup default 1.99.1` + `--component rustfmt clippy`（或沿用机器已有 stable-msvc） | 与 Linux 基线对齐（口径见决策 A） |
| A4 | Python ≥3.9 | 任意原生 CPython；**别用 MSYS2/conda 的作为 `python` 默认** | `tools/*.py` 用 `str.removeprefix`；路径/编码行为要可预期 |
| A5 | OpenSSH Server | `Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0` + 自启 + 防火墙 22 | 让我能连 |
| A6 | 长路径 | `LongPathsEnabled=1` | `target\...\build\ort-sys-<hash>\out\...` 超 260 |
| A7 | 明确不要装 | GNU 工具链/MSYS2 gcc 作默认、CMake、OpenSSL、完整 VS、WSL 内编译 | 造成"看起来能编、结果对不上"的混合环境 |
