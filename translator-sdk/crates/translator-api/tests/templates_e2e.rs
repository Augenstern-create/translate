//! P16-R：完整模板与路由统计（Dictionary / Rule / LocalNmt）。

#![cfg(feature = "mocks")]

use std::sync::Arc;

use translator_api::mocks::{MockDictionaryEngine, MockNmtEngine};
use translator_api::prelude::*;
use translator_api::{
    SentenceTemplates, TemplateTable, TranslateRequest, TranslationRoute, Translator,
};

fn table() -> Arc<dyn SentenceTemplates> {
    Arc::new(
        TemplateTable::from_json(
            r#"{
              "schema_version":1,"id":"templates.test","version":1,
              "source":"en","target":"zh-Hans",
              "pairs":[{"source":"Good morning.","target":"早上好。"}]
            }"#,
        )
        .unwrap(),
    )
}

fn build() -> Translator {
    Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .templates(table())
        .cache_capacity(8)
        .build()
        .unwrap()
}

fn req(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, LanguageTag::new("zh-Hans").unwrap())
        .with_source(LanguageTag::new("en").unwrap())
}

#[test]
fn 完整模板命中不调用模型() {
    let t = build();
    t.reset_route_stats();
    let out = pollster::block_on(t.translate_with_diagnostics(&req("Good morning."))).unwrap();
    assert_eq!(out.result.text, "早上好。");
    assert_eq!(out.diagnostics.route, TranslationRoute::Rule);
    assert!(!out.diagnostics.model_invoked);
    assert!(out.diagnostics.rule_matched);
    let s = t.route_stats();
    assert_eq!(s.rule_matches, 1);
    assert_eq!(s.nmt_calls, 0);
    assert_eq!(s.failures, 0);
}

#[test]
fn 模板只匹配完整输入() {
    let t = build();
    let out =
        pollster::block_on(t.translate_with_diagnostics(&req("Good morning my friend"))).unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::LocalNmt);
    assert!(out.diagnostics.model_invoked);
    assert_eq!(t.route_stats().nmt_calls, 1);
}

#[test]
fn 词典路由不调用模型() {
    let t = build();
    t.reset_route_stats();
    let out = pollster::block_on(t.translate_with_diagnostics(&req("hello"))).unwrap();
    assert_eq!(out.result.text, "你好");
    assert_eq!(out.diagnostics.route, TranslationRoute::Dictionary);
    assert!(!out.diagnostics.model_invoked);
    let s = t.route_stats();
    assert_eq!(s.dictionary_hits, 1);
    assert_eq!(s.nmt_calls, 0);
}

#[test]
fn 词典未命中回退模型并计数() {
    let t = build();
    t.reset_route_stats();
    let out = pollster::block_on(t.translate_with_diagnostics(&req("xyzzy"))).unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::LocalNmt);
    assert!(out.diagnostics.model_invoked);
    let s = t.route_stats();
    assert_eq!(s.nmt_calls, 1);
    assert_eq!(s.nmt_fallbacks, 1);
    assert_eq!(s.dictionary_hits, 0);
}

#[test]
fn 复杂句进入本地模型() {
    let t = build();
    t.reset_route_stats();
    let out = pollster::block_on(
        t.translate_with_diagnostics(&req("I did not tell him because I was afraid.")),
    )
    .unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::LocalNmt);
    assert!(out.diagnostics.model_invoked);
    assert_eq!(t.route_stats().nmt_calls, 1);
}

#[test]
fn 规则与模型结果不共享缓存() {
    let t = build();
    let rule = pollster::block_on(t.translate_with_diagnostics(&req("Good morning."))).unwrap();
    let nmt = pollster::block_on(t.translate_with_diagnostics(&req("Good morning."))).unwrap();
    // 第二次仍是 Rule（不是 NMT），且命中缓存。
    assert_eq!(nmt.diagnostics.route, TranslationRoute::Rule);
    assert!(nmt.diagnostics.cache_hit);
    assert_eq!(rule.result.text, nmt.result.text);
}
