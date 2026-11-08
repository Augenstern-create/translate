//! 词典包端到端测试：打包 → 打开 → 查询 → 适配成插件契约。
//!
//! 对应设计文档 §12.1 的「单测」与「契约测试」层次，
//! 覆盖资源文档 §10 的 D-01（词条查询）与 P-01（损坏资源）部分场景。

use std::sync::Arc;

use pack_dictionary::{pack, parse_cedict, PackOptions};
use translator_dictionary::{
    DictionaryEngine, DictionaryPackage, DictionaryProviderImpl, Provenance,
};
use translator_plugin_api::{DictionaryProvider, TranslationEngine};
use translator_types::{
    LanguageTag, LookupRequest, PluginApiVersion, TranslateRequest, TranslationErrorCode,
};

/// 真实 CC-CEDICT 格式的样本（含繁体、多义词、分类词）。
const SAMPLE: &str = "\
# CC-CEDICT
# License: CC BY-SA 4.0
#! version=1
貓 猫 [mao1] /cat/CL:隻|只[zhi1]/
計算機 计算机 [ji4 suan4 ji1] /computer/(Tw) calculator/
你好 你好 [ni3 hao3] /hello/hi/
世界 世界 [shi4 jie4] /world/
世界觀 世界观 [shi4 jie4 guan1] /worldview/
";

