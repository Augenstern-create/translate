//! # translator-runtime
//!
//! 把"翻译引擎"关进另一个**操作系统进程**里，通过带长度前缀的帧做 IPC。
//!
//! ## 为什么必须是进程
//!
//! P3.0 实测：模型文件缺失时 Bergamot 直接 `abort()`（退出码 134），权重被截断时 SIGSEGV。
//! 这两种失败**线程隔离不了**——`panic = "abort"` 让 `catch_unwind` 也失效。
//! 所以隔离边界只能是进程（`docs/12` §2.1，ADR-027）。
//!
//! ## 分层
//!
//! - [`protocol`]：帧格式 + 请求/响应类型。约束：4 字节大端长度、上限 8 MiB、
//!   版本号不匹配即拒绝、**长度在任何分配之前检查**。
//! - [`model`]：模型清单与校验（存在性 / 非符号链接 / 大小 / 流式 sha256），以及 RSS 采样。
//! - [`worker`]：子进程生命周期、请求超时、终止升级（shutdown 帧 → SIGTERM → SIGKILL）。
//! - [`transport`]：对上层暴露的 [`RuntimeTransport`] 契约。
//!
//! ## 明确不做的事
//!
//! - 不做静默重试：一次请求失败就是失败；重启只发生在**下一次请求**，且受预算限制。
//! - 不在 worker 死后自动改用远程 LLM（ADR-004 的本地优先不允许这种"降级"偷偷发生）。
//! - 非 Linux 平台没有 `/proc` 采样，`worker_rss_bytes` 返回 `None` 而不是猜一个数。

// 全 crate 禁止 unsafe；唯一例外是 worker.rs 里发 SIGTERM 的那一处，显式 allow。
#![deny(unsafe_code)]

pub mod config;
pub mod error;
pub mod model;
pub mod protocol;
pub mod snapshot;
pub mod transport;
pub mod worker;

pub use config::{render_bergamot_config, BergamotOptions};
pub use error::RuntimeError;
pub use model::{
    process_rss_bytes, rss_bytes_of, FileDigest, FileRole, FileSpec, ModelInfo, ModelManifest,
    ModelSpec, TranslationJob, TranslationReply, TransportMetrics, WorkerHealth,
};
pub use protocol::{MAX_FRAME_BYTES, PROTOCOL_VERSION};
pub use snapshot::{prune_stale_snapshots, ModelSnapshot};
pub use transport::{ProcessTransport, RuntimeTransport};
pub use worker::{WorkerConfig, WorkerProcess};
