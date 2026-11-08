//! T1.4：词典直译不能机械取第一义。
//!
//! 用真实 `pack-dictionary` 产物走完整 `Translator` 路径，验证：
//! - 单词直译返回多个不同义项（分号连接）；
//! - `lookup` 返回全部义项；
//! - 大小写不敏感查询（`January` / `january`）。

#![cfg(feature = "dictionary")]

use translator_api::{
    LanguageTag, LookupRequest, TextKind, TranslateOptions, TranslateRequest, TranslationMode,
    Translator,
};

fn pack_bank_package() -> tempfile::TempDir {
    let jsonl = concat!(
        "{\"headword\":\"bank\",\"pos\":\"noun\",\"definitions\":[\"银行, 堤, 岸\",\"[医] 库\"]}\n",
        "{\"headword\":\"January\",\"pos\":\"noun\",\"definitions\":[\"一月\"]}\n",
        "{\"headword\":\"memory leak\",\"pos\":\"noun\",\"definitions\":[\"内存泄漏\",\"[计] 内存泄漏\"]}\n",
    );
    let entries = pack_dictionary::parse_jsonl(std::io::Cursor::new(jsonl), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let opts = pack_dictionary::PackOptions {
        id: "dictionary.test.en-zh".to_string(),
        version: "0.0.1".to_string(),
        payload: translator_api::dictionary::PayloadKind::V2,
        language: "en".to_string(),
        target_languages: vec!["zh-Hans".to_string()],
        provenance: translator_api::dictionary::Provenance {
            name: "test".to_string(),
            source_url: "https://example.invalid/test".to_string(),
            revision: "test-1".to_string(),
            license_spdx: "MIT".to_string(),
            license_file: "LICENSE.txt".to_string(),
            retrieved_at: "2026-11-08".to_string(),
        },
        license_text: "MIT test fixture\n".to_string(),
    };
    pack_dictionary::pack(entries, dir.path(), &opts).unwrap();
    dir
}

fn translator_with_lemmatizer(dir: &tempfile::TempDir) -> Translator {
    Translator::builder()
        .dictionary_package(dir.path())
        .lemmatizer(std::sync::Arc::new(
            translator_analysis::EnglishLemmatizer::new(),
        ))
        .build()
        .expect("构建 Translator 失败")
}

fn translator(dir: &tempfile::TempDir) -> Translator {
    Translator::builder()
        .dictionary_package(dir.path())
        .build()
        .expect("构建 Translator 失败")
}

#[test]
fn 单词直译返回多个义项而不是只取第一义() {
    let dir = pack_bank_package();
    let translator = translator(&dir);
    let req = TranslateRequest::new("bank", LanguageTag::new("zh-Hans").unwrap());
    let result = pollster::block_on(translator.translate(&req)).unwrap();
    assert!(result.text.contains("银行"), "{}", result.text);
    assert!(
        result.text.contains("堤") || result.text.contains("岸"),
        "{}",
        result.text
    );
    assert!(result.text.contains("库"), "{}", result.text);
    assert!(
        result.text.contains('；'),
        "多义项应以分号分隔：{}",
        result.text
    );
}

#[test]
fn 查词返回全部义项与词性() {
    let dir = pack_bank_package();
    let translator = translator(&dir);
    let req = LookupRequest::new("bank")
        .with_language(LanguageTag::new("en").unwrap())
        .with_limit(5);
    let entries = pollster::block_on(translator.lookup(&req)).unwrap();
    let bank = entries.iter().find(|e| e.headword == "bank").unwrap();
    assert_eq!(bank.parts_of_speech, vec!["noun".to_string()]);
    assert_eq!(bank.senses.len(), 2);
    assert_eq!(bank.senses[0].definition, "银行, 堤, 岸");
}

#[test]
fn 查词未命中时使用词形候选() {
    let jsonl = "{\"headword\":\"run\",\"pos\":\"verb\",\"definitions\":[\"跑\"]}\n";
    let entries = pack_dictionary::parse_jsonl(std::io::Cursor::new(jsonl), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let opts = pack_dictionary::PackOptions {
        id: "dictionary.test.lemma".to_string(),
        version: "0.0.1".to_string(),
        payload: translator_api::dictionary::PayloadKind::V2,
        language: "en".to_string(),
        target_languages: vec!["zh-Hans".to_string()],
        provenance: translator_api::dictionary::Provenance {
            name: "test".to_string(),
            source_url: "https://example.invalid/test".to_string(),
            revision: "test-1".to_string(),
            license_spdx: "MIT".to_string(),
            license_file: "LICENSE.txt".to_string(),
            retrieved_at: "2026-11-08".to_string(),
        },
        license_text: "MIT test fixture\n".to_string(),
    };
    pack_dictionary::pack(entries, dir.path(), &opts).unwrap();

    let translator = translator_with_lemmatizer(&dir);
    let req = LookupRequest::new("running")
        .with_language(LanguageTag::new("en").unwrap())
        .with_limit(5);
    let entries = pollster::block_on(translator.lookup(&req)).unwrap();
    assert!(
        entries.iter().any(|e| e.headword == "run"),
        "running 应通过词形候选命中 run，实际 {entries:?}"
    );
}

#[test]
fn 上下文线索选择词义() {
    let dir = pack_bank_package();
    let translator = translator(&dir);
    let req = TranslateRequest::new("bank", LanguageTag::new("zh-Hans").unwrap());
    let river = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Word)
        .with_context_text("We sat on the river bank and watched the boats.");
    let out = pollster::block_on(translator.translate_with_options(&req, &river)).unwrap();
    assert!(
        out.text.contains("岸") || out.text.contains("堤"),
        "{}",
        out.text
    );
    let money = TranslateOptions::new(TranslationMode::Precise)
        .with_text_kind(TextKind::Word)
        .with_context_text("I need to go to the bank to withdraw money.");
    let out = pollster::block_on(translator.translate_with_options(&req, &money)).unwrap();
    assert!(out.text.contains("银行"), "{}", out.text);
}

#[test]
fn 英语查询大小写不敏感() {
    let dir = pack_bank_package();
    let translator = translator(&dir);
    for word in ["January", "january", "JANUARY"] {
        let req = LookupRequest::new(word)
            .with_language(LanguageTag::new("en").unwrap())
            .with_limit(5);
        let entries = pollster::block_on(translator.lookup(&req)).unwrap();
        assert!(
            entries.iter().any(|e| e.headword == "January"),
            "{word} 应命中 January，实际 {entries:?}"
        );
    }
}