fn pack_options() -> PackOptions {
    PackOptions {
        id: "dictionary.test.zh-en".to_string(),
        version: "0.0.1".to_string(),
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

fn build() -> tempfile::TempDir {
    build_with(translator_dictionary::PayloadKind::V2)
}

fn build_with(payload: translator_dictionary::PayloadKind) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("创建临时目录");
    let parsed = parse_cedict(std::io::Cursor::new(SAMPLE), None).expect("解析样本");
    let mut opts = pack_options();
    opts.payload = payload;
    pack(parsed.entries, dir.path(), &opts).expect("打包");
    dir
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

#[test]
fn 精确查询返回释义与许可来源() {
    let dir = build();
    let pkg = DictionaryPackage::open(dir.path(), true).unwrap();

    let entry = pkg.lookup_exact("计算机").unwrap().expect("应命中");

    assert_eq!(entry.headword, "计算机");
    assert_eq!(entry.senses[0].definition, "computer");
    // 每条释义都要能追溯来源（§7.1）
    assert_eq!(entry.senses[0].source_id, "cc-cedict");
    assert_eq!(entry.source_license, "CC-BY-SA-4.0");
    assert!(entry.readings[0].contains("ji4 suan4 ji1"));
}

#[test]
fn 繁体键命中同一条目() {
    let dir = build();
    let pkg = DictionaryPackage::open(dir.path(), false).unwrap();

    let simp = pkg.lookup_exact("计算机").unwrap().unwrap();
    let trad = pkg.lookup_exact("計算機").unwrap().expect("繁体也应命中");

    assert_eq!(trad.headword, simp.headword);
    assert_eq!(trad.senses, simp.senses.into_iter().collect::<Vec<_>>());
}

#[test]
fn 多义词的释义都保留() {
    let dir = build();
    let pkg = DictionaryPackage::open(dir.path(), false).unwrap();

    let entry = pkg.lookup_exact("计算机").unwrap().unwrap();
    let defs: Vec<&str> = entry.senses.iter().map(|s| s.definition.as_str()).collect();

    assert!(defs.contains(&"computer"));
    assert!(defs.contains(&"(Tw) calculator"));
}

#[test]
fn 前缀查询返回补全且不重复() {
    let dir = build();
    let pkg = DictionaryPackage::open(dir.path(), false).unwrap();

    let hits = pkg.lookup("世界", 5).unwrap();
    let headwords: Vec<&str> = hits.iter().map(|e| e.headword.as_str()).collect();

    assert!(headwords.contains(&"世界"));
    assert!(headwords.contains(&"世界观"));

    // 繁体键与简体键指向同一 record，结果里不得出现两次
    let mut sorted = headwords.clone();
    sorted.sort_unstable();
    let dedup_len = {
        let mut s = sorted.clone();
        s.dedup();
        s.len()
    };
    assert_eq!(
        sorted.len(),
        dedup_len,
        "前缀结果出现重复条目：{headwords:?}"
    );
}

#[test]
fn 前缀查询遵守limit() {
    let dir = build();
    let pkg = DictionaryPackage::open(dir.path(), false).unwrap();
    assert_eq!(pkg.lookup("世界", 1).unwrap().len(), 1);
    assert_eq!(pkg.lookup("世界", 0).unwrap().len(), 0);
}

#[test]
fn 不存在的词条返回none() {
    let dir = build();
    let pkg = DictionaryPackage::open(dir.path(), false).unwrap();
    assert!(pkg.lookup_exact("不存在的词").unwrap().is_none());
}

#[test]
fn 清单拒绝占位符() {
    let dir = build();
    let path = dir.path().join("manifest.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replace("test-rev", "REQUIRED");
    std::fs::write(&path, tampered).unwrap();

    let err = DictionaryPackage::open(dir.path(), false)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("REQUIRED") || err.contains("未填写"),
        "实际：{err}"
    );
}

#[test]
fn 引擎只为匹配的语言对声明支持() {
    let dir = build();
    let pkg = Arc::new(DictionaryPackage::open(dir.path(), false).unwrap());
    let engine = DictionaryEngine::new(pkg);

    assert!(engine.supports(&zh(), &en()));
    assert!(!engine.supports(&en(), &zh()), "该包只支持中→英");
    assert!(!engine.supports(&zh(), &LanguageTag::new("ja").unwrap()));
}

#[test]
fn 引擎命中时返回词典译文() {
    let dir = build();
    let pkg = Arc::new(DictionaryPackage::open(dir.path(), false).unwrap());
    let engine = DictionaryEngine::new(pkg);

    let req = TranslateRequest::new("你好", en()).with_source(zh());
    let result = pollster::block_on(engine.translate(&req)).unwrap();

    // T1.4：多个义项不再只返回第一义，而是用分号连接（最多 3 条）。
    assert_eq!(result.text, "hello；hi");
    assert_eq!(result.engine_id, "dictionary.package:dictionary.test.zh-en");
    assert_eq!(
        result.model_id.as_deref(),
        Some("dictionary.test.zh-en"),
        "结果应带上包包 id 以便追溯"
    );
}

#[test]
fn 引擎未命中返回可回退的错误码() {
    let dir = build();
    let pkg = Arc::new(DictionaryPackage::open(dir.path(), false).unwrap());
    let engine = DictionaryEngine::new(pkg);

    let req = TranslateRequest::new("喜欢", en()).with_source(zh());
    let err = pollster::block_on(engine.translate(&req)).unwrap_err();

    // Coordinator 依赖这个错误码触发 NMT 回退
    assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
}

#[test]
fn 提供者对不匹配语言返回空列表() {
    let dir = build();
    let pkg = Arc::new(DictionaryPackage::open(dir.path(), false).unwrap());
    let provider = DictionaryProviderImpl::new(pkg);

    let req = LookupRequest::new("hello").with_language(en());
    let entries = pollster::block_on(provider.lookup(&req)).unwrap();

    assert!(entries.is_empty(), "不匹配语言应返回空，而不是报错");
}

#[test]
fn 插件版本为当前版本() {
    let dir = build();
    let pkg = Arc::new(DictionaryPackage::open(dir.path(), false).unwrap());
    let engine = DictionaryEngine::new(pkg);

    assert_eq!(engine.descriptor().plugin_api, PluginApiVersion::CURRENT);
}

// ============================================================
// P2.5：payload v1 / v2
// ============================================================

/// 同一份词表分别打成 v1 与 v2，读出来的词条必须**逐字段相同**。
///
/// 这是 v2 的验收底线：省掉一次 B-tree 查找不能改变语义。
#[test]
fn v1与v2读出的词条完全一致() {
    let v1_dir = build_with(translator_dictionary::PayloadKind::V1);
    let v2_dir = build_with(translator_dictionary::PayloadKind::V2);

    let v1 = DictionaryPackage::open(v1_dir.path(), false).unwrap();
    let v2 = DictionaryPackage::open(v2_dir.path(), false).unwrap();
    assert_eq!(v1.payload_kind(), translator_dictionary::PayloadKind::V1);
    assert_eq!(v2.payload_kind(), translator_dictionary::PayloadKind::V2);

    for term in ["计算机", "計算機", "猫", "貓", "世界", "世界观", "你好"] {
        let a = v1.lookup_exact(term).unwrap().expect(term);
        let b = v2.lookup_exact(term).unwrap().expect(term);
        assert_eq!(a.headword, b.headword, "{term}");
        assert_eq!(a.normalized, b.normalized, "{term}");
        assert_eq!(a.language, b.language, "{term}");
        assert_eq!(a.readings, b.readings, "{term}");
        assert_eq!(a.parts_of_speech, b.parts_of_speech, "{term}");
        assert_eq!(a.senses, b.senses, "{term}");
        assert_eq!(a.source_id, b.source_id, "{term}");
        assert_eq!(a.source_license, b.source_license, "{term}");
        assert_eq!(a.confidence, b.confidence, "{term}");
    }

    // 前缀查询也一致
    let a: Vec<String> = v1
        .lookup("世界", 5)
        .unwrap()
        .into_iter()
        .map(|e| e.headword)
        .collect();
    let b: Vec<String> = v2
        .lookup("世界", 5)
        .unwrap()
        .into_iter()
        .map(|e| e.headword)
        .collect();
    assert_eq!(a, b);
}

/// 清单声明的格式与库文件自报的 `user_version` 不一致时必须拒收。
///
/// 真实事故场景：改了打包参数但忘了同步清单，结果"读得到词条但释义为空"。
#[test]
fn 清单与库格式不一致时拒绝打开() {
    let dir = build_with(translator_dictionary::PayloadKind::V2);
    let manifest_path = dir.path().join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    // 把 v2 清单改成 v1 声明（库文件仍是 v2）
    let patched = text
        .replace("\"manifest_schema_version\": 2", "\"schema_version\": 1")
        .replace("\"payload_schema_version\": \"dictionary.sqlite.v2\",", "");
    std::fs::write(&manifest_path, patched).unwrap();

    let err = DictionaryPackage::open(dir.path(), false).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("user_version"),
        "错误必须指出是格式不匹配：{text}"
    );
}

#[test]
fn v2清单声明未知payload被拒收() {
    let dir = build_with(translator_dictionary::PayloadKind::V2);
    let manifest_path = dir.path().join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(
        &manifest_path,
        text.replace("dictionary.sqlite.v2", "dictionary.sqlite.v9"),
    )
    .unwrap();
    let err = DictionaryPackage::open(dir.path(), false).unwrap_err();
    assert!(
        err.to_string().contains("payload_schema_version"),
        "实际：{err}"
    );
}

#[test]
fn 释义里出现分隔符时打包失败() {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = pack_options();
    opts.payload = translator_dictionary::PayloadKind::V2;
    let mut parsed = parse_cedict(std::io::Cursor::new(SAMPLE), None).expect("解析样本");
    parsed.entries[0]
        .definitions
        .push("bad\u{1f}definition".to_string());
    let err = pack(parsed.entries, dir.path(), &opts).unwrap_err();
    assert!(err.to_string().contains("U+001F"), "实际：{err}");
}
