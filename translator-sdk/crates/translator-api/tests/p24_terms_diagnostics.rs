//! P2.4 端到端测试：输出侧术语约束与结构化诊断。
//!
//! 三条对外承诺：
//!
//! 1. 术语约束只改**裸文本**，URL / 代码 / 模板变量里的内容一个字节都不动；
//! 2. `translate()` 与 `translate_with_diagnostics()` 给出**同一份译文**
//!    （诊断是附加信息，不是第二条流水线）；
//! 3. 诊断在缓存命中时与首次一致——不能"第二次就好了"。
//!
//! 运行方式：`cargo test --all-features`

#![cfg(all(feature = "mocks", feature = "rules"))]

use std::path::PathBuf;

use translator_api::mocks::MockNmtEngine;
use translator_api::prelude::*;
use translator_api::{TermPolicy, TranslationWarning, Translator};

fn table_json() -> &'static str {
    r#"{
        "schema_version": 1,
        "id": "terms.test.zh",
        "version": 2,
        "terms": [
            { "target": "zh-Hans", "preferred": "人工智能", "variants": ["AI"] },
            { "target": "zh-Hans", "preferred": "计算机", "variants": ["computer"] }
        ]
    }"#
}

/// 把术语表写到临时目录，返回路径（调用方负责保活 TempDir）。
fn write_table(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("terms.json");
    std::fs::write(&path, table_json()).unwrap();
    path
}

fn rules_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/rules/minimal.json")
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

fn builder() -> translator_api::TranslatorBuilder {
    Translator::builder()
        .engine(MockNmtEngine::new())
        .cache_capacity(8)
}

// ---------------- 术语约束 ----------------

#[test]
fn 术语约束会改写译文并报出位置() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder()
        .term_table_file(&table)
        .term_policy(TermPolicy::Enforce) // 改写必须显式开启（默认是 Warn）
        .build()
        .unwrap();

    let req = TranslateRequest::new("hello AI world", zh()).with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();

    assert!(
        out.result.text.contains("人工智能"),
        "实际：{}",
        out.result.text
    );
    assert!(!out.result.text.contains("AI"), "变体必须被替换掉");
    let term = out.diagnostics.term.expect("配了术语表就该有 trace");
    assert_eq!(term.id, "terms.test.zh");
    assert_eq!(term.version, 2);
    assert_eq!(term.policy, TermPolicy::Enforce);
    assert_eq!(term.replaced, 1);
    assert_eq!(term.violations.len(), 1);
    // 偏移指向输出文本
    let v = &term.violations[0];
    assert!(
        out.result.text[v.offset..].starts_with("人工智能"),
        "偏移 {} 处应是替换后的术语：{}",
        v.offset,
        out.result.text
    );
    // 警告只带事实，不带位置（位置在诊断里）
    assert!(out
        .result
        .warnings
        .iter()
        .any(|w| matches!(w, TranslationWarning::TermMismatch(_))));
}

#[test]
fn 默认策略只报告不改写() {
    // P2-S / S-03：默认必须是 Warn——不能悄悄改写用户译文
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder().term_table_file(&table).build().unwrap();
    assert_eq!(translator.term_policy(), TermPolicy::Warn);

    let req = TranslateRequest::new("hello AI", zh()).with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    assert!(
        out.result.text.contains("AI"),
        "默认不得改写：{}",
        out.result.text
    );
    let term = out.diagnostics.term.unwrap();
    assert_eq!(term.policy, TermPolicy::Warn);
    assert_eq!(term.violations.len(), 1, "仍然要报告违规");
    assert_eq!(term.replaced, 0);
}

#[test]
fn warn策略只报告不改文本() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder()
        .term_table_file(&table)
        .term_policy(TermPolicy::Warn)
        .build()
        .unwrap();

    let req = TranslateRequest::new("hello AI", zh()).with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    assert!(out.result.text.contains("AI"), "Warn 模式不得改文本");
    let term = out.diagnostics.term.unwrap();
    assert_eq!(term.violations.len(), 1, "违规照样要报");
    assert_eq!(term.replaced, 0);
    assert_eq!(term.policy, TermPolicy::Warn);
}

#[test]
fn off策略不跑守卫() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder()
        .term_table_file(&table)
        .term_policy(TermPolicy::Off)
        .build()
        .unwrap();

    let req = TranslateRequest::new("hello AI", zh()).with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    assert!(out.result.text.contains("AI"));
    assert!(out.diagnostics.term.is_none(), "Off 时连扫描都不该做");
    assert!(!out
        .result
        .warnings
        .iter()
        .any(|w| matches!(w, TranslationWarning::TermMismatch(_))));
}

