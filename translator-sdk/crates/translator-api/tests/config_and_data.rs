//! 装配与数据文件测试：配置校验、词典查询、规则包、模板加载。
//!
//! 重点不是"功能都能用"，而是**错误在装配期暴露**：
//! 配置写错、术语表冲突、模板 JSON 非法、词典包损坏，都必须在 `build()` 时
//! 报错，而不是等到用户点了"翻译"才失败。

use std::sync::Arc;

use translator_api::{
    dictionary, Glossary, LanguageTag, LookupRequest, TemplateScope, TemplateTable, TermPolicy,
    TextRules, TranslateRequest, TranslationEngine, TranslationErrorCode, Translator,
};
use translator_plugin_api::mocks::MockNmtEngine;

fn request(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, LanguageTag::new("zh-CN").unwrap())
        .with_source(LanguageTag::new("en").unwrap())
}

// ============================================================
// 配置
// ============================================================

#[test]
fn 未配置后端时装配直接失败() {
    let err = Translator::builder().build().unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::ConfigurationError);
    assert!(err.message.contains("后端"), "{}", err.message);
}

#[test]
fn 非回环地址必须用_https() {
    let err = Translator::builder()
        .http_config_toml(
            r#"
[backend.remote]
base_url = "http://relay.example/v1"
model = "m"
"#,
        )
        .err()
        .expect("明文远程地址应被拒绝");
    assert_eq!(err.code, TranslationErrorCode::ConfigurationError);
}

#[test]
fn 本机明文地址允许() {
    let translator = Translator::builder()
        .http_config_toml(
            r#"
[backend.local]
base_url = "http://127.0.0.1:8080/v1"
model = "local-model"
api_key_env = ""
"#,
        )
        .expect("本机明文应允许")
        .build()
        .expect("应可构造");
    assert_eq!(translator.backend_ids(), ["http.local"]);
    // 摘要里不能出现密钥（本来也没有）
    assert!(!translator.config_summary().contains("sk-"));
}

#[test]
fn 禁用后端不会被挂载() {
    let translator = Translator::builder()
        .http_config_toml(
            r#"
[backend.a]
base_url = "https://relay.example/v1"
model = "m"
enabled = false

[backend.b]
base_url = "https://relay.example/v1"
model = "m2"
"#,
        )
        .expect("配置应可解析")
        .build()
        .expect("应可构造");
    assert_eq!(translator.backend_ids(), ["http.b"]);
}

#[test]
fn 未知配置字段被拒绝() {
    let err = Translator::builder()
        .http_config_toml(
            r#"
[backend.a]
base_url = "https://relay.example/v1"
model = "m"
timeout_millis = 1000
"#,
        )
        .err()
        .expect("拼错的字段必须报错");
    assert_eq!(err.code, TranslationErrorCode::ConfigurationError);
}

// ============================================================
// 词典
// ============================================================

