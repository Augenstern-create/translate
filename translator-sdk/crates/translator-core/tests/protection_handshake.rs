//! T2：协调器与真实 NMT 后端之间的“结构化保护交接”。
//!
//! P3.0 的实测结论：`{name}` 这类标记会被模型吞掉。因此真实 NMT 后端用
//! `ProtectionPlan` 自己分段，协调器**不能**再打一次 `⟦Pn⟧` 标记。
//! 本测试用两个只回显输入的引擎，分别模拟“结构化后端”和“通用后端”。

use std::sync::Arc;

use translator_core::{Coordinator, CoordinatorConfig};
use translator_plugin_api::{BoxFuture, PluginRegistry, TranslationEngine};
use translator_types::{
    CancelToken, Capability, EngineDescriptor, LanguageTag, PluginApiVersion, TranslateRequest,
    TranslationError, TranslationResult,
};

struct EchoEngine {
    id: &'static str,
    structured: bool,
}

impl TranslationEngine for EchoEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: self.id,
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
                text: req.text.clone(),
                source: req.source.clone().unwrap(),
                target: req.target.clone(),
                engine_id: self.id.to_string(),
                model_id: None,
                elapsed_ms: 0,
                warnings: Vec::new(),
            })
        })
    }

    fn handles_protected_spans(&self) -> bool {
        self.structured
    }
}

fn build(engine: Arc<dyn TranslationEngine>) -> Coordinator {
    let mut registry = PluginRegistry::new();
    registry.register_engine(engine).unwrap();
    Coordinator::new(Arc::new(registry), CoordinatorConfig::default())
}

fn request() -> TranslateRequest {
    TranslateRequest::new(
        "Visit https://example.com/a?b=1 and email {name}@example.com now",
        LanguageTag::new("zh-Hans").unwrap(),
    )
}

#[test]
fn 结构化后端拿到原文且不会出现协调器标记() {
    let coordinator = build(Arc::new(EchoEngine {
        id: "nmt.structured.test",
        structured: true,
    }));
    let out = pollster::block_on(coordinator.translate_with_diagnostics(&request())).unwrap();

    assert!(
        out.result.text.contains("https://example.com/a?b=1"),
        "URL 必须原样返回：{}",
        out.result.text
    );
    assert!(
        out.result.text.contains("{name}@example.com"),
        "模板变量必须原样返回：{}",
        out.result.text
    );
    assert!(
        !out.result.text.contains('\u{27E6}'),
        "结构化后端不应看到协调器标记：{}",
        out.result.text
    );
    assert!(out.diagnostics.restore_missing.is_empty());
    assert_eq!(out.diagnostics.protected_spans.len(), 2);
}

#[test]
fn 通用后端仍走标记化保护并可还原() {
    let coordinator = build(Arc::new(EchoEngine {
        id: "nmt.legacy.test",
        structured: false,
    }));
    let out = pollster::block_on(coordinator.translate_with_diagnostics(&request())).unwrap();
    assert_eq!(
        out.result.text, "Visit https://example.com/a?b=1 and email {name}@example.com now",
        "标记必须完整还原"
    );
    assert!(out.diagnostics.restore_missing.is_empty());
}

#[test]
fn 取消令牌仍然生效() {
    let coordinator = build(Arc::new(EchoEngine {
        id: "nmt.structured.test",
        structured: true,
    }));
    let token = CancelToken::new();
    token.cancel();
    let err =
        pollster::block_on(coordinator.translate_cancellable(&request(), &token)).unwrap_err();
    assert_eq!(err.code, translator_types::TranslationErrorCode::Cancelled);
}
