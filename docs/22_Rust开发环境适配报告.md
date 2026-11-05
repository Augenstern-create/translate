# Rust 开发环境适配报告

**项目**：轻量跨平台混合翻译 SDK
**依据文档**：`docs/01_翻译SDK完整开发设计.md`、`docs/02_翻译资源候选与备选清单.md`
**主机**：nan-ubuntu-desktop
**核对日期**：2026-11-05
**修订**：v5 — 2026-11-05 启用 profile 编译配置（`/opt/rust/cargo/config.toml`）并验证生效；P0 骨架已落地，见 `03_P0骨架与验证报告.md`
**状态**：Rust 工具链已安装；其余系统库**未执行任何变更**（apt 全程 `-s` 模拟）

---

## 0. 关于上一版文档的重要更正

上一版按"**Web 后端 + Tauri/egui 桌面 GUI**"的假设撰写，这个假设是**错的**。依据 `01` 文档 §0「交付形式：Rust Library SDK；**不实现 UI**」与 §1.2「暂不实现：图形界面、划词热键」：

| 上一版的结论 | 实际情况 | 处理 |
|---|---|---|
| 需要补 `libgtk-3-dev`、`libwebkit2gtk-4.1-dev`、`librsvg2-dev` 等 6 个包（99 个依赖） | **完全不需要**，项目不含 GUI | 整段删除 |
| 需要 egui / iced / Tauri 选型建议 | 不适用 | 删除 |
| 补 `libxkbcommon-dev`、`libgl1-mesa-dev` 等 X11/Wayland 头文件 | 不需要 | 删除 |
| 建议"优先 egui，Tauri 编译太慢" | 无意义 | 删除 |

**保留并重新核实的内容**：系统硬件、C 编译链、网络与代理、沙箱权限边界、apt 候选包存在性。

**真正需要的是什么**（依据 `01` §3、§10、§13 与 `02` §6）：不是"桌面库"，而是 **FFI 绑定工具链 + CMake C++ 构建 + 移动端 NDK + 跨平台交叉编译 + Python 模型转换链**。这是完全不同的方向。

---

## 1. 结论摘要

1. **Rust 工具链已安装完成**（2026-11-05，`/opt/rust`，rustc/cargo 1.99.1，四目标 std 就位）。**P0 阶段现在可以直接开工。**
2. **真正的缺口不在"库"，在"工具链矩阵"**。三个整块缺失：
   - **Android NDK 完全没有**（连 `platforms`、`build-tools` 都没装），而 `01` §12.2 把 Android arm64 列为必须构建+必测平台；
   - **交叉编译工具链全空**（mingw-w64 / aarch64-linux-gnu / cargo-ndk / cargo-xwin 一个没有），而 `01` §10 要求产出 Windows + Linux arm64 + Android 产物；
   - **Python 模型转换链全空**（连 `pip` 都没有），而 `02` §5–§6 的模型量化、pnnx/ONNX 转换全部依赖它。
3. **三条平台级硬限制，装包解决不了**：macOS 目标无法在本机构建、Android arm64 必须物理机、`sudo` 被沙箱阻断。详见第 6 章。
4. **P0 阶段其实只需要 rustup**。`01` §13 的 P0 是"workspace + traits + Mock plugins"，零 native 依赖。**可以今天就开工**，其余依赖按阶段后置（第 4 章）。
5. **硬件对 NMT 推理友好**：CPU 有 AVX2 + FMA（Marian/intgemm 关键路径）、31 GiB 内存足够跑 M2M100-418M 甚至 MADLAD-3B 做桌面基准、613 G 磁盘、`/dev/kvm` 可用。
6. 全部所需 apt 包已核实**存在于 noble 官方源**，共 **26 个**，无一个不可得。

---

## 2. 环境实测清单

### 2.1 系统与硬件

| 项目 | 实测值 | 对项目的意义 |
|---|---|---|
| 发行版 | Ubuntu 24.04.5 LTS (noble) | ✅ 对应 `01` §10 的 "Ubuntu x64" 目标 |
| 内核 / 架构 | 7.0.0-42-generic / x86_64 | ✅ 本机构建目标 |
| CPU | Intel i5-8250U，4 核 8 线程 @1.6–3.4GHz | ⚠️ 编译慢；但推理够用 |
| **CPU 指令集** | **AVX、AVX2、FMA、SSE4.2**（无 AVX512） | ✅ Bergamot/intgemm 的 AVX2 快路径可用 |
| 内存 | 31 GiB（可用 27） + 59 GiB swap | ✅ 可本地跑 M2M100-418M、MADLAD-3B 桌面基准 |
| 磁盘 | 824G，剩 **613G** | ✅ 模型（数百 MB~GB 级）与多 toolchain 无压力 |
| inode / ulimit | 剩 5334 万 / nofile 1048576 | ✅ |
| cgroup 内存限制 | 无 | ✅ |
| 虚拟化 | 物理机（非容器/非 VM） | ✅ |
| `/dev/kvm` | 存在，CPU 有 vmx 标志 | ✅ Android 模拟器可硬件加速 |

### 2.2 GPU 与加速卡

| 项目 | 实测值 | 判断 |
|---|---|---|
| 集显 | Intel UHD Graphics 620 | 弱，ncnn Vulkan 仅可作功能验证 |
| 独显 | NVIDIA GeForce 930MX (GM108M, Maxwell) | ⚠️ 2017 年入门级，显存约 2 GB，**不适合任何 NMT 模型** |
| NVIDIA 驱动 | 已加载（nvidia / nvidia_drm / nvidia_uvm 模块），设备节点 `/dev/nvidia0` 等齐全 | — |
| CUDA Toolkit | **12.6 已安装**（`/usr/local/cuda-12.6`，nvcc 12.6.85） | ✅ ORT CUDA EP / CTranslate2 CUDA 可编译 |
| `nvidia-smi` | **报 `Failed to initialize NVML: Unknown Error`** | ⚠️ **需复核**（可能是沙箱阻断 ioctl，也可能是驱动与设备不匹配） |
| Vulkan | loader 已装，ICD 齐全（nvidia/intel/lvp 等 9 个） | ✅ 运行时可测 |

> **结论**：GPU 这条路对本项目价值有限。`02` §6 也明确"GPU 可用不代表一定优于 CPU，尤其移动设备短文本"。本项目以 **CPU 推理为基准**，GPU 仅作对照实验。`nvidia-smi` 的报错需在宿主终端确认，但不阻断任何 P0–P3 工作。

### 2.3 现有编译工具链

