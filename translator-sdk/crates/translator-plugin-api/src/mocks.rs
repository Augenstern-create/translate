//! Mock 插件：用于 CLI 演示、契约测试与路由测试。
//!
//! 这些实现的**唯一目的**是让整条链路在不加载任何真实模型的前提下跑通，
//! 从而验证「移除全部第三方资源后 SDK 仍可编译运行」这一交付判据
//! （设计文档文末）。输出内容带有明显标记，绝不冒充真实译文。

use std::sync::Arc;
use std::time::Instant;

use translator_types::{
    BackendDescriptor, Capability, CompatibilityReport, DictionaryEntry, EngineDescriptor,
    LanguageTag, LookupRequest, ModelManifest, NmtInput, NmtOutput, ProviderDescriptor,
    ResourceLimits, Sense, TranslateRequest, TranslationError, TranslationErrorCode,
    TranslationResult, TranslationWarning,
};

use crate::{BoxFuture, DictionaryProvider, InferenceBackend, LoadedModel, TranslationEngine};

// ============================================================
// 内置词表（仅用于演示与测试）
// ============================================================

/// (源语言, 目标语言, 原文小写, 译文)
const PHRASE_TABLE: &[(&str, &str, &str, &str)] = &[
    ("en", "zh-Hans", "hello", "你好"),
    ("en", "zh-Hans", "world", "世界"),
    ("en", "zh-Hans", "good morning", "早上好"),
    ("en", "zh-Hans", "cat", "猫"),
    ("zh-Hans", "en", "你好", "hello"),
    ("zh-Hans", "en", "猫", "cat"),
];

fn lookup_phrase(source: &LanguageTag, target: &LanguageTag, text: &str) -> Option<&'static str> {
    let needle = text.trim().to_lowercase();
    PHRASE_TABLE
        .iter()
        .find(|(s, t, src_text, _)| {
            *s == source.as_str() && *t == target.as_str() && *src_text == needle
        })
        .map(|(_, _, _, dst)| *dst)
}

// ============================================================
// 词典引擎
// ============================================================

/// 只做**精确词条**匹配。未命中时返回 `CapabilityUnavailable`，
/// 由 Coordinator 在 `AutoLocalFirst` 策略下回退到本地 NMT。
pub struct MockDictionaryEngine;

impl MockDictionaryEngine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl TranslationEngine for MockDictionaryEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor::new("mock.dictionary", [Capability::Dictionary], 100, "MIT")
    }

    fn supports(&self, source: &LanguageTag, target: &LanguageTag) -> bool {
        // 演示用：仅中英双向
        matches!(
            (source.as_str(), target.as_str()),
            ("en", "zh-Hans") | ("zh-Hans", "en")
        )
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
                    "词典引擎需要显式源语言（应由 Coordinator 先行识别）",
                )
            })?;

            let hit = lookup_phrase(&source, &req.target, &req.text).ok_or_else(|| {
                TranslationError::new(
                    TranslationErrorCode::CapabilityUnavailable,
                    "词典未命中：无精确词条",
                )
            })?;

            Ok(TranslationResult {
                text: hit.to_string(),
                source,
                target: req.target.clone(),
                engine_id: self.descriptor().id.to_string(),
                model_id: None,
                elapsed_ms: started.elapsed().as_millis() as u64,
                warnings: Vec::new(),
            })
        })
    }
}

// ============================================================
// 本地 NMT 引擎
// ============================================================

/// 模拟本地神经翻译。输出带 `[nmt ...]` 标记，便于肉眼确认路由走向。
pub struct MockNmtEngine {
    pub marker: &'static str,
    pub priority: i32,
}

impl MockNmtEngine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            marker: "nmt",
            priority: 50,
        })
    }

    pub fn with_priority(priority: i32) -> Arc<Self> {
        Arc::new(Self {
            marker: "nmt",
            priority,
        })
    }
}

