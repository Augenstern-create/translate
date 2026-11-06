# P5-01 Android 工具链就位与 Windows 验证矩阵报告

- 日期：2026-11-06
- 阶段：P5-01（工具链与验证脚手架；**不含** Windows 实体机实测——SSH 参数未到）
- 关联：`docs/01` §13（P5 验收面）、`docs/27` §7（P5 承接）、`docs/29`（勘察与路线对比）、
  `docs/29` §4.3（待确认三件事）
- 路线决定（用户 2026-11-06 选择）：**Windows 只走 B 路线（实体机 SSH，真机验证）**，不搭本机 KVM 虚拟机；
  Android 侧**装 NDK（约 3 GB）+ 用现有 x86_64 AVD 打通 JNI 调用链**
- 一句话结论：**Android 的"能编、能加载、能调用"三段链已在真机运行时（AVD）上证明成立，
  ARM64 交叉编译也产出了合规产物；但 x86_64 设备对 ARM64 产物是硬拒绝（`EM_AARCH64`），
  所以 P5 的"ARM64 物理机证据"仍然是空的，本轮只是把工具链这一格填掉。Windows 侧本轮只交付只读勘察脚本
  + 验证矩阵，真机数据等 SSH。**

---

## 1. 实际实现内容

### 1.1 Android NDK 安装（一次性，2.3 GB）

`cmdline-tools/latest/bin/sdkmanager` 当前只列 `ndk;29.*`（含 rc）与 `ndk;30.*`，没有 r27 LTS 条目；
选择最新稳定版 **`ndk;30.0.16248370`**：

```
$ yes | sdkmanager --install "ndk;30.0.16248370"
$ cat $ANDROID_SDK_ROOT/ndk/30.0.16248370/source.properties
Pkg.Desc = Android NDK
Pkg.Revision = 30.0.16248370
Pkg.ReleaseName = r30
$ du -sh $ANDROID_SDK_ROOT/ndk/30.0.16248370
2.3G
```

工具链指纹（后面所有产物都来自它，写进报告便于复现）：

```
$ .../ndk/30.0.16248370/toolchains/llvm/prebuilt/linux-x86_64/bin/x86_64-linux-android21-clang --version
Android (16134705, +pgo, +bolt, +lto, +mlgo, based on r574158c) clang version 21.0.0
Target: x86_64-unknown-linux-android21
```

`rustup target add x86_64-linux-android`（`aarch64-linux-android` 本机已有），当前已装 5 个 target：
`aarch64-linux-android`、`aarch64-unknown-linux-gnu`、`x86_64-linux-android`、
`x86_64-pc-windows-gnu`、`x86_64-unknown-linux-gnu`。

### 1.2 JNI 调用链冒烟（Rust cdylib → ART → 返回值）

用一个**零第三方依赖**的最小 crate（`crate-type = ["cdylib"]`，`#[no_mangle] extern "C"`
手写 `Java_HelloJNI_*` 符号，不引 `jni` crate），目的是让"失败原因只可能是工具链"：

```
$ export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER=$NDK/.../x86_64-linux-android21-clang
$ export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$NDK/.../aarch64-linux-android21-clang
$ cargo build --release --target x86_64-linux-android     # OK, 0.28 s
$ cargo build --release --target aarch64-linux-android    # OK, 0.11 s
```

设备侧（AVD `gms2`，android-34，x86_64，`sys.boot_completed=1` 用 9 s）：

```
$ javac --release 8 -d out HelloJNI.java
$ d8 --min-api 21 --output dex out/HelloJNI.class
$ adb push dex/classes.dex           /data/local/tmp/jni/classes.dex
$ adb push libjnismoke.so            /data/local/tmp/jni/lib/x86_64/libjnismoke.so
$ adb shell "app_process -Djava.class.path=/data/local/tmp/jni/classes.dex \
      -Djava.library.path=/data/local/tmp/jni/lib/x86_64 /system/bin HelloJNI"
SMOKE arch=x86_64 abi_fn=1 add=42
SMOKE OK
```

这一条同时证明四件事：① `System.loadLibrary` 能找到并 `dlopen` Rust 产物；
② `Java_<Class>_<method>` 符号命名与 ART 解析一致（返回值一路回到 Java）；
③ 不需要 `liblog`/`libandroid` 之类的隐式依赖（`NEEDED` 只有 `libdl.so`、`libc.so`）；
④ 宿主机 Rust 1.99.1 + NDK r30 的链接组合可用。

按 16 KiB 页要求检查 ARM64 产物（Android 15 起强制，提前对齐）：
`llvm-readelf -l` 的 `LOAD` 段对齐为 **`0x4000`**（不是旧的 `0x1000`），无需额外配置。

