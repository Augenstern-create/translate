//! P21-R：无损 Tokenizer 覆盖门禁。
#![cfg(all(feature = "dictionary", feature = "mocks"))]

use std::path::PathBuf;
use translator_api::mocks::MockNmtEngine;
use translator_api::prelude::*;
use translator_api::{TranslateOptions, TranslationRoute, Translator};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn build() -> Translator {
    Translator::builder()
        .engine(MockNmtEngine::new())
        .dictionary_package(root().join("resources/en-zh-ecdict"))
        .composition_file(root().join("fixtures/rules/en-zh.composition.json"))
        .semantic_composition_file(root().join("fixtures/rules/en-zh.composition.json"))
        .cache_capacity(32)
        .build()
        .unwrap()
}
fn route(t: &Translator, source: &str) -> TranslationRoute {
    let req = TranslateRequest::new(source, LanguageTag::new("zh-Hans").unwrap())
        .with_source(LanguageTag::new("en").unwrap());
    let options =
        TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
    pollster::block_on(t.translate_with_diagnostics_options(&req, &options))
        .unwrap()
        .diagnostics
        .route
}

#[test]
fn 数字和符号不得被规则无声丢弃() {
    let t = build();
    assert_eq!(route(&t, "There are 2 apples."), TranslationRoute::LocalNmt);
    assert_eq!(route(&t, "The price is 100%."), TranslationRoute::LocalNmt);
    assert_eq!(route(&t, "Total: $20."), TranslationRoute::LocalNmt);
    assert_eq!(route(&t, "She has two apples."), TranslationRoute::Rule);
}
