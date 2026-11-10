//! P20-C：局部语义帧、ChineseRealizer、候选排序与安全回退。
#![cfg(all(feature = "dictionary", feature = "mocks"))]

use std::path::PathBuf;

use translator_api::mocks::MockNmtEngine;
use translator_api::prelude::*;
use translator_api::{
    semantic::SemanticCompositionEngine, TranslateOptions, TranslationRoute, Translator,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn translator() -> Translator {
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
fn 语义组合对抗集() {
    let t = translator();
    let text =
        std::fs::read_to_string(root().join("fixtures/quality/semantic-adversarial-v1.jsonl"))
            .unwrap();
    let mut positive = 0;
    let mut negative = 0;
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
            positive += 1;
            assert_eq!(
                out.diagnostics.route,
                TranslationRoute::Rule,
                "{source}: {}",
                out.result.text
            );
            assert!(
                out.diagnostics.engine_id.starts_with("semantic:"),
                "{source} 应走 P20-C 语义层，实际引擎 {}",
                out.diagnostics.engine_id
            );
            let allowed: Vec<&str> = case["allowed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert!(
                allowed.contains(&out.result.text.as_str()),
                "{source} -> {}，不在允许译法 {:?}",
                out.result.text,
                allowed
            );
        } else {
            negative += 1;
            assert_eq!(
                out.diagnostics.route,
                TranslationRoute::LocalNmt,
                "{source} 不得命中规则：{}",
                out.result.text
            );
            assert!(out.diagnostics.model_invoked);
        }
    }
    assert_eq!(positive, 4);
    assert_eq!(negative, 3);
}

#[test]
fn 语义帧和中文生成器可独立检查() {
    let engine =
        SemanticCompositionEngine::from_file(root().join("fixtures/rules/en-zh.composition.json"))
            .unwrap();
    let entries = std::collections::HashMap::new();
    let analysis = engine.analyze("I don't want to go.", &entries);
    // 没有词典证据时不生成最终结果；拒绝原因保留。
    assert!(analysis.program.is_none());
    let analysis = engine.analyze("The light blue bag is heavy.", &entries);
    assert!(analysis.program.is_none());
    // 空词典不会伪造 frames；组合层必须得到正式词典证据。
    assert!(analysis.rejections.iter().any(|r| r.reason
        == translator_api::semantic::RejectionReason::UnsupportedPattern
        || r.reason == translator_api::semantic::RejectionReason::UnknownWord));
}

#[test]
fn 多语序候选由确定性排序选择() {
    // 直接验证 realizer 的候选，不依赖词典；中文生成器必须保留两个合法顺序。
    let frames = translator_api::semantic::MeaningProgram {
        frames: vec![translator_api::semantic::MeaningFrame {
            predicate: translator_api::semantic::Lexeme {
                word: "bought".into(),
                lemma: "buy".into(),
                zh: "买".into(),
                pos: "verb".into(),
                sense_id: None,
                targets: Vec::new(),
                span: translator_api::TextSpan::new(0, 0),
            },
            agent: Some(translator_api::semantic::EntityRef {
                head: translator_api::semantic::Lexeme {
                    word: "she".into(),
                    lemma: "she".into(),
                    zh: "她".into(),
                    pos: "pronoun".into(),
                    sense_id: None,
                    targets: Vec::new(),
                    span: translator_api::TextSpan::new(0, 0),
                },
                adjectives: vec![],
                determiner: None,
                quantity: None,
                coordinated: vec![],
                classifier: None,
                plural: false,
                span: translator_api::TextSpan::new(0, 0),
            }),
            patient: Some(translator_api::semantic::EntityRef {
                head: translator_api::semantic::Lexeme {
                    word: "phone".into(),
                    lemma: "phone".into(),
                    zh: "手机".into(),
                    pos: "noun".into(),
                    sense_id: None,
                    targets: Vec::new(),
                    span: translator_api::TextSpan::new(0, 0),
                },
                adjectives: vec![translator_api::semantic::Lexeme {
                    word: "new".into(),
                    lemma: "new".into(),
                    zh: "新".into(),
                    pos: "adjective".into(),
                    sense_id: None,
                    targets: Vec::new(),
                    span: translator_api::TextSpan::new(0, 0),
                }],
                determiner: Some(translator_api::semantic::Determiner::A),
                quantity: None,
                coordinated: vec![],
                classifier: Some("部".into()),
                plural: false,
                span: translator_api::TextSpan::new(0, 0),
            }),
            complement: None,
            collocation: None,
            comparison: None,
            modifiers: vec![translator_api::semantic::Modifier::Time("昨天".into())],
            polarity: translator_api::semantic::Polarity::Affirmative,
            negation_scope: None,
            tense_aspect: translator_api::semantic::TenseAspect::Past,
            question: translator_api::semantic::QuestionType::None,
            source_spans: vec![],
        }],
        relation: translator_api::semantic::ProgramRelation::Single,
    };
    let candidates = translator_api::ChineseRealizer.realize(&frames);
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].text, "她昨天买了一部新手机。");
    assert_eq!(candidates[1].text, "昨天她买了一部新手机。");
    assert_eq!(candidates[0].rank, 0);
    let selected = translator_api::semantic::CandidateSelector
        .select(&candidates)
        .unwrap();
    assert_eq!(selected.text, "她昨天买了一部新手机。");
}

