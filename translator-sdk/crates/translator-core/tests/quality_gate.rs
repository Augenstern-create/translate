//! T2：严格完整性门禁。
//!
//! 关键数字丢失时默认拒绝返回译文；关闭 `strict_integrity` 后只写诊断。
//! 这里用固定输出引擎，不依赖真实模型。

use std::sync::Arc;

use translator_core::{Coordinator, CoordinatorConfig};
use translator_plugin_api::{BoxFuture, PluginRegistry, TranslationEngine};
use translator_types::{
    Capability, EngineDescriptor, LanguageTag, PluginApiVersion, TranslateRequest,
    TranslationError, TranslationErrorCode, TranslationResult,
};

struct FixedEngine(&'static str);

impl TranslationEngine for FixedEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: "nmt.fixed.test",
            capabilities: &[Capability::Nmt],
            priority: 10,
            plugin_api: PluginApiVersion::CURRENT,
            license: "MIT",
        }
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            Ok(TranslationResult {
                text: self.0.to_string(),
                source: req.source.clone().unwrap(),
                target: req.target.clone(),
                engine_id: "nmt.fixed.test".to_string(),
                model_id: None,
                elapsed_ms: 0,
                warnings: Vec::new(),
            })
        })
    }
}

fn coordinator(strict: bool) -> Coordinator {
    let mut registry = PluginRegistry::new();
    registry
        .register_engine(Arc::new(FixedEngine("版本已就绪。")))
        .unwrap();
    Coordinator::new(
        Arc::new(registry),
        CoordinatorConfig {
            strict_integrity: strict,
            ..CoordinatorConfig::default()
        },
    )
}

fn request() -> TranslateRequest {
    TranslateRequest::new(
        "Version 2.10 is ready.",
        LanguageTag::new("zh-Hans").unwrap(),
    )
}

#[test]
fn 关键数字丢失默认拒绝返回() {
    let err = pollster::block_on(coordinator(true).translate(&request())).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::IntegrityFailure);
    assert!(err.message.contains("number_missing"), "{}", err.message);
}

#[test]
fn 关闭严格门禁后只写诊断() {
    let out =
        pollster::block_on(coordinator(false).translate_with_diagnostics(&request())).unwrap();
    assert_eq!(out.result.text, "版本已就绪。");
    let quality = out.diagnostics.quality.expect("必须有质量诊断");
    assert!(quality.has_fatal());
    assert_eq!(quality.numbers_in_source, 1);
    assert_eq!(quality.numbers_preserved, 0);
}

#[test]
fn 数字完整时正常返回() {
    let mut registry = PluginRegistry::new();
    registry
        .register_engine(Arc::new(FixedEngine("版本 2.10 已就绪。")))
        .unwrap();
    let coordinator = Coordinator::new(
        Arc::new(registry),
        CoordinatorConfig {
            strict_integrity: true,
            ..CoordinatorConfig::default()
        },
    );
    let out = pollster::block_on(coordinator.translate_with_diagnostics(&request())).unwrap();
    assert!(!out.diagnostics.quality.unwrap().has_fatal());
}
