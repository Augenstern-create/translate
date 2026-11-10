//! P18-L：新词典资源的实际 SDK 查询、安装、激活与回滚。
#![cfg(all(feature = "dictionary", feature = "resource"))]

use std::fs;
use std::path::PathBuf;

use translator_api::prelude::*;
use translator_api::{LookupRequest, Translator};
use translator_resource::store::ResourceStore;

fn package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/en-zh-ecdict")
}

fn lookup_found(translator: &Translator, term: &str) -> Vec<translator_api::DictionaryEntry> {
    let req = LookupRequest::new(term)
        .with_language(LanguageTag::new("en").unwrap())
        .with_limit(10);
    pollster::block_on(translator.lookup(&req)).unwrap()
}

#[test]
fn 新日常词典可安装查询并回滚() {
    let dir = tempfile::tempdir().unwrap();
    let store = ResourceStore::open(dir.path().join("resources")).unwrap();
    let id = "dictionary.ecdict.en-zh";

    let installed = store.install_from_dir(package_dir()).unwrap();
    assert_eq!(installed.resource.version, "0.2.0");
    assert!(installed.activated);

    let translator = Translator::builder()
        .dictionary_from_store(&store, id)
        .build()
        .unwrap();

    let phrase = lookup_found(&translator, "take off");
    assert!(!phrase.is_empty(), "短语 take off 必须可查询");
    assert!(phrase.iter().any(|e| e
        .senses
        .iter()
        .any(|s| { s.definition.contains("起飞") || s.definition.contains("脱下") })));

    let expression = lookup_found(&translator, "be afraid of");
    assert!(!expression.is_empty(), "日常表达 be afraid of 必须可查询");

    let stack = lookup_found(&translator, "stack");
    let senses: usize = stack.iter().map(|e| e.senses.len()).sum();
    assert!(senses >= 2, "stack 至少应保留技术义和日常义，实际 {senses}");

    // 用同一内容构造 0.3.0（只改清单版本），验证激活与回滚。
    let v3_dir = dir.path().join("pkg-v3");
    fs::create_dir_all(&v3_dir).unwrap();
    for entry in fs::read_dir(package_dir()).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), v3_dir.join(entry.file_name())).unwrap();
    }
    let manifest_path = v3_dir.join("manifest.json");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    fs::write(
        &manifest_path,
        manifest.replace("\"version\": \"0.2.0\"", "\"version\": \"0.3.0\""),
    )
    .unwrap();
    store.install_from_dir(&v3_dir).unwrap();
    store.activate(id, "0.3.0").unwrap();
    assert_eq!(store.active_manifest(id).unwrap().version, "0.3.0");
    assert_eq!(store.rollback(id).unwrap(), "0.2.0");
    assert_eq!(store.active_manifest(id).unwrap().version, "0.2.0");
}
