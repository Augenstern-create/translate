//! 面向宿主程序的翻译 SDK。
//!
//! # 只有两个动作
//!
//! ```no_run
//! # use translator_api::Translator;
//! # use translator_types::{LanguageTag, LookupRequest, TranslateRequest};
//! # async fn demo(t: &Translator) -> Result<(), Box<dyn std::error::Error>> {
//! let zh = LanguageTag::new("zh-CN")?;
//! let result = t.translate(&TranslateRequest::new("Hello.", zh.clone())).await?;
//! println!("{}", result.text);
//!
//! let entries = t.lookup(&LookupRequest::new("hello")).await?;
//! # let _ = entries;
//! # Ok(())
//! # }
//! ```
//!
//! 句子翻译与查词是**两种能力**（§7.2）：`translate()` 不会先查词典，
//! 也不会因为输入很短就自动改走词典。由宿主根据交互形式决定调用哪个。
//!
//! # 装配
//!
//! ```no_run
//! # use translator_api::Translator;
//! # fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let translator = Translator::builder()
//!     .http_config_file("translate.toml")?   // [backend.remote] / [backend.local]
//!     .glossary_file("glossary.json")?       // 可选
//!     .templates_file("templates.json")?     // 可选
//!     .cache_capacity(512)
//!     .build()?;
//! # let _ = translator;
//! # Ok(())
//! # }
//! ```
//!
//! 构建期就会校验配置与数据文件：错在装配阶段暴露，而不是第一次翻译时。

pub use translator_core::{
    BackendRouter, CacheStats, Coordinator, CoordinatorConfig, Glossary, GlossaryError, MatchMode,
    NoRules, NoTermGuard, Piece, ProtectedKind, ProtectedSpan, ProtectionPolicy,
    ProtectionStrategy, RuleOutcome, RulePreview, SentenceTemplates, SpanPolicy,
    StrategyPreference, TemplateScope, TemplateTable, TermEntry, TermPolicy, TermVerification,
    TextRules, TranslateOutput, TranslationDiagnostics,
};
pub use translator_dictionary as dictionary;
pub use translator_http::{
    engines_from_toml, BackendConfig, BackendsFile, HttpError, HttpErrorKind,
    HttpTranslationEngine, Pricing, PromptTemplate, ThinkingStyle, UsageSnapshot,
};
pub use translator_plugin_api::{
    DictionaryProvider, EngineOutput, InferenceBackend, LoadedModel, ModelIdentity, PluginRegistry,
    TranslationEngine, TranslationJob,
};
pub use translator_rules;
pub use translator_rules as rules;
pub use translator_types::*;

/// 插件 Mock（测试与示例用）。
#[cfg(feature = "mocks")]
pub use translator_plugin_api::mocks;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use translator_core::language::{HeuristicIdentifier, LanguageIdentifier, MorphologyAnalyzer};

/// 装配好的翻译器。
pub struct Translator {
    coordinator: Arc<Coordinator>,
    /// HTTP 后端（用于取用量/成本统计）。
    http_engines: Vec<Arc<HttpTranslationEngine>>,
    /// 已挂载的后端 id（按注册顺序）。
    backend_ids: Vec<String>,
    /// 默认执行选项（模式与文本类型）。
    default_options: TranslateOptions,
}

impl std::fmt::Debug for Translator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Translator")
            .field("backends", &self.backend_ids)
            .field("default_mode", &self.default_options.mode)
            .field("cache", &self.coordinator.cache_stats())
            .finish()
    }
}

impl Translator {
    pub fn builder() -> TranslatorBuilder {
        TranslatorBuilder::new()
    }

    /// 翻译（默认选项）。
    pub async fn translate(
        &self,
        req: &TranslateRequest,
    ) -> Result<TranslationResult, TranslationError> {
        self.coordinator
            .translate_with_options(req, &self.default_options)
            .await
    }

    /// 翻译（指定模式）。
    pub async fn translate_with_mode(
        &self,
        req: &TranslateRequest,
        mode: TranslationMode,
    ) -> Result<TranslationResult, TranslationError> {
        let mut options = self.default_options.clone();
        options.mode = mode;
        self.coordinator.translate_with_options(req, &options).await
    }

    /// 翻译（指定选项）。
    pub async fn translate_with_options(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
    ) -> Result<TranslationResult, TranslationError> {
        self.coordinator.translate_with_options(req, options).await
    }

    /// 翻译 + 诊断（指定选项）。
    pub async fn translate_with_diagnostics_options(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
    ) -> Result<TranslateOutput, TranslationError> {
        self.coordinator
            .translate_with_diagnostics_options(req, options)
            .await
    }

