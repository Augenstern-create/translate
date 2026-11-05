//! 端到端测试：规则包接入公共 API。
//!
//! 重点不是"JSON 能解析"（那是 `translator-rules` 自己的单元测试），
//! 而是：**规则真的改变了整条流水线的结果**，并且接口没被规则污染。
//!
//! 运行方式：`cargo test --workspace --all-features`

#![cfg(all(feature = "mocks", feature = "rules"))]

use std::path::PathBuf;

use translator_api::mocks::{
    MockBackend, MockDictionaryEngine, MockDictionaryProvider, MockNmtEngine,
};
use translator_api::prelude::*;
use translator_api::Translator;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/rules/minimal.json")
}

fn build_with_rules() -> Translator {
    Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .provider(MockDictionaryProvider::new())
        .backend(MockBackend::new())
        .rules_file(fixture())
        .cache_capacity(8)
        .build()
        .expect("规则包应能加载")
}

fn build_without_rules() -> Translator {
    Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .provider(MockDictionaryProvider::new())
        .backend(MockBackend::new())
        .cache_capacity(8)
        .build()
        .expect("构建失败")
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

#[test]
fn 规则让全角输入命中词典() {
    // 无规则：全角 "ｃａｔ" 查不到词条 → 回退 NMT
    let plain = build_without_rules();
    let req = TranslateRequest::new("ｃａｔ", zh()).with_source(en());
    let without = pollster::block_on(plain.translate(&req)).unwrap();
    assert_eq!(
        without.engine_id, "mock.nmt",
        "没有规则时全角输入不该命中词典"
    );

    // 有规则：NFKC 先把全角转成 "cat"，然后词典命中
    let with_rules = build_with_rules();
    let with = pollster::block_on(with_rules.translate(&req)).unwrap();
    assert_eq!(with.engine_id, "mock.dictionary", "规则应让词典命中");
    assert_eq!(with.text, "猫");
}

#[test]
fn 术语规则按语言对生效() {
    let translator = build_with_rules();

    // en → zh-Hans：term.llm 生效，缩写被展开后再交给引擎
    let req = TranslateRequest::new("the LLM works", zh()).with_source(en());
    let hit = pollster::block_on(translator.translate(&req)).unwrap();
    assert!(
        hit.text.contains("large language model"),
        "缩写应被展开，实际：{}",
        hit.text
    );

    // zh-Hans → en：同一个规则不该生效
    let req2 = TranslateRequest::new("the LLM works", en()).with_source(zh());
    let miss = pollster::block_on(translator.translate(&req2)).unwrap();
    assert!(
        miss.text.contains("the LLM works"),
        "语言对不匹配时规则不得生效，实际：{}",
        miss.text
    );
}

#[test]
fn explain_rules报告生效规则但不改公开结果结构() {
    let translator = build_with_rules();

    let preview = translator.explain_rules("ｃａｔ", &en(), &zh());
    assert_eq!(preview.rules_id.as_deref(), Some("rules.zh-en.minimal"));
    assert_eq!(preview.version, 1);
    assert_eq!(preview.rule_count, 3);
    assert_eq!(preview.text, "cat");
    assert_eq!(preview.applied, vec!["norm.nfkc".to_string()]);
}

#[test]
fn 未配置规则时预览为空且不报错() {
    let translator = build_without_rules();
    let preview = translator.explain_rules("ｃａｔ", &en(), &zh());
    assert!(preview.rules_id.is_none());
    assert_eq!(preview.version, 0);
    assert_eq!(preview.text, "ｃａｔ", "没有规则时应原样返回");
}

#[test]
fn 规则文件加载失败计入构建错误() {
    let err = Translator::builder()
        .engine(MockNmtEngine::new())
        .rules_file("/nonexistent/rules.json")
        .build()
        .expect_err("文件不存在时构建必须失败，不能静默降级为无规则");

    assert!(
        err.message.contains("规则包加载失败"),
        "错误信息应指明是规则包问题，实际：{}",
        err.message
    );
}

#[test]
fn 规则内容坏了会被拒绝加载() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.json");
    // 缺 samples：违反设计文档 §7.2「规则必须带测试样本」
    std::fs::write(
        &path,
        r#"{
            "schema_version": 1,
            "id": "rules.test.broken",
            "version": 1,
            "rules": [
                { "id": "term.x", "action": { "kind": "replace", "from": "a", "to": "b" } }
            ]
        }"#,
    )
    .unwrap();

    let err = Translator::builder()
        .engine(MockNmtEngine::new())
        .rules_file(&path)
        .build()
        .expect_err("没有金样本的规则必须被拒绝");
    assert!(
        err.message.contains("金样本"),
        "错误信息应指缺少金样本，实际：{}",
        err.message
    );
}

#[test]
fn 规则版本变化会改变缓存键() {
    // 规则版本进入缓存键：同一文本在 v2 规则包下不得复用 v1 的缓存结果。
    // 这里用"版本不同、行为相同"的两个包，只能通过 explain_rules 观察版本，
    // 缓存隔离本身由 CacheKey 的 rules_version 字段保证（见 cache 单测）。
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::read_to_string(fixture()).unwrap();
    let v1_path = dir.path().join("v1.json");
    std::fs::write(&v1_path, &base).unwrap();

    let v9_path = dir.path().join("v9.json");
    std::fs::write(&v9_path, base.replace("\"version\": 1", "\"version\": 9")).unwrap();

    let v1 = Translator::builder()
        .engine(MockNmtEngine::new())
        .rules_file(&v1_path)
        .build()
        .unwrap();
    let v9 = Translator::builder()
        .engine(MockNmtEngine::new())
        .rules_file(&v9_path)
        .build()
        .unwrap();

    assert_eq!(v1.explain_rules("cat", &en(), &zh()).version, 1);
    assert_eq!(v9.explain_rules("cat", &en(), &zh()).version, 9);
}
