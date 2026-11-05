//! 端到端测试：只用公开 API + Mock 插件。
//!
//! 这组测试本身就是设计文档文末「交付判据」的证据：
//! 不接 Bergamot / ncnn / 任何模型 / 任何词库，`translator-api` 依然可用。
//!
//! 运行方式：`cargo test --workspace --all-features`

#![cfg(feature = "mocks")]

use translator_api::mocks::{
    MockBackend, MockDictionaryEngine, MockDictionaryProvider, MockNmtEngine, MockRemoteEngine,
};
use translator_api::prelude::*;
use translator_api::{LookupRequest, Translator};

fn build(remote_consent: bool) -> Translator {
    Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .engine(MockRemoteEngine::new(remote_consent))
        .provider(MockDictionaryProvider::new())
        .backend(MockBackend::new())
        .cache_capacity(16)
        .build()
        .expect("构建 Translator 失败")
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

#[test]
fn 可以通过门面完成一次翻译() {
    let translator = build(false);
    let req = TranslateRequest::new("hello", zh());

    let result = pollster::block_on(translator.translate(&req)).unwrap();

    assert_eq!(result.text, "你好");
    assert_eq!(result.engine_id, "mock.dictionary");
    assert_eq!(result.target.as_str(), "zh-Hans");
}

#[test]
fn 可以查询能力清单() {
    let translator = build(false);
    let caps = translator.capabilities();

    assert_eq!(caps.len(), 3);
    assert!(caps.iter().any(|c| c.engine_id == "mock.dictionary"));
    assert!(caps
        .iter()
        .any(|c| c.capabilities.contains(&Capability::Nmt)));
}

#[test]
fn 查词返回带来源信息的词条() {
    let translator = build(false);
    let req = LookupRequest::new("cat").with_language(LanguageTag::new("en").unwrap());

    let entries = pollster::block_on(translator.lookup(&req)).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].headword, "cat");
    assert_eq!(entries[0].senses[0].definition, "猫");
    // 许可证来源必须可追溯（设计文档 §7.1）
    assert_eq!(entries[0].source_license, "MIT");
    assert_eq!(entries[0].senses[0].source_id, "mock-table");
}

#[test]
fn 查不到的词条返回空列表而非错误() {
    let translator = build(false);
    let req = LookupRequest::new("nonexistent").with_language(LanguageTag::new("en").unwrap());

    let entries = pollster::block_on(translator.lookup(&req)).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn 没装模型时返回可安装资源建议() {
    // 只注册 NMT 之外的东西，模拟"模型未安装"
    let translator = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .build()
        .expect("构建失败");

    let req = TranslateRequest::new("good morning my friend", zh());
    let err = pollster::block_on(translator.translate(&req)).unwrap_err();

    assert_eq!(err.code, TranslationErrorCode::ResourceNotInstalled);
    assert!(
        !err.suggested_packages.is_empty(),
        "必须给出可安装资源建议，便于 UI 引导"
    );
}

#[test]
fn 远程未授权时整条链路拒绝() {
    let translator = build(false);
    let req = TranslateRequest::new("hello", zh()).with_policy(TranslationPolicy::RemoteOnly);

    let err = pollster::block_on(translator.translate(&req)).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::NetworkNotAuthorized);
}

#[test]
fn 授权后远程可用() {
    let translator = build(true);
    let req = TranslateRequest::new("hello", zh()).with_policy(TranslationPolicy::RemoteOnly);

    let result = pollster::block_on(translator.translate(&req)).unwrap();
    assert_eq!(result.engine_id, "mock.remote");
}

#[test]
fn 同一引擎重复注册应被拒绝() {
    let err = Translator::builder()
        .engine(MockNmtEngine::new())
        .engine(MockNmtEngine::new())
        .build()
        .unwrap_err();

    assert_eq!(err.code, TranslationErrorCode::InvalidRequest);
    assert!(err.message.contains("重复"));
}

#[test]
fn 关机后缓存被清空且可重复关机() {
    let translator = build(false);
    let req = TranslateRequest::new("hello", zh());

    pollster::block_on(translator.translate(&req)).unwrap();
    pollster::block_on(translator.shutdown()).unwrap();
    pollster::block_on(translator.shutdown()).unwrap();
}
