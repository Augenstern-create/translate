//! P9.1 / DI-04：多个词典包并存时，lookup 不串来源。
#![cfg(feature = "dictionary")]

use std::path::Path;

use pack_dictionary::{pack, parse_jsonl, PackOptions};
use translator_api::{LanguageTag, LookupRequest, Translator};
use translator_dictionary::{PayloadKind, Provenance};

fn build_pkg(dir: &Path, name: &str, definition: &str) {
    let jsonl =
        format!("{{\"headword\":\"bank\",\"pos\":\"noun\",\"definitions\":[\"{definition}\"]}}\n");
    let entries = parse_jsonl(std::io::Cursor::new(jsonl), None).unwrap();
    let opts = PackOptions {
        id: format!("dictionary.test.{name}"),
        version: "0.1.0".into(),
        payload: PayloadKind::V2,
        language: "en".into(),
        target_languages: vec!["zh".into()],
        provenance: Provenance {
            name: name.into(),
            source_url: "https://example.invalid".into(),
            revision: "r1".into(),
            license_spdx: "CC0-1.0".into(),
            license_file: "LICENSE.txt".into(),
            retrieved_at: "2026-11-08".into(),
        },
        license_text: "CC0".into(),
    };
    std::fs::create_dir_all(dir).unwrap();
    pack(entries, dir, &opts).unwrap();
}

#[test]
fn di04_两个词典包来源不串() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    build_pkg(&a, "source-a", "銀行");
    build_pkg(&b, "source-b", "河岸");

    let translator = Translator::builder()
        .dictionary_package(&a)
        .dictionary_package(&b)
        .build()
        .unwrap();
    let entries = pollster::block_on(
        translator
            .lookup(&LookupRequest::new("bank").with_language(LanguageTag::new("en").unwrap())),
    )
    .unwrap();
    assert_eq!(entries.len(), 2, "两个包应各返回一条：{entries:?}");
    let sources: Vec<_> = entries.iter().map(|e| e.source_id.as_str()).collect();
    assert!(sources.contains(&"source-a"), "{sources:?}");
    assert!(sources.contains(&"source-b"), "{sources:?}");
    assert!(entries.iter().any(|e| e.senses[0].definition == "銀行"));
    assert!(entries.iter().any(|e| e.senses[0].definition == "河岸"));
}
