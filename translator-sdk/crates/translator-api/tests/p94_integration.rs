//! P9.4：LID 注入、lemma 辅助查词、保护文本与同音词 legacy 行为。
#![cfg(feature = "dictionary")]

use std::path::Path;
use std::sync::Arc;

use pack_dictionary::{pack, parse_jsonl, PackOptions};
use translator_analysis::EnglishLemmatizer;
use translator_api::{
    LanguageGuess, LanguageIdentifier, LanguageTag, LookupRequest, TranslateRequest, Translator,
};
use translator_dictionary::{PayloadKind, Provenance};
use translator_types::TranslationPolicy;

struct FixedLid(&'static str, f32);

impl LanguageIdentifier for FixedLid {
    fn identify(&self, _text: &str, _limit: usize) -> Vec<LanguageGuess> {
        vec![LanguageGuess {
            language: LanguageTag::new(self.0).unwrap(),
            confidence: self.1,
        }]
    }
    fn name(&self) -> &'static str {
        "fixed-test-lid"
    }
}

fn opts(id: &str, language: &str, target: &str, payload: PayloadKind) -> PackOptions {
    PackOptions {
        id: id.into(),
        version: "0.1.0".into(),
        payload,
        language: language.into(),
        target_languages: vec![target.into()],
        provenance: Provenance {
            name: "TEST".into(),
            source_url: "https://example.invalid".into(),
            revision: "r1".into(),
            license_spdx: "CC0-1.0".into(),
            license_file: "LICENSE.txt".into(),
            retrieved_at: "2026-11-08".into(),
        },
        license_text: "CC0".into(),
    }
}

fn build(dir: &Path, jsonl: &str, id: &str, language: &str, target: &str, payload: PayloadKind) {
    let entries = parse_jsonl(std::io::Cursor::new(jsonl), None).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    pack(entries, dir, &opts(id, language, target, payload)).unwrap();
}

#[test]
fn p94_01_默认detect行为保持启发式() {
    let t = Translator::builder().build().unwrap();
    let g = pollster::block_on(t.detect_language("你好")).unwrap();
    assert_eq!(g[0].language.as_str(), "zh-Hans");
    assert!(g.iter().any(|x| x.language.as_str() == "ja"));
    let j = pollster::block_on(t.detect_language("こんにちは")).unwrap();
    assert_eq!(j[0].language.as_str(), "ja");
}

#[test]
fn p94_02_p94_03_注入的lid生效且实例隔离() {
    let a = Translator::builder()
        .language_identifier(Arc::new(FixedLid("en", 0.99)))
        .build()
        .unwrap();
    let b = Translator::builder()
        .language_identifier(Arc::new(FixedLid("ja", 0.88)))
        .build()
        .unwrap();
    assert_eq!(
        pollster::block_on(a.detect_language("任何文本")).unwrap()[0]
            .language
            .as_str(),
        "en"
    );
    assert_eq!(
        pollster::block_on(b.detect_language("任何文本")).unwrap()[0]
            .language
            .as_str(),
        "ja"
    );
}

#[test]
fn p94_05_p94_06_lemma只在原词未命中时兜底() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("dict");
    build(
        &dir,
        "{\"headword\":\"child\",\"pos\":\"noun\",\"definitions\":[\"孩子\"]}\n",
        "dictionary.test.en-zh",
        "en",
        "zh",
        PayloadKind::V2,
    );
    let t = Translator::builder()
        .dictionary_package(&dir)
        .lemmatizer(Arc::new(EnglishLemmatizer::new()))
        .build()
        .unwrap();
    let req = LookupRequest::new("children").with_language(LanguageTag::new("en").unwrap());
    let hits = pollster::block_on(t.lookup(&req)).unwrap();
    assert_eq!(hits[0].headword, "child");
    assert_eq!(hits[0].senses[0].definition, "孩子");
    // 原词已经命中时不该被 lemma 覆盖
    let exact = LookupRequest::new("child").with_language(LanguageTag::new("en").unwrap());
    let hits = pollster::block_on(t.lookup(&exact)).unwrap();
    assert_eq!(hits[0].headword, "child");
}

#[test]
fn p94_07_保护文本不被lemma修改() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("dict");
    build(
        &dir,
        "{\"headword\":\"child\",\"definitions\":[\"孩子\"]}\n",
        "dictionary.test.en-zh",
        "en",
        "zh",
        PayloadKind::V2,
    );
    let t = Translator::builder()
        .dictionary_package(&dir)
        .lemmatizer(Arc::new(EnglishLemmatizer::new()))
        .build()
        .unwrap();
    let req = LookupRequest::new("https://example.com/{name}");
    let hits = pollster::block_on(t.lookup(&req)).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn p94_08_同音词lookup返回全部而translate保留首义legacy() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("dict");
    build(
        &dir,
        concat!(
            "{\"headword\":\"橋\",\"traditional\":\"はし\",\"definitions\":[\"bridge\"]}\n",
            "{\"headword\":\"箸\",\"traditional\":\"はし\",\"definitions\":[\"chopsticks\"]}\n",
            "{\"headword\":\"端\",\"traditional\":\"はし\",\"definitions\":[\"edge\"]}\n",
        ),
        "dictionary.test.ja-en",
        "ja",
        "en",
        PayloadKind::V3,
    );
    let t = Translator::builder()
        .dictionary_package(&dir)
        .build()
        .unwrap();
    let hits = pollster::block_on(
        t.lookup(&LookupRequest::new("はし").with_language(LanguageTag::new("ja").unwrap())),
    )
    .unwrap();
    assert_eq!(hits.len(), 3);
    let out = pollster::block_on(
        t.translate(
            &TranslateRequest::new("はし", LanguageTag::new("en").unwrap())
                .with_source(LanguageTag::new("ja").unwrap())
                .with_policy(TranslationPolicy::DictionaryOnly),
        ),
    )
    .unwrap();
    // legacy: DictionaryEngine::translate 仍只返回首义；歧义由 lookup/diagnostics 暴露
    assert_eq!(out.text, "bridge");
}