    /// 翻译 + 诊断。
    pub async fn translate_with_diagnostics(
        &self,
        req: &TranslateRequest,
    ) -> Result<TranslateOutput, TranslationError> {
        self.coordinator
            .translate_with_diagnostics_options(req, &self.default_options)
            .await
    }

    /// 可取消翻译。
    pub async fn translate_cancellable(
        &self,
        req: &TranslateRequest,
        cancel: &CancelToken,
    ) -> Result<TranslationResult, TranslationError> {
        self.coordinator
            .translate_cancellable_with_mode(req, cancel, self.default_options.mode)
            .await
    }

    /// 查词（本地词典）。
    pub async fn lookup(
        &self,
        req: &LookupRequest,
    ) -> Result<Vec<DictionaryEntry>, TranslationError> {
        self.coordinator.lookup(req).await
    }

    /// 语言识别（返回按置信度降序的候选）。
    pub fn detect_language(&self, text: &str, limit: usize) -> Vec<LanguageGuess> {
        HeuristicIdentifier.identify(text, limit)
    }

    pub fn capabilities(&self) -> Vec<CapabilityInfo> {
        self.coordinator.capabilities()
    }

    pub fn explain_route(&self, req: &TranslateRequest) -> Result<RouteDecision, TranslationError> {
        self.coordinator.explain_route(req)
    }

    pub fn clear_cache(&self) {
        self.coordinator.clear_cache();
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.coordinator.cache_stats()
    }

    pub fn route_stats(&self) -> RouteStats {
        self.coordinator.route_stats()
    }

    pub fn reset_route_stats(&self) {
        self.coordinator.reset_route_stats();
    }

    /// 已挂载的后端 id。
    pub fn backend_ids(&self) -> &[String] {
        &self.backend_ids
    }

    /// HTTP 后端列表（用量/成本统计）。
    pub fn http_engines(&self) -> &[Arc<HttpTranslationEngine>] {
        &self.http_engines
    }

    /// 汇总用量（所有 HTTP 后端）。
    pub fn usage(&self) -> Vec<(String, UsageSnapshot)> {
        self.http_engines
            .iter()
            .map(|engine| (engine.config().id.clone(), engine.usage()))
            .collect()
    }

    /// 估算费用（美元）。未配置单价的后端返回 `None`。
    pub fn estimated_cost_usd(&self) -> Option<f64> {
        let mut total = 0.0;
        let mut any = false;
        for engine in &self.http_engines {
            if let Some(cost) = engine.estimated_cost_usd() {
                total += cost;
                any = true;
            }
        }
        any.then_some(total)
    }

    /// 默认选项（只读）。
    pub fn default_options(&self) -> &TranslateOptions {
        &self.default_options
    }

    /// 一行配置摘要（不含密钥）。
    pub fn config_summary(&self) -> String {
        format!(
            "后端=[{}] 模式={} 缓存={}/{} 词表={}",
            self.backend_ids.join(", "),
            self.default_options.mode,
            self.coordinator.cache_stats().len,
            self.coordinator.cache_stats().capacity,
            self.coordinator
                .glossary()
                .map(|g| format!("{}@{}", g.id(), g.version()))
                .unwrap_or_else(|| "无".to_string())
        )
    }
}

/// 装配器。
///
/// 所有 `*_file` 方法都会**立即**读取并校验文件：配置错误在启动时暴露。
pub struct TranslatorBuilder {
    registry: PluginRegistry,
    config: CoordinatorConfig,
    http_engines: Vec<Arc<HttpTranslationEngine>>,
    default_mode: TranslationMode,
    default_text_kind: TextKind,
    dictionary_root: Option<PathBuf>,
    dictionary_label: Option<String>,
    glossary: Option<Arc<Glossary>>,
    templates: Option<Arc<dyn SentenceTemplates>>,
    rules: Option<Arc<dyn TextRules>>,
    lemmatizer: Option<Arc<dyn MorphologyAnalyzer>>,
    language_identifier: Option<Arc<dyn LanguageIdentifier>>,
}

impl Default for TranslatorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TranslatorBuilder {
    pub fn new() -> Self {
        Self {
            registry: PluginRegistry::new(),
            config: CoordinatorConfig::default(),
            http_engines: Vec::new(),
            default_mode: TranslationMode::Precise,
            default_text_kind: TextKind::Auto,
            dictionary_root: None,
            dictionary_label: None,
            glossary: None,
            templates: None,
            rules: None,
            lemmatizer: None,
            language_identifier: None,
        }
    }

    // ---------------- 后端 ----------------

    /// 从 `translate.toml` 装配所有启用的 HTTP 后端。
    pub fn http_config_file(mut self, path: impl AsRef<Path>) -> Result<Self, TranslationError> {
        let file = BackendsFile::from_file(path).map_err(http_config_error)?;
        self = self.http_backends_from(&file)?;
        Ok(self)
    }

