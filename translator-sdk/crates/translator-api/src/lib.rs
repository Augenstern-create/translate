//! 唯一受支持的对外入口。
//!
//! 设计文档 §0 的交付判据：
//! > 即使移除 Bergamot、ncnn、Mozilla 模型以及全部第三方词库，
//! > `translator-api` 仍应能以 Mock/空资源模式编译运行。
//!
//! 本 crate 因此**不依赖任何 native runtime、不依赖任何 HTTP 客户端、不依赖 SQLite**。

use std::sync::Arc;

use translator_core::{Coordinator, CoordinatorConfig};

// 插件契约：`pub use` 同时满足内部使用与对外导出，避免重复导入。
pub use translator_plugin_api::{
    BoxFuture, DictionaryProvider, InferenceBackend, LoadedModel, PluginRegistry, TranslationEngine,
};
// 类型层：请求 / 结果 / 错误 / 语言标签 等全部公开
pub use translator_types::*;

#[cfg(feature = "mocks")]
pub use translator_plugin_api::mocks;

#[cfg(feature = "dictionary")]
pub use translator_dictionary as dictionary;

#[cfg(feature = "rules")]
pub use translator_rules as rules;

// 规则契约：即使不开 `rules` feature，宿主也可以自己实现 `TextRules` 注入。
pub use translator_core::rules::{NoRules, RuleOutcome, RulePreview, TextRules};

/// 对外门面。构造后不可变，可跨线程共享（`Send + Sync`）。
pub struct Translator {
    coordinator: Coordinator,
}

impl Translator {
    pub fn builder() -> TranslatorBuilder {
        TranslatorBuilder::new()
    }

    /// 翻译入口。
    pub async fn translate(
        &self,
        req: &TranslateRequest,
    ) -> Result<TranslationResult, TranslationError> {
        self.coordinator.translate(req).await
    }

    /// 查词入口。与整句翻译分开建模（ADR-003）。
    pub async fn lookup(
        &self,
        req: &LookupRequest,
    ) -> Result<Vec<DictionaryEntry>, TranslationError> {
        self.coordinator.lookup(req).await
    }

    /// 语言识别。短文本必须返回多个候选，不强行判定。
    pub async fn detect_language(
        &self,
        text: &str,
    ) -> Result<Vec<LanguageGuess>, TranslationError> {
        let normalized = translator_core::text::normalize(text);
        if normalized.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "文本为空",
            ));
        }

        let primary = translator_core::text::guess_language(&normalized).ok_or_else(|| {
            TranslationError::new(
                TranslationErrorCode::UnsupportedLanguage,
                "无法从文本判断语言",
            )
        })?;

        let mut guesses = vec![LanguageGuess {
            language: primary.clone(),
            confidence: 0.6,
        }];

        // 汉字为中/日共享，补充一个候选而不是硬判
        let has_han = normalized
            .chars()
            .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));
        if has_han && primary.as_str() == "zh-Hans" {
            if let Ok(ja) = LanguageTag::new("ja") {
                guesses.push(LanguageGuess {
                    language: ja,
                    confidence: 0.25,
                });
            }
        }

        Ok(guesses)
    }

    pub fn capabilities(&self) -> Vec<CapabilityInfo> {
        self.coordinator.capabilities()
    }

    /// 预览规则包对一段输入做了什么（不执行翻译、不写缓存）。
    ///
    /// 规则 id 不进 `TranslationResult`：给公开枚举加变体会破坏宿主的穷尽匹配，
    /// 因此诊断入口单独提供。
    pub fn explain_rules(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> RulePreview {
        self.coordinator.explain_rules(text, source, target)
    }

    /// 只给出路由决策，不执行翻译（调试/诊断用）。
    pub fn explain_route(&self, req: &TranslateRequest) -> Result<RouteDecision, TranslationError> {
        self.coordinator.explain_route(req)
    }

    pub fn registry(&self) -> &PluginRegistry {
        self.coordinator.registry()
    }

    pub fn clear_cache(&self) {
        self.coordinator.clear_cache();
    }

    /// 释放资源。P0 无可释放资源，保留稳定签名供后续实现。
    pub async fn shutdown(&self) -> Result<(), TranslationError> {
        self.coordinator.clear_cache();
        Ok(())
    }
}

