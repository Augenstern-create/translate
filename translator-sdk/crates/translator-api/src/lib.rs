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

    pub fn cache_capacity(mut self, capacity: usize) -> Self {
        self.config.cache_capacity = capacity;
        self
    }

    pub fn rules_version(mut self, version: u32) -> Self {
        self.config.rules_version = version;
        self
    }

    /// 构造门面。注册阶段积累的错误在这里一次性暴露，
    /// 避免把错误分散到每个 builder 方法里。
    pub fn build(self) -> Result<Translator, TranslationError> {
        if let Some(err) = self.errors.into_iter().next() {
            return Err(err);
        }

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
