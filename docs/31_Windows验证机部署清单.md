# Windows 验证机部署清单（交给你执行）

- 日期：2026-11-06
- 用途：把一台 Windows 实体机准备成 P5 验证机，供 DSH 通过 SSH 连上来跑 W1～W10（见 `docs/30` §1.5）
- 交付物：本文 + 两个脚本 `tools/p5/windows-recon.ps1`（采集）+ `tools/p5/windows-preflight.py`（判定）
- 一句话：**必装 6 件（Git / MSVC Build Tools / rustup+1.99.1 / Python 3.12 / OpenSSH Server / 长路径开关），
  外加 3 个非安装前提（≥25 GB 空闲、NTFS 路径、`core.autocrlf=false`）；装完跑一条判定命令，
  `R1～R15` 无 FAIL 再给我 SSH。**

> 版本选择的唯一原则：**与 Linux 验证机对齐**（Rust 1.99.1 / Python 3.12）。跨平台比较里
> 工具链版本本身就是变量，不对齐的话 `cargo fmt --check` 这类检查会出现与代码无关的差异，
> 事后没法区分"平台差异"和"工具链差异"。

---

## 1. 硬件与系统门槛

| # | 项 | 要求 | 不满足会怎样 | 谁检查 |
|---|---|---|---|---|
| 1 | 系统 | **Windows 10 x64 1809（build 17763）及以上**，Win11 更好 | 1809 以下没有内置 OpenSSH 可选功能、长路径支持也不完整 | R1 |
| 2 | 架构 | **x64**（ARM64 不测：`ort` 无 ARM64 Windows 预编译包，且 P5 的 ARM64 面在 Android 侧） | 无法进行 | R2 |
| 3 | CPU | ≥4 逻辑核 | 能编但 `link.exe` 链接 ORT 会很难受 | R3 |
| 4 | 内存 | ≥8 GiB（**16 GiB 推荐**） | 链接期 OOM | R4 |
| 5 | 磁盘 | 系统卷 **≥25 GiB 空闲**（低于 15 GiB 直接不可行） | 装不下 | R5 |
| 6 | 文件系统 | **NTFS**，且仓库**不要**放 OneDrive/网络盘 | 长路径、符号链接、增量编译都会出怪问题 | R5 |
| 7 | 网络 | 能访问 `static.rust-lang.org`、`index.crates.io`、`static.crates.io`、`cdn.pyke.io` | 装不了工具链 / 构建 ORT 时下载失败 | 手工 |
| 8 | 显卡 | 有 DX12 能力的驱动（**仅 W6/ORT 相关**） | `ort` 在 Windows 上会解析到 DirectML 包（见 §6 与 D-92），无 DX12 时 CPU EP 仍可用但要实测 | 手工 |

磁盘预算（估算，非实测）：MSVC Build Tools + Win11 SDK ≈ 5–6 GB，Rust 工具链 ≈ 1.5 GB，
cargo registry ≈ 1 GB，`target/`（debug，全 feature）≈ 7–9 GB（Linux 实测 7.2 GB，Windows 量级相同）。

---

## 2. 必装清单

### B1 Git for Windows（≥2.40）

```powershell
winget install --id Git.Git -e
```
装不上 winget 就从 <https://git-scm.com/download/win> 下安装包。装完**立刻**设两个全局项（关键）：

```powershell
git config --system core.longpaths true
git config --global core.autocrlf false
```

- `core.autocrlf=false`：本仓库**没有 `.gitattributes`**，Windows 默认的"检出成 CRLF"会改变工作树字节数。
  `tools/p4/build-tokenizer.py --check`、`fixtures/**` 的 sha256 这类校验对字节敏感，CRLF 会制造无意义的失败。
- `core.longpaths=true`：`target\...\build\ort-sys-<hash>\out\...` 这类路径很容易超 260。

### B2 Visual Studio 2022 Build Tools（MSVC v143 + Windows 11 SDK）

只装 Build Tools，不需要完整 VS。（如果这台机器**已经**装了 VS 2022，那就够了，跳过本节的安装命令。）

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```
或下载 <https://aka.ms/vs/17/release/vs_buildtools.exe> 后：

```powershell
vs_buildtools.exe --quiet --wait --norestart --nocache `
  --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended
```

**为什么必须要它（两条，都是硬依赖）：**
1. Rust 的 `x86_64-pc-windows-msvc` 目标要 `link.exe`；
2. `rusqlite` 用的是 `bundled` feature（`libsqlite3-sys` 现场编 SQLite 的 C 源码），要 `cl.exe`。