| 组件 | 版本 | 状态 |
|---|---|---|
| gcc / g++ / cc | 13.3.0 | ✅ 实测编译链接通过 |
| **clang / clang++** | **18.1.3** | ✅ **已装**（2026-11-05），bindgen 与可选链接器均依赖 |
| **LLVM（llvm-config）** | **18.1.3** | ✅ 已装，pnnx 与 `libclang` 依赖 |
| **libclang 开发文件** | 18.1.3 | ✅ `/usr/lib/llvm-18/lib/libclang.so` + `clang-c/Index.h` 均在位 |
| GNU Make | 4.3 | ✅ |
| **Ninja** | **1.11.1** | ✅ 已装，CMake 构建提速 |
| **ccache** | **4.9.1** | ✅ 已装，默认缓存 `~/.cache/ccache`，实测 1 命中 / 1 未命中 |
| **protobuf-compiler** | **3.21.12** | ✅ 已装（`protoc`） |
| **zlib1g-dev** | 1.3 | ✅ 已装 |
| CMake | 3.28.3 | ✅ 满足 ncnn / ORT / Bergamot 一般要求（≥3.26） |
| GNU ld / binutils | 2.42 | ✅ |
| glibc | 2.39 | ✅ |
| pkg-config | 1.8.1 | ✅ |
| git | 2.43.0 | ✅ Bergamot 子模块拉取可用 |
| git-lfs | 已装 | ✅ 模型大文件下载可用 |
| Python3 | 3.12.3 | ✅（但见 2.6） |
| Java | OpenJDK 21.0.12 | ✅ Android 构建链前提 |

> 附带装上的依赖：`libicu-dev`、`libxml2-dev`、`libncurses-dev`、`libffi-dev`、`libz3-dev`、`libprotobuf-dev`、`libobjc-13-dev`、`libgc1` 等 35 个包，共占用约 929 MB。这些都是 clang/llvm 的正常依赖，无副作用。

### 2.4 Rust 工具链 —— ✅ **已安装（2026-11-05）**

安装位置为**自定义路径**，不是默认的 `~/.cargo` + `~/.rustup`：

| 项目 | 值 |
|---|---|
| `CARGO_HOME` | `/opt/rust/cargo` |
| `RUSTUP_HOME` | `/opt/rust/rustup` |
| 环境脚本 | `/opt/rust/env.sh`（已由 `~/.bashrc` 与 `~/.profile` 自动 source） |
| rustc | **1.99.1** (58cf8f9c3, 2026-10-13) |
| cargo | **1.99.1** (5f94df478, 2026-08-27) |
| rustup | **1.29.3** (edbc831ce, 2026-11-02) |
| 磁盘占用 | **2.0 G**（rustup 1.3 G + cargo 约 0.7 G 含 registry 缓存） |

**已装组件**：`rustfmt`、`clippy`、`rust-analyzer`、`llvm-tools`、`rust-std`（四目标）、`rustc`、`cargo`

**已装 cargo 扩展**（全部位于 `/opt/rust/cargo/bin`）：

| 工具 | 版本 | 用途 |
|---|---|---|
| `cargo-ndk` | 4.1.2 | Android 构建 |
| `cargo-nextest` | 0.9.151 | 并行测试 |
| `cargo-deny` | 0.20.2 | 许可证 / 安全审计 |
| `cargo-about` | 0.9.3 | 依赖许可证清单（`02` §9） |
| `cargo-xwin` | 0.23.1 | Windows MSVC 交叉编译 |
| `sccache` | 0.7.7（apt 包） | 编译缓存，已设 `RUSTC_WRAPPER` |

**已装目标平台**（对应 `01` §10）：

```
x86_64-unknown-linux-gnu    ← 本机
aarch64-unknown-linux-gnu   ← Ubuntu arm64
x86_64-pc-windows-gnu       ← Windows（MinGW ABI）
aarch64-linux-android       ← Android arm64
```

> 仍缺失的 Rust 侧工具只剩两个可选件：`cross`（Docker 跨平台构建）与 `mold`（链接器）。二者非必需，见 5.6。

**重要注意事项**：因为 `CARGO_HOME` 不是默认值，任何**不继承 shell 环境**的工具（IDE 插件、systemd 服务、cron、部分 CI 脚本）都需要显式设置 `CARGO_HOME=/opt/rust/cargo`、`RUSTUP_HOME=/opt/rust/rustup`、`PATH=/opt/rust/cargo/bin:$PATH`，否则会报找不到 `cargo` 或重新去 `~/.cargo` 建目录。

### 2.5 Android 工具链 —— **残缺**

| 项目 | 状态 | 说明 |
|---|---|---|
| `ANDROID_HOME` / `ANDROID_SDK_ROOT` | ✅ `/home/nan/Android/Sdk` | 环境变量已设 |
| `JAVA_HOME` / `NDK_HOME` | ❌ **均未设置** | 需补 |
| adb | ✅ 37.0.1 | |
| emulator | ✅ 37.2.12 | |
| sdkmanager / avdmanager / android CLI | ✅ 存在 | |
| **NDK** | ❌ **完全没有**（`~/Android/Sdk/ndk` 目录不存在） | **`01` §10 要求 Android arm64 + JNI，此项为硬阻断** |
| **platforms**（API level） | ❌ **一个都没装** | 无法编译 AAR / 无法指定 targetSdk |
| **build-tools** | ❌ **未装** | |
| 系统镜像 | ✅ 仅 `android-31 google_apis x86_64`、`android-34 playstore x86_64` | ⚠️ **全是 x86_64**，跑不了 arm64-v8a |
| gradle | ❌ 未装 | Rust SDK 本身不需要，仅封装 AAR 时需要 |

> **sdkmanager 远端清单拉取失败**（`Failed to download any source lists` / `IO exception while downloading manifest`）。已尝试 `-D` 参数（该工具不接受）与 `JAVA_OPTS` 代理注入，**均无效**。但已验证 **NDK 压缩包可从 `dl.google.com` 直接用 curl 下载**（直连 200，0.13s），见 5.3 的绕过方案。

### 2.6 Python 模型转换链 —— **全空**

| 组件 | 状态 |
|---|---|
| python3 | ✅ 3.12.3 |
| **pip / pip3** | ❌ **完全没有**（`No module named pip`） |
| venv | ✅ 可用（`python3-venv` 已装） |
| numpy / torch / onnx / onnxruntime / transformers / sentencepiece | ❌ **一个都没有** |
| `llvm-config`（pnnx 需要） | ❌（随 `llvm` 包提供） |

> 影响：`02` §5.2 要求"量化后的准确率回归"、§6 要求"ncnn 转换后与原框架逐阶段对比 logits"，这些**全部依赖 torch + pnnx + onnx**。没有 pip 这条链根本起不来。

### 2.7 交叉编译工具链 —— **全空**

```
x86_64-w64-mingw32-gcc     NOT FOUND      ← Windows 目标（01 §10）
aarch64-linux-gnu-gcc      NOT FOUND      ← Linux arm64 目标（01 §10）
arm-linux-gnueabihf-gcc    NOT FOUND
zig / cross                NOT FOUND
cargo-ndk / cargo-zigbuild NOT FOUND      ← Android 目标
```