### 1.3 负对照：x86_64 设备拒绝 ARM64 产物（这是本轮最重要的一条证据）

把同一份源码的 `aarch64` 产物推到 x86_64 设备上加载：

```
$ adb shell "app_process ... -Djava.library.path=/data/local/tmp/jni/lib/arm64 /system/bin HelloJNI"
Killed
$ adb logcat -d | grep UnsatisfiedLink
E AndroidRuntime: java.lang.UnsatisfiedLinkError: dlopen failed:
    "/data/local/tmp/jni/lib/arm64/libjnismoke.so" is for EM_AARCH64 (183) instead of EM_X86_64 (62)
    at java.lang.System.loadLibrary(System.java:1661)
    at HelloJNI.<clinit>(HelloJNI.java:2)
```

含义：**现有 AVD 在 ARM64 这件事上的证据力严格为零**，不是"慢一点/不完全等价"，而是加载器直接拒绝。
`docs/29` 的 D-83 因此从"推断"升级为"实测"：P5 的 ARM64 交付必须以
① 一台 ARM64 真机（`adb` 可连）或 ② 宿主机上的 arm64 镜像（本机 x86_64 上只能全模拟，性能数字不可用）
之一为前提。本轮的 `aarch64` 产物只能证明**编译与 ELF 合规**，不能证明**目标设备可运行**。

### 1.4 Windows 侧脚手架（只读勘察 + 拉取）

新增两个文件（`translator-sdk/tools/p5/`）：

| 文件 | 作用 | 关键约束 |
|---|---|---|
| `windows-recon.ps1` | 在 Windows 上采集环境，输出 `dsh-win-recon/v1` JSON | **纯只读**：不装不改、不写注册表、不启停服务；PS 5.1 兼容；脚本内**全 ASCII**（避免被代码页破坏）；输出夹 `DSH-RECON-BEGIN/END` 标记，便于剥掉 SSH banner |
| `windows-recon.sh` | 从 Linux 侧一条命令拉取并校验 | 脚本经 stdin 传给 `powershell -Command -`（**不落盘**、不受命令行长度限制）；`BatchMode=yes` + `ConnectTimeout=10` 快速失败；解码按 `utf-8 → cp936 → cp1252` 兜底（中文版 Windows 控制台会吐 CP936） |

采集项（对应 `docs/29` §4.3 要问的那几件事，全部可机器读）：
OS 版本/build/DisplayVersion、是否提权、`LongPathsEnabled`、活动代码页、CPU 核数与 VT 标志、
内存、各卷可用空间、`git/curl/tar/cmake/python/node`、`vswhere` 列出的 VS 安装、Windows SDK Lib 版本、
`cargo/rustc/rustup` 与已装 target、**sshd 服务状态与 22 端口监听**、Defender 实时保护与排除项、
WSL/Docker/choco/winget 是否存在、IPv4 列表。

### 1.5 Windows 验证矩阵（真机到货后按此表跑，逐项写证据）

| # | 项 | 命令 | 预期 | 证据类型 |
|---|---|---|---|---|
| W1 | 工具链 | `cargo --version`、`rustc -vV` | MSVC host，`host: x86_64-pc-windows-msvc` | 原文 |
| W2 | 格式 | `cargo fmt --check` | PASS | 退出码 |
| W3 | 静态检查 | `cargo clippy --all-targets -- -D warnings`（默认 + `--all-features` 两种） | PASS | 退出码 |
| W4 | 单测 | `cargo test`（默认 feature） | 与 Linux 同口径的用例集 | 计数对比 |
| W5 | 字典 | `cargo test -p translator-dictionary`（`rusqlite` 为 `bundled`，需本地编 SQLite C 源） | PASS（验证 MSVC 能编 C） | 原文 |
| W6 | ORT 后端 | `cargo test -p translator-nmt-ort --features ort-runtime` + 真实模型示例 | 视模型文件是否可传而定；`ort` 默认 feature 含 `download-binaries`+`copy-dylibs`，构建期会下载 win-x64 预编译 DLL（需外网） | 原文 + 版本 |
| W7 | **Bergamot 平台门禁** | 调用 `translator-runtime` 的 NMT 入口 | **`CapabilityUnavailable`**（`VERIFIED_PLATFORMS = ["linux"]`，`docs/29` D-84）——这是**预期不支持**，不是失败 | 错误码 |
| W8 | CLI mock 路径 | `translator-cli` 查词 + mock 翻译 | 输出与 Linux 逐字对比 | diff |
| W9 | 发行工具 | `tools/check-release.py --check`、`tools/p4-acceptance.py --check`、`tools/license-audit.py` | 预期 Python 侧可移植（三脚本只依赖 `subprocess`/`pathlib`），**未验证，不作承诺** | 原文 |
| W10 | 路径/编码 | 在含空格与中文的目录下跑 W4 | 不炸；若炸则记为真实缺陷 | 原文 |