impl TranslationEngine for MockNmtEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor::new("mock.nmt", [Capability::Nmt], self.priority, "MIT")
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            let started = Instant::now();
            let source = req
                .source
                .clone()
                .unwrap_or_else(|| LanguageTag::new("en").expect("静态语言标签必然合法"));

            Ok(TranslationResult {
                text: format!("[{} {}->{}] {}", self.marker, source, req.target, req.text),
                source: source.clone(),
                target: req.target.clone(),
                engine_id: self.descriptor().id.to_string(),
                model_id: Some("mock/marian-en-zh-lite".to_string()),
                elapsed_ms: started.elapsed().as_millis() as u64,
                warnings: Vec::new(),
            })
        })
    }
}

// ============================================================
// 远程 LLM 引擎
// ============================================================

/// 模拟远程大模型。**默认不会被自动选用**，
/// 只有 `RemoteOnly` 或显式授权才会走到这里（设计文档 ADR-004）。
pub struct MockRemoteEngine {
    /// 是否已获得宿主的联网授权。
    pub network_consent: bool,
}

impl MockRemoteEngine {
    pub fn new(network_consent: bool) -> Arc<Self> {
        Arc::new(Self { network_consent })
    }
}

impl TranslationEngine for MockRemoteEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor::new("mock.remote", [Capability::Remote], 10, "MIT")
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            if !self.network_consent {
                return Err(TranslationError::new(
                    TranslationErrorCode::NetworkNotAuthorized,
                    "远程引擎未获联网授权",
                ));
            }

            let started = Instant::now();
            let source = req
                .source
                .clone()
                .unwrap_or_else(|| LanguageTag::new("en").expect("静态语言标签必然合法"));

            Ok(TranslationResult {
                text: format!("[remote {}->{}] {}", source, req.target, req.text),
                source,
                target: req.target.clone(),
                engine_id: self.descriptor().id.to_string(),
                model_id: Some("mock/llm-remote".to_string()),
                elapsed_ms: started.elapsed().as_millis() as u64,
                warnings: vec![TranslationWarning::LanguageAssumed(
                    "远程引擎假定源语言为 en".to_string(),
                )],
            })
        })
    }
}

// ============================================================
// 词典提供者
// ============================================================

pub struct MockDictionaryProvider;

impl MockDictionaryProvider {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl DictionaryProvider for MockDictionaryProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor::new("mock.dictionary-provider", "MIT")
    }

    fn lookup<'a>(
        &'a self,
        req: &'a LookupRequest,
    ) -> BoxFuture<'a, Result<Vec<DictionaryEntry>, TranslationError>> {
        Box::pin(async move {
            let language = req
                .language
                .clone()
                .unwrap_or_else(|| LanguageTag::new("en").expect("静态标签合法"));

            let needle = req.term.trim().to_lowercase();
            let mut entries = Vec::new();

            for (src_lang, _dst_lang, src_text, dst_text) in PHRASE_TABLE {
                if *src_lang == language.as_str() && *src_text == needle {
                    entries.push(DictionaryEntry {
                        headword: req.term.clone(),
                        normalized: needle.clone(),
                        language: language.clone(),
                        readings: Vec::new(),
                        parts_of_speech: vec!["noun".to_string()],
                        senses: vec![Sense {
                            definition: dst_text.to_string(),
                            source_id: "mock-table".to_string(),
                        }],
                        source_id: "mock-table".to_string(),
                        source_license: "MIT".to_string(),
                        confidence: 1.0,
                    });
                    break;
                }
            }

            entries.truncate(req.limit);
            Ok(entries)
        })
    }
}

// ============================================================
// 推理后端
// ============================================================

pub struct MockBackend;

impl MockBackend {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl InferenceBackend for MockBackend {
    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor::new("mock.backend", "MIT")
    }

    fn supports_model(&self, model: &ModelManifest) -> CompatibilityReport {
        // 显式声明兼容性，不做字符串猜测。
        if model.architecture == "marian-encoder-decoder" {
            CompatibilityReport::ok()
        } else {
            CompatibilityReport::incompatible(format!(
                "MockBackend 仅支持 marian-encoder-decoder，收到 {}",
                model.architecture
            ))
        }
    }

    fn load<'a>(
        &'a self,
        _model: &'a ModelManifest,
        _limits: &'a ResourceLimits,
    ) -> BoxFuture<'a, Result<Arc<dyn LoadedModel>, TranslationError>> {
        Box::pin(async move { Ok(Arc::new(MockLoadedModel) as Arc<dyn LoadedModel>) })
    }
}