fn pack_test_dictionary() -> tempfile::TempDir {
    let jsonl = concat!(
        "{\"headword\":\"bank\",\"pos\":\"noun\",\"definitions\":[\"银行, 堤, 岸\"]}\n",
        "{\"headword\":\"memory leak\",\"pos\":\"noun\",\"definitions\":[\"内存泄漏\"]}\n",
    );
    let entries = pack_dictionary::parse_jsonl(std::io::Cursor::new(jsonl), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let opts = pack_dictionary::PackOptions {
        id: "dictionary.test.en-zh".to_string(),
        version: "0.0.1".to_string(),
        payload: dictionary::PayloadKind::V2,
        language: "en".to_string(),
        target_languages: vec!["zh-Hans".to_string()],
        provenance: dictionary::Provenance {
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

#[test]
fn 挂载词典包后可以查词() {
    let dir = pack_test_dictionary();
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .dictionary_package(dir.path())
        .expect("挂载词典失败")
        .build()
        .unwrap();

    let entries = pollster::block_on(translator.lookup(&LookupRequest::new("bank"))).unwrap();
    assert!(!entries.is_empty(), "应查到 bank");
    assert!(entries[0]
        .senses
        .iter()
        .any(|s| s.definition.contains("银行")));
}

#[test]
fn 词典路径不存在时报词典不可用() {
    let err = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .dictionary_package("/nonexistent/dictionary/path")
        .err()
        .expect("不存在的词典应报错");
    assert_eq!(err.code, TranslationErrorCode::DictionaryUnavailable);
}

#[test]
fn 查询空词与非法_limit_被拒绝() {
    let dir = pack_test_dictionary();
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .dictionary_package(dir.path())
        .expect("挂载词典失败")
        .build()
        .unwrap();

    let err = pollster::block_on(translator.lookup(&LookupRequest::new("  "))).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::InvalidRequest);

    let mut req = LookupRequest::new("bank");
    req.limit = 0;
    let err = pollster::block_on(translator.lookup(&req)).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::InvalidRequest);
}

// ============================================================
// 术语表
// ============================================================

#[test]
fn 术语表可以从文件加载() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("glossary.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"id":"glossary.test","version":4,"entries":[
            {"id":"t1","source":"pull request","target":"拉取请求","strict":true}]}"#,
    )
    .unwrap();

    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .glossary_file(&path)
        .expect("加载术语表失败")
        .term_policy(TermPolicy::Warn)
        .build()
        .unwrap();
    assert!(translator.config_summary().contains("glossary.test@4"));
}

#[test]
fn 术语表文件非法时报配置错误() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("glossary.json");
    std::fs::write(&path, "{ not json }").unwrap();
    let err = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .glossary_file(&path)
        .err()
        .expect("非法术语表应报错");
    assert_eq!(err.code, TranslationErrorCode::ConfigurationError);
}

#[test]
fn 术语表版本进入配置摘要() {
    let g = Glossary::from_json(
        r#"{"schema_version":1,"id":"g.x","version":9,"entries":[
            {"id":"a","source":"model","target":"模型"}]}"#,
    )
    .unwrap();
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .glossary(Arc::new(g))
        .build()
        .unwrap();
    assert!(translator.config_summary().contains("g.x@9"));
}

// ============================================================
// 模板
// ============================================================

#[test]
fn 模板文件非法时报配置错误() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("templates.json");
    std::fs::write(&path, r#"{"schema_version":9}"#).unwrap();
    let err = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .templates_file(&path)
        .err()
        .expect("非法模板应报错");
    assert_eq!(err.code, TranslationErrorCode::ConfigurationError);
}

#[test]
fn 模板精确匹配区分大小写() {
    let table = TemplateTable::from_json(
        r#"{"schema_version":2,"id":"t","version":1,"source":"en","target":"zh-CN",
            "entries":[{"source":"Save","target":"保存"}]}"#,
    )
    .unwrap()
    .as_arc();
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .templates(table)
        .template_scope(TemplateScope::none())
        .build()
        .unwrap();

    assert_eq!(
        pollster::block_on(translator.translate(&request("Save")))
            .unwrap()
            .text,
        "保存"
    );
    // 小写不命中模板 → 走模型
    let result = pollster::block_on(translator.translate(&request("save"))).unwrap();
    assert!(!result.engine_id.starts_with("template:"));
}

// ============================================================
// 规则包
// ============================================================

fn rule_engine() -> Arc<translator_api::rules::RuleEngine> {
    rule_engine_with_version(3)
}

/// 用占位符替换版本号：JSON 里的花括号太多，直接 `format!` 会把自己坑死。
fn rule_engine_with_version(version: u32) -> Arc<translator_api::rules::RuleEngine> {
    let json = r#"{
        "schema_version": 1,
        "id": "rules.test",
        "version": __VERSION__,
        "sample_pair": ["en", "zh-CN"],
        "rules": [
            {
                "id": "norm.nfkc",
                "priority": 10,
                "action": { "kind": "nfkc" },
                "samples": [{ "input": "ＡＢＣ", "expect": "ABC" }]
            },
            {
                "id": "term.llm",
                "priority": 20,
                "language_pairs": [["en", "zh-CN"]],
                "action": {
                    "kind": "replace",
                    "from": "LLM",
                    "to": "large language model",
                    "whole_word": true
                },
                "samples": [{
                    "input": "the LLM works",
                    "expect": "the large language model works",
                    "source": "en",
                    "target": "zh-CN"
                }]
            }
        ]
    }"#
    .replace("__VERSION__", &version.to_string());
    Arc::new(translator_api::rules::RuleEngine::from_json(&json).expect("规则包应可编译"))
}