W6 需要把 `/tmp/p4/ort-model/` 三个文件（约 300 MB 级）传过去；在拿到 SSH 与磁盘可用空间前不预设它一定跑得成，
矩阵里按"可选项"对待：跑不成不影响 W1–W5、W7–W8 的结论，但要如实写"未验证"。

### 1.6 本轮明确**没做**的事

- 没连任何 Windows 机器（没有 SSH 参数），所以 W1–W10 全是**空表**；
- 没建 KVM 虚拟机（用户已选 B 路线，`docs/29` §3 的 A 路线不执行）；
- 没改 CI（`ci.yml` 仍是 `ubuntu-24.04` 单 job；G11"CI 从未在真实 runner 上跑过"保持 open，
  Windows job 属于 C 路线，未获授权）；
- 没有 ARM64 真机，也没有 arm64 系统镜像；
- 没把 JNI 冒烟 crate 放进仓库（它在 `/tmp/jnismoke`，是一次性验证物；真正的 `translator-jni` 接口要按
  §13 的交付面设计，不适合拿冒烟代码占位）。

---

## 2. 可复现的测试结果

```bash
# ---- 0) 前置 ----
. /opt/rust/env.sh
export ANDROID_SDK_ROOT=$HOME/Android/Sdk
export PATH=$ANDROID_SDK_ROOT/platform-tools:$PATH

# ---- 1) NDK ----
yes | $ANDROID_SDK_ROOT/cmdline-tools/latest/bin/sdkmanager --install "ndk;30.0.16248370"
du -sh $ANDROID_SDK_ROOT/ndk/30.0.16248370                      # 2.3G

# ---- 2) rust target ----
rustup target add x86_64-linux-android                          # aarch64 已有
rustup target list --installed                                  # 5 个

# ---- 3) 交叉编译（两种 ABI）----
NDK=$ANDROID_SDK_ROOT/ndk/30.0.16248370/toolchains/llvm/prebuilt/linux-x86_64/bin
cd /tmp/jnismoke
CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER=$NDK/x86_64-linux-android21-clang \
  cargo build --release --target x86_64-linux-android
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$NDK/aarch64-linux-android21-clang \
  cargo build --release --target aarch64-linux-android

file target/*/release/libjnismoke.so
# x86_64: ELF 64-bit LSB shared object, x86-64   (437,240 B)
# aarch64: ELF 64-bit LSB shared object, ARM aarch64 (457,376 B)

$NDK/llvm-nm -D --defined-only target/aarch64-linux-android/release/libjnismoke.so | grep Java_
# 0000000000012d08 T Java_HelloJNI_abi
# 0000000000012d10 T Java_HelloJNI_add

$NDK/llvm-readelf -l target/aarch64-linux-android/release/libjnismoke.so | grep '^  LOAD'
# LOAD ... R E 0x4000   /   LOAD ... RW 0x4000   /  LOAD ... RW 0x4000   (16 KiB 页对齐)

$NDK/llvm-readelf -d target/aarch64-linux-android/release/libjnismoke.so | grep NEEDED
# libdl.so, libc.so   （无 liblog 等隐式依赖）

# ---- 4) AVD ----
$ANDROID_SDK_ROOT/emulator/emulator -avd gms2 -no-window -no-audio -no-snapshot \
    -no-boot-anim -gpu swiftshader_indirect &
adb wait-for-device && adb shell getprop sys.boot_completed      # 1（约 9 s）
adb shell getprop ro.product.cpu.abi                             # x86_64
adb shell getprop ro.build.version.sdk                           # 34

# ---- 5) 正向：x86_64 产物在 x86_64 设备上 ----
javac --release 8 -d out HelloJNI.java
$ANDROID_SDK_ROOT/cmdline-tools/latest/bin/d8 --min-api 21 --output dex out/HelloJNI.class
adb shell mkdir -p /data/local/tmp/jni/lib/x86_64
adb push dex/classes.dex /data/local/tmp/jni/classes.dex
adb push target/x86_64-linux-android/release/libjnismoke.so /data/local/tmp/jni/lib/x86_64/
adb shell "app_process -Djava.class.path=/data/local/tmp/jni/classes.dex \
    -Djava.library.path=/data/local/tmp/jni/lib/x86_64 /system/bin HelloJNI"
# SMOKE arch=x86_64 abi_fn=1 add=42
# SMOKE OK

# ---- 6) 负对照：aarch64 产物在 x86_64 设备上 ----
adb push target/aarch64-linux-android/release/libjnismoke.so /data/local/tmp/jni/lib/arm64/
adb shell "app_process -Djava.class.path=/data/local/tmp/jni/classes.dex \
    -Djava.library.path=/data/local/tmp/jni/lib/arm64 /system/bin HelloJNI"
# Killed（EXIT=137）
adb logcat -d | grep UnsatisfiedLink
# ... is for EM_AARCH64 (183) instead of EM_X86_64 (62)

adb emu kill

# ---- 7) Windows 脚手架自检（无 Windows 机器，用假 ssh 走通解析链）----
/tmp/pwsh/pwsh -NoProfile -Command \
  '$e=$null;$t=$null; $null=[System.Management.Automation.Language.Parser]::ParseFile(
     "translator-sdk/tools/p5/windows-recon.ps1",[ref]$t,[ref]$e); "errors=$($e.Count)"'
# errors=0（pwsh 7.4.6 AST 解析；注意：这只是语法通过，不等于在 PS 5.1 真机上跑过）

bash -n translator-sdk/tools/p5/windows-recon.sh                # 语法 OK
# 假 ssh 测试：CP936 编码的 JSON + banner → 正确解出，caption 中文未乱码
# decoded    : cp936
# os         : Microsoft Windows 11 家庭中文版 build 26100 (64-bit)
# 无标记时：exit 2 + 打印原始输出（已验）
```

