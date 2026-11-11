//! # translator-http
//!
//! OpenAI 兼容 Chat Completions 传输后端。远程转发站与本地模型服务走**同一套**代码
//! （§15.1），因此"换后端"只是换配置，不是换架构。
//!
//! ```no_run
//! use translator_http::{BackendsFile, HttpTranslationEngine};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let text = r#"
//! [backend.remote]
//! base_url = "https://your-relay.example/v1"
//! model = "configured-model-id"
//! model_revision = "2026-11-01"
//! api_key_env = "TRANSLATE_API_KEY"
//! "#;
//! let file = BackendsFile::from_toml_str(text)?;
//! let engines = HttpTranslationEngine::from_file(&file)?;
//! assert_eq!(engines.len(), 1);
//! # Ok(())
//! # }
//! ```
//!
//! ## 安全
//!
//! 配置文件里**永远**只写环境变量名（`api_key_env`），不写密钥本身（§12.1）。
//! 错误信息、日志、`Debug` 输出都不包含密钥。
//!
//! ## 成本
//!
//! [`UsageMeter`] 只做本地估算；硬性额度必须由服务端执行（§12.2）。
//! 重试次数有界且共享同一个截止时间，避免"一次翻译无限次调用"（§12.3）。

pub mod client;
pub mod config;
pub mod engine;
pub mod errors;
pub mod prompt;
pub mod protocol;
pub mod usage;

pub use client::{HttpOutcome, HttpRequest, HttpTransport};
pub use config::{BackendConfig, BackendsFile, HttpSettings, ThinkingStyle, DEFAULT_API_KEY_ENV};
pub use engine::{engines_from_toml, HttpTranslationEngine};
pub use errors::{HttpError, HttpErrorKind};
pub use prompt::{PromptInput, PromptTemplate, EMBEDDED_FAST_V1, EMBEDDED_PRECISE_V1};
pub use protocol::{
    build_request_body, parse_response, sanitize_translation, ChatCompletion, PromptMessage,
};
pub use usage::{Pricing, UsageMeter, UsageSnapshot};
