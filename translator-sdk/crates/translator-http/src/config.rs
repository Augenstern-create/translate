//! 后端配置：`translate.toml` 的 `[backend.<name>]` 段。
//!
//! # 设计约束（§9.2 / §12.1）
//!
//! - **密钥绝不写进配置文件**：只存"环境变量名"（`api_key_env`），值从进程环境读。
//!   这样配置文件可以被提交、被复制、被贴进 issue。
//! - 每个后端都有独立的 `id` / `model` / `model_revision` / 提示词版本，
//!   它们共同决定缓存身份：换模型必须让旧缓存失效（§11.2）。
//! - 不假设所有 OpenAI 兼容服务都支持同一批扩展字段：
//!   差异化字段通过 `extra_body` 显式给出，而不是猜。
//!
//! # 示例
//!
//! ```toml
//! [backend.remote]
//! enabled = true
//! base_url = "https://your-relay.example/v1"
//! model = "configured-model-id"
//! model_revision = "2026-11-01"
//! api_key_env = "TRANSLATE_API_KEY"
//!
//! [backend.local]
//! enabled = false
//! base_url = "http://127.0.0.1:8080/v1"
//! model = "local-translation-model"
//! api_key_env = ""
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::HttpError;

/// 默认的 API 密钥环境变量名（仅当配置未指定 `api_key_env` 且非空时使用）。
pub const DEFAULT_API_KEY_ENV: &str = "TRANSLATE_API_KEY";

/// 关闭"思考"的方式。不同 OpenAI 兼容服务的开关字段不同，不能靠猜。
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingStyle {
    /// 不发送任何额外字段（保持服务端默认）。
    #[default]
    None,
    /// vLLM / Qwen 一类：`chat_template_kwargs.enable_thinking = false`。
    ChatTemplate,
    /// OpenAI o 系列：`reasoning_effort = "none"`。
    ReasoningEffort,
    /// DeepSeek 风格：`thinking = {"type": "disabled"}`。
    ThinkingType,
}