| 检查 | 结果 |
|---|---|
| NDK r30 安装 | PASS（2.3 GB） |
| `x86_64-linux-android` / `aarch64-linux-android` 交叉编译 | PASS（0.28 s / 0.11 s） |
| ARM64 产物 ELF 合规（符号、16 KiB 对齐、无 liblog） | PASS |
| JNI 正向调用（x86_64 AVD） | PASS（`SMOKE OK`） |
| ARM64 产物在 x86_64 设备 | **拒绝**（`EM_AARCH64`，预期如此） |
| ARM64 产物在 ARM64 设备 | **未测**（无设备） |
| Windows 勘察脚本语法 | PASS（pwsh AST, 0 error） |
| Windows 勘察脚本在真机执行 | **未测**（无 SSH） |
| Windows 验证矩阵 W1–W10 | **未测**（空表） |

---

## 3. 与原设计的偏差及原因

- **D-86：NDK 版本选 r30（`30.0.16248370`）而不是 r27 LTS。**
  原因：本机 `sdkmanager` 的仓库索引里只有 `29.*`（含 rc）与 `30.*`，没有 r27；`docs/29` 原本写的
  "NDK 约 3 GB"是按 r27 估的，实际 r30 落盘 **2.3 GB**。影响：r30 的 clang 21 默认按 16 KiB 页对齐
  （`0x4000` 已实测），这对 Android 15+ 是好事；但 r30 是"当前最新"而非 LTS，若后续遇到
  与 `cargo-ndk` 版本不匹配，回退到 r29 稳定版即可，不需要改代码。**报告里所有 Android 数字都必须注明是 r30**。
- **D-87：`aarch64` 只做到"编得出来"，没做到"跑得起来"。**
  原因：没有 ARM64 真机，宿主机是 x86_64，`docs/29` §2.3 已经点明这个缺口。本轮的负对照把代价量化了：
  不是"效果差一点"，而是 `dlopen` 直接拒绝（`EM_AARCH64`）。影响：**P5 的"物理机基准"这一格仍然空着**，
  任何"Android 端已支持"的表述都必须带上"x86_64 功能预演"这个限定词。补救路径只有两条：
  ① 接一台 ARM64 真机（`adb` 可用即可，不要求 Play 认证）；② 宿主机装 arm64 镜像 + QEMU 全模拟
  （能跑通功能，但延迟/内存数字一律不可用，只能证明"能起来"）。
- **D-88：`d8` 的位置与 `app_process` 的正确调用姿势都踩了一次。**
  原因：本机没装 `build-tools`（`platforms/` 也是空的），但 `cmdline-tools/latest/bin` 里自带 `d8`，
  可直接用；而 `app_process` 用 `CLASSPATH` 环境变量传 dex 目录会**静默失败**（`EXIT=137`，
  logcat 里是 `library "libjnismoke.so" not found`），必须用 `-Djava.class.path=<dex 文件路径>` +
  `-Djava.library.path=<含 lib<name>.so 的目录>`。影响：这条写进 §2 的命令块，避免下一个人重复踩。
  （顺带：这就是"要求里没写、但复现时必须知道"的东西，属于本轮的真实收益。）
