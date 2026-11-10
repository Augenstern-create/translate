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

fn test_lexicon() -> translator_api::CompositionEngine {
    translator_api::CompositionEngine::from_file(
        root().join("fixtures/rules/en-zh.composition.json"),
    )
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

    // 关系级错误：交换主语/宾语节点的 source，Agent/Patient 必须拒绝。
    let mut swapped = output.chinese.clone();
    let mut nouns: Vec<usize> = swapped
        .nodes
        .iter()
        .filter(|node| node.kind == pipeline::ChineseNodeKind::Noun)
        .map(|node| node.id)
        .collect();
    nouns.sort_by_key(|id| swapped.nodes[*id].source);
    if nouns.len() >= 2 {
        let first = swapped.nodes[nouns[0]].source;
        swapped.nodes[nouns[0]].source = swapped.nodes[nouns[1]].source;
        swapped.nodes[nouns[1]].source = first;
        assert!(!pipeline::validate(&output.meaning, &swapped).valid);
    }

    // 否定作用域丢失必须拒绝。
    let mut no_neg = output.chinese.clone();
    for node in &mut no_neg.nodes {
        if node.kind == pipeline::ChineseNodeKind::Negation {
            node.source = None;
        }
    }
    assert!(!pipeline::validate(&output.meaning, &no_neg).valid);

    // 数量归属错误必须拒绝。
    let mut no_quantity = output.chinese.clone();
    for node in &mut no_quantity.nodes {
        if node.kind == pipeline::ChineseNodeKind::Quantity {
            node.source = None;
        }
    }
    assert!(!pipeline::validate(&output.meaning, &no_quantity).valid);

    let out = pipeline::analyze("She is reading a book.", &lexicon).expect("progressive");
    assert_eq!(out.text, "她正在读一本书。");
    let out = pipeline::analyze("I can swim.", &lexicon).expect("modal");
    assert_eq!(out.text, "我会游泳。");
    let out = pipeline::analyze("The door was opened by him.", &lexicon).expect("passive");
    assert_eq!(out.text, "门被他打开了。");
    let chart_graph = (pipeline::ChartParser { lexicon: &lexicon })
        .parse(&pipeline::tokenize_lossless(
            "She didn't buy two new books yesterday.",
        ))
        .expect("chart");
    assert!(chart_graph
        .edges
        .iter()
        .any(|edge| edge.relation == pipeline::SyntaxRelation::Head));
    let out = pipeline::analyze("I want to buy a book.", &lexicon).expect("want");
    assert_eq!(out.text, "我想买一本书。");
    let out = pipeline::analyze("If it rains, I will stay home.", &lexicon).expect("condition");
    assert_eq!(out.text, "如果下雨，我就待在家里。");
    assert!(out.condition.is_some());
    assert!(out
        .meaning
        .relations
        .iter()
        .any(|relation| { relation.kind == pipeline::MeaningRelationKind::Condition }));
    let mut bad = out.chinese.clone();
    bad.condition = None;
    assert!(!pipeline::validate(&out.meaning, &bad).valid);

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

fn semantic_roles(meaning: &pipeline::MeaningGraph) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for relation in &meaning.relations {
        let head = &meaning.nodes[relation.head];
        let dependent = &meaning.nodes[relation.dependent];
        out.push((
            format!("{:?}", relation.kind),
            format!("{}->{}", head.lemma, dependent.lemma),
        ));
    }
    out.sort();
    out
}

#[test]
fn 图变形不改变语义关系() {
    let lexicon = test_lexicon();
    let source = "She didn't buy two new books yesterday.";
    let stream = pipeline::tokenize_lossless(source);
    let graph = (pipeline::ChartParser { lexicon: &lexicon })
        .parse(&stream)
        .expect("chart");
    let transfer = pipeline::SemanticTransfer { lexicon: &lexicon };
    let original = transfer.transfer_graph(&graph).expect("meaning");
    let expected = semantic_roles(&original);
    assert!(expected
        .iter()
        .any(|(kind, value)| kind == "Agent" && value.contains("buy->she")));
    assert!(expected
        .iter()
        .any(|(kind, value)| kind == "Patient" && value.contains("buy->book")));
    assert!(expected.iter().any(|(kind, _)| kind == "Quantity"));
    assert!(expected.iter().any(|(kind, _)| kind == "Scope"));

    // 节点存储顺序打乱、ID 重映射后，语义角色必须不变。
    let n = graph.nodes.len();
    let mut reordered = pipeline::SyntaxGraph::default();
    for old in (0..n).rev() {
        let mut node = graph.nodes[old].clone();
        node.id = n - 1 - old;
        reordered.nodes.push(node);
    }
    for edge in &graph.edges {
        reordered.edges.push(pipeline::SyntaxEdge {
            head: n - 1 - edge.head,
            dependent: n - 1 - edge.dependent,
            relation: edge.relation.clone(),
        });
    }
    let reordered_meaning = transfer.transfer_graph(&reordered).expect("reordered");
    assert_eq!(semantic_roles(&reordered_meaning), expected);

    // 交换句法主语/宾语关系，语义 Agent/Patient 必须交换。
    let mut swapped = graph.clone();
    let clause = swapped
        .nodes
        .iter()
        .find(|node| node.kind == pipeline::SyntaxNodeKind::Clause)
        .unwrap()
        .id;
    let subject = swapped
        .edges
        .iter()
        .find(|edge| edge.head == clause && edge.relation == pipeline::SyntaxRelation::Subject)
        .unwrap()
        .dependent;
    let object = swapped
        .edges
        .iter()
        .find(|edge| edge.relation == pipeline::SyntaxRelation::Object)
        .unwrap()
        .dependent;
    for edge in &mut swapped.edges {
        if edge.head == clause && edge.relation == pipeline::SyntaxRelation::Subject {
            edge.dependent = object;
        }
        if edge.relation == pipeline::SyntaxRelation::Object {
            edge.dependent = subject;
        }
    }
    let swapped_meaning = transfer.transfer_graph(&swapped).expect("swapped");
    let roles = semantic_roles(&swapped_meaning);
    assert!(roles
        .iter()
        .any(|(kind, value)| kind == "Agent" && value.contains("buy->book")));
    assert!(roles
        .iter()
        .any(|(kind, value)| kind == "Patient" && value.contains("buy->she")));
}
