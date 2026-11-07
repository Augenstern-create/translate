//! P9.4 / EP-02：规则与术语文件内容变化必须改变资源指纹。
#![cfg(all(feature = "rules", feature = "dictionary"))]

use std::path::Path;

use translator_api::Translator;

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

fn rules_json(replacement: &str) -> String {
    format!(
        r#"{{
  "schema_version": 1, "id": "rules.test.ep02", "version": 1,
  "pipeline_version": 2, "license": "CC0-1.0", "sample_pair": ["zh-Hans", "en"],
  "rules": [{{ "id": "term.x", "priority": 10, "language_pairs": [["*", "*"]],
    "action": {{ "kind": "replace", "from": "机器人", "to": "{replacement}" }},
    "samples": [{{ "input": "机器人", "expect": "{replacement}" }}] }}]
}}"#
    )
}

fn terms_json(preferred: &str) -> String {
    format!(
        r#"{{ "schema_version": 1, "id": "terms.test.ep02", "version": 1,
  "terms": [{{ "target": "en", "preferred": "{preferred}", "variants": ["AI"] }}] }}"#
    )
}

#[test]
fn ep02_规则内容变化改变指纹() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.json");
    let b = dir.path().join("b.json");
    write(&a, &rules_json("robot-a"));
    write(&b, &rules_json("robot-b"));
    let ta = Translator::builder().rules_file(&a).build().unwrap();
    let tb = Translator::builder().rules_file(&b).build().unwrap();
    assert_ne!(ta.resource_fingerprint(), tb.resource_fingerprint());
    assert!(ta.resource_fingerprint().contains('#'));
}

#[test]
fn ep02_术语内容变化改变指纹() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.json");
    let b = dir.path().join("b.json");
    write(&a, &terms_json("artificial intelligence"));
    write(&b, &terms_json("machine intelligence"));
    let ta = Translator::builder().term_table_file(&a).build().unwrap();
    let tb = Translator::builder().term_table_file(&b).build().unwrap();
    assert_ne!(ta.resource_fingerprint(), tb.resource_fingerprint());
    assert!(ta.resource_fingerprint().contains('#'));
}
