//! P10.1-A4-3：ORT 内存预算与实际占用对照。
#![cfg(feature = "nmt-ort")]
use translator_api::{LanguageTag, Translator};
use translator_nmt_ort::OrtNmtConfig;

#[test]
#[ignore = "需要 P4_ORT_MODEL_DIR"]
fn p10_ort_budget_exceeds_and_allows() {
    let dir = std::env::var("P4_ORT_MODEL_DIR").expect("P4_ORT_MODEL_DIR");
    let cfg = OrtNmtConfig::new(
        &dir,
        LanguageTag::new("en").unwrap(),
        LanguageTag::new("zh-Hans").unwrap(),
    );
    let low = Translator::builder()
        .nmt_ort(cfg.clone())
        .memory_budget(Some(100 * 1024 * 1024))
        .build();
    assert!(low.is_err(), "100 MiB 预算应拒绝 600 MiB 估计");
    assert_eq!(
        low.unwrap_err().code,
        translator_types::TranslationErrorCode::OutOfBudget
    );
    let ok = Translator::builder()
        .nmt_ort(cfg)
        .memory_budget(Some(650 * 1024 * 1024))
        .build()
        .expect("650 MiB 应允许");
    let engines = ok.nmt_ort_engines();
    assert_eq!(engines.len(), 1);
    // 估计值 600 MiB；实际 1,000 次 Soak 的 Private 约 413 MiB，见 docs/59A。
    assert_eq!(engines[0].memory_footprint_bytes(), 600 * 1024 * 1024);
    println!("estimated_bytes={}", engines[0].memory_footprint_bytes());
}
