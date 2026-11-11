//! HTTP 翻译引擎：把 OpenAI 兼容的 Chat Completions 服务包成 [`TranslationEngine`]。
//!
//! 同一个实现同时服务两种部署（§15.1）：
//!
//! ```text
//! Translation SDK → HTTP API → 远程转发站 / 本地模型服务 → 模型
//! ```
//!
//! 因此"换后端"不改变 SDK、不改变协调器、不改变缓存语义：只换配置。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use translator_plugin_api::{EngineOutput, TranslationEngine, TranslationJob};
use translator_types::{
    CancelToken, Capability, EngineDescriptor, LanguageTag, TranslateRequest, TranslationError,
    TranslationErrorCode, TranslationMode,
};

use crate::client::{HttpRequest, HttpTransport};
use crate::config::{BackendConfig, BackendsFile, HttpSettings};
use crate::errors::{HttpError, HttpErrorKind};
use crate::prompt::{PromptInput, PromptTemplate};
use crate::protocol::{build_request_body, parse_response};
use crate::usage::{Pricing, UsageMeter, UsageSnapshot};

/// 引擎实现。
pub struct HttpTranslationEngine {
    id: String,
    config: BackendConfig,
    transport: HttpTransport,
    precise: PromptTemplate,
    fast: PromptTemplate,
    meter: Arc<UsageMeter>,
    /// 请求未显式指定源语言时使用的默认值（本引擎只声明 en→zh 语言对）。
    default_source: LanguageTag,
}

impl std::fmt::Debug for HttpTranslationEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTranslationEngine")
            .field("id", &self.id)
            .field("endpoint", &self.config.endpoint())
            .field("model", &self.config.model)
            .field("model_revision", &self.config.model_revision)
            .field("prompt", &self.precise.id())
            .finish()
    }
}

impl HttpTranslationEngine {
    /// 由配置构造（会立刻校验配置与提示词，而不是等到第一次翻译）。
    pub fn new(config: BackendConfig) -> Result<Self, HttpError> {
        config.validate()?;
        let id = config.engine_id(&config.id);
        let precise = PromptTemplate::for_mode(&config, TranslationMode::Precise)?;
        let fast = PromptTemplate::for_mode(&config, TranslationMode::Fast)?;

        let settings = HttpSettings::default();
        let meter = UsageMeter::new();
        let transport = HttpTransport::new(
            &settings,
            config.max_retries,
            config.max_concurrent_requests,
            Duration::from_millis(config.connect_timeout_ms),
            Arc::clone(&meter),
        );

        Ok(Self {
            id,
            config,
            transport,
            precise,
            fast,
            meter,
            default_source: LanguageTag::new("en")
                .expect("常量语言标签 en 一定合法"),
        })
    }

    /// 从配置文件构造所有启用的后端。
    pub fn from_config_file(path: impl AsRef<Path>) -> Result<Vec<Self>, HttpError> {
        let file = BackendsFile::from_file(path)?;
        Self::from_file(&file)
    }

    /// 从已解析的配置构造所有启用的后端（按配置文件名排序，顺序确定）。
    pub fn from_file(file: &BackendsFile) -> Result<Vec<Self>, HttpError> {
        let mut engines = Vec::new();
        for (_, backend) in file.enabled_backends()? {
            engines.push(Self::new(backend.clone())?);
        }
        Ok(engines)
    }

    pub fn config(&self) -> &BackendConfig {
        &self.config
    }

    /// 用量快照（成本提示用）。
    pub fn usage(&self) -> UsageSnapshot {
        self.meter.snapshot()
    }

    pub fn reset_usage(&self) {
        self.meter.reset();
    }

    /// 单价配置（未配置时 `None`，不做任何默认价格假设）。
    pub fn pricing(&self) -> Option<Pricing> {
        Pricing::from_option(
            self.config.price_input_per_million,
            self.config.price_output_per_million,
        )
    }

    /// 估算当前累计费用（美元）。
    pub fn estimated_cost_usd(&self) -> Option<f64> {
        self.usage().estimate_cost_usd(self.pricing())
    }

