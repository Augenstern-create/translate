//! P17-A：统一 Planner、共享证据、混合执行与远程兜底。
#![cfg(feature = "mocks")]

use std::sync::Arc;

use translator_api::mocks::{
    MockDictionaryEngine, MockDictionaryProvider, MockNmtEngine, MockRemoteEngine,
};
use translator_api::prelude::*;
use translator_api::{
    RemoteFallbackPolicy, SentenceTemplates, TemplateTable, TranslateOptions, TranslationRoute,
    Translator,
};
// `PatternKind` 目前由 core 直接暴露；API 公开类型未重复导出。
use translator_core as translator_core_reexport;

use translator_plugin_api::{BoxFuture, TranslationEngine};
use translator_types::{
    CancelToken, EngineDescriptor, LanguageTag, PluginApiVersion, TranslationResult,
};

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}
fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}
fn req(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, zh()).with_source(en())
}

fn templates() -> Arc<dyn SentenceTemplates> {
    Arc::new(
        TemplateTable::from_json(
            r#"{
              "schema_version":1,"id":"templates.p17","version":1,
              "source":"en","target":"zh-Hans",
              "pairs":[{"source":"Good morning.","target":"早上好。"}]
            }"#,
        )
        .unwrap(),
    )
}

#[test]
fn 共享证据为单词附带词义候选且不改变词典结果() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .provider(MockDictionaryProvider::new())
        .build()
        .unwrap();
    let out = pollster::block_on(t.translate_with_diagnostics(&req("hello"))).unwrap();
    assert_eq!(out.result.text, "你好");
    assert_eq!(out.diagnostics.route, TranslationRoute::Dictionary);
    let evidence = out.diagnostics.evidence.as_ref().expect("应有共享证据");
    assert!(evidence.dictionary_available);
    assert!(!evidence.lexical_candidates.is_empty(), "词典候选必须可见");
    assert_eq!(out.diagnostics.units.len(), 1);
    assert_eq!(out.diagnostics.units[0].route, TranslationRoute::Dictionary);
}

#[test]
fn 段落混合执行保留逐单元决策轨迹() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .templates(templates())
        .cache_capacity(16)
        .build()
        .unwrap();
    t.reset_route_stats();
    let options = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Paragraph)
        .with_paragraph_strategy(ParagraphStrategy::Sentence);
    let input = "hello\nGood morning.\nThis is a complex sentence.";
    let out =
        pollster::block_on(t.translate_with_diagnostics_options(&req(input), &options)).unwrap();

    assert_eq!(
        out.diagnostics.route,
        TranslationRoute::Mixed,
        "{}",
        out.result.text
    );
    assert!(out.result.text.contains("你好"), "{}", out.result.text);
    assert!(out.result.text.contains("早上好。"), "{}", out.result.text);
    assert!(out.result.text.contains("[nmt"), "{}", out.result.text);
    assert_eq!(out.diagnostics.units.len(), 3);
    assert_eq!(out.diagnostics.units[0].route, TranslationRoute::Dictionary);
    assert_eq!(out.diagnostics.units[1].route, TranslationRoute::Rule);
    assert_eq!(out.diagnostics.units[2].route, TranslationRoute::LocalNmt);
    let stats = t.route_stats();
    assert_eq!(stats.dictionary_hits, 1);
    assert_eq!(stats.rule_matches, 1);
    assert_eq!(stats.nmt_calls, 1);
}

#[test]
fn 远程回退默认关闭() {
    let t = Translator::builder()
        .engine(MockRemoteEngine::new(true))
        .build()
        .unwrap();
    let err = pollster::block_on(t.translate(&req("A sentence that needs a model."))).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::ResourceNotInstalled);
    assert_eq!(t.route_stats().remote_calls, 0);
}

#[test]
fn 本地不可用且显式授权时才走远程() {
    let t = Translator::builder()
        .engine(MockRemoteEngine::new(true))
        .remote_fallback(RemoteFallbackPolicy::OnLocalUnavailable)
        .build()
        .unwrap();
    t.reset_route_stats();
    let out =
        pollster::block_on(t.translate_with_diagnostics(&req("A sentence that needs a model.")))
            .unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::Remote);
    assert!(out.diagnostics.model_invoked);
    assert_eq!(
        out.diagnostics.fallback_reason,
        Some(FallbackReason::EngineFallback)
    );
    assert_eq!(t.route_stats().remote_calls, 1);
    assert_eq!(t.route_stats().nmt_calls, 0);
}

struct FailingNmt;

impl TranslationEngine for FailingNmt {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: "mock.failing.nmt",
            capabilities: &[Capability::Nmt],
            priority: 100,
            plugin_api: PluginApiVersion::CURRENT,
            license: "MIT",
        }
    }
    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }
    fn translate<'a>(
        &'a self,
        _req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            Err(TranslationError::new(
                TranslationErrorCode::BackendFailed,
                "injected local failure",
            ))
        })
    }
}

#[test]
fn 本地执行失败可按授权回退远程() {
    let t = Translator::builder()
        .engine(Arc::new(FailingNmt))
        .engine(MockRemoteEngine::new(true))
        .remote_fallback(RemoteFallbackPolicy::OnLocalFailure)
        .build()
        .unwrap();
    t.reset_route_stats();
    let out =
        pollster::block_on(t.translate_with_diagnostics(&req("A sentence that needs a model.")))
            .unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::Remote);
    assert_eq!(
        out.diagnostics.fallback_reason,
        Some(FallbackReason::EngineFallback)
    );
    let stats = t.route_stats();
    assert_eq!(stats.nmt_calls, 1, "失败的本地尝试也要计数");
    assert_eq!(stats.remote_calls, 1);
    assert_eq!(stats.remote_fallbacks, 1);
}

