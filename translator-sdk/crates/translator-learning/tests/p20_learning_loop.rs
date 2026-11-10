//! P20-K：隐私门、聚合、审核、版本发布与回滚。
use translator_learning::{
    LearningConsent, LearningInput, LearningStore, RecordOutcome, ReleaseStatus, ReviewStatus,
    SkipReason,
};
use translator_types::{FallbackReason, TranslationRoute};

fn input(source: &str, target: Option<&str>, reason: FallbackReason) -> LearningInput {
    LearningInput {
        source_language: "en".into(),
        target_language: "zh-Hans".into(),
        source_text: source.into(),
        translated_text: target.map(str::to_string),
        fallback_reason: reason,
        translation_route: TranslationRoute::LocalNmt,
        model_identity: Some("mock/model".into()),
        knowledge_version: "dict@1;rules@1".into(),
        context_key: None,
    }
}

#[test]
fn 默认关闭不写任何记录() {
    let dir = tempfile::tempdir().unwrap();
    let store = LearningStore::open(dir.path().join("l.sqlite")).unwrap();
    let out = store
        .record(
            &input(
                "memory footprint",
                Some("内存占用"),
                FallbackReason::PhraseMiss,
            ),
            &LearningConsent::default(),
        )
        .unwrap();
    assert_eq!(out, RecordOutcome::Skipped(SkipReason::PrivacyOff));
    assert_eq!(store.stats().unwrap().records, 0);
}

#[test]
fn 同文聚合与不同上下文隔离() {
    let dir = tempfile::tempdir().unwrap();
    let store = LearningStore::open(dir.path().join("l.sqlite")).unwrap();
    let consent = LearningConsent::local();
    let mut bank = input("bank", None, FallbackReason::SenseAmbiguous);
    bank.context_key = Some("river".into());
    store.record(&bank, &consent).unwrap();
    let mut bank2 = input("bank", None, FallbackReason::SenseAmbiguous);
    bank2.context_key = Some("money".into());
    store.record(&bank2, &consent).unwrap();
    store.record(&bank, &consent).unwrap();

    assert_eq!(store.stats().unwrap().records, 2);
    let occurrences: u64 = store.stats().unwrap().occurrences;
    assert_eq!(occurrences, 3);
    store.extract_candidates(1).unwrap();
    let candidates = store.list_candidates(None, 10).unwrap();
    assert_eq!(candidates.len(), 2, "不同上下文不得合并");
}

#[test]
fn 敏感内容与隐私关闭都不落正文() {
    let dir = tempfile::tempdir().unwrap();
    let store = LearningStore::open(dir.path().join("l.sqlite")).unwrap();
    let consent = LearningConsent::local();
    let out = store
        .record(
            &input("password=super-secret", None, FallbackReason::PhraseMiss),
            &consent,
        )
        .unwrap();
    assert_eq!(out, RecordOutcome::Skipped(SkipReason::SensitiveContent));
    assert_eq!(store.stats().unwrap().records, 0);
}

#[test]
fn 未验证候选不能发布() {
    let dir = tempfile::tempdir().unwrap();
    let store = LearningStore::open(dir.path().join("l.sqlite")).unwrap();
    let consent = LearningConsent::authorized_corpus();
    store
        .record(
            &input(
                "memory footprint",
                Some("内存占用"),
                FallbackReason::PhraseMiss,
            ),
            &consent,
        )
        .unwrap();
    store.extract_candidates(1).unwrap();
    let candidate = store.list_candidates(None, 10).unwrap().remove(0);
    assert_eq!(candidate.status, ReviewStatus::Raw);
    let err = store.stage_release("v1", &[candidate.id]).unwrap_err();
    assert!(err.to_string().contains("不是 verified"), "{err}");
}