#[test]
fn 语义结果进规则缓存且策略边界不被旁路() {
    let t = translator();
    t.reset_route_stats();
    let options =
        TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
    let req = TranslateRequest::new("I don't want to go.", LanguageTag::new("zh-Hans").unwrap())
        .with_source(LanguageTag::new("en").unwrap());
    let first = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
    assert_eq!(first.diagnostics.route, TranslationRoute::Rule);
    let second = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
    assert!(second.diagnostics.cache_hit);
    assert_eq!(t.route_stats().rule_matches, 1);

    let mut local = req.clone();
    local.policy = TranslationPolicy::LocalOnly;
    let out = pollster::block_on(t.translate_with_diagnostics_options(&local, &options)).unwrap();
    assert_eq!(out.diagnostics.route, TranslationRoute::LocalNmt);
    assert!(out.diagnostics.model_invoked);
}

#[test]
fn 候选验证拒绝错误否定范围() {
    let program = translator_api::semantic::MeaningProgram {
        frames: vec![translator_api::semantic::MeaningFrame {
            predicate: translator_api::semantic::Lexeme {
                word: "go".into(),
                lemma: "go".into(),
                zh: "去".into(),
                pos: "verb".into(),
                sense_id: None,
                targets: Vec::new(),
                span: translator_api::TextSpan::new(0, 0),
            },
            agent: Some(translator_api::semantic::EntityRef {
                head: translator_api::semantic::Lexeme {
                    word: "he".into(),
                    lemma: "he".into(),
                    zh: "他".into(),
                    pos: "pronoun".into(),
                    sense_id: None,
                    targets: Vec::new(),
                    span: translator_api::TextSpan::new(0, 0),
                },
                adjectives: vec![],
                determiner: None,
                quantity: None,
                coordinated: vec![],
                classifier: None,
                plural: false,
                span: translator_api::TextSpan::new(0, 0),
            }),
            patient: None,
            complement: None,
            collocation: None,
            comparison: None,
            modifiers: vec![],
            polarity: translator_api::semantic::Polarity::Negative,
            negation_scope: None,
            tense_aspect: translator_api::semantic::TenseAspect::Past,
            question: translator_api::semantic::QuestionType::None,
            source_spans: vec![],
        }],
        relation: translator_api::semantic::ProgramRelation::Single,
    };
    let selector = translator_api::semantic::CandidateSelector;
    let good = translator_api::semantic::RealizationCandidate {
        text: "他没去。".into(),
        rule_id: "good".into(),
        rank: 0,
        structure: None,
    };
    let bad = translator_api::semantic::RealizationCandidate {
        text: "他不想去。".into(),
        rule_id: "bad".into(),
        rank: 0,
        structure: None,
    };
    assert!(selector.validate(&program, &good).valid);
    let bad_result = selector.validate(&program, &bad);
    assert!(!bad_result.valid);
    assert!(bad_result.reasons.contains(&"wrong-negation"));
}

