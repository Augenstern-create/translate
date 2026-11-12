# 发布说明（Release Candidate）

当前版本：**v0.2.0-rc.1**（Cargo workspace `version`，`translator-cli --version` 输出一致）

> 这是**发布候选**，不是正式稳定版。翻译质量的人工验收仍未完成（见
> `docs/known-issues.md` K-08/K-19/K-21）；工程稳定性与跨平台运行已通过自动与实机验证。

## 1. 产物

| 平台 | 文件名 | 内容 |
|---|---|---|
| Windows x86_64 | `augenstern-translate-windows-x64.zip` | `translator-cli.exe`、`README.md`、`RELEASE.md`、`LICENSE-MIT`、`LICENSE-APACHE`、`THIRD_PARTY_NOTICES.md`、`translate.example.toml` |
| Linux x86_64 | `augenstern-translate-linux-x64.tar.gz` | 同上（二进制为 `translator-cli`） |

每个产物目录另有 `SHA256SUMS.txt`。校验：

```bash
sha256sum -c SHA256SUMS.txt
# Windows PowerShell
Get-FileHash .\augenstern-translate-windows-x64.zip -Algorithm SHA256
```

## 2. 运行环境

| 项 | 要求 |
|---|---|
| Windows | Windows 10/11 x86_64（实机验证于 Windows，使用 MSVC 工具链构建） |
| Linux | 在 Ubuntu 24.04（glibc 2.39）上构建；**只保证在该版本或更新的 glibc 2.39+ 发行版上运行**，不要假设可在更老的企业发行版上直接运行 |
| 网络 | 能访问配置的 OpenAI 兼容端点（https） |
| 磁盘 | 可选缓存文件目录需可写 |

检查动态依赖：`ldd translator-cli`；如需更老发行版支持，应在对应发行版内构建或使用 musl 目标（本版未提供）。

## 3. 快速开始

```bash
# 1) 解压
tar -xzf augenstern-translate-linux-x64.tar.gz && cd augenstern-translate-linux-x64

# 2) 配置：复制示例并按需修改 base_url / model
cp translate.example.toml translate.toml

# 3) 密钥只从环境变量读取（不要写进配置文件）
export TRANSLATE_API_KEY='...'       # 或自定义 api_key_env 指向的变量名

# 4) 检查配置
./translator-cli config check --config translate.toml

# 5) 翻译
./translator-cli translate "Hello, world." --config translate.toml

# 6) 本地网页界面（只监听 127.0.0.1）
./translator-cli serve --config translate.toml --port 8756
```

Windows 把 `./translator-cli` 换成 `translator-cli.exe`，环境变量用
`$env:TRANSLATE_API_KEY = "..."` 设置。

## 4. 配置要点

| 键 | 说明 |
|---|---|
| `[backend.<name>] base_url` | OpenAI 兼容端点，通常以 `/v1` 结尾 |
| `model` / `model_revision` | 模型名与版本；两者都会进入缓存键 |
| `api_key_env` | 存密钥的**环境变量名**；空字符串表示不发送鉴权头（本地服务） |
| `max_retries` | 传输层重试次数（429/5xx/连接重置）；401/403 不重试 |
| `disable_thinking` + `thinking_style` | 关闭模型"思考"（见下） |
| `[http] max_concurrent_requests` / `max_queue_size` | 全局并发与有界队列；队列满立即返回 `QueueFull` |
| `[translation] max_http_calls_per_request` | 一次逻辑请求的物理 HTTP 调用上限 |
| `[cache] file` | 可选磁盘缓存（**会保存译文**，默认关闭） |

### 关闭"思考"

部分模型默认输出很长的推理内容（延迟与费用都显著上升）。实测该转发站上：

| 设置 | 延迟 | completion tokens |
|---|---|---|
| 默认 | 4.4 s | 45（其中 38 是 reasoning） |
| `disable_thinking = true` + `thinking_style = "thinking_type"` | 2.1 s | 6 |

## 5. 常见问题

| 现象 | 处理 |
|---|---|
| `AuthenticationFailed` | 检查密钥环境变量名与值；401 不会自动重试 |
| `RateLimited` | 站点有并发/频率限制；降低 `max_concurrent_requests`，或提高 `max_retries` |
| `QueueFull` | 并发请求超过队列容量；降低提交速率 |
| `BudgetExceeded` | 单请求的物理调用预算用尽（默认 4）；检查是否频繁触发完整性重试 |
| `IntegrityFailure` | 译文没通过完整性/数字/语言检查；查看 `--json` 里的 `quality` |
| 代理不生效 | 只支持 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY` 环境变量；不支持系统代理设置与 PAC |
| 缓存写入失败 | 会打印原因但不影响本次翻译结果；检查目录权限 |

## 6. 数据与隐私

- 远程模型会收到待翻译的**原文**；本地部署的 OpenAI 兼容服务同样会把文本发给该服务。
- 磁盘缓存开启后，译文会**落盘**；默认关闭。
- 评测报告（`docs/eval/`）包含模型译文，不包含密钥。
- 日志默认不打印原文、译文与密钥。

## 7. 已知限制

见 `docs/known-issues.md`。与发布相关的重点：

1. 已在飞行的 `ureq` 请求无法立即中断（取消只保证 Future 及时返回、不再重试）。
2. 请求中断后仍可能计费。
3. 翻译质量的人工验收未完成。
4. 不支持 Android；不支持 macOS 构建产物。
5. 系统代理/PAC 不支持；TLS 证书校验不可关闭。
6. Linux 产物兼容边界为 glibc 2.39+。
