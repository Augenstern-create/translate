//! P21-R：局部 SyntaxGraph 与组合陈述结构。
#![cfg(all(feature = "dictionary", feature = "mocks"))]

use std::collections::HashMap;
use std::path::PathBuf;
use translator_api::mocks::MockNmtEngine;
use translator_api::prelude::*;
use translator_api::{SemanticCompositionEngine, TranslateOptions, TranslationRoute, Translator};

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

#[test]
fn 否定数量时间生成句法图() {
    let t = build();
    let engine =
        SemanticCompositionEngine::from_file(root().join("fixtures/rules/en-zh.composition.json"))
            .unwrap();
    let mut entries = HashMap::new();
    for word in engine.lookup_candidates("She didn't buy two new books yesterday.") {
        let req = LookupRequest::new(word.clone())
            .with_language(LanguageTag::new("en").unwrap())
            .with_limit(8);
        entries.insert(word, pollster::block_on(t.lookup(&req)).unwrap());
    }
    let analysis = engine.analyze("She didn't buy two new books yesterday.", &entries);
    assert!(analysis.program.is_some(), "应进入组合语义路径");
    let syntax = analysis.syntax.expect("应产生 SyntaxGraph");
    assert!(syntax.nodes.iter().any(|n| n.lemma == "she"));
    assert!(syntax.nodes.iter().any(|n| n.lemma == "buy"));
    assert!(syntax.nodes.iter().any(|n| n.lemma == "book"));
    assert!(syntax.relations.iter().any(|r| matches!(
        r.kind,
        translator_api::semantic::SyntaxRelationKind::Subject
    )));
    assert!(syntax
        .relations
        .iter()
        .any(|r| matches!(r.kind, translator_api::semantic::SyntaxRelationKind::Object)));
    assert!(syntax.relations.iter().any(|r| matches!(
        r.kind,
        translator_api::semantic::SyntaxRelationKind::Adverbial
    )));

    let req = TranslateRequest::new(
        "She didn't buy two new books yesterday.",
        LanguageTag::new("zh-Hans").unwrap(),
    )
    .with_source(LanguageTag::new("en").unwrap());
    let options =
        TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
    let out = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::Rule);
    assert_eq!(out.result.text, "她昨天没买两本新书。");

    let mut cond_entries = HashMap::new();
    for word in engine.lookup_candidates("If it rains, I will stay home.") {
        let req = LookupRequest::new(word.clone())
            .with_language(LanguageTag::new("en").unwrap())
            .with_limit(8);
        cond_entries.insert(word, pollster::block_on(t.lookup(&req)).unwrap());
    }
    let cond = engine.analyze("If it rains, I will stay home.", &cond_entries);
    assert!(cond.program.is_some());
    assert!(cond.syntax.unwrap().relations.iter().any(|r| matches!(
        r.kind,
        translator_api::semantic::SyntaxRelationKind::Condition
    )));

    for (source, expected) in [
        ("He runs a company.", "他经营一家公司。"),
        ("I take a photo.", "我拍一张照片。"),
        ("I take a book.", "我拿一本书。"),
    ] {
        let req = TranslateRequest::new(source, LanguageTag::new("zh-Hans").unwrap())
            .with_source(LanguageTag::new("en").unwrap());
        let options =
            TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
        let out = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
        assert_eq!(out.diagnostics.route, TranslationRoute::Rule, "{source}");
        assert_eq!(out.result.text, expected, "{source}");
    }
}
