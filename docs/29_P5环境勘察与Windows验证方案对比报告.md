# P5 环境勘察与 Windows 验证方案对比报告

- 日期：2026-11-06
- 阶段：P5 前置（环境勘察，无代码变更）
- 关联：`docs/01` §13（P5 = Android/跨平台：ARM64 JNI、物理机基准、桌面构建、四平台端到端）、
  `docs/21`（平台支持是证据表）、`docs/27` §7（P5 承接清单）
- 一句话结论：**Android 环境还在且能跑（x86_64，34 秒内开机完成），但 P5 要的是 ARM64——现有镜像帮不上；
  Windows 侧建议"本机 KVM 虚拟机做迭代 + 实体机（或 CI）出证据"，不要指望单一方案覆盖 P5 的全部验收。**

---

## 1. 本机硬件与系统现状（实测）

### 1.1 机器本体

| 项 | 实测值 | 命令 |
|---|---|---|
| 机型 | ASUSTeK **X542URV**（笔记本，裸机非虚拟机） | `cat /sys/class/dmi/id/product_name` |
| 系统 | Ubuntu **24.04.5 LTS**，kernel **7.0.0-42-generic**，GNOME/Wayland | `cat /etc/os-release`、`uname -r`、`$XDG_SESSION_TYPE` |
| CPU | **Intel i5-8250U**，4 核 8 线程，1.6–3.4 GHz，15 W 级 | `lscpu` |
| 虚拟化 | **VT-x 已开**，`kvm_intel` 已加载，`/dev/kvm` 存在且 `nan` 在 `kvm` 组 | `lscpu`、`lsmod`、`id` |
| 内存 | **31 GiB**（可用 26 GiB），swap **59.6 GiB**（`/dev/sdc2`） | `free -h`、`swapon --show` |
| 根分区 | `/dev/sdc3`，824 G，已用 186 G，**可用 597 G** | `df -h /` |
| GPU | Intel UHD 620 + NVIDIA **930MX**（Optimus，无独显直出） | `lspci \| grep -Ei "vga\|3d"` |
| 网络 | `enp2s0` = **192.168.1.10/24**，外网 200 OK（0.44 s 到 static.rust-lang.org） | `ip -br addr`、`curl -w` |
| 桌面 | `DISPLAY=:1`，Wayland（QEMU 可用 gtk/sdl/vnc） | `echo $DISPLAY`、`qemu -display help` |

### 1.2 磁盘（决定虚拟机放哪）

| 设备 | 容量 | 类型 | 说明 |
|---|---|---|---|
| `sdc` | 931.5 G | **SSD**（Great Wall GW1000） | 根分区所在；实测写 **449 MB/s**、读 **419 MB/s**（`dd` 1 GiB） |
| `sdb` | 232.9 G | **SSD**（Samsung 750 EVO） | 可放第二个镜像/快照 |
| `sda` | 465.8 G | **HDD**（Toshiba MQ01ABF050，5400 rpm） | 不适合放 Windows 系统盘 |

### 1.3 虚拟化工具链现状

| 组件 | 状态 | 影响 |
|---|---|---|
| `qemu-system-x86_64` **8.2.2** | 已装，`-accel help` 有 **kvm + tcg** | 可直接开虚拟机，不必装 VirtualBox |
| `qemu-img` / `qemu-io` | 已装（`/usr/bin`） | 建盘/快照/转换 |
| **OVMF**（含 `secboot`） | 已装（`2024.02`） | UEFI 启动、Windows 11 的 SecureBoot 条件之一 |
| virtio 设备 | `virtio-blk/net/9p/serial/rng/balloon` 齐全 | 磁盘/网络/共享目录都有加速路径 |
| Samba **4.19** | 已装（`smbd`/`nmbd`） | 可与 Windows 客户机共享构建产物 |
| **libvirt / virt-manager** | **未装** | 只能用 `qemu-system-x86_64` 命令行（够用，见 §3.1） |
| **swtpm** | **未装**（`apt-cache policy` 有候选 0.7.3） | 装 Windows 11 才需要（TPM 2.0） |
| **virtiofsd / spice 客户端** | 未装 | 共享目录可退回 9p 或 SMB；显示可退回 gtk/vnc |
| IOMMU | 有（`/sys/kernel/iommu_groups` 20 个组） | 理论上可直通；但这是笔记本 Optimus，**不建议**做 GPU 直通 |

