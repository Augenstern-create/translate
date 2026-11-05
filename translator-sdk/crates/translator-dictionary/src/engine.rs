//! 把 [`DictionaryPackage`] 适配成插件契约的两套接口：
//! 一个 [`TranslationEngine`]（单词直译）和一个 [`DictionaryProvider`]（查词）。

use std::sync::Arc;
use std::time::Instant;

use translator_plugin_api::{BoxFuture, DictionaryProvider, TranslationEngine};
use translator_types::{
    Capability, DictionaryEntry, EngineDescriptor, LanguageTag, LookupRequest, PluginApiVersion,
    ProviderDescriptor, TranslateRequest, TranslationError, TranslationErrorCode,
    TranslationResult,
};

use crate::{DictionaryError, DictionaryPackage};

const ENGINE_ID: &str = "dictionary.package";
const PROVIDER_ID: &str = "dictionary.package.provider";

/// 基于词典包的翻译引擎：只做**精确词条**直译。
///
/// 未命中时返回 `CapabilityUnavailable`，由 Coordinator 在 `AutoLocalFirst`
/// 下自动回退到本地 NMT（设计文档 §6）。
pub struct DictionaryEngine {
    package: Arc<DictionaryPackage>,
    priority: i32,
}

impl DictionaryEngine {
    pub fn new(package: Arc<DictionaryPackage>) -> Self {
        Self {
            package,
            priority: 100,
        }
    }

    pub fn with_priority(package: Arc<DictionaryPackage>, priority: i32) -> Self {
        Self { package, priority }
    }

    pub fn package(&self) -> &DictionaryPackage {
        &self.package
    }
}

impl TranslationEngine for DictionaryEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: ENGINE_ID,
            capabilities: &[Capability::Dictionary],
            priority: self.priority,
            plugin_api: PluginApiVersion::CURRENT,
            // 引擎本身的许可是 SDK 的；数据许可在 manifest 里单独声明
            license: "Apache-2.0 OR MIT",
        }
    }

    fn supports(&self, source: &LanguageTag, target: &LanguageTag) -> bool {
        source.as_str() == self.package.language()
            && self
                .package
                .manifest()
                .target_languages
                .iter()
                .any(|t| t == target.as_str())
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            let started = Instant::now();

            let source = req.source.clone().ok_or_else(|| {
                TranslationError::new(
                    TranslationErrorCode::InvalidRequest,
                    "词典引擎需要显式源语言",
                )
            })?;

            let entry = self
                .package
                .lookup_exact(req.text.trim())
                .map_err(TranslationError::from)?
                .ok_or_else(|| {
                    TranslationError::new(
                        TranslationErrorCode::CapabilityUnavailable,
                        "词典未命中：无精确词条",
                    )
                })?;

            let text = entry
                .senses
                .first()
                .map(|s| s.definition.clone())
                .unwrap_or_default();

            if text.is_empty() {
                return Err(TranslationError::new(
                    TranslationErrorCode::BackendFailed,
                    "词条没有释义",
                ));
            }

            Ok(TranslationResult {
                text,
                source,
                target: req.target.clone(),
                engine_id: ENGINE_ID.to_string(),
                model_id: Some(self.package.manifest().id.clone()),
                elapsed_ms: started.elapsed().as_millis() as u64,
                warnings: Vec::new(),
            })
        })
    }
}

/// 基于词典包的查词提供者。
pub struct DictionaryProviderImpl {
    package: Arc<DictionaryPackage>,
}

impl DictionaryProviderImpl {
    pub fn new(package: Arc<DictionaryPackage>) -> Self {
        Self { package }
    }

    pub fn package(&self) -> &DictionaryPackage {
        &self.package
    }
}

impl DictionaryProvider for DictionaryProviderImpl {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: PROVIDER_ID,
            plugin_api: PluginApiVersion::CURRENT,
            license: "Apache-2.0 OR MIT",
        }
    }

    fn lookup<'a>(
        &'a self,
        req: &'a LookupRequest,
    ) -> BoxFuture<'a, Result<Vec<DictionaryEntry>, TranslationError>> {
        Box::pin(async move {
            // 语言不匹配时返回空列表，而不是报错：
            // 多词典共存时由 Coordinator 汇总，单个包不认识该语言属正常情况。
            if let Some(lang) = &req.language {
                if lang.as_str() != self.package.language() {
                    return Ok(Vec::new());
                }
            }

            self.package
                .lookup(req.term.trim(), req.limit)
                .map_err(|e: DictionaryError| TranslationError::from(e))
        })
    }
}