验证（`link.exe` **不会**出现在 PATH 里，这是正常的，用 vswhere 查组件）：

```powershell
& "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" `
    -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationVersion
# 期望：17.9 以上
```

### B3 Rust 工具链（**钉 1.99.1**）

```powershell
winget install --id Rustlang.Rustup -e
# 重开一个终端，让 %USERPROFILE%\.cargo\bin 进 PATH，然后：
rustup toolchain install 1.99.1 --profile minimal --component rustfmt clippy
rustup default 1.99.1
rustc -vV
```

- `host:` 必须是 **`x86_64-pc-windows-msvc`**。
- 如果 `1.99.1` 在这台机器上装不到（版本已过期/镜像没有），退到 `rustup default stable` 也行，
  **但要在给我的消息里说明**——那样 `cargo fmt --check` 的差异我会按"工具链差异"记，不算代码问题。
- **不要**装 `x86_64-pc-windows-gnu` 目标去"试试 GNU 工具链"（见 §4）。

### B4 Python 3.12（≥3.9）

```powershell
winget install --id Python.Python.3.12 -e
# 重开终端
python --version   # 期望 Python 3.12.x
```

- 必须 **≥3.9**：`tools/check-release.py` 用了 `str.removeprefix`。
- 只要标准库，**不用 pip 装任何包**。
- 若 `python` 弹出 Microsoft Store，说明走的是"应用执行别名"，去
  `设置 → 应用 → 高级应用设置 → 应用执行别名` 关掉 `python.exe/python3.exe` 的别名，或用 `py -3` 验证。

### B5 OpenSSH Server（服务 + 自启 + 防火墙）

管理员 PowerShell：

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Start-Service sshd
Set-Service -Name sshd -StartupType Automatic
New-NetFirewallRule -Name sshd -DisplayName 'OpenSSH Server (sshd)' `
    -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