### 1.4 性能基线（给"虚拟机够不够用"一个参照）

| 基准 | 结果 |
|---|---|
| Python 单线程 `sum(i*i for i in range(10**7))` | **0.94 s** |
| `openssl speed -evp aes-128-gcm`（单核） | **≈ 5.0 GB/s**（16 KiB 块） |
| 根分区顺序读写 | 449 / 419 MB/s |

参照意义：这是一台 2017 年的 15 W 四核低压本。**Windows 客户机功能上完全跑得动，但编译/链接会明显慢**
（Rust + MSVC 链接是重 IO/CPU 任务，4 vCPU 下大约是本机 Linux 原生的一半到三分之二）。

### 1.5 已装开发工具

`rustc 1.99.1` / `cargo 1.99.1`（`/opt/rust/env.sh`）、Python 3.12、Node（DSH 运行在 npx 缓存里）。

---

## 2. Android 环境：还在，能跑，但不是 P5 要的那个架构

### 2.1 找到了什么

```
~/Android/Sdk                      8.3 GB
├── cmdline-tools/latest
├── emulator           37.2.12
├── platform-tools     37.0.1
├── licenses
└── system-images
    ├── android-31/google_apis/x86_64            （3.1 GB 级）
    └── android-34/google_apis_playstore/x86_64  （4.3 GB 级）

~/.android/avd/gms2.avd            3.1 GB（含用户数据）
~/Android/vm/                      空目录（大概是当初想放虚拟机镜像的位置）
```

AVD `gms2` 的关键配置：`abi.type=x86_64`、`hw.cpu.ncore=4`、`hw.ramSize=2G`、
`disk.dataPartition.size=10G`、`hw.gpu.enabled=no`、`image.sysdir.1=system-images/android-34/google_apis_playstore/x86_64/`。

### 2.2 实测：它现在还能启动

```
$ ~/Android/Sdk/emulator/emulator -avd gms2 -no-window -no-audio -no-snapshot \
      -no-boot-anim -gpu swiftshader_indirect
$ adb devices
emulator-5554   device                     # 约 20 s 内出现
$ adb shell getprop sys.boot_completed
1                                          # 首次轮询（启动后 ~35 s）即为 1
$ adb shell getprop ro.build.version.release   -> 14
$ adb shell getprop ro.product.cpu.abi         -> x86_64
$ adb shell "cat /proc/cpuinfo | grep -c processor" -> 8
$ adb shell "cat /proc/meminfo | head -1"      -> MemTotal: 2534120 kB
$ adb emu kill                                 -> OK，无残留 qemu/emulator 进程
```

结论：**KVM 加速有效，Android 14 + Play Store 镜像可用，2.4 GiB RAM / 8 逻辑 CPU**。
它适合做"Android 端能把 SDK 跑起来"的**预演**，但要注意它是 x86_64。

### 2.3 P5 的缺口（这条比"有没有模拟器"重要）

`docs/01` §13 对 P5 的要求是 **ARM64 JNI + 物理机基准 + 四平台端到端**。对照现状：

| P5 需要 | 现状 | 缺口 |
|---|---|---|
| **ARM64** JNI 产物 | 只有 **x86_64** 系统镜像 | 需 `system-images;android-34;google_apis;arm64-v8a`（x86 主机上是**全指令翻译，极慢**），或一台**真机** |
| NDK 编译 JNI | **未装 NDK** | 需 `sdkmanager "ndk;<ver>"` 或独立 NDK |
| 物理机基准 | 无 Android 真机 | 只有真机能给"物理机基准"证据 |
| 桌面四平台构建 | 只有 Linux 有证据（见 §4.1） | 需要 Windows（macOS 暂不在范围内） |

**建议**：Android 侧先做两件不依赖真机的事——① 装 NDK，用 **x86_64 AVD** 打通 JNI 调用链（验证接口与打包，不代表性能）；
② 同时准备一台 **ARM64 真机**（任意中端机，adb 可连即可）出"物理机基准"。**不要**用 arm64 模拟器冒充物理机基准。

---

## 3. Windows：三条路，各自能回答不同的问题

