//! 后端错误分类。
//!
//! 目标：调用方能区分**网络失败 / 内容失败 / 配置错误 / 额度不足**，
//! 而不是统一看到一句"翻译失败"（§12.3）。
//!
//! 分类同时决定两件事：
//!
//! 1. 是否值得重试（传输层抖动 vs 密钥错误）；
//! 2. 映射到哪个 [`TranslationErrorCode`]，让宿主能做正确的 UI（提示配置 / 提示充值 / 提示稍后再试）。

use std::fmt;

use translator_types::{TranslationError, TranslationErrorCode};

/// 错误类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpErrorKind {
    /// 配置缺失或非法（缺 base_url、非 https、未知模型、未知提示词版本…）。
    Config,
    /// 密钥缺失/无效/被拒绝（401 / 403）。
    Auth,
    /// 被限流（429）。
    RateLimited,
    /// 额度/余额不足（402，或对端明确的余额错误）。
    BudgetExceeded,
    /// 本次逻辑请求的**物理调用预算**已用完（SDK 侧限制）。
    BudgetExceededLocal,
    /// HTTP 执行队列已满（有界队列拒绝）。
    QueueFull,
    /// 连接失败 / DNS 失败 / 5xx：后端暂时不可用。
    Unavailable,
    /// 超过截止时间。
    Timeout,
    /// 已取消。
    Cancelled,
    /// 响应不符合契约（非 JSON、缺 choices、空 content…）。
    InvalidResponse,
    /// 其它传输层错误（TLS 握手、协议错误）。
    Transport,
}

impl HttpErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Auth => "auth",
            Self::RateLimited => "rate_limited",
            Self::BudgetExceeded => "budget_exceeded",
            Self::BudgetExceededLocal => "budget_exceeded_local",
            Self::QueueFull => "queue_full",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::InvalidResponse => "invalid_response",
            Self::Transport => "transport",
        }
    }

    /// 传输层是否值得重试（**原样**重发）。
    ///
    /// 配置错误、鉴权失败、余额不足、响应非法都**不**重试：
    /// 原样重发只会重复失败并可能重复计费。
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Unavailable | Self::Transport
        )
    }

    pub const fn code(self) -> TranslationErrorCode {
        match self {
            Self::Config => TranslationErrorCode::ConfigurationError,
            Self::Auth => TranslationErrorCode::AuthenticationFailed,
            Self::RateLimited => TranslationErrorCode::RateLimited,
            Self::BudgetExceeded => TranslationErrorCode::BudgetExceeded,
            Self::BudgetExceededLocal => TranslationErrorCode::BudgetExceeded,
            Self::QueueFull => TranslationErrorCode::QueueFull,
            Self::Unavailable => TranslationErrorCode::BackendUnavailable,
            Self::Timeout => TranslationErrorCode::BackendTimeout,
            Self::Cancelled => TranslationErrorCode::Cancelled,
            Self::InvalidResponse => TranslationErrorCode::InvalidBackendResponse,
            Self::Transport => TranslationErrorCode::BackendUnavailable,
        }
    }
}

/// 后端错误。
///
/// `message` 会被交到宿主与日志里，因此**不得**包含：API 密钥、完整请求头、
/// 用户原文。只允许包含状态码、对端错误摘要（截断）与后端 id。
#[derive(Clone, Debug)]
pub struct HttpError {
    pub kind: HttpErrorKind,
    /// HTTP 状态码（若走到了响应阶段）。
    pub status: Option<u16>,
    pub message: String,
    /// 已尝试次数（含首次）。用于诊断成本失控。
    pub attempts: u32,
}

impl HttpError {
    pub fn new(kind: HttpErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            message: message.into(),
            attempts: 1,
        }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(HttpErrorKind::Config, message)
    }

    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(HttpErrorKind::InvalidResponse, message)
    }

    pub fn cancelled() -> Self {
        Self::new(HttpErrorKind::Cancelled, "请求已取消")
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(HttpErrorKind::Timeout, message)
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }

    /// 按状态码分类。对端返回的错误体已由调用方截断后塞进 `detail`。
    pub fn from_status(status: u16, detail: &str) -> Self {
        let kind = match status {
            401 | 403 => HttpErrorKind::Auth,
            402 => HttpErrorKind::BudgetExceeded,
            429 => HttpErrorKind::RateLimited,
            408 | 504 => HttpErrorKind::Timeout,
            500..=599 => HttpErrorKind::Unavailable,
            // 4xx 且不是上面几种：请求/配置问题，重试无意义
            400..=499 => HttpErrorKind::Config,
            _ => HttpErrorKind::Transport,
        };
        let detail = truncate(detail, 300);
        let message = if detail.is_empty() {
            format!("后端返回 HTTP {status}")
        } else {
            format!("后端返回 HTTP {status}：{detail}")
        };
        Self::new(kind, message).with_status(status)
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)?;
        if self.attempts > 1 {
            write!(f, "（已尝试 {} 次）", self.attempts)?;
        }
        Ok(())
    }
}

impl std::error::Error for HttpError {}

impl From<HttpError> for TranslationError {
    fn from(err: HttpError) -> Self {
        TranslationError::new(err.kind.code(), err.to_string())
    }
}

/// 截断到 `max` 个字符（按字符而不是字节，避免劈开 UTF-8）。
pub(crate) fn truncate(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 状态码分类覆盖关键分支() {
        assert_eq!(HttpError::from_status(401, "").kind, HttpErrorKind::Auth);
        assert_eq!(HttpError::from_status(403, "").kind, HttpErrorKind::Auth);
        assert_eq!(
            HttpError::from_status(402, "").kind,
            HttpErrorKind::BudgetExceeded
        );
        assert_eq!(
            HttpError::from_status(429, "").kind,
            HttpErrorKind::RateLimited
        );
        assert_eq!(
            HttpError::from_status(503, "").kind,
            HttpErrorKind::Unavailable
        );
        assert_eq!(HttpError::from_status(400, "").kind, HttpErrorKind::Config);
    }

    #[test]
    fn 只有传输类错误可重试() {
        assert!(HttpErrorKind::RateLimited.is_retryable());
        assert!(HttpErrorKind::Unavailable.is_retryable());
        assert!(!HttpErrorKind::Auth.is_retryable());
        assert!(!HttpErrorKind::BudgetExceeded.is_retryable());
        assert!(!HttpErrorKind::Config.is_retryable());
        assert!(!HttpErrorKind::InvalidResponse.is_retryable());
    }

    #[test]
    fn 错误映射到不同错误码() {
        let auth: TranslationError = HttpError::new(HttpErrorKind::Auth, "x").into();
        let budget: TranslationError = HttpError::new(HttpErrorKind::BudgetExceeded, "x").into();
        assert_eq!(auth.code, TranslationErrorCode::AuthenticationFailed);
        assert_eq!(budget.code, TranslationErrorCode::BudgetExceeded);
        assert_ne!(auth.code, budget.code);
    }

    #[test]
    fn 截断不劈开多字节字符() {
        let text = "中文中文中文";
        let cut = truncate(text, 2);
        assert!(cut.starts_with("中文"));
        assert!(cut.chars().count() <= 3);
    }
}