struct MockLoadedModel;

impl LoadedModel for MockLoadedModel {
    fn infer<'a>(
        &'a self,
        input: &'a NmtInput,
    ) -> BoxFuture<'a, Result<NmtOutput, TranslationError>> {
        Box::pin(async move {
            Ok(NmtOutput {
                text: format!(
                    "[mock-infer {}->{}] {}",
                    input.source, input.target, input.text
                ),
            })
        })
    }

    fn estimated_bytes(&self) -> u64 {
        0
    }
}

// ============================================================
// 契约测试：所有引擎必须共同通过的行为基线
// （设计文档 §12.1 第 2 项「契约测试」）
// ============================================================

#[cfg(test)]
mod contract_tests {
    use super::*;

    /// 收集全部 Mock 引擎，任何新引擎都应加进这里，
    /// 从而自动继承下面这组契约检查。
    fn all_engines() -> Vec<Arc<dyn TranslationEngine>> {
        vec![
            MockDictionaryEngine::new(),
            MockNmtEngine::new(),
            MockRemoteEngine::new(true),
        ]
    }

    #[test]
    fn 引擎描述必须自洽() {
        for engine in all_engines() {
            let d = engine.descriptor();

            assert!(!d.id.is_empty(), "引擎 id 不得为空");
            assert!(
                !d.capabilities.is_empty(),
                "引擎 {} 必须声明至少一种能力",
                d.id
            );
            assert!(
                d.plugin_api.accepts(crate::PluginApiVersion::CURRENT),
                "引擎 {} 的 API 版本必须兼容当前 SDK",
                d.id
            );
            assert!(!d.license.is_empty(), "引擎 {} 必须声明许可证", d.id);
        }
    }

    #[test]
    fn 不支持的调用必须返回结构化错误而非崩溃() {
        for engine in all_engines() {
            let src = LanguageTag::new("en").unwrap();
            let dst = LanguageTag::new("zh-Hans").unwrap();

            if !engine.supports(&src, &dst) {
                continue;
            }

            let req = TranslateRequest::new("契约测试", dst);
            let result = pollster::block_on(engine.translate(&req));

            if let Ok(result) = result {
                assert!(!result.text.is_empty(), "成功结果不得为空文本");
                assert_eq!(
                    result.engine_id,
                    engine.descriptor().id,
                    "结果里的 engine_id 必须与描述一致"
                );
            }
        }
    }

    #[test]
    fn 词典未命中必须返回未命中错误() {
        let engine = MockDictionaryEngine::new();
        let src = LanguageTag::new("en").unwrap();
        let dst = LanguageTag::new("zh-Hans").unwrap();

        let req = TranslateRequest::new("definitely-not-in-table", dst).with_source(src);
        let err = pollster::block_on(engine.translate(&req)).unwrap_err();

        assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
    }

    #[test]
    fn 远程引擎未授权必须拒绝() {
        let engine = MockRemoteEngine::new(false);
        let req = TranslateRequest::new("hi", LanguageTag::new("zh-Hans").unwrap());

        let err = pollster::block_on(engine.translate(&req)).unwrap_err();
        assert_eq!(err.code, TranslationErrorCode::NetworkNotAuthorized);
    }

    #[test]
    fn 后端只能对显式兼容的架构返回兼容() {
        let backend = MockBackend::new();
        let src = LanguageTag::new("en").unwrap();
        let dst = LanguageTag::new("zh-Hans").unwrap();

        let good = ModelManifest {
            id: "m1".into(),
            architecture: "marian-encoder-decoder".into(),
            source_language: src.clone(),
            target_languages: vec![dst.clone()],
            estimated_peak_ram_bytes: None,
        };
        assert!(backend.supports_model(&good).compatible);

        let bad = ModelManifest {
            id: "m2".into(),
            architecture: "ncnn-param".into(),
            source_language: src,
            target_languages: vec![dst],
            estimated_peak_ram_bytes: None,
        };
        assert!(
            !backend.supports_model(&bad).compatible,
            "不得靠名字猜测兼容"
        );
    }
}