### 3.1 方案 A — 本机 KVM 虚拟 Windows（推荐做迭代/异常路径）

**可行性（已逐项验证）**：KVM 可用（`qemu … -accel kvm -smp 8` 正常接受），q35 机型在（`pc-q35-*`），
OVMF（含 secboot）在，virtio 设备齐，Samba 在。**Windows 10 现在就能装；Windows 11 只差 `swtpm`**（`apt install swtpm swtpm-tools` 即可）。

建议配置（给 4C/8T、31 GiB、SATA SSD 定的一套）：

| 项 | 建议值 | 理由 |
|---|---|---|
| 机型/固件 | `q35` + OVMF（Win11 用 `OVMF_CODE_4M.secboot.fd`） | q35 更接近现代 PC；UEFI 是 Win11 硬要求 |
| vCPU | **4**（最多 6） | 宿主还要跑 DSH/编译；8 全给客户机会把双方都拖慢 |
| 内存 | **8 GiB**（Win11 最低 4 GiB） | 宿主可用 26 GiB，留足余量；swap 59 GiB 兜底 |
| 磁盘 | **60 GB qcow2** 放 `/dev/sdc`（根 SSD） | 稀疏分配，实测约 15–20 GB 起；快照也在这 |
| 磁盘控制器 | `virtio-blk`（需装 virtio-win 驱动）或 `sata`（免驱动） | 先可用优先：首装用 SATA，装完驱动再换 virtio |
| 网络 | `virtio-net` + user-mode NAT（`-netdev user,hostfwd=tcp::3389-:3389`） | 免桥接、免 root 网络配置；宿主可 RDP 进去 |
| 显示 | `gtk`（本地看）或 `-vnc :1`（远程看） | 构建/测试其实可以全程 headless + SSH |
| 共享目录 | SMB（宿主 `smbd`）或 `python3 -m http.server` | virtiofsd 未装；9p 在 Windows 上体验差 |
| 快照 | `qemu-img snapshot -c <name>`（关机态） | **这是方案 A 最大的价值**：装坏了 5 秒回滚 |
| 许可 | Win10/11 评估版即可（180 天，可重置） | 只做构建/测试，不进生产 |

**成本**：ISO 5–6 GB 下载 + 安装 30–60 min + 装 Rust/MSVC 工具链 30 min；磁盘 60 GB。

**能回答**：`cargo build/test` 在 Windows 上过不过、路径/编码/CRLF/文件锁问题、安装与卸载、
崩溃后残留、**快照回滚下的可重复性**、CLI 端到端。
**不能回答**：真实驱动/杀软/组策略干扰、真实硬件性能、"物理机基准"。

### 3.2 方案 B — 你给一台 Windows 实体机的 SSH（推荐做验收证据）

**需要的准备（缺一项就跑不起来）**：

1. **Windows 版本**：10 22H2 / 11 23H2+ / Server 2019+（告诉我哪一个，我按它写验证矩阵）；
2. **OpenSSH Server** 已启用且开机自启（`Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0`；
   `Set-Service sshd -StartupType Automatic`）；
3. **可达性**：如果它也在 `192.168.1.0/24`，直接给内网 IP 即可（我这台是 192.168.1.10）；
   否则需要端口映射或跳板；
4. **账号**：能 `sudo`/管理员（装 Rust 工具链、VS Build Tools 或 MSVC linker 必须要）；
5. **凭证方式**：密钥（推荐）或密码 + 是否允许我装软件；
6. **可接受的影响**：会在上面建 `C:\p5` 工作目录、装 `rustup` + MSVC 构建工具（约 3–6 GB）、
   运行真实测试（会写临时文件、会起进程）。**没有快照可回滚**，所以要提前说清哪些不能碰。

**能回答**：真实平台上的构建与运行、真实文件系统/杀软/权限行为、物理机性能基准、
将来"Windows 上的 worker 隔离/取消/恢复"实现是否成立。
**不能回答**：干净安装、反复回滚、多版本并存（除非有多台或允许重装）。

### 3.3 方案 C — CI 加 `windows-latest`（建议同时做，成本最低）

现状：`.github/workflows/ci.yml` 只有 `runs-on: ubuntu-24.04`，`grep -c windows` = **0**。

