//! 路由与协调器行为测试。
//!
//! 对应设计文档 §6「AutoLocalFirst 的推荐决策」与 §12.1「自动化层次」第 1、3 项。

use std::sync::Arc;

use translator_core::{Coordinator, CoordinatorConfig};
use translator_plugin_api::{
    mocks::{MockDictionaryEngine, MockNmtEngine, MockRemoteEngine},
    PluginRegistry,
};
use translator_types::{
    Capability, LanguageTag, TranslateRequest, TranslationErrorCode, TranslationPolicy,
};

fn registry() -> Arc<PluginRegistry> {
    let mut registry = PluginRegistry::new();
    registry
        .register_engine(MockDictionaryEngine::new())
        .expect("注册词典引擎");
    registry
        .register_engine(MockNmtEngine::new())
        .expect("注册 NMT 引擎");
    registry
        .register_engine(MockRemoteEngine::new(false))
        .expect("注册远程引擎");
    Arc::new(registry)
}

fn coordinator() -> Coordinator {
    Coordinator::new(registry(), CoordinatorConfig::default())
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

#[test]
fn 单词应路由到词典() {
    let coord = coordinator();
    let req = TranslateRequest::new("hello", zh());

    let decision = coord.explain_route(&req).unwrap();
    assert_eq!(decision.engine_id, "mock.dictionary");
    assert_eq!(decision.capability, Capability::Dictionary);
}

#[test]
fn 整句应路由到本地nmt() {
    let coord = coordinator();
    let req = TranslateRequest::new("good morning my friend", zh());

    let decision = coord.explain_route(&req).unwrap();
    assert_eq!(decision.engine_id, "mock.nmt");
    assert_eq!(decision.capability, Capability::Nmt);
}

#[test]
fn 无词典覆盖的语言对应退化到nmt() {
    let coord = coordinator();
    // 词典只支持中英，ja 目标应落到 NMT
    let ja = LanguageTag::new("ja").unwrap();
    let req = TranslateRequest::new("hello", ja);

    let decision = coord.explain_route(&req).unwrap();
    assert_eq!(decision.engine_id, "mock.nmt");
}

#[test]
fn localonly策略下不得选择远程() {
    let coord = coordinator();
    let req = TranslateRequest::new("hello", zh()).with_policy(TranslationPolicy::LocalOnly);

    let decision = coord.explain_route(&req).unwrap();
    assert_ne!(decision.capability, Capability::Remote);
}

#[test]
fn remoteonly策略应选中远程引擎() {
    let coord = coordinator();
    let req = TranslateRequest::new("hello", zh()).with_policy(TranslationPolicy::RemoteOnly);

    let decision = coord.explain_route(&req).unwrap();
    assert_eq!(decision.capability, Capability::Remote);
}

#[test]
fn 指定不存在的引擎应报错() {
    let coord = coordinator();
    let req = TranslateRequest::new("hello", zh())
        .with_policy(TranslationPolicy::ExplicitEngine("no.such.engine".into()));

    let err = coord.explain_route(&req).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
}

#[test]
fn 词典未命中应自动回退到nmt() {
    let coord = coordinator();
    // "elephant" 是单词形态（会先选词典），但词表里没有
    let req = TranslateRequest::new("elephant", zh());

    let result = pollster::block_on(coord.translate(&req)).unwrap();

    assert_eq!(result.engine_id, "mock.nmt", "应回退到 NMT");
    assert!(result.text.contains("elephant"));
}

#[test]
fn 词典命中不应回退() {
    let coord = coordinator();
    let req = TranslateRequest::new("cat", zh());

    let result = pollster::block_on(coord.translate(&req)).unwrap();
    assert_eq!(result.engine_id, "mock.dictionary");
    assert_eq!(result.text, "猫");
}

#[test]
fn 远程引擎未授权时应拒绝() {
    let coord = coordinator();
    let req = TranslateRequest::new("hello", zh()).with_policy(TranslationPolicy::RemoteOnly);

    let err = pollster::block_on(coord.translate(&req)).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::NetworkNotAuthorized);
}

#[test]
fn 源语言与目标语言相同应报错() {
    let coord = coordinator();
    let req = TranslateRequest::new("hello", en()).with_source(en());

    let err = pollster::block_on(coord.translate(&req)).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::InvalidRequest);
}

#[test]
fn 空文本应报错() {
    let coord = coordinator();
    let req = TranslateRequest::new("   ", zh());

    let err = pollster::block_on(coord.translate(&req)).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::InvalidRequest);
}

#[test]
fn 第二次相同请求应命中缓存() {
    let coord = coordinator();
    let req = TranslateRequest::new("good morning my friend", zh());

    let first = pollster::block_on(coord.translate(&req)).unwrap();
    assert!(
        !first
            .warnings
            .iter()
            .any(|w| matches!(w, translator_types::TranslationWarning::CacheHit)),
        "首次不应命中缓存"
    );

    let second = pollster::block_on(coord.translate(&req)).unwrap();
    assert!(
        second
            .warnings
            .iter()
            .any(|w| matches!(w, translator_types::TranslationWarning::CacheHit)),
        "第二次应命中缓存"
    );
    assert_eq!(second.elapsed_ms, 0);
    assert_eq!(first.text, second.text);
}

#[test]
fn 占位符应被保护并完整还原() {
    let coord = coordinator();
    let req = TranslateRequest::new("email me at {address} now", zh());

    let result = pollster::block_on(coord.translate(&req)).unwrap();

    assert!(result.text.contains("{address}"), "占位符必须还原");
    assert!(
        !result.warnings.iter().any(|w| matches!(
            w,
            translator_types::TranslationWarning::PlaceholderRestoreFailed(_)
        )),
        "不应产生还原失败警告"
    );
}