    /// 从已解析的后端配置装配。
    pub fn http_backends(mut self, file: &BackendsFile) -> Result<Self, TranslationError> {
        self = self.http_backends_from(file)?;
        Ok(self)
    }

    /// 从 TOML 文本装配（嵌入式宿主/测试）。
    pub fn http_config_toml(mut self, toml: &str) -> Result<Self, TranslationError> {
        let file = BackendsFile::from_toml_str(toml).map_err(http_config_error)?;
        self = self.http_backends_from(&file)?;
        Ok(self)
    }

    fn http_backends_from(mut self, file: &BackendsFile) -> Result<Self, TranslationError> {
        for (_, backend) in file.enabled_backends().map_err(http_config_error)? {
            let engine = HttpTranslationEngine::new(backend.clone()).map_err(http_config_error)?;
            self = self.http_backend(engine)?;
        }
        Ok(self)
    }

    /// 挂载一个已构造的 HTTP 后端。
    pub fn http_backend(mut self, engine: HttpTranslationEngine) -> Result<Self, TranslationError> {
        let engine = Arc::new(engine);
        self.registry
            .register_engine(engine.clone() as Arc<dyn TranslationEngine>)?;
        self.http_engines.push(engine);
        Ok(self)
    }

    /// 注册任意引擎（测试/自定义后端）。
    pub fn engine(mut self, engine: Arc<dyn TranslationEngine>) -> Result<Self, TranslationError> {
        self.registry.register_engine(engine)?;
        Ok(self)
    }

    /// 注册词典提供者。
    pub fn provider(
        mut self,
        provider: Arc<dyn DictionaryProvider>,
    ) -> Result<Self, TranslationError> {
        self.registry.register_provider(provider)?;
        Ok(self)
    }

    /// 挂载离线词典包（同时注册词典引擎与 provider）。
    pub fn dictionary_package(
        mut self,
        root: impl Into<PathBuf>,
    ) -> Result<Self, TranslationError> {
        let root = root.into();
        let (engine, provider) = translator_dictionary::open_as_plugins(&root).map_err(|e| {
            TranslationError::new(
                TranslationErrorCode::DictionaryUnavailable,
                format!("打开词典包 {} 失败：{e}", root.display()),
            )
        })?;
        let label = format!(
            "{}@{}",
            engine.package().manifest().id,
            engine.package().manifest().version
        );
        self.registry.register_engine(engine)?;
        self.registry.register_provider(provider)?;
        self.dictionary_root = Some(root);
        self.dictionary_label = Some(label);
        Ok(self)
    }

    // ---------------- 术语 / 模板 / 规则 ----------------

    pub fn glossary(mut self, glossary: Arc<Glossary>) -> Self {
        self.glossary = Some(glossary);
        self
    }

    pub fn glossary_file(mut self, path: impl AsRef<Path>) -> Result<Self, TranslationError> {
        let glossary = Glossary::from_file(path).map_err(|e| {
            TranslationError::new(
                TranslationErrorCode::ConfigurationError,
                format!("术语表加载失败：{e}"),
            )
        })?;
        self.glossary = Some(Arc::new(glossary));
        Ok(self)
    }

    pub fn term_policy(mut self, policy: TermPolicy) -> Self {
        self.config.term_policy = policy;
        self
    }

    pub fn templates(mut self, templates: Arc<dyn SentenceTemplates>) -> Self {
        self.templates = Some(templates);
        self
    }

    pub fn templates_file(mut self, path: impl AsRef<Path>) -> Result<Self, TranslationError> {
        let table = TemplateTable::from_file(path).map_err(|e| {
            TranslationError::new(
                TranslationErrorCode::ConfigurationError,
                format!("模板加载失败：{e}"),
            )
        })?;
        self.templates = Some(table.as_arc());
        Ok(self)
    }

    pub fn template_scope(mut self, scope: TemplateScope) -> Self {
        self.config.template_scope = scope;
        self
    }

    pub fn rules(mut self, rules: Arc<dyn TextRules>) -> Self {
        self.rules = Some(rules);
        self
    }

    /// 加载声明式规则包（JSON 或编译后的二进制）。
    pub fn rules_file(mut self, path: impl AsRef<Path>) -> Result<Self, TranslationError> {
        let bytes = std::fs::read(path.as_ref()).map_err(|e| {
            TranslationError::new(
                TranslationErrorCode::ConfigurationError,
                format!("读取规则包失败：{e}"),
            )
        })?;
        let pack = translator_rules::RuleEngine::from_bytes(&bytes).map_err(|e| {
            TranslationError::new(
                TranslationErrorCode::ConfigurationError,
                format!("规则包加载失败：{e}"),
            )
        })?;
        self.rules = Some(Arc::new(pack));
        Ok(self)
    }

