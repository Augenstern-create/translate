//! P19-G：组合式日常句型、上下文选义与安全回退。
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
        .cache_capacity(64)
        .build()
        .unwrap()
}

fn req(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, LanguageTag::new("zh-Hans").unwrap())
        .with_source(LanguageTag::new("en").unwrap())
}

fn out(text: &str) -> translator_api::TranslateOutput {
    let t = build();
    pollster::block_on(t.translate_with_diagnostics_options(
        &req(text),
        &TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence),
    ))
    .unwrap()
}

fn assert_rule(input: &str, expected: &str) {
    let got = out(input);
    assert_eq!(
        got.diagnostics.route,
        TranslationRoute::Rule,
        "{input} -> {}",
        got.result.text
    );
    assert!(got.diagnostics.rule_matched, "{input}");
    assert!(!got.diagnostics.model_invoked, "{input}");
    assert!(
        got.result.text.contains(expected),
        "{input} -> {}，期望包含 {expected}",
        got.result.text
    );
}

#[test]
fn 参数化礼貌祈使与不同宾语() {
    assert_rule("Please open the door.", "请打开门");
    assert_rule("Please close the window.", "请关窗");
    assert_rule("Please open the box.", "请打开盒子");
    assert_rule("Please restart the service.", "请重新启动服务");
    assert_rule("Please turn on the light.", "请打开灯");
}

#[test]
fn 简单陈述与主语宾语组合() {
    assert_rule("I like coffee.", "我喜欢咖啡");
    assert_rule("She reads books.", "她读书");
    assert_rule("The server uses memory.", "服务器使用内存");
}

#[test]
fn 意愿能力进行时与过去动作() {
    assert_rule("I want to buy a ticket.", "我想买一张票");
    assert_rule("She wants to eat an apple.", "她想吃一个苹果");
    assert_rule("I can swim.", "我会游泳");
    assert_rule("She is reading a book.", "她正在读一本书");
    assert_rule("He went home.", "他回家了");
    assert_rule("She booked a room.", "她预订了一个房间");
    assert_rule("I bought a ticket.", "我买了一张票");
}

#[test]
fn 否定疑问与be结构() {
    assert_rule("I don't know.", "我不知道");
    assert_rule("She doesn't like coffee.", "她不喜欢");
    assert_rule("Do you like tea?", "你喜欢茶吗");
    assert_rule("Where is my phone?", "我的手机在哪里");
    assert_rule("I am tired.", "我很累");
    assert_rule("I'm afraid of dogs.", "我害怕狗");
    assert_rule("If it rains, I'll stay home.", "如果下雨");
}

#[test]
fn 复杂句不套用简单规则而回退模型() {
    let got = out("Please open the door that was locked yesterday.");
    assert_eq!(got.diagnostics.route, TranslationRoute::LocalNmt);
    assert!(got.diagnostics.model_invoked);
    assert!(got.result.text.contains("Please open the door"));
}

#[test]
fn 短语优先于拆词() {
    let got = out("I am afraid of dogs.");
    assert_eq!(got.diagnostics.route, TranslationRoute::Rule);
    assert!(got.result.text.contains("害怕"));
    assert!(!got.result.text.contains("的狗"));
}

#[test]
fn 上下文词性选择book与light() {
    // book：名词“书” vs 动词“预订”；light：名词“灯” vs 形容词“轻”。
    assert_rule("I read a book.", "我读一本书");
    assert_rule("She booked a room.", "她预订了一个房间");
    assert_rule("Please turn on the light.", "请打开灯");
    assert_rule("The bag is light.", "包很轻");
    assert_rule("The bag is light blue.", "浅蓝色");
}

#[test]
fn 组合结果进规则缓存且不会跨策略复用() {
    let t = build();
    t.reset_route_stats();
    let options =
        TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
    let req = req("I like coffee.");
    let first = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
    assert_eq!(first.diagnostics.route, TranslationRoute::Rule);
    let second = pollster::block_on(t.translate_with_diagnostics_options(&req, &options)).unwrap();
    assert!(second.diagnostics.cache_hit);
    assert_eq!(t.route_stats().rule_matches, 1);

    let mut local = req.clone();
    local.policy = TranslationPolicy::LocalOnly;
    let local_out =
        pollster::block_on(t.translate_with_diagnostics_options(&local, &options)).unwrap();
    assert_eq!(local_out.diagnostics.route, TranslationRoute::LocalNmt);
    assert!(local_out.diagnostics.model_invoked);
}

#[test]
fn 分词保留原文跨度() {
    let engine = translator_api::CompositionEngine::from_file(
        root().join("fixtures/rules/en-zh.composition.json"),
    )
    .unwrap();
    let analysis = engine.analyze("She booked a room.");
    assert_eq!(
        analysis
            .tokens
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>(),
        vec!["she", "booked", "a", "room"]
    );
    assert_eq!(
        &"She booked a room."[analysis.tokens[1].start..analysis.tokens[1].end],
        "booked"
    );
    assert_eq!(
        &"She booked a room."[analysis.tokens[3].start..analysis.tokens[3].end],
        "room"
    );
}