#[test]
fn 审核发布与回滚完整闭环() {
    let dir = tempfile::tempdir().unwrap();
    let store = LearningStore::open(dir.path().join("l.sqlite")).unwrap();
    let consent = LearningConsent::authorized_corpus();
    for _ in 0..2 {
        store
            .record(
                &input(
                    "memory footprint",
                    Some("内存占用"),
                    FallbackReason::PhraseMiss,
                ),
                &consent,
            )
            .unwrap();
    }
    store
        .record(
            &input(
                "cache stampede",
                Some("缓存击穿"),
                FallbackReason::PhraseMiss,
            ),
            &consent,
        )
        .unwrap();
    store.extract_candidates(1).unwrap();
    let candidates = store.list_candidates(None, 10).unwrap();
    assert_eq!(candidates.len(), 2);
    let first = candidates
        .iter()
        .find(|c| c.source_text.contains("memory"))
        .unwrap();
    let second = candidates
        .iter()
        .find(|c| c.source_text.contains("cache"))
        .unwrap();
    assert_eq!(first.proposed_target.as_deref(), Some("内存占用"));

    store
        .review_candidate(first.id, ReviewStatus::Verified, Some("人工核对"))
        .unwrap();
    let v1 = store.stage_release("v1", &[first.id]).unwrap();
    assert_eq!(v1.status, ReleaseStatus::Staged);
    assert_eq!(v1.entry_count, 1);
    let v1 = store.publish_release("v1").unwrap();
    assert_eq!(v1.status, ReleaseStatus::Published);
    assert_eq!(store.active_release().unwrap().unwrap().version, "v1");
    assert_eq!(
        store
            .list_candidates(Some(ReviewStatus::Released), 10)
            .unwrap()
            .len(),
        1,
        "发布后候选状态必须是 released"
    );

    store
        .review_candidate(second.id, ReviewStatus::Verified, Some("人工核对"))
        .unwrap();
    store.stage_release("v2", &[second.id]).unwrap();
    store.publish_release("v2").unwrap();
    assert_eq!(store.active_release().unwrap().unwrap().version, "v2");

    let rolled = store.rollback_release().unwrap();
    assert_eq!(rolled.version, "v1");
    assert_eq!(rolled.status, ReleaseStatus::Published);
    assert_eq!(store.active_release().unwrap().unwrap().version, "v1");
    let released = store
        .list_candidates(Some(ReviewStatus::Released), 10)
        .unwrap();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].id, first.id);
}

#[test]
fn 发布文件带摘要且可追踪() {
    let dir = tempfile::tempdir().unwrap();
    let store = LearningStore::open(dir.path().join("l.sqlite")).unwrap();
    let consent = LearningConsent::authorized_corpus();
    store
        .record(
            &input(
                "memory footprint",
                Some("内存占用"),
                FallbackReason::PhraseMiss,
            ),
            &consent,
        )
        .unwrap();
    store.extract_candidates(1).unwrap();
    let candidate = store.list_candidates(None, 10).unwrap().remove(0);
    store
        .review_candidate(candidate.id, ReviewStatus::Verified, None)
        .unwrap();
    let info = store.stage_release("v1", &[candidate.id]).unwrap();
    assert_eq!(info.manifest_sha256.len(), 64);
    let knowledge =
        std::fs::read_to_string(std::path::Path::new(&info.directory).join("knowledge.json"))
            .unwrap();
    assert!(knowledge.contains("memory footprint"));
    assert!(knowledge.contains("\"review_status\": \"verified\""));
}

#[test]
fn 清理会删除记录与候选() {
    let dir = tempfile::tempdir().unwrap();
    let store = LearningStore::open(dir.path().join("l.sqlite")).unwrap();
    let consent = LearningConsent::authorized_corpus();
    store
        .record(
            &input(
                "memory footprint",
                Some("内存占用"),
                FallbackReason::PhraseMiss,
            ),
            &consent,
        )
        .unwrap();
    store.extract_candidates(1).unwrap();
    assert_eq!(store.stats().unwrap().records, 1);
    store.clear_all().unwrap();
    let stats = store.stats().unwrap();
    assert_eq!(stats.records, 0);
    assert_eq!(stats.candidates, 0);
}

#[test]
fn 发布产物可直接交给资源仓库并回滚() {
    use translator_resource::store::ResourceStore;

    let dir = tempfile::tempdir().unwrap();
    let store = LearningStore::open(dir.path().join("l.sqlite")).unwrap();
    let consent = LearningConsent::authorized_corpus();
    store
        .record(
            &input(
                "memory footprint",
                Some("内存占用"),
                FallbackReason::PhraseMiss,
            ),
            &consent,
        )
        .unwrap();
    store
        .record(
            &input(
                "cache stampede",
                Some("缓存击穿"),
                FallbackReason::PhraseMiss,
            ),
            &consent,
        )
        .unwrap();
    store.extract_candidates(1).unwrap();
    let candidates = store.list_candidates(None, 10).unwrap();
    for candidate in &candidates {
        store
            .review_candidate(candidate.id, ReviewStatus::Verified, Some("test"))
            .unwrap();
    }

    let v1 = store.stage_release("v1", &[candidates[0].id]).unwrap();
    let resource_store = ResourceStore::open(dir.path().join("resources")).unwrap();
    let installed = resource_store.install_from_dir(&v1.directory).unwrap();
    assert_eq!(installed.resource.id, "knowledge.learning.en-zh");
    assert_eq!(installed.resource.kind.as_str(), "knowledge");
    assert!(installed.activated, "首个版本应自动激活");

    let v2 = store.stage_release("v2", &[candidates[1].id]).unwrap();
    resource_store.install_from_dir(&v2.directory).unwrap();
    resource_store
        .activate("knowledge.learning.en-zh", "v2")
        .unwrap();
    assert_eq!(
        resource_store
            .active_manifest("knowledge.learning.en-zh")
            .unwrap()
            .version,
        "v2"
    );
    assert_eq!(
        resource_store.rollback("knowledge.learning.en-zh").unwrap(),
        "v1"
    );
}
