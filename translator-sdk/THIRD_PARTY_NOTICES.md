# 第三方声明（Third-Party Notices）

> 生成方式见文末"如何复核"。本文件是**发行物的一部分**：任何人拿到本 SDK 的源码或二进制，
> 都应该能从这里查到"里面有哪些第三方代码、它们各是什么许可"。

## 0. 结论先说

1. **本仓库不包含模型权重，也不包含 Bergamot 原生二进制。** 两者由使用者按
   `fixtures/nmt/models.lock.json` 与本文件 §2 的坐标在本地获取/构建。
2. **Mozilla 模型权重的再分发许可尚未确认**，因此本项目**不打包、不缓存、不分发权重**，
   也不宣称"可分发的离线模型包"。这条是发行红线，由 `tools/check-release.py` 与 CI 强制执行。
3. Cargo 依赖（53 个包）全部落在宽松许可范围内（MIT / Apache-2.0 / BSD / ISC / Zlib /
   Unlicense / Unicode-3.0 / MPL-2.0 的组合），没有 GPL/AGPL/SSPL 类传染性许可。

## 1. 本项目自身

| 项 | 值 |
|---|---|
| 声明许可 | `Apache-2.0 OR MIT`（见各 `Cargo.toml` 的 `license` 字段） |
| 许可正文 | **尚未入库**（`LICENSE-MIT` / `LICENSE-APACHE` 缺失） |
| 影响 | 源码可用，但"发行物附带许可正文"这一条还没满足；见 `docs/21_P3.4报告.md` 的 D 记录 |
| 发布状态 | 所有 crate 均 `publish = false`，**不会被误发布到 crates.io** |

## 2. 非 Cargo 资产（不入库、不分发）

| 资产 | 许可 / 状态 | 分发方式 |
|---|---|---|
| Mozilla Firefox Translations en→zh-Hans v3.0 权重（4 个 registry 文件 + 1 个 ssplit 前缀表） | **未确认**（registry 无 license 字段） | 使用者自行下载，校验 SHA-256；本仓库不含 |
| `bergamot-translator` CLI @ `9271618` | 上游 MPL-2.0 | 使用者本地构建（`tools/p3/build-bergamot.sh`）；本项目只按路径启动，不链接、不分发 |
| 词典数据（`resources/zh-en`） | 本项目自建，来源见 `docs/02` | 用 `tools/pack-dictionary` 本地生成；不入库 |

详细坐标、哈希、许可状态与判定理由：**`licenses/model-assets.json`**。

### 2.1 为什么"按路径启动"能避开 MPL-2.0 的传染

MPL-2.0 是**文件级** copyleft：它要求"被修改的 MPL 文件"继续以 MPL 发布，
但不要求链接到它的其他代码换许可。本项目的边界更远一层——native 是**独立进程**，
通过 JSON 行协议通信，不做静态或动态链接。因此：

- 我们**不修改** bergamot 源码（只为参考 CLI 加了按行输出的补丁，见 `tools/p3/bergamot-cli-lines.patch`，
  该补丁本身也在 MPL 项目内）；
- 我们**不打包**它的二进制；
- 若将来改为随包分发或静态链接，必须重新评估，并在此文件与 `licenses/model-assets.json` 中更新。

## 3. Cargo 依赖清单（53 个包，`--all-features`）

机器可读版本：`licenses/third-party.json`（含 checksum 与归一化后的表达式）。