#[test]
fn 回退原因区分缺词缺短语与规则未覆盖() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .build()
        .unwrap();
    let word = pollster::block_on(t.translate_with_diagnostics(&req("xyzzy"))).unwrap();
    assert_eq!(
        word.diagnostics.fallback_reason,
        Some(FallbackReason::DictionaryMiss)
    );

    let phrase =
        pollster::block_on(t.translate_with_diagnostics(&req("memory footprint"))).unwrap();
    assert_eq!(
        phrase.diagnostics.fallback_reason,
        Some(FallbackReason::PhraseMiss)
    );

    let sentence = pollster::block_on(
        t.translate_with_diagnostics(&req("The server is ready and the client is waiting.")),
    )
    .unwrap();
    assert_eq!(
        sentence.diagnostics.fallback_reason,
        Some(FallbackReason::ComplexSyntax)
    );
}

#[test]
fn 词典未命中回退模型后再次请求命中回退缓存() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .cache_capacity(8)
        .build()
        .unwrap();
    t.reset_route_stats();
    let first = pollster::block_on(t.translate_with_diagnostics(&req("xyzzy"))).unwrap();
    assert_eq!(first.diagnostics.route, TranslationRoute::LocalNmt);
    assert!(!first.diagnostics.cache_hit);
    let second = pollster::block_on(t.translate_with_diagnostics(&req("xyzzy"))).unwrap();
    assert!(second.diagnostics.cache_hit, "回退键必须能被后续请求命中");
    assert_eq!(t.route_stats().nmt_calls, 1);
    assert_eq!(t.route_stats().cache_hits, 1);
}

#[test]
fn 远程回退也走缓存而不是重复计费() {
    let t = Translator::builder()
        .engine(MockRemoteEngine::new(true))
        .remote_fallback(RemoteFallbackPolicy::OnLocalUnavailable)
        .cache_capacity(8)
        .build()
        .unwrap();
    t.reset_route_stats();
    pollster::block_on(t.translate(&req("A sentence that needs a model."))).unwrap();
    let second =
        pollster::block_on(t.translate_with_diagnostics(&req("A sentence that needs a model.")))
            .unwrap();
    assert!(second.diagnostics.cache_hit);
    assert_eq!(t.route_stats().remote_calls, 1);
}

#[test]
fn 模板命中在共享证据中标记完整覆盖() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .templates(templates())
        .build()
        .unwrap();
    let out = pollster::block_on(t.translate_with_diagnostics(&req("Good morning."))).unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::Rule);
    let evidence = out.diagnostics.evidence.as_ref().expect("模板证据");
    assert!(evidence.has_complete_pattern());
    assert!(evidence.has_pattern(translator_core_reexport::PatternKind::Template));
}

#[test]
fn 文档内词典规则模型可以混合并保留代码块() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .templates(templates())
        .cache_capacity(32)
        .build()
        .unwrap();
    let input =
        "# hello\n\nGood morning.\n\nThis is a complex sentence.\n\n```rust\nfn main() {}\n```\n";
    let options =
        TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Document);
    let out =
        pollster::block_on(t.translate_with_diagnostics_options(&req(input), &options)).unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::Mixed);
    assert_eq!(out.diagnostics.units.len(), 3);
    assert_eq!(out.diagnostics.units[0].route, TranslationRoute::Dictionary);
    assert_eq!(out.diagnostics.units[1].route, TranslationRoute::Rule);
    assert_eq!(out.diagnostics.units[2].route, TranslationRoute::LocalNmt);
    assert!(out.result.text.contains("```rust\nfn main() {}\n```"));
    assert!(out.result.text.contains("你好"));
    assert!(out.result.text.contains("早上好。"));
}

#[test]
fn 上下文选义走协调器缓存且不同上下文不串() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .provider(MockDictionaryProvider::new())
        .cache_capacity(8)
        .build()
        .unwrap();
    let base = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Word)
        .with_context_id("doc#1");
    let a = TranslateOptions {
        context_text: Some("first context".to_string()),
        ..base.clone()
    };
    let b = TranslateOptions {
        context_text: Some("second context".to_string()),
        ..base
    };
    let first =
        pollster::block_on(t.translate_with_diagnostics_options(&req("hello"), &a)).unwrap();
    let second =
        pollster::block_on(t.translate_with_diagnostics_options(&req("hello"), &b)).unwrap();
    assert_eq!(first.diagnostics.strategy, "word-context");
    assert_eq!(second.diagnostics.strategy, "word-context");
    assert!(!first.diagnostics.cache_hit);
    assert!(!second.diagnostics.cache_hit, "上下文正文变化必须换缓存键");
    let again =
        pollster::block_on(t.translate_with_diagnostics_options(&req("hello"), &a)).unwrap();
    assert!(again.diagnostics.cache_hit);
}

#[test]
fn 显式远程策略不得被上下文选义旁路() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .provider(MockDictionaryProvider::new())
        .build()
        .unwrap();
    let options = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Word)
        .with_context_text("some context");
    let mut request = req("hello");
    request.policy = TranslationPolicy::RemoteOnly;
    let err = pollster::block_on(t.translate_with_options(&request, &options)).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::ResourceNotInstalled);
}

#[test]
fn 可取消入口也经过结构规划且预取消直接返回() {
    let t = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .templates(templates())
        .build()
        .unwrap();
    let options = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Paragraph)
        .with_paragraph_strategy(ParagraphStrategy::Sentence);
    let input = "hello\nGood morning.\nThis is a complex sentence.";
    let token = CancelToken::new();
    token.cancel();
    let err = pollster::block_on(t.translate_with_diagnostics_cancellable_options(
        &req(input),
        &token,
        &options,
    ))
    .unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::Cancelled);
    assert_eq!(t.route_stats().nmt_calls, 0);
}