    fn prompt_for(&self, mode: TranslationMode) -> &PromptTemplate {
        if mode.is_precise() {
            &self.precise
        } else {
            &self.fast
        }
    }

    /// 真实执行一次翻译。
    async fn run(&self, job: &TranslationJob<'_>) -> Result<EngineOutput, TranslationError> {
        if let Some(token) = job.cancel {
            if token.is_cancelled() {
                return Err(TranslationError::cancelled());
            }
        }
        if let Some(deadline) = job.deadline {
            if Instant::now() >= deadline {
                return Err(TranslationError::new(
                    TranslationErrorCode::BackendTimeout,
                    "请求在调用后端前已超过截止时间",
                ));
            }
        }

        let prompt = self.prompt_for(job.mode);
        let target = job.target.as_str();
        let messages = prompt.build_messages(
            &PromptInput {
                text: job.text,
                glossary: job.glossary,
                placeholders: job.placeholders,
                retry_hint: job.retry_hint,
            },
            target,
        );

        let max_tokens = job
            .max_output_tokens
            .unwrap_or(self.config.max_output_tokens);
        let body = build_request_body(&self.config, &messages, max_tokens);

        let api_key = self.config.resolve_api_key()?;

        let request = HttpRequest {
            url: self.config.endpoint(),
            api_key,
            body,
            timeout: Duration::from_millis(self.config.timeout_ms),
        };

        let cancel = job.cancel.cloned();
        let outcome = self.transport.send(request, cancel, job.deadline).await?;
        let completion = parse_response(outcome.status, &outcome.body)?;

        if let Some(usage) = completion.usage {
            self.meter.record_tokens(usage);
        }

        // 模型身份：配置里显式声明的部署版本优先；否则用服务端回报的实际模型名。
        let model_revision = self
            .config
            .model_revision
            .clone()
            .or_else(|| completion.model.clone());

        Ok(EngineOutput {
            text: completion.text,
            model_id: Some(completion.model.unwrap_or_else(|| self.config.model.clone())),
            model_revision,
            usage: completion.usage,
            warnings: Vec::new(),
        })
    }

    fn job_from_request<'a>(
        &'a self,
        req: &'a TranslateRequest,
        placeholders: &'a [String],
        glossary: &'a [translator_types::TermConstraint],
        cancel: Option<&'a CancelToken>,
    ) -> TranslationJob<'a> {
        let deadline = req
            .timeout_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms));
        TranslationJob {
            text: &req.text,
            original_text: &req.text,
            // 未解析源语言时按英语处理：本引擎只声明 en→zh 语言对。
            source: req.source.as_ref().unwrap_or(&self.default_source),
            target: &req.target,
            mode: TranslationMode::Precise,
            text_kind: translator_types::TextKind::Sentence,
            glossary,
            placeholders,
            retry_hint: None,
            deadline,
            cancel,
            max_output_tokens: None,
        }
    }
}