价值：① 把"Windows 能不能编译/测试"变成每次提交都跑的回归；② 直接为发行阻断项 **G11（CI 未在真实
runner 上运行）** 提供证据——GitHub 托管的 `windows-latest` 就是真实机器；③ 不占本机资源。
局限：没有交互式调试、单 job 6 h 上限、装 MSVC/依赖较慢、测不到"安装/卸载/崩溃恢复"这类破坏性场景。

### 3.4 三者对比

| 维度 | A 本机 KVM 虚拟机 | B 实体机 SSH | C CI `windows-latest` |
|---|---|---|---|
| 平台真实性 | 中（虚拟硬件） | **高（真机）** | 高（云端真机，但环境固定） |
| 快照/回滚 | **有（qcow2 快照）** | 无 | 每次全新，但不是"我的环境" |
| 破坏性测试（安装/卸载/kill -9） | **适合** | 谨慎、可能污染 | 不适合 |
| 交互式调试 | 有（RDP/GTK） | 有（SSH） | 差 |
| 性能基准可信度 | 低（4 vCPU 虚拟） | **高** | 中（云端规格固定） |
| 一次性成本 | ISO + 1–2 h | 几乎为零（你已有机） | 几乎为零（写 workflow） |
| 持续成本 | 占本机 8 GiB + 4 vCPU | 占你的机器时间 | 免费额度内 |
| 对 G11 的贡献 | 无 | 无（不是 CI） | **有** |
| 主要风险 | Win11 需 swtpm；性能差 | 无快照、可能与你的工作冲突 | 6 h 上限、依赖下载慢 |

---

## 4. 推荐与理由

### 4.1 先说一个决定性事实：Windows 上现在能跑什么、不能跑什么

我刚查了平台门禁的实际落点（不是凭印象）：

- `translator-runtime/transport.rs:145` 里 `ensure_supported()` 把 **worker 传输层**限制在 `linux`；
  `VERIFIED_PLATFORMS = ["linux"]`，其他平台一律 `CapabilityUnavailable`（这是 ADR-043 的证据表设计，不是 bug）。
- **ORT 后端没有任何平台判断**（只有 feature 隔离），而 ONNX Runtime 本身支持 Windows。
- 词典（`rusqlite`/mmap）、规则、`translator-api` 门面、`translator-cli` 的 Mock 路径都是纯 Rust。

所以 **Windows 上立刻可验证的是**：`cargo build/test`、CLI、词典/规则、**ORT 后端的真实翻译**；
**不可验证的是**：Bergamot worker 那条链路（会被平台门禁拒掉，需要 P5 自己的 Windows 传输实现）。
这条直接决定了验证矩阵该长什么样——不然到 Windows 上第一件事就是撞 `CapabilityUnavailable`，白跑一趟。

### 4.2 推荐组合（不是三选一）

| 目的 | 用哪个 | 什么时候 |
|---|---|---|
| 每次提交的 Windows 回归 | **C**（`windows-latest` job） | 现在就加，成本最低，且顺手为 G11 供证 |
| 迭代开发 + 破坏性场景（安装/卸载/崩溃残留） | **A**（本机 KVM） | 我搭，先 Win10 22H2（**不装 swtpm 也能开工**），需要时升级 Win11 |
| 验收证据 / 物理机基准 | **B**（你的实体机） | P5 验收阶段，届时再打通 SSH |

如果**只能选一个**：**选 B**。因为 P5 原文要"物理机基准"，那只有真机给得了；
但代价是全程没有快照，装坏了只能你手动救。所以更稳的是 **A 先跑通，B 做验收**。

### 4.3 我需要你确认的（三件）

1. **Windows 路线**：要我①先搭本机 KVM（Win10 22H2 起步 / 直接 Win11 + swtpm），还是②你给实体机 SSH 参数，
   还是③两条都走（我先搭 A，你把 B 的 SSH 准备起来）；
2. **若走 B**：Windows 版本、是否同内网（`192.168.1.0/24`）、OpenSSH Server 是否已开、账号是否有管理员、
   允许我装 rustup + MSVC 构建工具（约 3–6 GB）、有无不能碰的目录/服务；
3. **Android 侧**：是否装 NDK（约 3 GB）+ 是否有一台 ARM64 真机可接 adb（P5 的"物理机基准"只能靠它）。