    // ---------------- 行为配置 ----------------

    pub fn translation_mode(mut self, mode: TranslationMode) -> Self {
        self.default_mode = mode;
        self
    }

    pub fn text_kind(mut self, kind: TextKind) -> Self {
        self.default_text_kind = kind;
        self
    }

    pub fn cache_capacity(mut self, capacity: usize) -> Self {
        self.config.cache_capacity = capacity;
        self
    }

    /// 关键完整性失败时是否拒绝返回译文。
    ///
    /// 默认 `true`。设为 `false` 只用于诊断/研究，**不得**用于产品。
    pub fn strict_integrity(mut self, strict: bool) -> Self {
        self.config.strict_integrity = strict;
        self
    }

    /// 内容完整性失败后的额外尝试次数（默认 1）。
    pub fn content_retry(mut self, retries: u32) -> Self {
        self.config.content_retry = retries;
        self
    }

    pub fn remote_fallback(mut self, policy: RemoteFallbackPolicy) -> Self {
        self.config.remote_fallback = policy;
        self
    }

    pub fn preferred_backend(mut self, id: impl Into<String>) -> Self {
        self.config.preferred_backend = Some(id.into());
        self
    }

    pub fn protection_policy(mut self, policy: ProtectionPolicy) -> Self {
        self.config.protection = policy;
        self
    }

    pub fn privacy_scope(mut self, scope: impl Into<String>) -> Self {
        self.config.privacy_scope = scope.into();
        self
    }

    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.config.domain = Some(domain.into());
        self
    }

    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.config.default_timeout_ms = ms;
        self
    }

    pub fn max_input_chars(mut self, chars: usize) -> Self {
        self.config.max_input_chars = chars;
        self
    }

    pub fn lemmatizer(mut self, lemmatizer: Arc<dyn MorphologyAnalyzer>) -> Self {
        self.lemmatizer = Some(lemmatizer);
        self
    }

    pub fn language_identifier(mut self, identifier: Arc<dyn LanguageIdentifier>) -> Self {
        self.language_identifier = Some(identifier);
        self
    }

    /// 装配。
    pub fn build(mut self) -> Result<Translator, TranslationError> {
        if self.registry.engines().is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::ConfigurationError,
                "没有可用的翻译后端：请配置 [backend.<name>] 或注册引擎",
            ));
        }

        self.config.glossary = self.glossary.take();
        self.config.templates = self.templates.take();
        self.config.rules = self.rules.take();
        self.config.lemmatizer = self.lemmatizer.take();
        self.config.language_identifier = self.language_identifier.take();
        self.config.default_mode = self.default_mode;

        // 资源指纹：词典/术语/模板/规则版本必须进缓存键
        let mut fingerprint: Vec<String> = Vec::new();
        if let Some(label) = &self.dictionary_label {
            fingerprint.push(format!("dict:{label}"));
        }
        if let Some(glossary) = &self.config.glossary {
            fingerprint.push(format!("glossary:{}@{}", glossary.id(), glossary.version()));
        }
        if let Some(templates) = &self.config.templates {
            fingerprint.push(format!(
                "templates:{}@{}",
                templates.id(),
                templates.version()
            ));
        }
        if let Some(rules) = &self.config.rules {
            fingerprint.push(format!("rules:{}@{}", rules.id(), rules.version()));
        }
        self.config.resource_fingerprint = fingerprint.join(";");

        let backend_ids: Vec<String> = self
            .registry
            .engines()
            .iter()
            .map(|e| e.descriptor().id)
            .collect();

        let mut options = TranslateOptions::new(self.default_mode);
        options.text_kind = self.default_text_kind;

        Ok(Translator {
            coordinator: Arc::new(Coordinator::new(Arc::new(self.registry), self.config)),
            http_engines: self.http_engines,
            backend_ids,
            default_options: options,
        })
    }
}

fn http_config_error(err: HttpError) -> TranslationError {
    TranslationError::new(err.kind.code(), err.to_string())
}

/// 常用类型的再导出。
pub mod prelude {
    pub use crate::Translator;
    pub use translator_types::{
        CancelToken, LanguageTag, LookupRequest, TextKind, TranslateOptions, TranslateRequest,
        TranslationError, TranslationErrorCode, TranslationMode, TranslationPolicy,
        TranslationResult,
    };
}

/// 便捷：英语 → 简体中文的请求。
pub fn en_to_zh(text: impl Into<String>) -> Result<TranslateRequest, TranslationError> {
    let zh = LanguageTag::new("zh-CN")?;
    let en = LanguageTag::new("en")?;
    Ok(TranslateRequest::new(text, zh).with_source(en))
}
