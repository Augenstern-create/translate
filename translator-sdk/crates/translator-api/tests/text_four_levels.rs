//! P12-Q：Word / Sentence / Paragraph / Document 四级文本策略。
//!
//! 只用公开 API + Mock 插件验证组织方式与结构还原；真实模型质量在
//! `tests/quality/` 与 `docs/63` 中评估。

#![cfg(feature = "mocks")]

use translator_api::mocks::{MockDictionaryEngine, MockNmtEngine};
use translator_api::prelude::*;
use translator_api::{ParagraphStrategy, TextKind, TranslateOptions, TranslateRequest, Translator};

fn build() -> Translator {
    Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .cache_capacity(32)
        .build()
        .expect("构建 Translator 失败")
}

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn request(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, zh()).with_source(en())
}

#[test]
fn word_走词典并返回多义项() {
    let t = build();
    let options = TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Word);
    let out = pollster::block_on(t.translate_with_diagnostics_options(&request("hello"), &options))
        .unwrap();
    assert_eq!(out.result.text, "你好");
    assert_eq!(out.diagnostics.text_kind, TextKind::Word);
    assert_eq!(out.diagnostics.strategy, "word");
}

#[test]
fn sentence_保持单路径() {
    let t = build();
    let options =
        TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
    let out = pollster::block_on(
        t.translate_with_diagnostics_options(&request("Please restart the server."), &options),
    )
    .unwrap();
    assert!(
        out.result.text.contains("Please restart the server."),
        "{}",
        out.result.text
    );
    assert_eq!(out.diagnostics.text_kind, TextKind::Sentence);
    assert_eq!(out.diagnostics.strategy, "sentence");
}

#[test]
fn paragraph_逐句策略保留句子顺序与分隔() {
    let t = build();
    let options = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Paragraph)
        .with_paragraph_strategy(ParagraphStrategy::Sentence);
    let out = pollster::block_on(t.translate_with_diagnostics_options(
        &request("First sentence. Second sentence."),
        &options,
    ))
    .unwrap();
    assert_eq!(out.diagnostics.text_kind, TextKind::Paragraph);
    assert_eq!(out.diagnostics.strategy, "paragraph-sentence");
    let first = out.result.text.find("First sentence").expect("第一句");
    let second = out.result.text.find("Second sentence").expect("第二句");
    assert!(first < second, "顺序必须保持：{}", out.result.text);
}

#[test]
fn paragraph_句群数量可预测() {
    let t = build();
    let options = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Paragraph)
        .with_paragraph_strategy(ParagraphStrategy::Group);
    let out = pollster::block_on(
        t.translate_with_diagnostics_options(&request("One. Two. Three. Four."), &options),
    )
    .unwrap();
    assert_eq!(out.diagnostics.strategy, "paragraph-group");
    // Mock NMT 每个句群输出一次 `[nmt ...]`，4 句 → 2 组。
    assert_eq!(
        out.result.text.matches("[nmt").count(),
        2,
        "{}",
        out.result.text
    );
}

#[test]
fn auto_多句自动按段落处理() {
    let t = build();
    let options = TranslateOptions::new(TranslationMode::Precise);
    let out = pollster::block_on(t.translate_with_diagnostics_options(
        &request("First sentence. Second sentence."),
        &options,
    ))
    .unwrap();
    assert_eq!(out.diagnostics.text_kind, TextKind::Paragraph);
    assert!(out.diagnostics.strategy.starts_with("paragraph-"));
    assert_ne!(
        out.diagnostics.strategy, "paragraph-best",
        "2 句普通段落不应为候选比较付 3× 延迟"
    );
    assert!(out.result.text.contains("First sentence"));
    assert!(out.result.text.contains("Second sentence"));
}

#[test]
fn auto_三句段落使用_quality_budget() {
    let t = build();
    let options = TranslateOptions::new(TranslationMode::Precise);
    let out = pollster::block_on(t.translate_with_diagnostics_options(
        &request("First sentence. Second sentence. Third sentence."),
        &options,
    ))
    .unwrap();
    assert_eq!(out.diagnostics.strategy, "paragraph-best");
}

#[test]
fn paragraph_best_选择候选并标记策略() {
    let t = build();
    let options = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Paragraph)
        .with_paragraph_strategy(ParagraphStrategy::Best);
    let out = pollster::block_on(t.translate_with_diagnostics_options(
        &request("First sentence. Second sentence."),
        &options,
    ))
    .unwrap();
    assert_eq!(out.diagnostics.strategy, "paragraph-best");
    assert!(out.result.text.contains("First sentence"));
    assert!(out.result.text.contains("Second sentence"));
}

#[test]
fn paragraph_context_只取当前句并标记策略() {
    let t = build();
    let options = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Paragraph)
        .with_paragraph_strategy(ParagraphStrategy::Context);
    let out = pollster::block_on(t.translate_with_diagnostics_options(
        &request("First sentence. Second sentence."),
        &options,
    ))
    .unwrap();
    assert_eq!(out.diagnostics.strategy, "paragraph-context");
    assert!(out.result.text.contains("Second sentence"));
}

#[test]
fn document_保留标题列表前缀和代码块() {
    let t = build();
    let input = "# Title\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```\n";
    let options =
        TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Document);
    let out = pollster::block_on(t.translate_with_diagnostics_options(&request(input), &options))
        .unwrap();
    assert_eq!(out.diagnostics.text_kind, TextKind::Document);
    assert_eq!(out.diagnostics.strategy, "document");
    assert!(
        out.result.text.contains("# "),
        "标题前缀丢失：{}",
        out.result.text
    );
    assert!(
        out.result.text.contains("- "),
        "列表前缀丢失：{}",
        out.result.text
    );
    assert!(
        out.result.text.contains("```rust\nfn main() {}\n```"),
        "代码块必须逐字节保留：{}",
        out.result.text
    );
}

#[test]
fn 文本类型与策略进入缓存身份() {
    let t = build();
    let req = request("First sentence. Second sentence.");
    let sentence =
        TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
    let paragraph = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Paragraph)
        .with_paragraph_strategy(ParagraphStrategy::Whole);
    let a = pollster::block_on(t.translate_with_diagnostics_options(&req, &sentence)).unwrap();
    let b = pollster::block_on(t.translate_with_diagnostics_options(&req, &paragraph)).unwrap();
    assert!(!a.diagnostics.cache_hit);
    assert!(!b.diagnostics.cache_hit, "不同文本类型/策略不得共享缓存");
    let a2 = pollster::block_on(t.translate_with_diagnostics_options(&req, &sentence)).unwrap();
    assert!(a2.diagnostics.cache_hit);
}

#[test]
fn 上下文身份进入缓存键() {
    let t = build();
    let req = request("The server is ready.");
    let base = TranslateOptions::new(TranslationMode::Precise).with_text_kind(TextKind::Sentence);
    let ctx = base.clone().with_context_id("doc-1#p3");
    let a = pollster::block_on(t.translate_with_diagnostics_options(&req, &base)).unwrap();
    let b = pollster::block_on(t.translate_with_diagnostics_options(&req, &ctx)).unwrap();
    assert!(!a.diagnostics.cache_hit);
    assert!(!b.diagnostics.cache_hit, "不同上下文不得共享缓存");
}
