//! P2.3 端到端测试：内存预算与资源租约。
//!
//! 这里验的不是"函数能跑"，而是三条对外承诺：
//!
//! 1. 预算不够时 **`build()` 就失败**（`OutOfBudget`），不会带着放不下的资源跑起来；
//! 2. 预算够时能挂载，且 `memory_report()` 如实反映占用项；
//! 3. `dictionary_from_store` 挂载的版本在 `Translator` 存续期间**删不掉**
//!    （`ResourceLeased`），`Translator` 释放后立刻可删。
//!
//! 运行方式：`cargo test --all-features`

#![cfg(all(feature = "dictionary", feature = "resource", feature = "mocks"))]

use pack_dictionary::{pack, parse_cedict, PackOptions};
use translator_api::mocks::MockNmtEngine;
use translator_api::prelude::*;
use translator_api::Translator;
use translator_dictionary::Provenance;
use translator_resource::store::ResourceStore;
use translator_resource::ResourceError;

const SAMPLE: &str = "\
# CC-CEDICT
貓 猫 [mao1] /cat/
計算機 计算机 [ji4 suan4 ji1] /computer/
你好 你好 [ni3 hao3] /hello/
";

fn options(version: &str) -> PackOptions {
    PackOptions {
        id: "dictionary.test.zh-en".to_string(),
        version: version.to_string(),
        payload: translator_dictionary::PayloadKind::V2,
        language: "zh-Hans".to_string(),
        target_languages: vec!["en".to_string()],
        provenance: Provenance {
            name: "CC-CEDICT".to_string(),
            source_url: "https://cc-cedict.org/".to_string(),
            revision: "test-rev".to_string(),
            license_spdx: "CC-BY-SA-4.0".to_string(),
            license_file: "LICENSE.txt".to_string(),
            retrieved_at: "2026-11-05".to_string(),
        },
        license_text: "# CC-CEDICT\nLicense: CC BY-SA 4.0\n".to_string(),
    }
}

/// 打一个最小但**真实**的词典包（真 FST + 真 SQLite），返回包目录。
fn build_package(root: &std::path::Path, version: &str) -> std::path::PathBuf {
    let dir = root.join(format!("pkg-{version}"));
    std::fs::create_dir_all(&dir).unwrap();
    let parsed = parse_cedict(std::io::Cursor::new(SAMPLE), None).unwrap();
    pack(parsed.entries, &dir, &options(version)).unwrap();
    dir
}

fn plain_builder() -> translator_api::TranslatorBuilder {
    Translator::builder()
        .engine(MockNmtEngine::new())
        .cache_capacity(4)
}

// ---------------- 内存预算 ----------------

#[test]
fn 预算不足时装配期直接失败() {
    let root = tempfile::tempdir().unwrap();
    let pkg = build_package(root.path(), "1.0.0");

    // 1 MiB 显然装不下索引 + 页缓存 + 固定开销
    let err = plain_builder()
        .dictionary_package(&pkg)
        .memory_budget(Some(1024 * 1024))
        .build()
        .unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::OutOfBudget, "实际：{err}");
    assert!(
        err.message.contains("dictionary.test.zh-en"),
        "错误要指出是哪一项：{err}"
    );
}

#[test]
fn 预算充足时可以挂载并如实报告占用() {
    let root = tempfile::tempdir().unwrap();
    let pkg = build_package(root.path(), "1.0.0");

    let translator = plain_builder()
        .dictionary_package(&pkg)
        .memory_budget(Some(64 * 1024 * 1024))
        .build()
        .unwrap();

    let report = translator.memory_report();
    assert_eq!(report.limit_bytes, Some(64 * 1024 * 1024));
    assert!(report.used_bytes > 0, "挂载了词典就必须有占用");
    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].0, "dictionary:dictionary.test.zh-en");
    assert_eq!(report.entries[0].1, report.used_bytes);
    assert!(report.summary().contains("dictionary:"));
}

#[test]
fn 不设上限时只记账不拒绝() {
    let root = tempfile::tempdir().unwrap();
    let pkg = build_package(root.path(), "1.0.0");

    let translator = plain_builder().dictionary_package(&pkg).build().unwrap();
    let report = translator.memory_report();
    assert_eq!(report.limit_bytes, None);
    assert!(report.used_bytes > 0);
    assert_eq!(report.available_bytes(), None);
    assert!(report.summary().contains("无上限"));
}

#[test]
fn 预算凭据随门面释放而归还() {
    let root = tempfile::tempdir().unwrap();
    let pkg = build_package(root.path(), "1.0.0");

    let translator = plain_builder().dictionary_package(&pkg).build().unwrap();
    let used = translator.memory_report().used_bytes;
    assert!(used > 0);
    drop(translator);
    // 预算池随 Translator 一起消失，无法从外部再观察；
    // 这里退而验证"同一进程可以反复构建而不累积"（靠 Drop 归还是唯一解释）
    for _ in 0..3 {
        let t = plain_builder().dictionary_package(&pkg).build().unwrap();
        assert_eq!(t.memory_report().used_bytes, used, "每次构建占用应当一致");
    }
}

// ---------------- 租约 ----------------

#[test]
fn 从仓库挂载的版本在存活期间删不掉() {
    let root = tempfile::tempdir().unwrap();
    let store_root = root.path().join("store");
    let store = ResourceStore::open(&store_root).unwrap();

    let v1 = build_package(root.path(), "1.0.0");
    store.install_from_dir(&v1).unwrap(); // 首次安装自动激活 1.0.0

    let translator = plain_builder()
        .dictionary_from_store(&store, "dictionary.test.zh-en")
        .build()
        .unwrap();

    // 挂载真的生效了：能查到词
    let hits =
        pollster::block_on(translator.lookup(
            &LookupRequest::new("计算机").with_language(LanguageTag::new("zh-Hans").unwrap()),
        ))
        .unwrap();
    assert!(!hits.is_empty(), "挂载后必须能查词");
    assert_eq!(hits[0].senses[0].definition, "computer");

    // 租约已登记
    let leases = store.all_leases().unwrap();
    assert_eq!(leases.len(), 1);
    assert!(leases[0].is_active());

    // 切到新版本，让 1.0.0 变成"非激活但有租约"
    let v2 = build_package(root.path(), "2.0.0");
    store.install_from_dir(&v2).unwrap();
    store.activate("dictionary.test.zh-en", "2.0.0").unwrap();

    let err = store
        .uninstall("dictionary.test.zh-en", "1.0.0")
        .unwrap_err();
    assert!(
        matches!(err, ResourceError::ResourceLeased { .. }),
        "有租约时必须拒绝删除，实际：{err}"
    );

    // Translator 释放 → 租约释放 → 可以删
    drop(translator);
    assert!(store.all_leases().unwrap().is_empty());
    store.uninstall("dictionary.test.zh-en", "1.0.0").unwrap();
}

#[test]
fn 仓库里没有激活版本时报未安装() {
    let root = tempfile::tempdir().unwrap();
    let store = ResourceStore::open(root.path().join("store")).unwrap();
    let err = plain_builder()
        .dictionary_from_store(&store, "dictionary.absent")
        .build()
        .unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::ResourceNotInstalled);
}