impl ThinkingStyle {
    /// 返回该风格关闭思考时要合并进请求体的字段。
    pub fn disable_fields(self) -> Option<serde_json::Value> {
        match self {
            Self::None => None,
            Self::ChatTemplate => Some(serde_json::json!({
                "chat_template_kwargs": { "enable_thinking": false }
            })),
            Self::ReasoningEffort => Some(serde_json::json!({ "reasoning_effort": "none" })),
            Self::ThinkingType => Some(serde_json::json!({
                "thinking": { "type": "disabled" }
            })),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_timeout_ms() -> u64 {
    30_000
}
fn default_connect_timeout_ms() -> u64 {
    5_000
}
fn default_max_output_tokens() -> u32 {
    2048
}
fn default_max_retries() -> u32 {
    2
}
fn default_max_concurrent() -> usize {
    4
}
fn default_language_pairs() -> Vec<String> {
    vec!["en->zh-CN".to_string(), "en->zh-Hans".to_string()]
}
fn default_prompt_lang_pair() -> String {
    "en_zh".to_string()
}
fn default_prompt_precise() -> String {
    "precise_v1".to_string()
}
fn default_prompt_fast() -> String {
    "fast_v1".to_string()
}

/// 单个后端配置。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    /// 后端 id（`http.remote.deepseek` 里的一段）。留空时用 map 的键。
    #[serde(default)]
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// API 根地址，例如 `https://relay.example/v1` 或 `http://127.0.0.1:8080/v1`。
    /// 结尾带不带 `/chat/completions` 都可以。
    pub base_url: String,
    /// 服务端模型标识（必须与 `model_revision` 一起进缓存身份）。
    pub model: String,
    /// 部署版本标识。远程服务可能在别名不变时替换模型，因此**必须**显式给出
    /// （不可得时也要人工指定一个部署版本串）。
    #[serde(default)]
    pub model_revision: Option<String>,
    /// 存放密钥的环境变量名。**不存密钥本身。**
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// 整次请求（含读响应体）的总超时。
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// 建连超时。
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// 单次请求输出上限。
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    /// 采样温度。翻译默认 0.0（确定性越高越好）。
    #[serde(default)]
    pub temperature: f32,
    /// 传输层最多重试次数（首次之外）。
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// 支持的语言对，形如 `en->zh-CN`。
    #[serde(default = "default_language_pairs")]
    pub language_pairs: Vec<String>,
    /// 提示词目录；`None` 表示使用编译进二进制的内置提示词。
    #[serde(default)]
    pub prompt_dir: Option<PathBuf>,
    /// 提示词子目录（语言对），例如 `en_zh`。
    #[serde(default = "default_prompt_lang_pair")]
    pub prompt_lang_pair: String,
    #[serde(default = "default_prompt_precise")]
    pub prompt_precise: String,
    #[serde(default = "default_prompt_fast")]
    pub prompt_fast: String,
    /// 额外请求体字段（原样合并到顶层）。用于供应商特有参数。
    #[serde(default)]
    pub extra_body: Option<serde_json::Value>,
    /// 是否请求服务端关闭"思考"。
    #[serde(default)]
    pub disable_thinking: bool,
    /// 关闭思考所用字段风格；`disable_thinking = false` 时忽略。
    #[serde(default)]
    pub thinking_style: ThinkingStyle,
    /// 是否使用流式传输。**v1 固定 false**：未在真实服务上验证过流式解析，
    /// 明确拒绝而不是"看起来能用"。
    #[serde(default)]
    pub stream: bool,
    /// 单后端并发请求上限（保护本机与对端）。
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_requests: usize,
    /// 允许明文 http（默认只允许本机回环地址）。
    #[serde(default)]
    pub allow_insecure_http: bool,
    /// 输入计价（美元 / 百万 token），仅用于本地估算。
    #[serde(default)]
    pub price_input_per_million: Option<f64>,
    /// 输出计价（美元 / 百万 token）。
    #[serde(default)]
    pub price_output_per_million: Option<f64>,
}

impl BackendConfig {
    /// 校验配置。**构造后立即调用**，不要拖到第一次翻译。
    pub fn validate(&self) -> Result<(), HttpError> {
        if self.base_url.trim().is_empty() {
            return Err(HttpError::config("base_url 不能为空"));
        }
        if self.model.trim().is_empty() {
            return Err(HttpError::config("model 不能为空"));
        }
        let url = self.base_url.trim();
        let scheme_ok = url.starts_with("http://") || url.starts_with("https://");
        if !scheme_ok {
            return Err(HttpError::config(format!(
                "base_url 必须以 http:// 或 https:// 开头，收到：{url}"
            )));
        }
        if self.stream {
            return Err(HttpError::config(
                "stream = true 尚未在真实服务上验证；请先设为 false",
            ));
        }
        if self.timeout_ms == 0 {
            return Err(HttpError::config("timeout_ms 必须大于 0"));
        }
        if self.max_output_tokens == 0 {
            return Err(HttpError::config("max_output_tokens 必须大于 0"));
        }
        if self.max_concurrent_requests == 0 {
            return Err(HttpError::config("max_concurrent_requests 必须大于 0"));
        }
        if !url.starts_with("https://") && !self.allow_insecure_http && !is_loopback(url) {
            return Err(HttpError::config(format!(
                "非回环地址必须使用 https（{url}）。若确为内网明文服务，请显式设置 allow_insecure_http = true"
            )));
        }
        for pair in &self.language_pairs {
            if !pair.contains("->") {
                return Err(HttpError::config(format!(
                    "language_pairs 条目必须形如 en->zh-CN，收到：{pair}"
                )));
            }
        }
        if let Some(extra) = &self.extra_body {
            if !extra.is_object() {
                return Err(HttpError::config("extra_body 必须是 JSON 对象"));
            }
        }
        Ok(())
    }

    /// 完整的 chat completions 端点。
    pub fn endpoint(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }

    /// 解析后的密钥环境变量名（`None` 表示不发鉴权头）。
    pub fn api_key_env_name(&self) -> Option<&str> {
        match self.api_key_env.as_deref() {
            Some("") => None,
            Some(name) => Some(name),
            None => Some(DEFAULT_API_KEY_ENV),
        }
    }

    /// 从环境变量读取密钥。错误信息里**绝不**出现密钥本身。
    pub fn resolve_api_key(&self) -> Result<Option<String>, HttpError> {
        let Some(name) = self.api_key_env_name() else {
            return Ok(None);
        };
        match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => Ok(Some(value.trim().to_string())),
            Ok(_) => Err(HttpError::new(
                crate::errors::HttpErrorKind::Auth,
                format!("环境变量 {name} 为空，未配置可用的后端密钥"),
            )),
            Err(_) => Err(HttpError::new(
                crate::errors::HttpErrorKind::Auth,
                format!("环境变量 {name} 未设置，未配置可用的后端密钥"),
            )),
        }
    }

    /// 该后端是否声明支持某语言对。
    pub fn supports_pair(&self, source: &str, target: &str) -> bool {
        self.language_pairs
            .iter()
            .any(|pair| match pair.split_once("->") {
                Some((s, t)) => s.trim().eq_ignore_ascii_case(source) && t.trim() == target,
                None => false,
            })
    }

    /// 引擎 id：`http.<name>`。
    pub fn engine_id(&self, name: &str) -> String {
        if self.id.trim().is_empty() {
            format!("http.{name}")
        } else {
            format!("http.{}", self.id.trim())
        }
    }

    /// 提示词版本标识（用于缓存身份）。
    pub fn prompt_id(&self, fast: bool) -> String {
        let version = if fast {
            &self.prompt_fast
        } else {
            &self.prompt_precise
        };
        format!("{}/{}", self.prompt_lang_pair, version)
    }

    /// 采样/生成配置摘要（进缓存身份）。
    pub fn generation_fingerprint(&self) -> String {
        let extra = self
            .extra_body
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default();
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        use sha2::Digest as _;
        hasher.update(self.temperature.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(self.max_output_tokens.to_string().as_bytes());
        hasher.update(b"|");
        hasher.update(if self.disable_thinking { b"1" } else { b"0" });
        hasher.update(b"|");
        hasher.update(format!("{:?}", self.thinking_style).as_bytes());
        hasher.update(b"|");
        hasher.update(extra.as_bytes());
        let digest = hasher.finalize();
        hex16(&digest)
    }
}

fn is_loopback(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host = rest
        .split(['/', ':', '?'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host == "::1"
}

fn hex16(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(16);
    for b in bytes.iter().take(8) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 全局 HTTP 设置。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpSettings {
    /// 所有后端共享的并发上限（0 表示只受各后端自己的限制）。
    #[serde(default)]
    pub max_concurrent_requests: usize,
    /// User-Agent。
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
}

fn default_user_agent() -> String {
    format!("augenstern-translate/{}", env!("CARGO_PKG_VERSION"))
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            max_concurrent_requests: 0,
            user_agent: default_user_agent(),
        }
    }
}

/// 配置文件根结构。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackendsFile {
    #[serde(default)]
    pub http: HttpSettings,
    /// `[backend.<name>]`。用 BTreeMap 保证遍历顺序确定（缓存/诊断可复现）。
    #[serde(default)]
    pub backend: BTreeMap<String, BackendConfig>,
}

impl BackendsFile {
    pub fn from_toml_str(text: &str) -> Result<Self, HttpError> {
        let mut file: Self = toml::from_str(text)
            .map_err(|e| HttpError::config(format!("配置文件解析失败：{e}")))?;
        // 用 map 的键补齐空 id，避免两个后端都叫 `http.`
        for (name, backend) in file.backend.iter_mut() {
            if backend.id.trim().is_empty() {
                backend.id = name.clone();
            }
        }
        Ok(file)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, HttpError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| {
            HttpError::config(format!("读取配置文件 {} 失败：{e}", path.display()))
        })?;
        Self::from_toml_str(&text)
    }

    /// 取所有启用且通过校验的后端，按名字排序。
    pub fn enabled_backends(&self) -> Result<Vec<(&str, &BackendConfig)>, HttpError> {
        let mut out = Vec::new();
        for (name, backend) in &self.backend {
            if !backend.enabled {
                continue;
            }
            backend.validate()?;
            out.push((name.as_str(), backend));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> BackendConfig {
        BackendConfig {
            id: "remote.test".into(),
            enabled: true,
            base_url: "https://relay.example/v1".into(),
            model: "m".into(),
            model_revision: Some("2026-11-01".into()),
            api_key_env: Some("TRANSLATE_API_KEY".into()),
            timeout_ms: 30_000,
            connect_timeout_ms: 5_000,
            max_output_tokens: 2048,
            temperature: 0.0,
            max_retries: 2,
            language_pairs: default_language_pairs(),
            prompt_dir: None,
            prompt_lang_pair: "en_zh".into(),
            prompt_precise: "precise_v1".into(),
            prompt_fast: "fast_v1".into(),
            extra_body: None,
            disable_thinking: false,
            thinking_style: ThinkingStyle::None,
            stream: false,
            max_concurrent_requests: 4,
            allow_insecure_http: false,
            price_input_per_million: None,
            price_output_per_million: None,
        }
    }

    #[test]
    fn 端点拼接不重复() {
        let mut cfg = base();
        assert_eq!(
            cfg.endpoint(),
            "https://relay.example/v1/chat/completions"
        );
        cfg.base_url = "https://relay.example/v1/".into();
        assert_eq!(
            cfg.endpoint(),
            "https://relay.example/v1/chat/completions"
        );
        cfg.base_url = "https://relay.example/v1/chat/completions".into();
        assert_eq!(
            cfg.endpoint(),
            "https://relay.example/v1/chat/completions"
        );
    }

    #[test]
    fn 非回环明文地址默认被拒绝() {
        let mut cfg = base();
        cfg.base_url = "http://relay.example/v1".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.message.contains("https"), "{err}");

        cfg.base_url = "http://127.0.0.1:8080/v1".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn 流式未验证时直接拒绝() {
        let mut cfg = base();
        cfg.stream = true;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn 语言对匹配() {
        let cfg = base();
        assert!(cfg.supports_pair("en", "zh-CN"));
        assert!(cfg.supports_pair("EN", "zh-Hans"));
        assert!(!cfg.supports_pair("ja", "zh-CN"));
    }

    #[test]
    fn 配置解析使用_map_键补_id() {
        let file = BackendsFile::from_toml_str(
            r#"
[backend.remote]
base_url = "https://relay.example/v1"
model = "configured-model-id"

[backend.local]
base_url = "http://127.0.0.1:8080/v1"
model = "local-translation-model"
"#,
        )
        .unwrap();
        assert_eq!(file.backend["remote"].id, "remote");
        assert_eq!(file.backend["local"].id, "local");
        assert_eq!(file.backend["remote"].engine_id("remote"), "http.remote");
        // 默认值来自 serde default，而不是"空配置"
        assert_eq!(file.backend["remote"].timeout_ms, 30_000);
        assert_eq!(file.backend["remote"].max_output_tokens, 2048);
        assert!(!file.backend["remote"].stream);
    }

    #[test]
    fn 未知字段被拒绝而不是静默忽略() {
        // 拼错 `timeout_ms` 必须报错，否则用户以为设了超时其实没设
        let err = BackendsFile::from_toml_str(
            r#"
[backend.remote]
base_url = "https://relay.example/v1"
model = "m"
timeout_millis = 1000
"#,
        )
        .unwrap_err();
        assert!(err.message.contains("解析失败"), "{err}");
    }

    #[test]
    fn 生成配置指纹对参数变化敏感() {
        let cfg = base();
        let a = cfg.generation_fingerprint();
        let mut cfg2 = base();
        cfg2.temperature = 0.7;
        assert_ne!(a, cfg2.generation_fingerprint());

        let mut cfg3 = base();
        cfg3.extra_body = Some(serde_json::json!({"top_p": 0.9}));
        assert_ne!(a, cfg3.generation_fingerprint());
    }

    #[test]
    fn 密钥缺失时报鉴权错误而不是崩溃() {
        let mut cfg = base();
        cfg.api_key_env = Some("DEFINITELY_NOT_SET_ENV_VAR_12345".into());
        let err = cfg.resolve_api_key().unwrap_err();
        assert_eq!(err.kind, crate::errors::HttpErrorKind::Auth);
        // 错误信息里不能出现"密钥本身"（这里本来就没有），也不能是空话
        assert!(err.message.contains("DEFINITELY_NOT_SET_ENV_VAR_12345"));
    }

    #[test]
    fn 空环境变量名表示不鉴权() {
        let mut cfg = base();
        cfg.api_key_env = Some(String::new());
        assert!(cfg.resolve_api_key().unwrap().is_none());
    }
}