```

验证：

```powershell
Get-Service sshd                              # Running
Get-NetTCPConnection -LocalPort 22 -State Listen
```

> `Add-WindowsCapability` 需要联网（走 Windows Update 拉 FoD）。若这台机器离线，
> 告诉我，我改成"你手动传 `OpenSSH-Win64.zip` + `Install-sshd.ps1`"的方案。

### B6 长路径开关（注册表）

管理员：

```powershell
reg add "HKLM\SYSTEM\CurrentControlSet\Control\FileSystem" /v LongPathsEnabled /t REG_DWORD /d 1 /f
```
（`git config --system core.longpaths true` 已在 B1 里设过，两者是不同层面，都要。）

### B7 目录与仓库落地

```powershell
mkdir C:\dev
# 方式 A（推荐，带完整历史与 tag）：本机 Linux 侧
#   git bundle create /tmp/tw.bundle --all
#   scp /tmp/tw.bundle user@win:/c/dev/
# Windows 侧：
git clone C:\dev\tw.bundle C:\dev\translate-workspace
cd C:\dev\translate-workspace
git tag        # 应看到 p0 … p5.1
```

- **不要**放在 `C:\Users\<你>\OneDrive\...`、桌面同步目录、或网络盘。
- 路径尽量短：`C:\dev\translate-workspace`（29 MB，不含 `target/`）。
- 若直接拷 zip 也行，但**必须**保留 `.git`，否则 `freeze-manifest.py`（要用 `git show`）和我核对 tag 都会失效。

### B8 空闲空间确认

不需要"装"，但请确认 `C:` 剩 **≥25 GiB**：`Get-PSDrive C | Select-Object Used,Free`。

---

## 3. 可选（不做也能开始，但会影响结论覆盖面）

| # | 项 | 命令 | 收益 |
|---|---|---|---|
| O1 | Defender 排除 | `Add-MpPreference -ExclusionPath "C:\dev\translate-workspace","$env:USERPROFILE\.cargo"` | 增量编译快一倍左右；不做就是慢，不影响正确性 |
| O2 | ORT 模型文件（110 MB） | 从本机 `/tmp/p4/ort-model/` 传 3 个文件过去（`encoder_model_quantized.onnx` 51 MB、`decoder_model_merged_quantized.onnx` 58 MB、`tokenizer.json` 1.7 MB） | 只有传了才能跑 W6 的真实翻译；不传就只验"能构建" |
| O3 | 非内网可达 | 端口映射/内网穿透到 22 | 只有不在 `192.168.1.0/24` 时才需要 |
| O4 | 没有 winget | 手工下安装包（见 §8 的 URL 表） | Win10 1809 可能没有 App Installer |

---

## 4. 明确**不要**装的东西（省时间，也避免自伤）

| 不要装 | 原因 |
|---|---|
| MinGW-w64 / MSYS2 的 gcc / `x86_64-pc-windows-gnu` 目标 | 本项目的 Windows 验收口径是 **MSVC**；混入 GNU 会让 `link.exe` 查找、`ort` 的 DLL 名、`libsqlite3-sys` 的编译方式全部错位，排查成本远大于收益 |
| CMake | `Cargo.lock` 里 cmake 依赖数为 **0**，没有任何 crate 需要它 |
| OpenSSL（含 vcpkg 版） | `native-tls` 在 Windows 走 **schannel**，`openssl-sys` 只在非 Windows 目标出现；装了反而可能被 `OPENSSL_DIR` 之类的环境变量误导 |
| Visual Studio 完整版 | Build Tools 足够（C++ 工作负载相同），省 10 GB+ |
| WSL2 / Docker Desktop | 不需要。若已装，**不要**在 WSL 里编译本项目——那测的是 Linux，不是 Windows |
| Node.js / Java / Android SDK / NDK | Android 面在本机 Linux 上（`docs/30` §1.1）；Windows 机只做 Windows 验证 |
| 任何"优化/清理/安全卫士"类软件 | 会改 PATH、拦进程、动注册表，制造不可复现的环境 |

---

## 5. 装完怎么自检（这一步做完再给我 SSH）

自检是**机器判定**，不是"看着差不多"。两种跑法：

**跑法一（推荐，SSH 通了我这边一条命令）**

```bash
# 本机 Linux 侧
cd /home/nan/deepseek-harness/translate-workspace/translator-sdk
tools/p5/windows-recon.sh user@<windows-ip> -o win-recon.json
tools/p5/windows-preflight.py win-recon.json
```

**跑法二（SSH 还没通，你自己在 Windows 上跑）**

```powershell
# 先在 Windows 上放好 windows-recon.ps1（拷过去即可），然后：
powershell -NoProfile -ExecutionPolicy Bypass -File windows-recon.ps1 > "$env:TEMP\recon.txt"
# 把 DSH-RECON-BEGIN 与 DSH-RECON-END 之间的那段（就是 JSON）存成 recon.json，
# 连同 windows-preflight.py 一起放到同目录，然后：
python windows-preflight.py recon.json
```

判定项（`R1`～`R15`，编号固定，报告里引用同一套）：

| id | 判定 | FAIL 条件 |
|---|---|---|
| R1 | 系统版本 | build < 17763；Win10 记 WARN |
| R2 | 架构 | 非 64-bit |
| R3 | CPU | 逻辑核 < 4 → WARN |
| R4 | 内存 | < 6 GiB FAIL；< 14 GiB WARN |
| R5 | 系统卷 | 空闲 < 15 GiB FAIL；< 25 GiB WARN；非 NTFS 记 WARN |
| R6 | Rust host | 缺失，或 ≠ `x86_64-pc-windows-msvc` |
| R7 | rustfmt/clippy | 组件缺失 |
| R8 | Python | 缺失，或 < 3.9 |
| R9 | Git | 缺失 |
| R10 | sshd | 服务不存在/未运行 FAIL；非 Automatic 或 22 未监听 WARN |
| R11 | 长路径 | `LongPathsEnabled` ≠ 1 |
| R12 | MSVC 工具集 | vswhere 查不到 `VC.Tools.x86.x64` |
| R13 | 工具链对齐 | rustc 不是 1.99.1 → WARN（只影响 fmt 比对口径） |
| R14 | 提权 | 仅 INFO（只读勘察不需要） |
| R15 | Defender 排除 | 仅 INFO |

**期望输出**：`结论：PASS（PASS 13 / FAIL 0 / WARN 0～2 / INFO 2）`。
有 FAIL 就先按提示修；带 WARN 也可以给我连，我在报告里如实写。

---

## 6. 连上之后"看起来像失败、但其实是预期"的三件事

先讲清楚，免得你看到红字以为环境坏了：

1. **Bergamot 在 Windows 上必然不可用**——`translator-runtime` 的平台门禁只认 `linux`
   （`VERIFIED_PLATFORMS = ["linux"]`，`docs/29` D-84）。所以 W7 的正确结果是
   **返回 `CapabilityUnavailable`**，不是"翻译失败"。这条我会写成**显式断言**。
2. **`ort` 在 Windows 上会去下 DirectML 版的预编译包**（`docs/31` D-92）：`ort-sys` 的预编译索引里
   Linux 有 `none` 行（我们用的是静态链接的 CPU 版），而 **`x86_64-pc-windows-msvc` 只有
   `directml / webgpu / openvino / nvrtx / cuda13` 等带 EP 的行，没有 `none`**；默认 feature 下
   `resolve_dist()` 会走到"第一个候选"（directml）。含义有三点：① 首次构建会从 `cdn.pyke.io`
   拉包（约 100–200 MB 量级）；② 产物旁边会多出 `onnxruntime.dll`（`copy-dylibs`）；
   ③ **Windows 的 ORT 性能数字与 Linux 不是同一个 runtime 变体，不能直接并排比**——这条我会写进报告口径。
   真要 CPU-only，需要自建 ONNX Runtime 并设 `ORT_LIB_LOCATION`，那是另一件事，先不做。
3. **`cargo test` 里本来就有一批 `ignored`**（Linux 上 60 个），需要真实模型/真机 worker 的用例默认跳过；
   Windows 上只会更多，不会更少。数量差异我会逐条对照，不拿"数字不同"当结论。

---

## 7. 我连上之后的权限边界

- **会做**：只读勘察（`systeminfo` 级）、`cargo build/test/fmt/clippy`、跑 `translator-cli`、
  必要时把模型文件传上去（若你允许 O2）。
- **会写**：`%USERPROFILE%\.cargo`（registry 缓存）、仓库内 `target/`、`%TEMP%`、以及
  `P4_ORT_MODEL_DIR` 指向的目录（若 O2 启用）。除此之外不写。
- **不会做**：装/卸系统组件、改注册表、启停服务、改防火墙、改 PATH、重启机器。
  需要这些的时候我会先问你，而不是"顺手做了"。
- 全程留证据：每条命令、每份原始输出进 `docs/32`（Windows 与 Linux 的并排对比报告）。

---

## 8. 参考来源（装不到时用这些）

| 组件 | 官方地址 | winget id |
|---|---|---|
| Git for Windows | <https://git-scm.com/download/win> | `Git.Git` |
| VS 2022 Build Tools | <https://aka.ms/vs/17/release/vs_buildtools.exe> | `Microsoft.VisualStudio.2022.BuildTools` |
| rustup | <https://win.rustup.rs/x86_64> | `Rustlang.Rustup` |
| Python 3.12 | <https://www.python.org/downloads/windows/> | `Python.Python.3.12` |
| OpenSSH Server | Windows 可选功能 | `Add-WindowsCapability`（见 B5） |

---

## 9. 与本轮设计的关系（偏差登记）

- **D-92：`ort` 的 Windows 预编译包没有"纯 CPU"行，默认会解析到 DirectML 变体。**
  依据：`ort-sys 2.0.0-rc.15` 的 `build/download/dist.tsv` 里 `x86_64-pc-windows-msvc` 只有带 EP 的行，
  而 `x86_64-unknown-linux-gnu` 有 `none` 行；`build/download/resolve.rs` 在"没有精确匹配"时取
  `candidates.first()`（按共同 feature 数降序，全为 0 时保持表内顺序 → directml）。
  **这是读代码得出的，未在真机实测**，所以写成"预期"，真机结果若不同以实测为准。
  影响：Windows 的 ORT 数字与 Linux **不是同一 runtime 变体**，不能并排算倍数（ADR-052 的口径纪律）。
- **D-93：部署判定也做成机器可查（`tools/p5/windows-preflight.py`，`R1～R15`），但它本身没在真机跑过。**
  这是 `docs/30` D-89 的延伸：脚本先写后验。已用两份构造的 recon JSON 验过 PASS/FAIL/JSON 三条路径，
  真机跑出的第一份结果才作数。
- **D-94：`docs/31` 本来是 `docs/30` §4.3 里预留的"Windows 与 Linux 并排对比报告"，现在被本文占用，
  并排对比顺延为 `docs/32`。** 原因：部署清单要比对比报告先到你手上；编号是空位，不是覆盖，
  已在 `docs/30` 里同步改掉引用。
- **D-95：Rust 版本钉 1.99.1 而不是"最新 stable"。** 原因：与 Linux 验证机一致，避免 `rustfmt` 版本差
  制造假失败。代价：若该版本在 Windows 上不可得，就要退到 stable 并把差异显式记成工具链差异（R13 会 WARN）。