### 2.8 网络与依赖源

| 目标 | 结果 | 用途 |
|---|---|---|
| static.rust-lang.org | 200 (0.35s) | rustup 安装 |
| index.crates.io | 200 (0.26s) | cargo 拉包 |
| github.com | 200 (0.37s) | 源码 / 子模块 |
| **huggingface.co** | 200 (1.6s) | OPUS-MT / M2M100 / NLLB 模型 |
| **mozilla.github.io/translations/firefox-models/** | 200 (0.39s) | Firefox 模型注册表（`01` §8.1 首要来源） |
| github.com/browsermt/bergamot-translator | 200 (0.94s) | Bergamot 源码 |
| **dl.google.com** | **200 (0.13s，直连可通)** | **NDK 下载** |
| pypi.org | 200 (1.4s) | Python 包 |
| pypi.tuna.tsinghua.edu.cn | 200 (14.2s，慢) | pip 镜像备选 |
| cc-cedict.org / kaikki.org | 200 | 词典数据 |
| cdn.pyke.io | 301（重定向正常） | `ort` crate 预编译 ONNX Runtime 下载 |

- 代理：`http://127.0.0.1:7890`（`http_proxy`/`https_proxy` 已设，`no_proxy` 覆盖内网段）
- `~/.cargo/config.toml` 不存在；**直连 crates.io 正常，无需配镜像**
- 无 `/etc/pip.conf`；pip 装好后建议配清华镜像（官方源 1.4s 可用但镜像更稳）

### 2.9 其他工具

| 工具 | 状态 |
|---|---|
| docker | ✅ `/usr/bin/docker`（`cross` 跨平台构建可用） |
| snap | ✅ |
| vim / tmux / jq / unzip / zip / openssl / perl | ✅ |
| 代码编辑器 / IDE | ❌ 无 VSCode / nvim；**rust-analyzer 已随 Rust 装好**（/opt/rust/cargo/bin），接 IDE 时注意设置 `CARGO_HOME` |
| git 全局身份 | ❌ **未配 `user.name` / `user.email`**，提交会失败 |
| `sqlite3` CLI | ⚠️ 仅 Android SDK 内一份，系统无独立 CLI |

---

## 3. 缺口清单（映射到设计文档）

图例：**A = 阻断**（不解决就无法推进对应阶段）｜**B = 阶段必需**｜**C = 可选/优化**

### A 级 — 阻断

| # | 缺口 | 影响的设计要求 | 解决方式 | 执行者 |
|---|---|---|---|---|
| A1 | ~~Rust 工具链全空~~ | `01` §3 全部 crate、§13 P0 | ✅ **已解决**：rustup 装于 `/opt/rust` | 已完成（AI） |
| A2 | ~~**`clang` + `libclang-dev`**~~ | `01` §3 `runtime-bergamot`/`runtime-ncnn`、§10.1 C ABI；`02` §6 所有 FFI 方案 | ✅ **已解决**：clang/llvm 18.1.3 已装，bindgen 端到端验证通过 | 已完成（你） |
| A3 | **Android NDK** | `01` §10「Android arm64 + JNI」、§12.2 CI 必须、§13 P5 | 手动下载 zip（见 5.3） | 你 |
| A4 | **交叉编译工具链** | `01` §10 要求产出 Windows x64 / Linux arm64 / Android 产物 | apt + cargo 子命令 | 你 + AI |

> **关于 A2 的一个细节**：系统里其实**已有 `libclang-18.so.18` 和 `libclang-cpp.so.18.1`**（运行时库），但**缺 `libclang.so` 符号链接与头文件**。`bindgen` 通过动态加载 `libclang.so` 工作，所以 `libclang-dev` **仍然必须装**，不能因为有 `.so.18` 就跳过。

### B 级 — 阶段必需

| # | 缺口 | 影响 | 解决方式 |
|---|---|---|---|
| B1 | **`pip` 缺失** | `02` §5.2 量化回归、§6 模型转换全部无法进行 | `python3-pip` |
| B2 | **torch / onnx / onnxruntime / transformers / sentencepiece / pnnx** | 同上；pnnx 是 ncnn 官方转换工具 | pip（见 5.5） |
| B3 | **`ninja-build`** | Bergamot / ncnn / ORT 的 CMake 构建，make 可用但慢 2–3 倍 | apt |
| B4 | **`protobuf-compiler`** | ORT、ncnn 工具链、部分模型解析 | apt |
| B5 | **Android `platforms` + `build-tools`** | 编译 AAR / 指定 API level | sdkmanager 或手动 |
| B6 | **rustup 目标平台** | `aarch64-linux-android`、`aarch64-unknown-linux-gnu`、`x86_64-pc-windows-gnu` | `rustup target add` |
| B7 | **`cargo-ndk`** | Android 构建的事实标准工具 | `cargo install` |
| B8 | **`llvm`（含 llvm-config）** | pnnx 需要；`libclang` 已够 bindgen，但 pnnx 需完整 llvm | apt |

### C 级 — 可选/优化

| # | 缺口 | 用途 |
|---|---|---|
| C1 | `ccache` | 加速 C++ 部分（Bergamot/ncnn）重复构建 |
| C2 | `qemu-user-static` | **在 x86 主机上运行 arm64 Linux 二进制做冒烟测试**（值得装） |
| C3 | `mold` / `sccache` | Rust 链接与编译缓存加速（4 核机器收益明显） |
| C4 | `libsqlite3-dev` | 用系统 SQLite 而非 `bundled`；用 bundled 可跳过 |
| C5 | `zlib1g-dev` | 部分压缩 crate |
| C6 | `libvulkan-dev` + `vulkan-tools` | ncnn Vulkan 后端开发与 `vulkaninfo` 诊断 |
| C7 | `libboost-dev` / `opencl-headers` | 视 Bergamot / ncnn 实际 CMake 配置而定，**先不装，遇错再补** |
| C8 | `musl-tools` | 静态单文件分发（`01` 未强制要求） |
| C9 | ~~`cargo-deny` / `cargo-about`~~ | ✅ **已装**（0.20.2 / 0.9.3）——`02` §9 的 SPDX 审核工具已就位 |
| C10 | `gradle` | 仅封装 AAR 时需要 |

### 总计

- apt 需装：**26 个包**，已全部核实存在于 noble 源（`apt-cache policy` 逐个确认，**0 个缺失**）
- Rust 侧：工具链 1 套 + 目标平台 3–4 个 + cargo 子命令 5–6 个

---

## 4. 按实施阶段的最小依赖矩阵

> 这是本报告最实用的部分：**不必一次装齐，按 `01` §13 的阶段推进**。

| 阶段 | 工作内容（`01` §13） | 本阶段真正需要什么 | 当前阻断？ |
|---|---|---|---|
| **P0** 架构骨架 | workspace、types、traits、composition root、Mock plugins | **仅 rustup**。零 native 依赖 | ✅ **依赖已就绪，可立即开工** |
| **P1** 字典/规则 | CC-CEDICT/Kaikki 子集、FST/SQLite、规则、语言 ID | `libsqlite3-dev` 或 `rusqlite` bundled；fastText 需 `libclang-dev` | ✅ **FFI 已就绪，无阻断** |
| **P2** 资源管理 | Manifest、校验、回滚、租约、预算 | 无新增（纯 Rust） | ✅ |
| **P3** 首个 NMT | 选定单方向 Marian，Bergamot 打通 | `clang`+`libclang-dev`+`ninja` ✅ 全部就位、Bergamot 源码与子模块 | ✅ **依赖齐全，可开工** |
| **P4** 插件替换 | Bergamot vs ncnn/ORT 对照 | + `protobuf-compiler`、**Python 链（pip/torch/pnnx/onnx）**、可选 `libvulkan-dev` | ⚠️ B1/B2/B4 |
| **P5** Android/跨平台 | ARM64 JNI、物理机基准；桌面构建 | **NDK + cargo-ndk + rustup targets + mingw-w64 + aarch64-linux-gnu**、Android platforms/build-tools | ⚠️ A3 + A4 + B5/B6/B7 |
| **P6** Remote LLM | 网络授权、Provider、密钥注入 | `libssl-dev` ✅（已有）或改用 rustls（零依赖） | ✅ |
| **P7** SDK 发行 | 文档、签名、许可证目录 | + `cargo-deny`/`cargo-about`、`musl-tools`（可选） | ✅ |

**建议执行顺序**：`A1(rustup) → P0 开工 → A2(基础 FFI 库) → P1 → B3 → P3 → P4 前补 Python 链 → P5 前补 Android/交叉编译`。

---

## 5. 安装方案

### 5.1 Rust 工具链 —— ✅ **已执行完成**

**为什么不用 `apt install rustc`**：源里是 **1.75.0**（2023-12），已严重落后，`01` §3 用到的现代 crate 会大量报 "requires rustc 1.8x"。

按你的要求安装到 **`/opt/rust`**（而非默认 `~/.cargo`），实际执行的脚本见工作区 `install_rust_opt.sh`，核心命令：

```bash
export RUSTUP_HOME=/opt/rust/rustup
export CARGO_HOME=/opt/rust/cargo
export PATH="$CARGO_HOME/bin:$PATH"

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain stable

rustup component add rustfmt clippy llvm-tools-preview rust-analyzer
rustup target add aarch64-unknown-linux-gnu x86_64-pc-windows-gnu aarch64-linux-android
```

**结果**：rustc / cargo 1.99.1、rustup 1.29.3，四个目标平台 std 全部就位，`cargo new` + `cargo build` + `cargo run` 实测通过。

**环境注入方式**（已自动完成）：

```bash
# /opt/rust/env.sh —— 由安装脚本生成
export RUSTUP_HOME=/opt/rust/rustup
export CARGO_HOME=/opt/rust/cargo
case ":$PATH:" in
  *":$CARGO_HOME/bin:"*) ;;
  *) export PATH="$CARGO_HOME/bin:$PATH" ;;
esac
```

`~/.bashrc` 与 `~/.profile` 末尾各追加了一个带标记的配置块：

```bash
# >>> Rust toolchain (/opt/rust) — added by DeepSeek Harness 2026-11-05 >>>
if [ -f /opt/rust/env.sh ]; then . /opt/rust/env.sh; fi
# <<< Rust toolchain (/opt/rust) <<<
```

卸载方式：删除 `~/.bashrc`/`~/.profile` 中上述标记块，再 `rm -rf /opt/rust` 即可完全清除（未向 `/usr` 写入任何文件）。

**cargo 子命令安装情况**（全部完成）：

| 工具 | 用途 | 状态 |
|---|---|---|
| `cargo-ndk` | Android 构建入口 | ✅ **v4.1.2** |
| `cargo-nextest` | 并行测试 | ✅ **v0.9.151** |
| `cargo-deny` | 许可证 / 安全审计（`02` §9） | ✅ **v0.20.2** |
| `cargo-about` | 生成依赖许可证清单 | ✅ **v0.9.3** |
| `cargo-xwin` | Windows MSVC 交叉编译 | ✅ **v0.23.1** |

> ⚠️ **`cargo-about` 必须加 `--features cli`**，否则 `cargo install` 会「编译成功但装不上」，只打印一行容易被忽略的警告：
>
> ```
> warning: none of the package's binaries are available for install using the selected features
>   bin "cargo-about" requires the features: `cli`
> ```
> 正确命令：`cargo install cargo-about --locked --features cli`
>
> 另外注意：一条命令装多个包时，cargo 的收尾 Summary 只列**成功**的那些（如 `Successfully installed cargo-deny, cargo-xwin!`），**漏装不会出现在 Summary 里**。批量安装后务必逐个 `--version` 验证。

**验证命令**：

```bash
source /opt/rust/env.sh
rustc --version && cargo --version && rustup target list --installed
for t in cargo-deny cargo-about cargo-xwin cargo-nextest sccache; do "$t" --version; done
cargo new /tmp/hello && cd /tmp/hello && cargo run
```

### 5.2 基础 C/C++ 与 FFI 绑定 —— ✅ **已执行完成**

你实际执行的命令（35 个包，下载 134 MB，解压后 929 MB）：

```bash
sudo apt install -y clang libclang-dev llvm ninja-build ccache protobuf-compiler zlib1g-dev
```

安装结果：

```
clang          18.1.3
clang++        18.1.3
llvm-config    18.1.3
protoc         3.21.12
ninja          1.11.1
ccache         4.9.1
```

**bindgen 依赖检查**：

```
/usr/lib/llvm-18/lib/libclang.so   → ../../x86_64-linux-gnu/libclang-18.so.18   ✅
/usr/lib/llvm-18/include/clang-c/Index.h                                        ✅
```

**端到端验证（真实调用 bindgen，非仅检查文件存在）**：建了一个 `build-dependencies = bindgen 0.71.1` 的工程，对 `wrapper.h`（`#include <math.h>`）生成绑定并编译运行：

```rust
// build.rs 生成，src/main.rs 中 include!
unsafe extern "C" {
    pub fn sqrt(__x: f64) -> f64;
}
```

运行结果 `sqrt(9.0) = 3`，构建 9.7 s。**结论：FFI 绑定链路完全打通。**

**clang 作链接器的验证**：在工程内 `.cargo/config.toml` 设置 `linker = "clang"`，`cargo build` + 运行成功。说明 5.6 里那段配置的 linker 部分是可行的（只差 `mold`）。

**ccache 验证**：同一源文件编译两次，命中 1 次 / 未命中 1 次，工作正常。默认缓存目录 `~/.cache/ccache`。

> **给 Bergamot / ncnn 用 ccache**：CMake 构建时设置
> ```bash
> export CMAKE_C_COMPILER_LAUNCHER=ccache
> export CMAKE_CXX_COMPILER_LAUNCHER=ccache
> ```
> 之后 CMake 会通过 ccache 包装 gcc/clang。注意这与 Rust 侧的 `sccache` 是**两套独立机制**，互不干扰。

### 5.3 Android NDK（**推荐手动下载，绕过 sdkmanager**）

已确认 `sdkmanager --list` 在本机无法拉取远端清单（见 2.5），但 NDK 压缩包可直连下载。已核实 Google 仓库中可用版本：

```
android-ndk-r27b / r27c / r27d / r28 / r28b / r28c / r29 / r30 (linux)
最新 r30 约 704 MB
```

推荐 **r27d**（与多数 Rust/cargo-ndk 生态兼容性最稳，`01` §10 未指定版本，可先取一个 LTS 系列）：

```bash
# 1) 下载（直连可用，0.13s 响应）
cd ~/Android
curl -L -O https://dl.google.com/android/repository/android-ndk-r27d-linux.zip

# 2) 解压到 SDK 的 ndk 目录（cargo-ndk 会自动识别 ~/Android/Sdk/ndk/*）
mkdir -p ~/Android/Sdk/ndk
unzip -q android-ndk-r27d-linux.zip -d ~/Android/Sdk/ndk/
mv ~/Android/Sdk/ndk/android-ndk-r27d ~/Android/Sdk/ndk/27.3.13750724   # 改名为数字版本目录，便于工具识别

# 3) 环境变量（写进 ~/.bashrc）
export ANDROID_NDK_HOME="$HOME/Android/Sdk/ndk/27.3.13750724"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export JAVA_HOME="/usr/lib/jvm/java-21-openjdk-amd64"

# 4) 验证
"$ANDROID_NDK_HOME/ndk-build" --version
```

> 若坚持用 sdkmanager，需先解决代理问题（Java 不读 `http_proxy`）；可试 `sdkmanager --install "ndk;27.3.13750724"`，失败则回退到上面的手动方案。

**另需补 `platforms` 与 `build-tools`**（除非只做 `.so`、不做 AAR）：

```bash
sdkmanager --install "platforms;android-34" "build-tools;34.0.0"
# 若 sdkmanager 仍不可用，可从 https://dl.google.com/android/repository/ 手动取对应 zip
```

### 5.4 交叉编译工具链（需 sudo）

```bash
# Linux arm64 目标
sudo apt install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu

# Windows 目标（MinGW ABI）
sudo apt install -y mingw-w64 gcc-mingw-w64-x86-64

# 在 x86 上跑 arm64 冒烟测试（强烈建议）
sudo apt install -y qemu-user-static
```

`~/.cargo/config.toml` 链接器配置：

```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"

[target.x86_64-pc-windows-gnu]
linker = "x86_64-w64-mingw32-gcc"
ar = "x86_64-w64-mingw32-ar"

[target.aarch64-linux-android]
# 通常由 cargo-ndk 自动设置，无需手写

[build]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]   # 装 lld 后启用
```

> **注意**：`01` §10 表格写的是 Windows "MSVC/MinGW ABI 要固定"。MinGW 可在 Linux 交叉编译；**MSVC ABI 需 Windows 或 `cargo-xwin`**（`cargo install cargo-xwin`，依赖 xwin 下载 Windows SDK）。若产品要面向 Windows 主流用户，建议 **MSVC + GitHub Actions Windows runner**，MinGW 仅作本地验证。

### 5.5 Python 模型转换链（需 sudo 装 pip，其余 pip 内装）

对应 `02` §5–§6 的量化与转换验证：

```bash
# 1) 装 pip（需 sudo）
sudo apt install -y python3-pip

# 2) 建独立虚拟环境（避免污染系统 Python）
python3 -m venv ~/.venvs/nmt-convert
source ~/.venvs/nmt-convert/bin/activate

# 3) 配镜像（可选，官方源可用但镜像更稳）
pip config set global.index-url https://pypi.tuna.tsinghua.edu.cn/simple

# 4) 核心依赖
pip install --upgrade pip
pip install torch --index-url https://download.pytorch.org/whl/cpu   # CPU 版足够（GPU 是 930MX，无意义）
pip install onnx onnxruntime transformers sentencepiece sacremoses
pip install pnnx          # ncnn 官方模型转换工具，02 §6 路径必需
```

**验证**：

```bash
python -c "import torch, onnx, onnxruntime, transformers, sentencepiece; print(torch.__version__)"
pnnx --help | head -3
```

> 体积提示：torch CPU 版约 200 MB+，整套约 1.5–2 GB。磁盘 613 G 无压力。
> **GPU 说明**：930MX 显存约 2 GB，跑不动任何有用模型，装 CUDA 版 torch 纯属浪费，CPU 版即可。

### 5.6 编译加速（部分已完成）

**sccache —— ✅ 已安装并验证可用**

```bash
sudo apt install -y sccache          # 已执行，得到 v0.7.7
echo 'export RUSTC_WRAPPER=sccache' >> /opt/rust/env.sh   # 已执行
```

实测结果（带 `serde` 依赖的工程，`cargo clean` 后重建）：

| 指标 | 冷缓存 | 命中缓存 |
|---|---|---|
| 构建耗时 | 4.64 s | **2.26 s** |
| Cache hits | 0 | **8 / 8 可缓存单元** |
| Cache write errors | 0 | 0 |

缓存目录 `~/.cache/sccache`，上限 10 GiB，当前已用 129 MB。

> **版本说明**：apt 提供的是 **0.7.7**（官方最新已到 0.10.x）。跨了两年版本，但**与 rustc 1.99.1 实测完全兼容**，无警告、无失败。若将来升级 rustc 后出现缓存异常，可执行 `sccache --stop-server && sccache --start-server` 或直接 `cargo install sccache --locked` 装新版。

> **临时关闭**：调试编译缓存相关问题时 `unset RUSTC_WRAPPER` 即可，无需改 `env.sh`。
>
> **注意**：`RUSTC_WRAPPER` 是**全局生效**的，未来所有项目（含 `cargo install`）都会走 sccache。这是预期行为，但若发现某个构建行为异常，先怀疑这里。

**profile 配置 —— ✅ 已启用（2026-11-05）**

已写入 **`/opt/rust/cargo/config.toml`**（注意路径：因为 `CARGO_HOME=/opt/rust/cargo`，全局配置在这里，不是 `~/.cargo/config.toml`）：

```toml
[profile.dev]
debug = 1

[profile.dev.package."*"]
opt-level = 2

[net]
retry = 3
```

**实测确认生效**（`cargo build -v` 观察 rustc 参数）：

| 编译对象 | 观察到的参数 |
|---|---|
| 依赖 crate（如 `pollster`） | `-C debuginfo=1` + **`-C opt-level=2`** ✅ |
| 自身 crate | `-C debuginfo=1`（无 `opt-level=2`，保持快速编译）✅ |

`translator-cli` 的 debug 产物 6.1 MB（若 `debug=2` 会显著更大）。

**mold —— ⬜ 未安装**（可选，唯一还没装的可选项）

```bash
sudo apt install -y mold
```

装完后**才可**把下面两行加入 `config.toml`：

```toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"                              # ← 已验证可用（见 5.2）
rustflags = ["-C", "link-arg=-fuse-ld=mold"]  # ← 需 mold，尚未安装
```

> ⚠️ **`linker = "clang"` 已验证可行**，但 `-fuse-ld=mold` 依赖尚未安装的 `mold`。**现在把这两行加进去会导致所有构建报错**。当前配置刻意不含它们。

**其他可选**

```bash
sudo apt install -y libvulkan-dev vulkan-tools    # ncnn Vulkan 后端开发与诊断
```

### 5.7 工程规范（无需 sudo）

```bash
git config --global user.name  "你的名字"
git config --global user.email "you@example.com"
git config --global init.defaultBranch main
```

---

## 6. 平台级硬限制（装包解决不了）

### 6.1 macOS 目标无法在本机构建 ❌

`01` §10 要求 `macOS arm64/x64` 产物、§12.2 要求 macOS CI 必须通过。但：

- 交叉编译到 `aarch64-apple-darwin` 需要 **Apple SDK 与 Xcode 工具链**，法律与技术上均要求 macOS 主机
- 本项目还要链接 **Bergamot / ncnn 的 C++ 原生库**，交叉编译难度进一步放大，实际不可行

**对策**：macOS 产物走 **GitHub Actions `macos-14` runner**（github.com 已验证可达）。本机只负责 Linux x64 + Android + Windows(MinGW) 的本地验证。

### 6.2 Android arm64 必须物理机 ❌

`01` §12.2 明确"Android arm64 … **必须使用物理机**"。当前：

- 本机系统镜像**只有 x86_64**（android-31/34），跑不了 arm64-v8a
- x86 主机上跑 arm64 模拟器无 KVM 加速，速度不可用
- `/dev/kvm` 存在，x86_64 镜像可以正常加速——但**那验证的是 x86_64 ABI，不是 arm64**

**对策**：需要一台 ARM64 Android 真机（USB 调试，adb 已就绪）。在拿到真机之前，`01` §9.3 的 **Android 200–250 MB 峰值内存预算无法验证**，P5 阶段无法验收。

### 6.3 `sudo` 被沙箱阻断 ⚠️

```
sudo: The "no new privileges" flag is set
/proc/self/status → NoNewPrivs: 1
```

| 操作 | AI 能否执行 |
|---|---|
| rustup / cargo install / 写代码 / 编译 / 跑测试 | ✅ 能 |
| **拉取新 crate 依赖（写 `CARGO_HOME/registry`）** | ❌ **不能**，见下方说明 |
| `sudo apt install`、`sdkmanager --install`（写 SDK 目录） | ❌ 不能 |
| 下载 NDK zip 到 `~/Android`（SDK 目录在 `~` 下，**不在工作区内**） | ⚠️ 受 workspace-write 限制，需授权 |

> **⚠️ 一个容易被忽略但很实际的约束**：因为 `CARGO_HOME=/opt/rust/cargo` 在工作区之外，AI 在沙箱内执行 `cargo build` 时，**只要工程需要下载任何新的 crate，就会失败**：
>
> ```
> error: failed to open `/opt/rust/cargo/registry/cache/.../cexpr-0.6.0.crate`
> Caused by: Permission denied (os error 13)
> ```
>
> 已下载过的依赖不受影响，纯本地增量编译正常。**涉及新增依赖的构建需要提权执行，或由你在自己终端跑一次 `cargo fetch` / `cargo build`**。
>
> 每次提权都需要你点确认，所以更省事的流程是：**AI 改完代码 → 你在终端跑一次 `cargo build` → AI 继续在已拉取的依赖基础上做增量开发**。

**对策**：第 5 章的 apt 与 NDK 步骤由你在自己终端执行；Rust 侧全部可由 AI 代劳。

### 6.4 `sdkmanager` 远端清单故障 ⚠️

```
Warning: Failed to download any source lists!
Warning: IO exception while downloading manifest
```

已试过 `JAVA_OPTS` 注入代理、`-D` 参数（工具不接受），均失败。且 `dl.google.com` 直连是通的（200/0.13s），说明不是网络不通，而是 sdkmanager 自身的 manifest 拉取逻辑有问题。

**对策**：直接用 `curl` 下载官方 zip（5.3 已给出可用的确切 URL 与版本列表）。

---

## 7. 环境视角下的项目风险

| # | 风险 | 依据 | 建议 |
|---|---|---|---|
| 1 | **P5 阶段可能长期卡在"无 arm64 真机"** | `01` §12.2 | 尽早确认真机来源，否则 `01` §9.3 内存预算永远处于"待实测" |
| 2 | **macOS 从未在本地构建，可能藏编译错误** | `01` §10 | 从 P0 起就把 GitHub Actions 的 macOS job 建起来，别等到 P7 |
| 3 | **Bergamot 的 C++ 子模块构建未在本机验证过** | `01` §8.1 要求 Bergamot 基线 | P3 第一件事就是在本机打通 Bergamot 编译，失败则触发 `02` §7 的替换矩阵 |
| 4 | **ncnn 的 seq2seq 适配风险由 Python 链能否跑通前置决定** | `01` §8.3、`02` §6 | pip/torch/pnnx 必须在 P4 前就绪，否则连"验证可行性"都做不到 |
| 5 | ~~**许可证审计缺少工具支撑**~~ | `02` §9 `provenance.json` | ✅ **已解决**：`cargo-deny` + `cargo-about` 已装，下一步是把 SPDX 清单纳入 CI |
| 6 | **4 核 CPU 导致 Bergamot/ncnn 全量构建时间长** | 硬件 | `ninja` + `ccache` + `-j8`，并把 C++ 依赖做成可缓存的预编译产物 |
| 7 | **`nvidia-smi` 报 NVML 错误** | 2.2 | 与本项目弱相关（GPU 无价值），但需确认是否影响其他用途 |
| 8 | **Rust 默认 `debug=2` 会产生数 GB 中间产物** | 磁盘虽足 | `profile.dev.debug = 1`，详见 5.6 |

---

## 8. 验收清单

### 环境盘点（已完成）
- [x] 系统 / 内核 / 架构 / 虚拟化
- [x] CPU 指令集（AVX2+FMA 确认）
- [x] 内存 / 磁盘 / inode / ulimit / cgroup
- [x] GPU / CUDA / Vulkan ICD
- [x] C/C++ 编译链实测（gcc 编译运行通过）
- [x] Rust 工具链全盘扫描（审计时为**全空**，现已安装，见第 9 章）
- [x] Android SDK/NDK/platforms/build-tools 逐项核查
- [x] 交叉编译工具链逐个 `command -v`
- [x] Python 与 pip / 各科学计算包导入测试
- [x] 26 个 apt 候选包存在性核实（0 缺失）
- [x] 项目相关站点可达性（HF / Mozilla registry / Bergamot / dl.google.com / pypi）
- [x] sdkmanager 故障复现与绕过方案验证（NDK zip 可直下）
- [x] 沙箱权限边界确认

### 待你决策
- [x] ~~是否现在装 Rust（A1）？~~ —— **已装，见第 9 章**
- [ ] NDK 用哪个版本？（建议 r27d；`01` 未指定）
- [ ] Windows 目标走 **MinGW**（可本地交叉编译）还是 **MSVC**（需 Windows/CI）？
- [ ] 是否接受"macOS 只能靠 GitHub Actions"这一约束？
- [ ] arm64 Android 真机什么时候能到位？（决定 P5 排期）
- [ ] `02` §6 的 ncnn 路线是否现在就投入？（若要，Python 链需提前到 P3 前）

### 待执行
- [x] 5.1 rustup + 组件 + 目标平台（**已于 2026-11-05 完成，见第 9 章**）
- [x] 5.1 cargo 工具：`cargo-ndk` `cargo-nextest` `cargo-deny` `cargo-about` `cargo-xwin`（**全部完成**）
- [x] 5.2 基础 FFI 库：clang / libclang-dev / llvm / ninja / ccache / protoc / zlib1g-dev（**已完成，bindgen 端到端验证通过**）
- [x] 5.6 sccache 编译缓存（**已装并实测命中**）
- [x] 5.6 profile 编译配置 `debug=1` + `opt-level=2`（**已写入 `/opt/rust/cargo/config.toml` 并验证生效**）
- [ ] 5.3 NDK 手动下载 + 环境变量（你）—— **下一优先级**
- [ ] 5.4 交叉编译工具链 `mingw-w64` + `gcc-aarch64-linux-gnu`（你，sudo）
- [ ] 5.5 Python 模型转换链（你，sudo + pip）
- [ ] 5.6 `mold` 与 `cross`（可选）
- [ ] 5.7 git 身份（AI 可代劳）

---

## 9. 安装执行记录（2026-11-05）

### 9.1 已完成的操作

| # | 操作 | 目标位置 | 结果 |
|---|---|---|---|
| 1 | 创建 `CARGO_HOME` / `RUSTUP_HOME` 目录 | `/opt/rust/cargo`、`/opt/rust/rustup` | ✅ |
| 2 | 下载并执行 rustup-init（`--profile minimal --no-modify-path`） | — | ✅ stable 1.99.1 |
| 3 | 添加组件 `rustfmt` `clippy` `rust-analyzer` `llvm-tools` | — | ✅ 全部成功 |
| 4 | 添加目标 `aarch64-unknown-linux-gnu` `x86_64-pc-windows-gnu` `aarch64-linux-android` | — | ✅ 3 个 rust-std |
| 5 | 生成 `/opt/rust/env.sh` | `/opt/rust/env.sh` | ✅ |
| 6 | 向 `~/.bashrc` 与 `~/.profile` 追加带标记的 source 块 | 用户 home | ✅ 已生效（交互式与登录式 shell 均验证） |
| 7 | 冒烟测试 `cargo new` + `build` + `run` | `/tmp`、工作区 | ✅ 输出 `Hello, world!` |
| 8 | `cargo install cargo-ndk` / `cargo-nextest` | `/opt/rust/cargo/bin` | ✅ cargo-ndk 4.1.2、nextest 0.9.151 |
| 9 | `cargo install cargo-deny cargo-about cargo-xwin --locked`（用户执行） | `/opt/rust/cargo/bin` | ⚠️ cargo-deny 0.20.2 ✅、cargo-xwin 0.23.1 ✅、**cargo-about 静默失败** |
| 10 | `cargo install cargo-about --locked --features cli`（补装） | `/opt/rust/cargo/bin` | ✅ 0.9.3 |
| 11 | `sudo apt install sccache` + 追加 `RUSTC_WRAPPER` | `/usr/bin`、`/opt/rust/env.sh` | ✅ 0.7.7，实测缓存命中正常 |
| 12 | `sudo apt install clang libclang-dev llvm ninja-build ccache protobuf-compiler zlib1g-dev`（用户执行） | 系统 `/usr` | ✅ 35 个包，134 MB 下载 / 929 MB 磁盘 |
| 13 | bindgen 端到端验证（`bindgen 0.71.1` + `math.h`） | 工作区临时工程 | ✅ 生成绑定并运行 `sqrt(9.0)=3` |
| 14 | clang 作 Rust 链接器验证 | 工作区临时工程 | ✅ 构建运行通过 |
| 15 | ccache 命中验证 | 工作区临时缓存目录 | ✅ 1 hit / 1 miss |

**仅写入的位置**：`/opt/rust/**`、`~/.bashrc`、`~/.profile`。**未向 `/usr`、`/usr/local`、系统包管理器写入任何内容**，卸载即删除这三处。

### 9.2 验证证据

```
$ source /opt/rust/env.sh
$ rustc --version
rustc 1.99.1 (58cf8f9c3 2026-10-13)
$ cargo --version
cargo 1.99.1 (5f94df478 2026-08-27)
$ rustup --version
rustup 1.29.3 (edbc831ce 2026-11-02)
$ rustup target list --installed
aarch64-linux-android
aarch64-unknown-linux-gnu
x86_64-pc-windows-gnu
x86_64-unknown-linux-gnu
```

登录式与非交互式 shell 均已确认 `cargo` 在 PATH 中：

```
$ bash -lc 'command -v cargo'
/opt/rust/cargo/bin/cargo
$ bash -ic 'command -v rustc'
/opt/rust/cargo/bin/rustc
```

配套工具版本：

```
clippy          0.1.99
rustfmt         1.10.0-stable
rust-analyzer   1.99.1
cargo-ndk       4.1.2
cargo-nextest   0.9.151
cargo-deny      0.20.2
cargo-about     0.9.3
cargo-xwin      0.23.1
sccache         0.7.7
```

功能验证：在临时工程上依次执行 `cargo clippy`（通过）、`cargo fmt --check`（通过）、`cargo nextest run`（正常识别零测试），全部可用。总占用 2.0 G（sccache 缓存另计 129 MB）。

### 9.3 交叉编译实测（重要结论）

用同一份 `fn main(){ println!("{}", env::consts::ARCH); }` 对三个目标尝试 `cargo build --target`，结果：

| 目标 | 编译 std | 链接 | 结论 |
|---|---|---|---|
| `aarch64-unknown-linux-gnu` | ✅ | ❌ `linking with cc failed` | 缺 `aarch64-linux-gnu-gcc`，**符合预期** |
| `x86_64-pc-windows-gnu` | ✅ | ❌ `linker x86_64-w64-mingw32-gcc not found` | 缺 `mingw-w64`，**符合预期** |
| `aarch64-linux-android` | ✅ | ❌ `linking with cc failed` | 缺 NDK，**符合预期** |

> **这条实测很有价值**：它证明 **Rust 侧的交叉编译能力已完全就绪**（代码能编译到目标架构），剩余失败**纯粹是缺少 C 链接器**。也就是说，5.3（NDK）与 5.4（交叉链接器）装完，这三个目标立刻就能产出二进制，不需要再动 Rust 配置。

### 9.4 仍欠缺的 Rust 侧工具

Rust 工具链本体已完备，只剩两个**可选**项：

```bash
# 可选：更快的链接器（4 核机器收益明显；装完才可启用 5.6 的 config.toml）
sudo apt install -y mold

# 可选：Docker 化的跨平台构建（Linux/arm64 交叉时比手动配工具链省事）
cargo install cross --locked
```

> **注意**：5.6 里那段 `linker = "clang"` + `-fuse-ld=mold` 的配置依赖 `clang`（属 5.2）与 `mold`，**两者都还没装**，现在写进配置会让所有构建失败。

### 9.5 踩坑记录（本次实际遇到）

| # | 现象 | 原因 | 处理 |
|---|---|---|---|
| 1 | `cargo install cargo-about` 编译成功却装不上，Summary 只显示 `Successfully installed cargo-deny, cargo-xwin!` | cargo-about 的 bin 需要 `cli` feature | 加 `--features cli` 重装。**批量安装后必须逐个 `--version` 验证**，Summary 只列成功的包 |
| 2 | sccache 报 `Cache write errors: 8` / `Cache errors: 8`，缓存目录不存在 | **假象**：当时在沙箱内执行，`~/.cache` 在工作区外被拒写 | 提权后重测：写错误 0，重建命中 8/8，耗时 4.64s → 2.26s。sccache 本身没问题 |
| 3 | `ldconfig` 里能看到 `libclang-18.so.18`，容易误以为 bindgen 可用 | 只有运行时库，没有 `libclang.so` 符号链接与头文件 | `libclang-dev` 仍然必须装（见 3.A2）。装完确认 `/usr/lib/llvm-18/lib/libclang.so` 存在 |
| 4 | 沙箱内 `cargo build` 报 `failed to open .../registry/cache/cexpr-0.6.0.crate: Permission denied` | `CARGO_HOME=/opt/rust/cargo` 在工作区外，**新依赖无法下载** | 提权执行，或你在终端先跑一次 `cargo build`。已下载的依赖不受影响 |
| 5 | `ccache` 直接跑报 `Permission denied` | 同上，默认缓存目录 `~/.cache/ccache` 在工作区外 | 指定 `CCACHE_DIR` 到工作区内可验证；真实使用（你终端）不受影响 |

> 第 2、4、5 条有共同的根因，值得单独记住：**本机沙箱会拒绝一切工作区外的写入**。凡涉及 `~/.cache`、`~/.config`、`~/Android`、`/opt/rust/cargo/registry` 的报错，先排除沙箱假象，再判断工具本身是否故障。

---

## 附录 A：本次探查执行的关键命令

```bash
# 系统与硬件
cat /etc/os-release; uname -a; lscpu; free -h; df -hT /; nproc
grep -o -E 'avx2|avx512f|fma' /proc/cpuinfo | sort -u
lspci | grep -Ei 'vga|3d|display'; ls /usr/share/vulkan/icd.d/

# Rust
for c in rustc cargo rustup rustfmt cargo-clippy cargo-ndk cross sccache mold; do command -v $c; done
find / -maxdepth 4 -name rustc -type f -not -path '*/proc/*'

# C/C++ 实测
printf '#include <stdio.h>\nint main(void){puts("ok");}\n' > /tmp/t.c && gcc /tmp/t.c -o /tmp/t && /tmp/t
clang --version; cmake --version; ninja --version; protoc --version

# Android
echo $ANDROID_HOME; ls ~/Android/Sdk; ls ~/Android/Sdk/ndk
~/Android/Sdk/cmdline-tools/latest/bin/sdkmanager --list_installed
~/Android/Sdk/cmdline-tools/latest/bin/sdkmanager --list          # 复现清单拉取失败
curl -s https://dl.google.com/android/repository/repository2-3.xml | grep -o 'android-ndk-r[0-9]*[a-z]*-linux\.zip'

# 交叉编译
for c in x86_64-w64-mingw32-gcc aarch64-linux-gnu-gcc zig cross cargo-ndk; do command -v $c; done

# Python
python3 -V; python3 -m pip --version; python3 -c "import torch, onnx"

# 网络
curl -sI -o /dev/null -w '%{http_code}\n' https://dl.google.com/android/repository/repository2-3.xml
env | grep -i proxy

# 权限
sudo -n true; grep NoNewPrivs /proc/self/status

# apt 候选与安装影响
apt-cache policy <pkg>; apt-get -s install -y <pkg group>
```

## 附录 B：本轮环境结论速查表

| 维度 | 状态 | 一句话 |
|---|---|---|
| 硬件 | ✅ | 编译偏慢但推理够用，AVX2+FMA 齐 |
| 系统 | ✅ | Ubuntu 24.04 x64，正对目标平台 |
| C/C++ 构建 | ✅ | gcc 13.3 + cmake 3.28 可用 |
| FFI 绑定 | ✅ | **clang/LLVM 18.1.3 已装，bindgen 实测生成绑定并运行成功** |
| Rust | ✅ | **1.99.1 已装于 `/opt/rust`**，四目标 std 就位，冒烟测试通过 |
| Rust 扩展 | ✅ | ndk / nextest / deny / about / xwin 全部装好，sccache 实测命中 |
| Android | ❌ | NDK/platforms/build-tools 全缺，SDK 只有 x86_64 镜像 |
| 交叉编译 | ❌ | 四条目标链一条都没有 |
| Python 转换链 | ❌ | 连 pip 都没有 |
| 网络 | ✅ | 所有项目相关站点可达 |
| 权限 | ⚠️ | sudo 被沙箱阻断，apt 须你执行 |
| macOS | ❌ | 本机无解，必须 CI |
| arm64 真机 | ❌ | 本机无解，必须物理设备 |

---

**判据回顾**（`01` 文末"交付判据"）：即使移除 Bergamot、ncnn、Mozilla 模型与全部第三方词库，`translator-api` 仍应能以 Mock/空资源模式编译运行。

**这一点对环境的启示**：P0 的编译可行性**完全不依赖**第 5 章的任何一个 apt 包。所以**装完 rustup 就能立刻验证架构骨架**，不要在环境准备上等太久。
