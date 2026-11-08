//! T4：Precise / Fast 模式契约（docs/57 §4）。
//!
//! 这里只验证**公开 API 合同**：
//! - 默认 Precise；Fast 必须显式选择；
//! - 单请求覆盖不影响 Builder 默认值；
//! - 模式进入缓存身份，两种模式不互相复用结果；
//! - 旧 `TranslateRequest` 结构体字面量仍然编译（不新增字段）。

#![cfg(feature = "mocks")]

use translator_api::mocks::{MockDictionaryEngine, MockNmtEngine};
use translator_api::prelude::*;
use translator_api::{TranslateRequest, TranslationMode, Translator};

fn build(mode: Option<TranslationMode>) -> Translator {
    let mut builder = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .cache_capacity(8);
    if let Some(mode) = mode {
        builder = builder.translation_mode(mode);
    }
    builder.build().expect("构建 Translator 失败")
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

#[test]
fn 默认模式是精准模式() {
    let translator = build(None);
    assert_eq!(translator.translation_mode(), TranslationMode::Precise);
}

#[test]
fn 可以显式选择快速模式() {
    let translator = build(Some(TranslationMode::Fast));
    assert_eq!(translator.translation_mode(), TranslationMode::Fast);
}

#[test]
fn 单请求覆盖不改变默认模式() {
    let translator = build(None);
    let req = TranslateRequest::new("good morning", zh());
    let out =
        pollster::block_on(translator.translate_with_diagnostics_mode(&req, TranslationMode::Fast))
            .unwrap();
    assert_eq!(out.diagnostics.mode, TranslationMode::Fast);
    assert_eq!(translator.translation_mode(), TranslationMode::Precise);
}

#[test]
fn 两种模式不共享缓存() {
    let translator = build(None);
    let req = TranslateRequest::new("the library opens at nine", zh());

    let precise =
        pollster::block_on(translator.translate_with_diagnostics(&req)).expect("Precise 失败");
    assert_eq!(precise.diagnostics.mode, TranslationMode::Precise);
    assert!(!precise.diagnostics.cache_hit, "首次请求不应命中缓存");

    let fast =
        pollster::block_on(translator.translate_with_diagnostics_mode(&req, TranslationMode::Fast))
            .expect("Fast 失败");
    assert_eq!(fast.diagnostics.mode, TranslationMode::Fast);
    assert!(
        !fast.diagnostics.cache_hit,
        "Fast 不得命中 Precise 的缓存（docs/57 §4.2）"
    );

    let precise_again = pollster::block_on(translator.translate_with_diagnostics(&req))
        .expect("第二次 Precise 失败");
    assert!(
        precise_again.diagnostics.cache_hit,
        "同模式重复请求应命中缓存"
    );
}

#[test]
fn 旧结构体字面量仍然编译() {
    // 这是兼容性合同本身：给 TranslateRequest 加字段会让这段代码无法编译。
    let req = TranslateRequest {
        text: "hello".to_string(),
        source: None,
        target: zh(),
        policy: TranslationPolicy::default(),
        glossary_id: None,
        timeout_ms: None,
    };
    let translator = build(None);
    let result = pollster::block_on(translator.translate(&req)).unwrap();
    assert_eq!(result.target, zh());
}

#[test]
fn 快速模式仍检查数字完整性() {
    let translator = build(Some(TranslationMode::Fast));
    // mock NMT 会原样回显文本，数字自然保留；这里验证 Fast 的诊断里有 quality 记录。
    let req = TranslateRequest::new("release 2.10 at 09:30", zh());
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).unwrap();
    let quality = out.diagnostics.quality.expect("Fast 也必须跑数字检查");
    assert_eq!(quality.mode, TranslationMode::Fast);
    assert_eq!(quality.numbers_preserved, quality.numbers_in_source);
}
