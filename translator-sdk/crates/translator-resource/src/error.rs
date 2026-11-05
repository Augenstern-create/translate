//! 资源层错误。
//!
//! 刻意不依赖 `translator-types`：资源管理器是要被 CLI、安装器、宿主共同使用的基础设施，
//! 让它只依赖 `serde` 与 `sha2`，比顺着 `translator-api` 层层拉依赖更好维护。

use std::fmt;

#[derive(Debug)]
pub enum ResourceError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// 清单本身不合法（缺字段、字段互斥、版本号为空等）。
    InvalidManifest(String),
    /// Manifest 协议版本不受支持（**明确拒收，不猜**）。
    UnsupportedManifestSchema(u32),
    /// Payload 存储版本不受支持（例如将来的 dictionary.sqlite.v2）。
    UnsupportedPayloadSchema(String),
    /// 资源声明的目标平台不包含当前平台。
    UnsupportedPlatform(String),
    /// 依赖的资源不在已安装集合里。
    MissingDependency {
        id: String,
        requirement: String,
    },
    /// 文件缺失、大小或 sha256 不匹配。
    IntegrityFailure {
        path: String,
        expected: String,
        actual: String,
    },
    /// 清单里出现绝对路径或 `..`，属于路径穿越。
    UnsafePath(String),
    /// 源目录里出现符号链接（P2 不跟随链接，避免读到资源包之外的文件）。
    SymlinkNotAllowed(String),
    AlreadyInstalled {
        id: String,
        version: String,
    },
    NotInstalled {
        id: String,
        version: String,
    },
    NotFound(String),
    /// 目标版本正在使用（当前激活版本），拒绝删除或覆盖。
    ResourceInUse {
        id: String,
        version: String,
    },
    /// 该版本还有**有效租约**（P2.3）：有进程正持有它，拒绝删除。
    /// 仍被其他已安装资源依赖（`docs/17` LC-07）。
    ///
    /// 与 `ResourceLeased` 的区别：租约是"某个进程正在用"，依赖是"某个已安装包的清单
    /// 声明需要这个版本"。前者会自己过期，后者不会——删了就是永久破坏。
    ReversedDependency {
        id: String,
        version: String,
        dependents: Vec<String>,
    },
    ResourceLeased {
        id: String,
        version: String,
        holders: Vec<String>,
    },
    /// 等不到 `state.json` 的跨进程锁（P2.3）。
    LockTimeout {
        path: String,
        holder: String,
    },
    /// 版本号字符串非法（用于目录名，禁止分隔符与 `..`）。
    InvalidVersion(String),
    /// 状态文件损坏：宁可报错也不猜。
    CorruptState(String),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO 错误：{e}"),
            Self::Json(e) => write!(f, "JSON 解析失败：{e}"),
            Self::InvalidManifest(msg) => write!(f, "清单不合法：{msg}"),
            Self::UnsupportedManifestSchema(v) => write!(
                f,
                "不支持的 manifest_schema_version={v}（本 SDK 支持 1 与 2）"
            ),
            Self::UnsupportedPayloadSchema(v) => {
                write!(f, "不支持的 payload_schema_version={v}")
            }
            Self::UnsupportedPlatform(p) => write!(f, "资源不支持当前平台：{p}"),
            Self::MissingDependency { id, requirement } => {
                write!(f, "缺少依赖资源：{id} {requirement}")
            }
            Self::IntegrityFailure {
                path,
                expected,
                actual,
            } => write!(f, "完整性校验失败：{path} 期望 {expected} 实际 {actual}"),
            Self::UnsafePath(p) => write!(f, "清单里出现不安全的路径：{p}"),
            Self::SymlinkNotAllowed(p) => write!(f, "不允许符号链接：{p}"),
            Self::AlreadyInstalled { id, version } => {
                write!(
                    f,
                    "资源已安装：{id}@{version}（未覆盖，请先卸载或用新版本号）"
                )
            }
            Self::NotInstalled { id, version } => write!(f, "资源未安装：{id}@{version}"),
            Self::NotFound(id) => write!(f, "资源不存在：{id}"),
            Self::ResourceInUse { id, version } => {
                write!(f, "资源 {id}@{version} 正在使用（当前激活版本），拒绝删除")
            }
            Self::ReversedDependency {
                id,
                version,
                dependents,
            } => write!(f, "资源 {id}@{version} 仍被依赖：{}", dependents.join(", ")),
            Self::ResourceLeased {
                id,
                version,
                holders,
            } => write!(
                f,
                "资源 {id}@{version} 有 {} 个有效租约，拒绝删除：{}",
                holders.len(),
                holders.join("; ")
            ),
            Self::LockTimeout { path, holder } => write!(
                f,
                "等待状态锁超时（持有者：{holder}，锁文件：{path}）；\
                 确认没有其它进程在使用该仓库后可手动删除该文件"
            ),
            Self::InvalidVersion(v) => write!(f, "版本号非法：{v}"),
            Self::CorruptState(msg) => write!(f, "状态文件损坏：{msg}"),
        }
    }
}

impl std::error::Error for ResourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ResourceError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ResourceError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub type ResourceResult<T> = Result<T, ResourceError>;
