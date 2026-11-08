//! P13-Q3：整段/整篇源文触发的术语上下文。

use std::sync::Arc;

use translator_core::terms::{
    TargetTermGuard, TermContext, TermOutcome, TermPolicy, TermViolation,
};
use translator_core::{Coordinator, CoordinatorConfig};
use translator_plugin_api::PluginRegistry;
use translator_types::LanguageTag;

struct ContextGuard;

impl TargetTermGuard for ContextGuard {
    fn id(&self) -> &'static str {
        "terms.test.context"
    }
    fn version(&self) -> u32 {
        1
    }
    fn apply(&self, text: &str, _source: &LanguageTag, _target: &LanguageTag) -> TermOutcome {
        TermOutcome::unchanged(text)
    }

    fn apply_with_context(&self, ctx: &TermContext<'_>) -> TermOutcome {
        // 只有整段源文里出现 memory leak 时才修正后文的 内存泄露。
        if !ctx.source_text.contains("memory leak") {
            return TermOutcome::unchanged(ctx.target_text);
        }
        if ctx.target_text.contains("内存泄露") {
            TermOutcome {
                text: ctx.target_text.replace("内存泄露", "内存泄漏"),
                violations: vec![TermViolation::new("内存泄漏", "内存泄露", 12)],
            }
        } else {
            TermOutcome::unchanged(ctx.target_text)
        }
    }
}

#[test]
fn 整段源文触发后文术语修正() {
    let registry = PluginRegistry::new();
    let config = CoordinatorConfig {
        term_guard: Some(Arc::new(ContextGuard)),
        term_policy: TermPolicy::Enforce,
        ..CoordinatorConfig::default()
    };
    let coordinator = Coordinator::new(Arc::new(registry), config);
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let (text, trace) = coordinator.enforce_terms_on_text(
        "A memory leak was fixed. The service is stable now.",
        &zh,
        "修复了一个内存泄露。服务现在稳定。",
    );
    assert_eq!(text, "修复了一个内存泄漏。服务现在稳定。");
    assert_eq!(trace.unwrap().replaced, 1);
}

#[test]
fn 源文没有触发词时不改() {
    let registry = PluginRegistry::new();
    let config = CoordinatorConfig {
        term_guard: Some(Arc::new(ContextGuard)),
        term_policy: TermPolicy::Enforce,
        ..CoordinatorConfig::default()
    };
    let coordinator = Coordinator::new(Arc::new(registry), config);
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let (text, trace) = coordinator.enforce_terms_on_text(
        "The service is stable now.",
        &zh,
        "修复了一个内存泄露。",
    );
    assert_eq!(text, "修复了一个内存泄露。");
    assert_eq!(trace.unwrap().replaced, 0);
}