- **D-89：Windows 脚手架是"先写后验"，且只验了一半。**
  原因：没有 SSH 参数，只能在 Linux 侧用假 `ssh` 走通解析链（banner + CP936 + 标记剥离 + 无标记失败路径），
  并用 pwsh 7.4.6 做 AST 解析。**没有**在 PowerShell 5.1 真机上执行过，也没有验证
  `Get-MpPreference`/`Get-NetTCPConnection` 在目标系统上是否存在（缺失时脚本会降级为空值而不是报错，
  这一点是刻意设计，但"降级"不等于"采集成功"）。影响：拿到 SSH 后第一件事就是跑它，
  若与预期不符，改脚本而不是改报告。
- **D-90：CI 未动（C 路线未获授权），G11 继续 open。**
  原因：用户明确"只走 B"。影响：`windows-latest` 的回归护栏暂时没有，Windows 侧的可重复性完全依赖
  实体机 + 本轮脚本；若之后想要"每次提交都在 Windows 上跑一遍"，那是 C 路线，需要单独决策（ADR 已留位）。
- **D-91：JNI 冒烟代码没进仓库。**
  原因：它只为验证工具链，接口形态（`jni` crate 封装 / 手写 `extern "C"` / 是否暴露 `translator-api` 的
  同步面）属于 P5 正式设计的一部分，用冒烟代码占位会诱导后面的人把它当基线。影响：`translator-jni`
  仍是待建 crate；本轮产物在 `/tmp/jnismoke`，报告里给了完整源码结构（22 行 Rust + 20 行 Java），可重建。

---

## 4. 下一步（阻塞点只剩一个：SSH 参数）

**要开始 Windows 真机验证，我需要你给：**

1. **`user@host`（以及端口，若不是 22）**——若 Windows 机器在 `192.168.1.0/24` 内且本机 `192.168.1.10`
   能直连，给内网 IP 即可；若不在同一网段，给可达方式（端口映射/内网穿透/跳板）。
2. **认证方式**：公钥（给我 `authorized_keys` 里的注释即可确认）还是密码（**不要贴密码**，
   建议临时建一个专用账号或临时开 key）。
3. **OpenSSH Server 是否已启用并自启**：`Get-Service sshd` 为 `Running`、`StartType=Automatic`；
   若没开，需要你在机器上执行 `Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0`（需管理员）。
4. **账号权限**：是否管理员（装 rustup + MSVC 生成工具需要，合计约 3–6 GB；若不希望提权，
   我可以用 `rustup` 装到用户目录 + VS Build Tools 的"仅用户"安装，但 `link.exe` 可能仍需管理员）。
5. **能否安装**：rustup（约 1–2 GB）、MSVC Build Tools + Windows SDK（约 2–4 GB）、
   以及可选地下载 `ort` 的 win-x64 预编译运行时（约 100–200 MB）；**有没有不能碰的目录/服务**。
6. **是否有外网**（`static.rust-lang.org` 与 `github.com` 可达），以及磁盘上至少 **15 GB** 空闲。

拿到之后我的顺序是：`windows-recon.sh` 只读勘察 → 装工具链 → 按 §1.5 的 W1–W10 逐项跑并留原文证据 →
在 `docs/31` 里给出 Windows 与 Linux 的并排对比（含 W7 的 `CapabilityUnavailable` 断言）→ ADR-054 落定。

Android 侧剩下的只有"真机"这一个变量：

- 你手上若有 **ARM64 手机**（Android 8+ 即可），开 USB 调试接上，`adb devices` 能看到就够了；
  我会在它上面跑同一套 JNI 冒烟 + `translator-cli`，并补 P5 要求的物理机基准（延迟/内存按 `docs/26` 的口径）。
- 没有的话，本轮的 ARM64 结论就停在"仅编译合规"，这条会写进 P5 总结的"未验证项"。

---

## 5. 附：本轮新增/改动的文件

| 文件 | 变更 |
|---|---|
| `tools/p5/windows-recon.ps1` | 新增（只读勘察，`dsh-win-recon/v1`） |
| `tools/p5/windows-recon.sh` | 新增（远程拉取 + 解码兜底 + 校验 + 摘要打印） |
| `docs/30_*.md` | 本报告 |
| `docs/01` §14（ADR-054～056、修订记录 v1.16） | 记录路线决策与证据分层 |
| `README.md`、`CHANGELOG.md` | 记录 P5-01 |