#[test]
fn 候选验证拒绝主客体交换() {
    let entity = |word: &str, zh: &str| translator_api::semantic::EntityRef {
        head: translator_api::semantic::Lexeme {
            word: word.into(),
            lemma: word.into(),
            zh: zh.into(),
            pos: "noun".into(),
            sense_id: None,
            targets: Vec::new(),
            span: translator_api::TextSpan::new(0, 0),
        },
        adjectives: vec![],
        determiner: None,
        quantity: None,
        coordinated: vec![],
        classifier: None,
        plural: false,
        span: translator_api::TextSpan::new(0, 0),
    };
    let program = translator_api::semantic::MeaningProgram {
        frames: vec![translator_api::semantic::MeaningFrame {
            predicate: translator_api::semantic::Lexeme {
                word: "chase".into(),
                lemma: "chase".into(),
                zh: "追".into(),
                pos: "verb".into(),
                sense_id: None,
                targets: Vec::new(),
                span: translator_api::TextSpan::new(0, 0),
            },
            agent: Some(entity("dog", "狗")),
            patient: Some(entity("cat", "猫")),
            complement: None,
            collocation: None,
            comparison: None,
            modifiers: vec![],
            polarity: translator_api::semantic::Polarity::Affirmative,
            negation_scope: None,
            tense_aspect: translator_api::semantic::TenseAspect::Simple,
            question: translator_api::semantic::QuestionType::None,
            source_spans: vec![],
        }],
        relation: translator_api::semantic::ProgramRelation::Single,
    };
    let selector = translator_api::semantic::CandidateSelector;
    let good = translator_api::semantic::RealizationCandidate {
        text: "狗追猫。".into(),
        rule_id: "good".into(),
        rank: 0,
        structure: None,
    };
    let bad = translator_api::semantic::RealizationCandidate {
        text: "猫追狗。".into(),
        rule_id: "bad".into(),
        rank: 0,
        structure: None,
    };
    assert!(selector.validate(&program, &good).valid);
    let bad_result = selector.validate(&program, &bad);
    assert!(!bad_result.valid);
    assert!(bad_result.reasons.contains(&"wrong-role-order"));
}

#[test]
fn 候选结构trace拒绝错误施事() {
    let entity = |word: &str, zh: &str| translator_api::semantic::EntityRef {
        head: translator_api::semantic::Lexeme {
            word: word.into(),
            lemma: word.into(),
            zh: zh.into(),
            pos: "noun".into(),
            span: translator_api::TextSpan::new(0, 0),
            sense_id: None,
            targets: vec![],
        },
        adjectives: vec![],
        determiner: None,
        quantity: None,
        coordinated: vec![],
        classifier: None,
        plural: false,
        span: translator_api::TextSpan::new(0, 0),
    };
    let program = translator_api::semantic::MeaningProgram {
        frames: vec![translator_api::semantic::MeaningFrame {
            predicate: translator_api::semantic::Lexeme {
                word: "chase".into(),
                lemma: "chase".into(),
                zh: "追".into(),
                pos: "verb".into(),
                span: translator_api::TextSpan::new(0, 0),
                sense_id: None,
                targets: vec![],
            },
            agent: Some(entity("dog", "狗")),
            patient: Some(entity("cat", "猫")),
            complement: None,
            collocation: None,
            comparison: None,
            modifiers: vec![],
            polarity: translator_api::semantic::Polarity::Affirmative,
            negation_scope: None,
            tense_aspect: translator_api::semantic::TenseAspect::Simple,
            question: translator_api::semantic::QuestionType::None,
            source_spans: vec![],
        }],
        relation: translator_api::semantic::ProgramRelation::Single,
    };
    let bad = translator_api::semantic::RealizationCandidate {
        text: "猫追狗。".into(),
        rule_id: "bad".into(),
        rank: 0,
        structure: Some(translator_api::semantic::ChineseStructure {
            clauses: vec![translator_api::semantic::ChineseClause {
                subject: "猫".into(),
                predicate: "追".into(),
                object: "狗".into(),
                time: None,
                negation: String::new(),
                modal: String::new(),
                aspect: String::new(),
                passive_agent: None,
            }],
            relation: translator_api::semantic::ChineseRelation::Single,
            condition: None,
        }),
    };
    let result = translator_api::semantic::CandidateSelector.validate(&program, &bad);
    assert!(!result.valid);
    assert!(result.reasons.contains(&"trace-subject-mismatch"));
}
