//! 运行时错误与对外错误码映射（`docs/12` §3.2 的表）。
//!
//! 这一层存在的理由：**native 侧的错误形态和 SDK 的错误形态不是一回事**。
//! Bergamot/Marian 在缺文件时 `abort()`、在权重被截断时 SIGSEGV（P3.0 实测），
//! 而宿主需要的是可分类、可决策、可重试的错误。所以映射集中在这里，
//! 不散落在各调用点。

use std::fmt;

use translator_types::TranslationErrorCode;

/// 运行时（worker / 传输层）错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    /// 模型文件不存在或不是普通文件。
    NotInstalled(String),
    /// 文件存在但大小/摘要与清单不符，或清单自相矛盾。
    IntegrityFailure(String),
    /// 本平台/本构建不具备该能力（例如非 Linux 上启动进程 worker、或重启预算耗尽）。
    CapabilityUnavailable(String),
    /// worker 启动失败或加载失败（进程起来了但 Load 没成功）。
    InitFailed(String),
    /// worker 已就绪但本次请求失败（含 worker 崩溃）。
    BackendFailed(String),
    /// 等待超时。**不代表 native 推理已经停止**（`docs/12` §2.1）。
    Timeout {
        operation: &'static str,
        waited_ms: u64,
    },
    /// 调用方取消（同样不代表 native 已停止）。
    Cancelled,
    /// 协议层错误：帧格式、JSON、请求/响应不匹配。
    Protocol(String),
    /// worker 进程已退出。
    ///
    /// 同时带退出码和信号：被 `abort()`/SIGSEGV 杀掉的进程**没有退出码**
    /// （`ExitStatus::code()` 返回 `None`），只看 code 会把 P3.0 的两种崩溃
    /// 都显示成"没有信息"。信号号才是诊断的关键。
    WorkerExited {
        code: Option<i32>,
        signal: Option<i32>,
        detail: String,
    },
}

impl RuntimeError {
    /// 映射到对外错误码（`docs/12` §3.2）。
    pub fn code(&self) -> TranslationErrorCode {
        match self {
            Self::NotInstalled(_) => TranslationErrorCode::ResourceNotInstalled,
            Self::IntegrityFailure(_) => TranslationErrorCode::IntegrityFailure,
            Self::CapabilityUnavailable(_) => TranslationErrorCode::CapabilityUnavailable,
            Self::InitFailed(_) => TranslationErrorCode::RuntimeInitFailed,
            Self::BackendFailed(_) | Self::WorkerExited { .. } => {
                TranslationErrorCode::BackendFailed
            }
            Self::Timeout { .. } => TranslationErrorCode::Timeout,
            Self::Cancelled => TranslationErrorCode::Cancelled,
            // 协议错误在对外语义上属于"后端给了无法理解的东西"
            Self::Protocol(_) => TranslationErrorCode::BackendFailed,
        }
    }

    /// 是否允许"换一次新 worker 再试"。
    ///
    /// 刻意很保守：只有**确定是 worker 侧死掉**的情况才允许重启后重试；
    /// 数据完整性问题（摘要不符）重试一万次也不会变好。
    pub fn retryable_after_restart(&self) -> bool {
        matches!(self, Self::WorkerExited { .. } | Self::BackendFailed(_))
    }

    /// 诊断用的简短标签（不含原文，`docs/12` §3.1 要求不把明文写进日志）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotInstalled(_) => "not_installed",
            Self::IntegrityFailure(_) => "integrity_failure",
            Self::CapabilityUnavailable(_) => "capability_unavailable",
            Self::InitFailed(_) => "init_failed",
            Self::BackendFailed(_) => "backend_failed",
            Self::Timeout { .. } => "timeout",
            Self::Cancelled => "cancelled",
            Self::Protocol(_) => "protocol",
            Self::WorkerExited { .. } => "worker_exited",
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled(msg) => write!(f, "模型文件缺失：{msg}"),
            Self::IntegrityFailure(msg) => write!(f, "模型完整性校验失败：{msg}"),
            Self::CapabilityUnavailable(msg) => write!(f, "运行时能力不可用：{msg}"),
            Self::InitFailed(msg) => write!(f, "运行时初始化失败：{msg}"),
            Self::BackendFailed(msg) => write!(f, "运行时后端失败：{msg}"),
            Self::Timeout {
                operation,
                waited_ms,
            } => write!(f, "等待 {operation} 超时（{waited_ms} ms）"),
            Self::Cancelled => f.write_str("调用方取消"),
            Self::Protocol(msg) => write!(f, "worker 协议错误：{msg}"),
            Self::WorkerExited {
                code,
                signal,
                detail,
            } => {
                write!(f, "worker 进程已退出")?;
                if let Some(code) = code {
                    write!(f, "（退出码 {code}）")?;
                }
                if let Some(signal) = signal {
                    write!(f, "（被信号 {signal}{} 终止）", signal_hint(*signal))?;
                }
                write!(f, "：{detail}")
            }
        }
    }
}

fn signal_hint(signal: i32) -> &'static str {
    match signal {
        6 => " / SIGABRT",
        9 => " / SIGKILL",
        11 => " / SIGSEGV",
        15 => " / SIGTERM",
        _ => "",
    }
}

impl std::error::Error for RuntimeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 错误码映射与审查表一致() {
        let cases: [(RuntimeError, TranslationErrorCode); 9] = [
            (
                RuntimeError::NotInstalled("x".into()),
                TranslationErrorCode::ResourceNotInstalled,
            ),
            (
                RuntimeError::IntegrityFailure("x".into()),
                TranslationErrorCode::IntegrityFailure,
            ),
            (
                RuntimeError::CapabilityUnavailable("x".into()),
                TranslationErrorCode::CapabilityUnavailable,
            ),
            (
                RuntimeError::InitFailed("x".into()),
                TranslationErrorCode::RuntimeInitFailed,
            ),
            (
                RuntimeError::BackendFailed("x".into()),
                TranslationErrorCode::BackendFailed,
            ),
            (
                RuntimeError::Timeout {
                    operation: "translate",
                    waited_ms: 10,
                },
                TranslationErrorCode::Timeout,
            ),
            (RuntimeError::Cancelled, TranslationErrorCode::Cancelled),
            (
                RuntimeError::Protocol("x".into()),
                TranslationErrorCode::BackendFailed,
            ),
            (
                RuntimeError::WorkerExited {
                    code: None,
                    signal: Some(6),
                    detail: "abort".into(),
                },
                TranslationErrorCode::BackendFailed,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.code(), expected, "{error}");
        }
    }

    #[test]
    fn 只有worker死掉才允许重启后重试() {
        assert!(RuntimeError::WorkerExited {
            code: None,
            signal: Some(11),
            detail: String::new()
        }
        .retryable_after_restart());
        assert!(!RuntimeError::IntegrityFailure("sha256 不符".into()).retryable_after_restart());
        assert!(!RuntimeError::Timeout {
            operation: "translate",
            waited_ms: 1
        }
        .retryable_after_restart());
    }
}