### 4.4 我会怎么干（拿到确认后）

1. **A 路线**：`qemu-img` 建 60 GB qcow2 → q35+OVMF 装 Win10 → 装 rustup+MSVC → 跑
   `cargo build/test` + `translator-cli` + ORT 真实翻译 → 打第一个快照 → 写进 `docs/30` 的验证矩阵。
2. **C 路线**：给 `ci.yml` 加 `windows-latest` job（fmt/clippy/test，跳过需要 Linux worker 的用例），
   保留 Linux job 不变；同时把"Windows 上 Bergamot 被平台门禁拒绝"写成显式断言（而不是让它偶然失败）。
3. **B 路线**：拿到 SSH 后先做只读勘察（`systeminfo`、`Get-ComputerInfo`、磁盘/编码/locale），
   再装工具链，再跑同一套矩阵，最后与 A 的结果并排对比。

---

## 5. 可复现的勘察命令（本轮实际跑过的）

```bash
# 机器本体
cat /etc/os-release; uname -r; lscpu | sed -n '1,20p'; free -h; df -h /
cat /sys/class/dmi/id/product_name
ls -l /dev/kvm; id; /usr/sbin/kvm-ok; lsmod | grep -E '^kvm'

# 磁盘与速率
lsblk -d -o NAME,SIZE,ROTA,MODEL
dd if=/dev/zero of=/tmp/disk_test.bin bs=1M count=1024 conv=fdatasync   # 449 MB/s
dd if=/tmp/disk_test.bin of=/dev/null bs=1M iflag=direct                  # 419 MB/s

# 虚拟化工具链
qemu-system-x86_64 -accel help; qemu-system-x86_64 -display help
ls /usr/share/OVMF/; dpkg -l ovmf
command -v swtpm virsh virt-install VBoxManage wine
qemu-system-x86_64 -device help | grep -E 'virtio-(blk|net|9p|gpu)'
timeout 8 qemu-system-x86_64 -machine q35 -accel kvm -cpu host -smp 8 -m 512 -display none

# Android（存在性与可启动性）
ls ~/Android/Sdk; du -sh ~/Android/Sdk ~/.android
~/Android/Sdk/emulator/emulator -version | head -3
~/Android/Sdk/cmdline-tools/latest/bin/sdkmanager --list_installed
~/Android/Sdk/emulator/emulator -list-avds
~/Android/Sdk/emulator/emulator -avd gms2 -no-window -no-audio -no-snapshot -gpu swiftshader_indirect &
~/Android/Sdk/platform-tools/adb devices
~/Android/Sdk/platform-tools/adb shell getprop sys.boot_completed      # -> 1
~/Android/Sdk/platform-tools/adb emu kill
```

---

## 6. 与设计的偏差（P5 还没开始，但已经有三条要先记下来）

| 编号 | 偏差 | 原因 / 影响 |
|---|---|---|
| **D-83** | Android 侧只有 **x86_64** 镜像，P5 要 **ARM64** JNI | 之前搭这套是为了跑普通 App，不是给 SDK 做 ARM64 验证。x86 主机上的 arm64 镜像是全指令翻译，**不能**用作"物理机基准"；要么装 arm64 镜像只做功能预演，要么用真机 |
| **D-84** | Windows 上 **Bergamot worker 路径必然不可用**（平台门禁 + Unix 语义） | `VERIFIED_PLATFORMS = ["linux"]`。所以 Windows 的验证矩阵**不能**照抄 Linux 的，否则一上手就撞 `CapabilityUnavailable`；ORT 后端反而是 Windows 上能跑通的那个（无平台判断） |
| **D-85** | 本机没有 libvirt/virt-manager/swtpm，虚拟化是"裸 qemu 命令行"状态 | 能用（qemu 8.2 + KVM + OVMF 已在），但没有图形化管理、Win11 需补 `swtpm`。选择"命令行 + 脚本"而不是装整套 libvirt，是为了让 VM 配置本身可 diff、可复现（和项目一贯的"证据可复现"一致） |

（本轮无代码变更，因此"实际实现内容"部分只包含勘察与结论；`docs/01` 的 ADR 暂不新增——
等你在 §4.3 选定路线后，再把"平台验证三层策略"落成 ADR。）
