//! 平台门禁：哪些平台有"被验证过"的实现。
//!
//! 为什么要单独一个模块（P3.4）：原来这段判断写成 `#[cfg(target_os = "linux")]`
//! 的两条分支，**在 Linux 上编译时非 Linux 分支直接被裁掉**，于是"非 Linux 会明确报
//! `CapabilityUnavailable`"这句话没有任何测试能覆盖。改成接收 `os: &str` 的纯函数后，
//! 平台判定可以在 Linux 上被单测。
//!
//! 判定表是**证据表**，不是"能不能编译"：`std::process` 到处都能编译，
//! 但终止升级、`/proc` 采样、`kill -9` 恢复这些语义只在 Linux 上实测过（docs/12 §3.1）。

use crate::error::RuntimeError;

/// 有实测证据的平台（`std::env::consts::OS` 的取值）。
pub const VERIFIED_PLATFORMS: &[&str] = &["linux"];

/// 某个平台的支持状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformSupport {
    /// 有实测证据，可以正常使用。
    Verified,
    /// 没有证据；调用会被显式拒绝，而不是给一个没测过的实现。
    Unverified { os: String },
}

impl PlatformSupport {
    /// 是否可用。
    pub fn is_supported(&self) -> bool {
        matches!(self, PlatformSupport::Verified)
    }

    /// 人话解释（给日志/诊断用）。
    pub fn reason(&self) -> String {
        match self {
            PlatformSupport::Verified => "该平台有实测证据".to_string(),
            PlatformSupport::Unverified { os } => format!(
                "进程隔离 worker 只在 Linux 上验证过（终止升级、/proc 采样、kill -9 恢复都依赖 Linux 语义）；\
                 当前平台 {os} 暂不可用。SDK 在这里返回 CapabilityUnavailable，而不是启用未验证的实现。"
            ),
        }
    }
}

/// 按 `os` 判定支持状态（`os` 用 [`std::env::consts::OS`] 的取值：`linux` / `windows` / `macos` / `android` …）。
pub fn support_for(os: &str) -> PlatformSupport {
    if VERIFIED_PLATFORMS.contains(&os) {
        PlatformSupport::Verified
    } else {
        PlatformSupport::Unverified { os: os.to_string() }
    }
}

/// 当前运行平台的支持状态。
pub fn current() -> PlatformSupport {
    support_for(std::env::consts::OS)
}

/// 按 `os` 判定；不支持的平台返回 [`RuntimeError::CapabilityUnavailable`]。
pub fn ensure_supported_on(os: &str) -> Result<(), RuntimeError> {
    match support_for(os) {
        PlatformSupport::Verified => Ok(()),
        support @ PlatformSupport::Unverified { .. } => {
            Err(RuntimeError::CapabilityUnavailable(support.reason()))
        }
    }
}

/// 当前平台门禁（真实路径）。
pub fn ensure_supported() -> Result<(), RuntimeError> {
    ensure_supported_on(std::env::consts::OS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_types::TranslationErrorCode;

    #[test]
    fn linux有实测证据() {
        assert!(support_for("linux").is_supported());
        assert!(ensure_supported_on("linux").is_ok());
        assert_eq!(
            support_for("linux"),
            PlatformSupport::Verified,
            "linux 必须是 Verified，否则整个 P3 的结论都不成立"
        );
    }

    #[test]
    fn 其他平台明确报能力不可用而不是静默降级() {
        for os in ["windows", "macos", "android", "ios", "freebsd", "openbsd"] {
            let support = support_for(os);
            assert!(!support.is_supported(), "{os} 不该被当作已验证平台");
            assert_eq!(support, PlatformSupport::Unverified { os: os.to_string() });
            let err = ensure_supported_on(os).expect_err("未验证平台必须报错");
            assert_eq!(
                err.code(),
                TranslationErrorCode::CapabilityUnavailable,
                "{os} 的错误码必须是 CapabilityUnavailable"
            );
            let text = err.to_string();
            assert!(text.contains(os), "错误信息要说明是哪个平台：{text}");
            assert!(
                text.contains("Linux"),
                "错误信息要说明为什么（只在 Linux 验证过）：{text}"
            );
        }
    }

    #[test]
    fn 证据表只有linux() {
        // 收紧这里等于"宣布支持新平台"，必须附带该平台的真机证据
        const { assert!(VERIFIED_PLATFORMS.len() == 1) };
        assert_eq!(VERIFIED_PLATFORMS[0], "linux");
    }

    #[test]
    fn 当前平台与编译目标一致() {
        let support = current();
        if cfg!(target_os = "linux") {
            assert!(support.is_supported(), "{support:?}");
            assert!(ensure_supported().is_ok());
        } else {
            assert!(!support.is_supported(), "{support:?}");
        }
    }
}