| crate | 版本 | 声明许可 |
|---|---|---|
| `ahash` | 0.8.12 | MIT OR Apache-2.0 |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 |
| `cc` | 1.11.0 | MIT OR Apache-2.0 |
| `cfg-if` | 1.0.5 | MIT OR Apache-2.0 |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 |
| `digest` | 0.10.7 | MIT OR Apache-2.0 |
| `errno` | 0.3.14 | MIT OR Apache-2.0 |
| `fallible-iterator` | 0.3.0 | MIT/Apache-2.0（历史写法，按 OR 解释） |
| `fallible-streaming-iterator` | 0.1.9 | MIT/Apache-2.0（历史写法，按 OR 解释） |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT |
| `find-msvc-tools` | 0.1.15 | MIT OR Apache-2.0 |
| `fst` | 0.4.7 | Unlicense/MIT（历史写法，按 OR 解释） |
| `generic-array` | 0.14.7 | MIT |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 |
| `hashbrown` | 0.14.5 | MIT OR Apache-2.0 |
| `hashlink` | 0.9.1 | MIT OR Apache-2.0 |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 |
| `libc` | 0.2.191 | MIT OR Apache-2.0 |
| `libsqlite3-sys` | 0.30.1 | MIT |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `memchr` | 2.8.3 | Unlicense OR MIT |
| `memmap2` | 0.9.12 | MIT OR Apache-2.0 |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 |
| `pkg-config` | 0.3.34 | MIT OR Apache-2.0 |
| `pollster` | 0.4.0 | Apache-2.0/MIT（历史写法，按 OR 解释） |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 |
| `quote` | 1.0.47 | MIT OR Apache-2.0 |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| `rusqlite` | 0.32.1 | MIT |
| `rustix` | 1.1.6 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `serde` | 1.0.229 | MIT OR Apache-2.0 |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 |
| `smallvec` | 1.16.4 | MIT OR Apache-2.0 |
| `syn` | 2.0.119 | MIT OR Apache-2.0 |
| `syn` | 3.0.9 | MIT OR Apache-2.0 |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 |
| `tinyvec` | 1.13.3 | Zlib OR Apache-2.0 OR MIT |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 |
| `unicode-ident` | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| `unicode-normalization` | 0.1.26 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| `vcpkg` | 0.2.15 | MIT/Apache-2.0（历史写法，按 OR 解释） |
| `version_check` | 0.9.5 | MIT/Apache-2.0（历史写法，按 OR 解释） |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 |
| `zerocopy` | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT |
| `zerocopy-derive` | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT |
| `zmij` | 1.0.23 | MIT |

### 3.1 需要留意的几条

- **`fst` / `memchr`：`Unlicense OR MIT`**。Unlicense 属公有领域奉献，允许无条件使用；
  我们按 MIT 分支履行义务（保留版权与许可声明）。
- **6 个包使用历史写法 `MIT/Apache-2.0`**（`fallible-iterator`、`fallible-streaming-iterator`、
  `fst`、`pollster`、`vcpkg`、`version_check`）。这不是合法 SPDX；按历史约定视为"任选其一"。
  `tools/license-audit.py` 会归一化并**在清单里标记 `legacy_notation: true`**，不悄悄改掉。
- **`r-efi`：`MIT OR Apache-2.0 OR LGPL-2.1-or-later`**。OR 关系下我们选 MIT，
  因此 LGPL 分支不适用；清单按表达式求值判定，而不是看字符串里有没有 "LGPL"。
- **`rusqlite` + `libsqlite3-sys`（bundled SQLite）**：SQLite 为公有领域；
  `rusqlite` 为 MIT。bundled 意味着不依赖系统 SQLite，代价是编译期要跑 C 编译器。
- **`unicode-normalization` / `unicode-ident`：`(MIT OR Apache-2.0) AND Unicode-3.0`**。
  AND 关系下两组都要满足：Unicode-3.0 要求保留其数据文件声明，已在依赖自身包内满足。

## 4. 如何复核

```bash
cd translator-sdk
. /opt/rust/env.sh

# 1) 清单是否与当前依赖树一致 + 策略是否通过
python3 tools/license-audit.py --check

# 2) 依赖树变了就重新生成，然后**人工审阅 diff**
python3 tools/license-audit.py --write
git diff licenses/third-party.json

# 3) 发行红线（权重/大文件/打包清单/坐标一致性）
python3 tools/check-release.py

# 4) 有本地权重时顺带校验哈希
NMT_MODEL_DIR=/path/to/models python3 tools/check-release.py --verbose
```
