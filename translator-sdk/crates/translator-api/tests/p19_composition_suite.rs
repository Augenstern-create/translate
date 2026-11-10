//! P19-G 独立组合测试：正例必须命中，反例必须回退 NMT。
#![cfg(all(feature = "dictionary", feature = "mocks"))]

use std::path::PathBuf;

use translator_api::mocks::MockNmtEngine;
use translator_api::prelude::*;
use translator_api::{TranslateOptions, TranslationRoute, Translator};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn translator() -> Translator {
    Translator::builder()
        .engine(MockNmtEngine::new())
        .dictionary_package(root().join("resources/en-zh-ecdict"))
        .composition_file(root().join("fixtures/rules/en-zh.composition.json"))
        .cache_capacity(128)
        .build()
        .unwrap()
}

#[test]
fn 独立组合测试集正反例() {
    let t = translator();
    let path = root().join("fixtures/quality/composition-v1.jsonl");
    let text = std::fs::read_to_string(path).unwrap();
    let mut positives = 0;
    let mut negatives = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let case: serde_json::Value = serde_json::from_str(line).unwrap();
        let source = case["source"].as_str().unwrap();
        let expect_rule = case["expect_rule"].as_bool().unwrap();
        let req = TranslateRequest::new(source, LanguageTag::new("zh-Hans").unwrap())
            .with_source(LanguageTag::new("en").unwrap());
        let options =
            TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
        let out = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
        if expect_rule {
            positives += 1;
            assert_eq!(
                out.diagnostics.route,
                TranslationRoute::Rule,
                "{} 应命中规则，实际 {}：{}",
                source,
                out.diagnostics.route,
                out.result.text
            );
            let expected = case["reference"].as_str().unwrap();
            assert_eq!(out.result.text, expected, "{} 的规则译文不匹配", source);
        } else {
            negatives += 1;
            assert_eq!(
                out.diagnostics.route,
                TranslationRoute::LocalNmt,
                "{} 是反例，不得命中规则：{}",
                source,
                out.result.text
            );
            assert!(out.diagnostics.model_invoked, "{} 应回退模型", source);
        }
    }
    assert_eq!(positives, 36);
    assert_eq!(negatives, 5);
}
