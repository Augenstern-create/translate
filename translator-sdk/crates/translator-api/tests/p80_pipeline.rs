//! docs/80：首个纵向样本的完整新数据流。
#![cfg(all(feature = "dictionary", feature = "mocks"))]

use std::path::PathBuf;
use translator_api::mocks::MockNmtEngine;
use translator_api::pipeline;
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

#[test]
fn 纵向链路生成全部中间结构() {
    let t = build();
    let lexicon = translator_api::CompositionEngine::from_file(
        root().join("fixtures/rules/en-zh.composition.json"),
    )
    .unwrap();
    let output = pipeline::analyze("She didn't buy two new books yesterday.", &lexicon).unwrap();
    assert!(output.tokens.coverage.complete);
    for token in &output.tokens.tokens {
        assert!(token.span.end <= output.tokens.source.len());
        assert!(output.tokens.source.is_char_boundary(token.span.start));
        assert!(output.tokens.source.is_char_boundary(token.span.end));
        if token.derived_from.is_none() {
            assert_eq!(
                &output.tokens.source[token.span.start..token.span.end],
                token.surface
            );
        }
    }
    assert!(
        !pipeline::tokenize_lossless("She bought two books 20.")
            .coverage
            .complete
    );
    assert!(
        !pipeline::tokenize_lossless("She bought 2 books.")
            .coverage
            .complete
    );
    assert!(output.syntax.nodes.iter().any(|n| n.surface == "she"));
    assert!(output
        .syntax
        .edges
        .iter()
        .any(|e| matches!(e.relation, pipeline::SyntaxRelation::Subject)));
    assert!(output
        .syntax
        .edges
        .iter()
        .any(|e| matches!(e.relation, pipeline::SyntaxRelation::Negation)));
    assert!(output
        .syntax
        .edges
        .iter()
        .any(|e| matches!(e.relation, pipeline::SyntaxRelation::Quantity)));
    assert!(output
        .syntax
        .edges
        .iter()
        .any(|e| matches!(e.relation, pipeline::SyntaxRelation::Modifier)));
    assert!(output
        .syntax
        .edges
        .iter()
        .any(|e| matches!(e.relation, pipeline::SyntaxRelation::Adverbial)));
    assert!(output
        .meaning
        .nodes
        .iter()
        .any(|n| n.kind == pipeline::MeaningNodeKind::Event));
    assert!(output
        .meaning
        .relations
        .iter()
        .any(|r| matches!(r.kind, pipeline::MeaningRelationKind::Scope)));
    assert!(output
        .meaning
        .relations
        .iter()
        .any(|r| matches!(r.kind, pipeline::MeaningRelationKind::Agent)));
    assert!(output
        .meaning
        .relations
        .iter()
        .any(|r| matches!(r.kind, pipeline::MeaningRelationKind::Patient)));
    assert!(output
        .meaning
        .relations
        .iter()
        .any(|r| matches!(r.kind, pipeline::MeaningRelationKind::Time)));
    assert!(output
        .meaning
        .relations
        .iter()
        .any(|r| matches!(r.kind, pipeline::MeaningRelationKind::Quantity)));
    assert!(output
        .meaning
        .relations
        .iter()
        .any(|r| matches!(r.kind, pipeline::MeaningRelationKind::Modifier)));
    assert!(!output.meaning.nodes.iter().any(|n| n.lemma == "昨天"));
    assert!(output.meaning.nodes.iter().any(|n| n.lemma == "yesterday"));
    assert!(output
        .chinese
        .nodes
        .iter()
        .any(|n| n.kind == pipeline::ChineseNodeKind::Quantity && n.text == "两"));
    assert!(output
        .chinese
        .nodes
        .iter()
        .any(|n| n.kind == pipeline::ChineseNodeKind::Classifier && n.text == "本"));
    assert!(output.validation.valid);
    assert_eq!(
        output.validation.mapped_meanings,
        output.validation.total_meanings
    );
    assert_eq!(output.text, "她昨天没买两本新书。");
    let mut bad = output.chinese.clone();
    bad.nodes
        .retain(|n| n.kind != pipeline::ChineseNodeKind::Verb);
    assert!(!pipeline::validate(&output.meaning, &bad).valid);

    let out = pipeline::analyze("She is reading a book.", &lexicon).expect("progressive");
    assert_eq!(out.text, "她正在读一本书。");
    let out = pipeline::analyze("I can swim.", &lexicon).expect("modal");
    assert_eq!(out.text, "我会游泳。");
    let out = pipeline::analyze("The door was opened by him.", &lexicon).expect("passive");
    assert_eq!(out.text, "门被他打开了。");
    let out = pipeline::analyze("I want to buy a book.", &lexicon).expect("want");
    assert_eq!(out.text, "我想买一本书。");
    let out = pipeline::analyze("If it rains, I will stay home.", &lexicon).expect("condition");
    assert_eq!(out.text, "如果下雨，我就待在家里。");
    assert!(out.condition.is_some());

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
}