impl std::fmt::Debug for Translator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let registry = self.coordinator.registry();
        f.debug_struct("Translator")
            .field("engines", &registry.engines().len())
            .field("providers", &registry.providers().len())
            .field("backends", &registry.backends().len())
            .finish()
    }
}

/// 组装器（composition root 的一部分：把插件注入 Coordinator）。
pub struct TranslatorBuilder {
    registry: PluginRegistry,
    config: CoordinatorConfig,
    errors: Vec<TranslationError>,
    #[cfg(feature = "dictionary")]
    dictionary_roots: Vec<std::path::PathBuf>,
    rules: Option<Arc<dyn TextRules>>,
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
            errors: Vec::new(),
            #[cfg(feature = "dictionary")]
            dictionary_roots: Vec::new(),
            rules: None,
        }
    }

    pub fn engine(mut self, engine: Arc<dyn TranslationEngine>) -> Self {
        if let Err(err) = self.registry.register_engine(engine) {
            self.errors.push(err);
        }
        self
    }

    pub fn provider(mut self, provider: Arc<dyn DictionaryProvider>) -> Self {
        if let Err(err) = self.registry.register_provider(provider) {
            self.errors.push(err);
        }
        self
    }

    pub fn backend(mut self, backend: Arc<dyn InferenceBackend>) -> Self {
        if let Err(err) = self.registry.register_backend(backend) {
            self.errors.push(err);
        }
        self
    }

    /// 挂载一个离线词典包目录（需启用 `dictionary` feature）。
    ///
    /// 包在 `build()` 时才真正打开并校验；打开失败会计入构建错误，
    /// 不会静默退化。
    #[cfg(feature = "dictionary")]
    pub fn dictionary_package(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.dictionary_roots.push(root.into());
        self
    }

    /// 注入文本规则包（不需要 `rules` feature：`TextRules` 是公开契约）。
    ///
    /// 规则包的 `version()` 会自动进入缓存键，因此**换规则必须换版本**，
    /// 否则旧缓存会被错误复用。
    pub fn rules(mut self, rules: Arc<dyn TextRules>) -> Self {
        self.rules = Some(rules);
        self
    }

    /// 从文件加载规则包（JSON 或二进制形态，需启用 `rules` feature）。
    ///
    /// 加载失败计入构建错误，不会静默退化成「没有规则」——
    /// 规则是为了纠正译文，静默失效比报错更危险。
    #[cfg(feature = "rules")]
    pub fn rules_file(mut self, path: impl AsRef<std::path::Path>) -> Self {
        match translator_rules::RuleEngine::load(path) {
            Ok(engine) => self.rules = Some(Arc::new(engine)),
            Err(e) => self.errors.push(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!("规则包加载失败：{e}"),
            )),
        }
        self
    }

    pub fn cache_capacity(mut self, capacity: usize) -> Self {
        self.config.cache_capacity = capacity;
        self
    }

    /// 手动指定规则版本。仅在**没有**注入 `rules` 时生效
    /// （注入了规则包时以规则包自报版本为准）。
    pub fn rules_version(mut self, version: u32) -> Self {
        self.config.rules_version = version;
        self
    }

    /// 构造门面。注册阶段积累的错误在这里一次性暴露，
    /// 避免把错误分散到每个 builder 方法里。
    pub fn build(mut self) -> Result<Translator, TranslationError> {
        // 先挂载词典包：把包内引擎与查词提供者注册进同一个注册表
        #[cfg(feature = "dictionary")]
        {
            let roots = std::mem::take(&mut self.dictionary_roots);
            for root in roots {
                match translator_dictionary::open_as_plugins(&root) {
                    Ok((engine, provider)) => {
                        if let Err(e) = self.registry.register_engine(engine) {
                            self.errors.push(e);
                        }
                        if let Err(e) = self.registry.register_provider(provider) {
                            self.errors.push(e);
                        }
                    }
                    Err(e) => self.errors.push(TranslationError::from(e)),
                }
            }
        }

        if let Some(err) = self.errors.into_iter().next() {
            return Err(err);
        }

        self.config.rules = self.rules.take();

        Ok(Translator {
            coordinator: Coordinator::new(Arc::new(self.registry), self.config),
        })
    }
}

/// 常用类型的预导入。
pub mod prelude {
    pub use crate::Translator;
    pub use translator_types::{
        Capability, LanguageTag, LookupRequest, TranslateRequest, TranslationError,
        TranslationErrorCode, TranslationPolicy, TranslationResult,
    };
}
