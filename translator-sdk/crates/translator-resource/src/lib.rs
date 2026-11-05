//! 资源协议与资源仓库（P2.1 + P2.2）。
//!
//! 这个 crate 只做两件事，都刻意与 `translator-api` 解耦：
//!
//! 1. **协议**（`manifest` / `compat`）：资源清单的 v1/v2 解析、双版本号
//!    （`manifest_schema_version` / `payload_schema_version`）、兼容性判定；
//! 2. **仓库**（`store`）：安装、逐文件校验、原子激活、回滚、安全清理。
//!
//! ```no_run
//! use translator_resource::store::ResourceStore;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let store = ResourceStore::open("resources-store")?;
//! let outcome = store.install_from_dir("path/to/package")?;
//! println!("已安装 {}@{}（激活：{}）", outcome.resource.id, outcome.resource.version, outcome.activated);
//! let dir = store.active_dir("dictionary.cc-cedict.zh-en")?;
//! println!("激活目录：{}", dir.display());
//! # Ok(())
//! # }
//! ```
//!
//! 明确**不在**本 crate 范围内的（见 `docs/06_P2.1-P2.2_资源协议与仓库报告.md`）：
//! 租约与内存预算（P2.3）、执行计划与缓存指纹（P2.4）、SQLite 查询优化（P2.5）。

pub mod compat;
pub mod error;
pub mod manifest;
pub mod store;

pub use compat::{
    check, current_platform, ensure_compatible, CompatibilityIssue, CompatibilityReport,
    HostSupport,
};
pub use error::{ResourceError, ResourceResult};
pub use manifest::{
    Dependency, ManifestSchema, PackageFile, PayloadSchema, Provenance, ResourceKind,
    ResourceManifest, MANIFEST_SCHEMA_V1, MANIFEST_SCHEMA_V2,
};
pub use store::{
    read_manifest_from, sha256_file, verify_files, InstallOutcome, InstalledResource,
    RecoverReport, ResourceStatus, ResourceStore,
};
