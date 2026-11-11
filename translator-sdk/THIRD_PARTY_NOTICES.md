# 第三方声明（Third-Party Notices）

本文件是发行物的一部分：任何人拿到本 SDK 的源码或二进制，都应能查到
"里面有哪些第三方代码、各是什么许可"。

## 0. 结论先说

1. **本仓库不包含任何模型权重**，也不包含 llama.cpp / ONNX Runtime 等推理运行时二进制。
   本地模型由使用者自行获取与部署（本 SDK 只通过 HTTP 与之通信）。
2. 依赖树（102 个 crate，含传递依赖）全部落在宽松许可范围内：
   MIT / Apache-2.0 / BSD / ISC / Zlib / Unlicense / Unicode-3.0。
   没有 GPL / AGPL / SSPL 类传染性许可。
3. 打包的 SQLite（`rusqlite` 的 `bundled` 特性）属于 **Public Domain**。

## 1. 本项目自身

| 组件 | 许可 |
|---|---|
| translator-types / plugin-api / core / http / dictionary / rules / api | Apache-2.0 OR MIT |
| translator-analysis（研究线） | Apache-2.0 OR MIT |
| apps/translator-cli / tools/pack-dictionary / tools/quality-runner | Apache-2.0 OR MIT |

## 2. 直接依赖（首个非 dev 依赖版本）

| crate | 许可 | 用途 |
|---|---|---|
| `serde`, `serde_json` | MIT OR Apache-2.0 | 配置/协议/数据文件 |
| `toml` | MIT OR Apache-2.0 | 后端配置 |
| `sha2` | MIT OR Apache-2.0 | 资源指纹、提示词指纹 |
| `fst` | Unlicense OR MIT | 词典索引 |
| `rusqlite`（bundled SQLite） | MIT（SQLite 本身 Public Domain） | 词典释义 |
| `memmap2` | MIT OR Apache-2.0 | 只读映射 |
| `unicode-normalization` | (MIT OR Apache-2.0) AND Unicode-3.0 | 规则包规范化 |
| `ureq` | MIT OR Apache-2.0 | HTTP 客户端 |
| `rustls` + `ring` | Apache-2.0 AND ISC（ring）；Apache-2.0/MIT（rustls） | TLS |
| `webpki-roots` | MPL-2.0 | 根证书 |
| `pollster` | Apache-2.0/MIT | 测试/CLI 驱动 Future |
| `tempfile` | MIT OR Apache-2.0 | 测试 |

完整依赖清单可用 `cargo metadata` 或 `cargo tree` 复核。

## 3. 外部服务与资源（不随本仓库分发）

| 资源 | 说明 |
|---|---|
| 远程转发站 / 云端模型 | 由使用者自行配置（`base_url` + `api_key_env`），本仓库不含密钥 |
| 本地模型权重 | 由使用者自行获取并遵守其许可；本 SDK 不打包、不缓存、不分发 |
| llama.cpp / vLLM / TGI 等运行时 | 由使用者自行安装；本 SDK 只通过 HTTP 调用 |
| 离线词典数据 | `translator-dictionary` 支持打包，数据许可由数据包 manifest 单独声明 |

## 4. 如何复核

```bash
cd translator-sdk
cargo metadata --format-version 1 | python3 -c "import json,sys; ..."   # 或 cargo tree
cargo deny check licenses        # 若安装了 cargo-deny
```

本文件不替代各依赖自带 LICENSE 文件；分发二进制时应同时附带依赖的许可文本。
