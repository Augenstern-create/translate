//! P21-R：未参与规则开发的组合泛化集（新词/数量/时间/并列/WSD）。
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
fn 未见组合泛化正反例() {
    let t = build();
    let text =
        std::fs::read_to_string(root().join("fixtures/quality/p21-unseen-v1.jsonl")).unwrap();
    let mut positive = 0;
    let mut negative = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let case: serde_json::Value = serde_json::from_str(line).unwrap();
        let source = case["source"].as_str().unwrap();
        let expected_rule = case
            .get("expect_rule")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let req = TranslateRequest::new(source, LanguageTag::new("zh-Hans").unwrap())
            .with_source(LanguageTag::new("en").unwrap());
        let options =
            TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
        let out = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
        if expected_rule {
            positive += 1;
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
        } else {
            negative += 1;
            assert_eq!(
                out.diagnostics.route,
                TranslationRoute::LocalNmt,
                "{source} 不得接管：{}",
                out.result.text
            );
            assert!(out.diagnostics.model_invoked);
        }
    }
    assert_eq!(positive, 14);
    assert_eq!(negative, 1);
}
