//! P9.1-S / DI-09～DI-12：v3 多值索引与 v2 单值兼容。
use std::path::Path;

use pack_dictionary::{pack, parse_jsonl, PackOptions};
use translator_dictionary::{DictionaryPackage, PayloadKind, Provenance};

const JSONL: &str = concat!(
    "{\"headword\":\"橋\",\"traditional\":\"はし\",\"definitions\":[\"bridge\"]}\n",
    "{\"headword\":\"箸\",\"traditional\":\"はし\",\"definitions\":[\"chopsticks\"]}\n",
    "{\"headword\":\"端\",\"traditional\":\"はし\",\"definitions\":[\"edge\"]}\n",
    "{\"headword\":\"食べる\",\"traditional\":\"たべる\",\"definitions\":[\"to eat\"]}\n",
);

fn build(dir: &Path, payload: PayloadKind) {
    let entries = parse_jsonl(std::io::Cursor::new(JSONL), None).unwrap();
    let opts = PackOptions {
        id: format!("dictionary.test.ja-en.{}", payload.as_str()),
        version: "0.1.0".into(),
        payload,
        language: "ja".into(),
        target_languages: vec!["en".into()],
        provenance: Provenance {
            name: "JMdict".into(),
            source_url: "https://example.invalid".into(),
            revision: "r1".into(),
            license_spdx: "CC-BY-SA-4.0".into(),
            license_file: "LICENSE.txt".into(),
            retrieved_at: "2026-11-08".into(),
        },
        license_text: "CC BY-SA 4.0".into(),
    };
    pack(entries, dir, &opts).unwrap();
}

fn defs(pkg: &DictionaryPackage, term: &str) -> Vec<String> {
    pkg.lookup_exact_all(term)
        .unwrap()
        .into_iter()
        .map(|e| e.senses[0].definition.clone())
        .collect()
}

#[test]
fn di09_同音多义返回全部且不混词义() {
    let root = tempfile::tempdir().unwrap();
    build(root.path(), PayloadKind::V3);
    let pkg = DictionaryPackage::open(root.path(), true).unwrap();
    let d = defs(&pkg, "はし");
    assert!(d.contains(&"bridge".to_string()));
    assert!(d.contains(&"chopsticks".to_string()));
    assert!(d.contains(&"edge".to_string()));
    assert_eq!(d.len(), 3);
}

#[test]
fn di10_汉字词条各自独立() {
    let root = tempfile::tempdir().unwrap();
    build(root.path(), PayloadKind::V3);
    let pkg = DictionaryPackage::open(root.path(), true).unwrap();
    assert_eq!(defs(&pkg, "橋"), vec!["bridge"]);
    assert_eq!(defs(&pkg, "箸"), vec!["chopsticks"]);
    assert_eq!(defs(&pkg, "端"), vec!["edge"]);
}

#[test]
fn di11_旧v2单值包仍可读取() {
    let root = tempfile::tempdir().unwrap();
    build(root.path(), PayloadKind::V2);
    let pkg = DictionaryPackage::open(root.path(), true).unwrap();
    assert_eq!(pkg.payload_kind(), PayloadKind::V2);
    // v2 只有一个值；lookup_exact_all 仍应返回 1 条，不因新 API 失效
    let all = pkg.lookup_exact_all("はし").unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].headword, "橋");
}

#[test]
fn di12_多值顺序稳定且去重() {
    let root = tempfile::tempdir().unwrap();
    build(root.path(), PayloadKind::V3);
    let pkg = DictionaryPackage::open(root.path(), true).unwrap();
    let a: Vec<_> = pkg
        .lookup_exact_all("はし")
        .unwrap()
        .into_iter()
        .map(|e| e.headword)
        .collect();
    let b: Vec<_> = pkg
        .lookup_exact_all("はし")
        .unwrap()
        .into_iter()
        .map(|e| e.headword)
        .collect();
    assert_eq!(a, b);
    assert_eq!(a, vec!["橋", "箸", "端"]);
}
