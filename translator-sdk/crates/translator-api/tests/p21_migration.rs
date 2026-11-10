//! P21-R：P19 原有正例迁移到 MeaningFrame + ChineseRealizer 后必须保持输出。
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
        .cache_capacity(128)
        .build()
        .unwrap()
}

#[test]
fn p19正例迁移后保持精确输出() {
    let t = build();
    let text =
        std::fs::read_to_string(root().join("fixtures/quality/composition-v1.jsonl")).unwrap();
    let mut positives = 0;
    let mut semantic = 0;
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
                "{source}: {}",
                out.result.text
            );
            if out.diagnostics.engine_id.starts_with("semantic:") {
                semantic += 1;
            }
            let expected = case["reference"].as_str().unwrap();
            assert_eq!(out.result.text, expected, "{source} 迁移后译文不一致");
        } else {
            // P20/P21 有意扩展了并列祈使；旧 P19 反例的单独 5/5 由
            // p19_composition_suite 冻结验证，这里只迁移正例。
        }
    }
    assert_eq!(positives, 36);
    assert!(
        semantic >= 20,
        "至少 20 条 P19 正例应走新语义/生成路径，实际 {semantic}"
    );
}
