//! P21-R：剩余 P19 路径迁移与条件/比较可靠子集。
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
        .cache_capacity(64)
        .build()
        .unwrap()
}

#[test]
fn 条件比较与剩余旧路径() {
    let t = build();
    let text =
        std::fs::read_to_string(root().join("fixtures/quality/p21-advanced-v1.jsonl")).unwrap();
    let mut count = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let case: serde_json::Value = serde_json::from_str(line).unwrap();
        let source = case["source"].as_str().unwrap();
        let req = TranslateRequest::new(source, LanguageTag::new("zh-Hans").unwrap())
            .with_source(LanguageTag::new("en").unwrap());
        let options =
            TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
        let out = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
        assert_eq!(
            out.diagnostics.route,
            TranslationRoute::Rule,
            "{source}: {}",
            out.result.text
        );
        let allowed: Vec<&str> = case["allowed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            allowed.contains(&out.result.text.as_str()),
            "{source} -> {}，允许 {:?}",
            out.result.text,
            allowed
        );
        count += 1;
    }
    assert_eq!(count, 8);
}