impl TranslationEngine for HttpTranslationEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor::new(
            self.id.clone(),
            [Capability::Nmt, Capability::Remote],
            50,
            "Apache-2.0 OR MIT",
        )
    }

    fn supports(&self, source: &LanguageTag, target: &LanguageTag) -> bool {
        self.config.supports_pair(source.as_str(), target.as_str())
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> translator_plugin_api::BoxFuture<'a, Result<translator_types::TranslationResult, TranslationError>>
    {
        Box::pin(async move {
            let job = self.job_from_request(req, &[], &[], None);
            let out = self.run(&job).await?;
            Ok(translator_types::TranslationResult {
                text: out.text,
                source: req
                    .source
                    .clone()
                    .unwrap_or_else(|| self.default_source.clone()),
                target: req.target.clone(),
                engine_id: self.id.clone(),
                model_id: out.model_id,
                elapsed_ms: 0,
                warnings: out.warnings,
            })
        })
    }

    fn translate_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> translator_plugin_api::BoxFuture<'a, Result<translator_types::TranslationResult, TranslationError>>
    {
        Box::pin(async move {
            let job = self.job_from_request(req, &[], &[], Some(cancel));
            let out = self.run(&job).await?;
            Ok(translator_types::TranslationResult {
                text: out.text,
                source: req
                    .source
                    .clone()
                    .unwrap_or_else(|| self.default_source.clone()),
                target: req.target.clone(),
                engine_id: self.id.clone(),
                model_id: out.model_id,
                elapsed_ms: 0,
                warnings: out.warnings,
            })
        })
    }

    /// 缓存身份：模型 + 部署版本 + 提示词版本 + 生成参数。
    ///
    /// 少了任何一项都会出现"换了模型/提示词却命中旧译文"（§11.2）。
    fn cache_identity(&self) -> Option<String> {
        Some(format!(
            "{}|{}|{}|{}",
            self.config.model,
            self.config
                .model_revision
                .as_deref()
                .unwrap_or("<unversioned>"),
            self.precise.id(),
            self.config.generation_fingerprint()
        ))
    }

    /// 协调器负责加保护标记；本引擎**不**自己切分受保护跨度。
    fn handles_protected_spans(&self) -> bool {
        false
    }

    fn translate_job<'a>(
        &'a self,
        job: &'a TranslationJob<'a>,
    ) -> translator_plugin_api::BoxFuture<'a, Result<EngineOutput, TranslationError>> {
        Box::pin(async move { self.run(job).await })
    }
}

/// 便捷构造：从 TOML 文本得到引擎（测试与嵌入式宿主使用）。
pub fn engines_from_toml(text: &str) -> Result<Vec<HttpTranslationEngine>, HttpError> {
    let file = BackendsFile::from_toml_str(text)?;
    HttpTranslationEngine::from_file(&file)
}

/// 把 HTTP 错误转成统一错误（对外便利函数）。
pub fn http_error_to_translation(err: HttpError) -> TranslationError {
    err.into()
}

/// 错误类别到错误码的映射表（文档/测试共用）。
pub fn error_code_for(kind: HttpErrorKind) -> TranslationErrorCode {
    kind.code()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_toml(base_url: &str) -> String {
        format!(
            r#"
[backend.test]
base_url = "{base_url}"
model = "configured-model-id"
model_revision = "2026-11-01"
api_key_env = ""
max_retries = 0
max_concurrent_requests = 1
"#
        )
    }

    #[test]
    fn 构造时立刻校验配置() {
        let err = engines_from_toml(&config_toml("ftp://nope")).unwrap_err();
        assert_eq!(err.kind, HttpErrorKind::Config);
    }

    #[test]
    fn 引擎声明语言对与能力() {
        let engines = engines_from_toml(&config_toml("http://127.0.0.1:9/v1")).unwrap();
        assert_eq!(engines.len(), 1);
        let engine = &engines[0];
        let descriptor = engine.descriptor();
        assert!(descriptor.supports(Capability::Nmt));
        assert!(descriptor.supports(Capability::Remote));
        assert_eq!(descriptor.id, "http.test");
        assert!(engine.supports(
            &LanguageTag::new("en").unwrap(),
            &LanguageTag::new("zh-CN").unwrap()
        ));
        assert!(!engine.supports(
            &LanguageTag::new("ja").unwrap(),
            &LanguageTag::new("zh-CN").unwrap()
        ));
        assert!(!engine.handles_protected_spans());
    }

    #[test]
    fn 缓存身份包含模型版本与提示词版本() {
        let engines = engines_from_toml(&config_toml("http://127.0.0.1:9/v1")).unwrap();
        let identity = engines[0].cache_identity().unwrap();
        assert!(identity.contains("configured-model-id"));
        assert!(identity.contains("2026-11-01"));
        assert!(identity.contains("precise_v1"));
    }

    #[test]
    fn 默认不假设价格() {
        let engines = engines_from_toml(&config_toml("http://127.0.0.1:9/v1")).unwrap();
        assert_eq!(engines[0].estimated_cost_usd(), None);
    }
}
