//! P17-A / P20-K：学习链隐私门、后台记录主链不被阻塞。
#![cfg(all(feature = "mocks", feature = "learning"))]

use std::sync::Arc;

use translator_api::mocks::{MockDictionaryEngine, MockNmtEngine};
use translator_api::prelude::*;
use translator_api::{
    learning::{LearningConsent, LearningStore, PrivacyLevel},
    Translator,
};

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}
fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}
fn req(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, zh()).with_source(en())
}

fn store(dir: &tempfile::TempDir) -> Arc<LearningStore> {
    Arc::new(LearningStore::open(dir.path().join("learning.sqlite")).unwrap())
}

fn translator(store: Arc<LearningStore>, consent: LearningConsent) -> Translator {
    Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .learning(store, consent)
        .build()
        .unwrap()
}

#[test]
fn 隐私关闭时不启动学习运行时() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("learning.sqlite");
    let store = Arc::new(LearningStore::open(&path).unwrap());
    // Off 仍允许宿主打开库用于查看，但 Translator 不应启动记录线程。
    let t = translator(store, LearningConsent::default());
    pollster::block_on(t.translate(&req("memory footprint"))).unwrap();
    t.flush_learning();
    assert!(t.learning_store().is_none());
    assert!(t.learning_stats().is_none());
}

#[test]
fn 本地授权记录源文但不记录模型译文() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let t = translator(store.clone(), LearningConsent::local());
    pollster::block_on(t.translate(&req("memory footprint"))).unwrap();
    t.flush_learning();

    let stats = store.stats().unwrap();
    assert_eq!(stats.records, 1);
    assert_eq!(stats.records_with_source_text, 1);
    assert_eq!(stats.records_with_translation, 0, "模型译文不是可信标签");
    let runtime = t.learning_stats().unwrap();
    assert_eq!(runtime.recorded, 1);
    assert_eq!(runtime.errors, 0);
}

#[test]
fn 授权语料才保存候选译文并仍需审核() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let t = translator(store.clone(), LearningConsent::authorized_corpus());
    pollster::block_on(t.translate(&req("memory footprint"))).unwrap();
    t.flush_learning();

    let stats = store.stats().unwrap();
    assert_eq!(stats.records_with_translation, 1);
    let candidates = store.list_candidates(None, 10).unwrap();
    if !candidates.is_empty() {
        assert!(
            candidates
                .iter()
                .all(|c| c.status != translator_api::learning::ReviewStatus::Verified),
            "自动提取的候选绝不能被当成已验证知识"
        );
    }
}

#[test]
fn 敏感凭据不进入学习库() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let t = translator(store.clone(), LearningConsent::local());
    pollster::block_on(t.translate(&req("Authorization: Bearer secret-token-value"))).unwrap();
    t.flush_learning();

    assert_eq!(store.stats().unwrap().records, 0);
    assert_eq!(t.learning_stats().unwrap().skipped, 1);
}

#[test]
fn 词典命中不产生学习记录() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let t = translator(store.clone(), LearningConsent::local());
    pollster::block_on(t.translate(&req("hello"))).unwrap();
    t.flush_learning();
    assert_eq!(store.stats().unwrap().records, 0);
}

#[test]
fn 学习关闭后不再新增记录() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let t = translator(store.clone(), LearningConsent::local());
    pollster::block_on(t.translate(&req("memory footprint"))).unwrap();
    t.flush_learning();
    assert_eq!(store.stats().unwrap().records, 1);

    // 重新构建一个 Off 的 Translator，使用同一个库；不得再写。
    let off = translator(
        store.clone(),
        LearningConsent {
            level: PrivacyLevel::Off,
            ..LearningConsent::default()
        },
    );
    pollster::block_on(off.translate(&req("another missing phrase"))).unwrap();
    off.flush_learning();
    assert_eq!(store.stats().unwrap().records, 1);
}