#[test]
fn 规则包在送模型之前生效() {
    let engine = Arc::new(RecordingEngine::new());
    let translator = Translator::builder()
        .engine(engine.clone() as Arc<dyn TranslationEngine>)
        .expect("注册引擎失败")
        .rules(rule_engine() as Arc<dyn TextRules>)
        .build()
        .unwrap();

    let result = pollster::block_on(translator.translate(&request("the LLM works"))).unwrap();
    assert_eq!(result.text, "the large language model works");
    assert_eq!(
        engine.last_input(),
        "the large language model works",
        "规则必须在送模型前生效"
    );
}

#[test]
fn 规则版本进入缓存身份() {
    let engine = Arc::new(RecordingEngine::new());
    let v3 = Translator::builder()
        .engine(engine.clone() as Arc<dyn TranslationEngine>)
        .expect("注册引擎失败")
        .rules(rule_engine_with_version(3) as Arc<dyn TextRules>)
        .build()
        .unwrap();
    pollster::block_on(v3.translate(&request("the LLM works"))).unwrap();
    pollster::block_on(v3.translate(&request("the LLM works"))).unwrap();
    assert_eq!(engine.calls(), 1, "同一规则版本应命中缓存");
    assert_eq!(v3.cache_stats().len, 1);

    // 换规则版本 → 必须重新翻译（否则改了规则却复用旧译文）
    let v4 = Translator::builder()
        .engine(engine.clone() as Arc<dyn TranslationEngine>)
        .expect("注册引擎失败")
        .rules(rule_engine_with_version(4) as Arc<dyn TextRules>)
        .build()
        .unwrap();
    pollster::block_on(v4.translate(&request("the LLM works"))).unwrap();
    assert_eq!(engine.calls(), 2, "规则版本变化必须让缓存失效");
}

#[test]
fn 编译失败的规则包被拒绝() {
    let err = translator_api::rules::RuleEngine::from_json(
        r#"{"schema_version":1,"id":"r","version":1,"sample_pair":["en","zh-CN"],
            "rules":[{"id":"a","action":{"kind":"replace","from":"x","to":"y"},
                      "samples":[{"input":"x","expect":"z"}]}]}"#,
    );
    assert!(err.is_err(), "金样本失败必须拒绝加载");
}

// ============================================================
// 测试用引擎：记录送进来的文本
// ============================================================

struct RecordingEngine {
    calls: std::sync::atomic::AtomicUsize,
    last: std::sync::Mutex<String>,
}

impl RecordingEngine {
    fn new() -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            last: std::sync::Mutex::new(String::new()),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn last_input(&self) -> String {
        self.last.lock().unwrap().clone()
    }
}

impl TranslationEngine for RecordingEngine {
    fn descriptor(&self) -> translator_api::EngineDescriptor {
        translator_api::EngineDescriptor::new(
            "mock.recording",
            [translator_api::Capability::Nmt],
            50,
            "MIT",
        )
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> translator_plugin_api::BoxFuture<
        'a,
        Result<translator_api::TranslationResult, translator_api::TranslationError>,
    > {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last.lock().unwrap() = req.text.clone();
            Ok(translator_api::TranslationResult {
                text: req.text.clone(),
                source: req
                    .source
                    .clone()
                    .unwrap_or_else(|| LanguageTag::new("en").unwrap()),
                target: req.target.clone(),
                engine_id: "mock.recording".to_string(),
                model_id: None,
                elapsed_ms: 0,
                warnings: Vec::new(),
            })
        })
    }
}