#[test]
fn 受保护跨度里的内容不被术语改写() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder()
        .term_table_file(&table)
        .term_policy(TermPolicy::Enforce)
        .build()
        .unwrap();

    let req = TranslateRequest::new(
        "see https://example.invalid/AI and `let AI = 1` and {AI} but AI free",
        zh(),
    )
    .with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();

    let text = &out.result.text;
    assert!(
        text.contains("https://example.invalid/AI"),
        "URL 被改了：{text}"
    );
    assert!(text.contains("`let AI = 1`"), "代码被改了：{text}");
    assert!(text.contains("{AI}"), "模板变量被改了：{text}");
    assert!(text.contains("but 人工智能 free"), "裸文本没改：{text}");
    assert_eq!(
        out.diagnostics.term.unwrap().violations.len(),
        1,
        "只有裸文本那一处算违规"
    );
    // 受保护跨度也进了诊断
    assert_eq!(out.diagnostics.protected_spans.len(), 3);
}

#[test]
fn 没有术语表时诊断里没有term字段() {
    let translator = builder().build().unwrap();
    let req = TranslateRequest::new("hello AI", zh()).with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    assert!(out.diagnostics.term.is_none());
    assert!(translator.explain_terms("AI", &en(), &zh()).is_none());
}

#[test]
fn 术语预览不改动也不翻译() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder().term_table_file(&table).build().unwrap();

    let preview = translator
        .explain_terms("AI 与 computer", &en(), &zh())
        .unwrap();
    assert_eq!(preview.text, "人工智能 与 计算机");
    assert_eq!(preview.violations.len(), 2);
}

// ---------------- 诊断 ----------------

#[test]
fn 两条入口给出同一份译文() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder()
        .term_table_file(&table)
        .rules_file(rules_fixture())
        .build()
        .unwrap();

    let req = TranslateRequest::new("hello AI world", zh()).with_source(en());
    let plain = pollster::block_on(translator.translate(&req)).unwrap();
    // 先清缓存，保证两次都是真实执行（第二次否则会多一条 CacheHit 警告）
    translator.clear_cache();
    let diag = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    assert_eq!(plain.text, diag.result.text, "诊断不得改变译文");
    assert_eq!(plain.warnings, diag.result.warnings);
}

#[test]
fn 诊断带出规则命中与路由理由() {
    let translator = builder().rules_file(rules_fixture()).build().unwrap();
    let req = TranslateRequest::new("robot", zh()).with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();

    let rules = out.diagnostics.rules.expect("配了规则包就该有 trace");
    assert!(rules.id.contains("minimal"), "实际：{}", rules.id);
    assert_eq!(rules.version, 1);
    assert!(!out.diagnostics.route_reason.is_empty());
    assert_eq!(out.diagnostics.rules_version, 1);
}

#[test]
fn 诊断记录源语言是否为猜测() {
    let translator = builder().build().unwrap();

    let explicit = pollster::block_on(
        translator
            .translate_with_diagnostics(&TranslateRequest::new("hello", zh()).with_source(en())),
    )
    .unwrap();
    assert!(!explicit.diagnostics.source_assumed);

    // 不指定 source：由启发式猜出 en
    let guessed = pollster::block_on(
        translator.translate_with_diagnostics(&TranslateRequest::new("hello world", zh())),
    )
    .unwrap();
    assert!(guessed.diagnostics.source_assumed);
    assert_eq!(guessed.result.source, en());
}

#[test]
fn 缓存命中时诊断与首次一致() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder().term_table_file(&table).build().unwrap();

    let req = TranslateRequest::new("hello AI world", zh()).with_source(en());
    let first = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    assert!(!first.diagnostics.cache_hit);

    let second = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    assert!(second.diagnostics.cache_hit, "第二次应命中缓存");
    assert_eq!(second.result.text, first.result.text);
    assert_eq!(
        second.diagnostics.term.as_ref().unwrap().violations,
        first.diagnostics.term.as_ref().unwrap().violations,
        "缓存命中不得让违规消失"
    );
    assert_eq!(
        second.diagnostics.protected_spans,
        first.diagnostics.protected_spans
    );
    assert!(second
        .result
        .warnings
        .iter()
        .any(|w| matches!(w, TranslationWarning::CacheHit)));
}

#[test]
fn 术语表版本进入资源指纹() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder().term_table_file(&table).build().unwrap();
    let req = TranslateRequest::new("hello", zh()).with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    assert!(
        out.diagnostics
            .resource_fingerprint
            .contains("terms.test.zh@2"),
        "指纹里必须有术语表版本：{}",
        out.diagnostics.resource_fingerprint
    );
}

#[test]
fn 摘要可读且包含关键项() {
    let dir = tempfile::tempdir().unwrap();
    let table = write_table(dir.path());
    let translator = builder().term_table_file(&table).build().unwrap();
    let req = TranslateRequest::new("AI", zh()).with_source(en());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    let summary = out.diagnostics.summary();
    assert!(summary.contains("mock.nmt"), "{summary}");
    assert!(summary.contains("术语="), "{summary}");
}
