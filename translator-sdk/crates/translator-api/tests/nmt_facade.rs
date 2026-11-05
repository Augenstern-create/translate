//! P3.1-E：真实本地 NMT 必须能从**唯一公开入口**（`Translator`）用起来。
//!
//! 两层：
//! - 不需要模型的部分（路由、词典/NMT 分离、无静默远程回退）留在默认门禁；
//! - 真实 Bergamot 部分标 `#[ignore]`，需 `NMT_BERGAMOT_BIN` / `NMT_MODEL_DIR` 与
//!   `--features nmt-bergamot`，并且必须显式 `--ignored --test-threads=1`。
//!
//! 运行：
//! ```text
//! cargo test -p translator-api --all-features --test nmt_facade
//! cargo test -p translator-api --all-features --test nmt_facade -- --ignored --test-threads=1
//! ```

#![cfg(feature = "mocks")]

use translator_api::mocks::{
    MockBackend, MockDictionaryEngine, MockDictionaryProvider, MockNmtEngine, MockRemoteEngine,
};
use translator_api::{
    BoxFuture, Capability, EngineDescriptor, LanguageTag, PluginApiVersion, TranslateRequest,
    TranslationEngine, TranslationError, TranslationErrorCode, TranslationPolicy,
    TranslationResult, Translator,
};

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

/// 一个"坏了"的本地 NMT：用来证明本地失败不会被悄悄换成远程。
struct FailingNmt;

impl TranslationEngine for FailingNmt {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: "test.nmt.failing",
            capabilities: &[Capability::Nmt],
            priority: 100,
            plugin_api: PluginApiVersion::CURRENT,
            license: "test",
        }
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        _req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async {
            Err(TranslationError::new(
                TranslationErrorCode::RuntimeInitFailed,
                "native 进程起不来（测试用故障引擎）",
            ))
        })
    }
}

fn build_with(
    engines: impl FnOnce(translator_api::TranslatorBuilder) -> translator_api::TranslatorBuilder,
) -> Translator {
    let builder = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .engine(MockRemoteEngine::new(false))
        .provider(MockDictionaryProvider::new())
        .backend(MockBackend::new())
        .cache_capacity(16);
    engines(builder).build().expect("构建 Translator 失败")
}

// ------------------------------------------------------------ 默认门禁部分

#[test]
fn f03_单词查询与句子翻译分离_词典策略不误用nmt() {
    let translator = build_with(|b| b);
    let req = TranslateRequest::new("hello", zh())
        .with_source(en())
        .with_policy(TranslationPolicy::DictionaryOnly);
    let result = pollster::block_on(translator.translate(&req)).expect("词典翻译失败");
    assert_eq!(
        result.engine_id, "mock.dictionary",
        "单词不该走 NMT（ADR-003）"
    );

    let lookup = pollster::block_on(
        translator.lookup(&translator_api::LookupRequest::new("hello").with_language(en())),
    )
    .expect("查词失败");
    assert!(!lookup.is_empty(), "查词应有结果");
}

#[test]
fn f04_translate与translate_with_diagnostics共用同一条执行路径() {
    let translator = build_with(|b| b);
    let req = TranslateRequest::new("hello", zh()).with_source(en());
    let plain = pollster::block_on(translator.translate(&req)).expect("翻译失败");
    let diagnosed =
        pollster::block_on(translator.translate_with_diagnostics(&req)).expect("诊断翻译失败");
    assert_eq!(plain.text, diagnosed.result.text);
    assert_eq!(plain.engine_id, diagnosed.result.engine_id);
    assert!(!diagnosed.diagnostics.summary().is_empty());
}

#[test]
fn f05_本地失败不得自动回退远程() {
    let translator = build_with(|b| b.engine(std::sync::Arc::new(FailingNmt)));
    let req = TranslateRequest::new("Good morning, have a nice day!", zh()).with_source(en());
    let err = pollster::block_on(translator.translate(&req)).unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::RuntimeInitFailed,
        "本地失败必须原样上报，不能被远程结果掩盖：{err:?}"
    );
}

#[test]
fn f06_未授权远程时显式远程策略必须被拒绝() {
    let translator = build_with(|b| b);
    let req = TranslateRequest::new("Good morning, have a nice day!", zh())
        .with_source(en())
        .with_policy(TranslationPolicy::RemoteOnly);
    let err = pollster::block_on(translator.translate(&req)).unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::NetworkNotAuthorized,
        "ADR-004：默认无联网授权：{err:?}"
    );
}

// ----------------------------------------------------- 真实 Bergamot 部分

#[cfg(feature = "nmt-bergamot")]
fn real_config() -> translator_api::nmt_bergamot::BergamotNmtConfig {
    let mut config = translator_api::nmt_bergamot::BergamotNmtConfig::from_env()
        .expect("需要 NMT_BERGAMOT_BIN / NMT_MODEL_DIR / NMT_WORKER_BIN");
    config.runtime_build = std::env::var("NMT_RUNTIME_BUILD").unwrap_or_else(|_| "p3.1cde".into());
    config
}

#[cfg(feature = "nmt-bergamot")]
fn real_translator() -> Translator {
    build_with(|b| {
        let engine = std::sync::Arc::new(
            translator_api::nmt_bergamot::BergamotNmtEngine::new(real_config())
                .expect("构造引擎失败"),
        );
        b.engine(engine)
    })
}

#[cfg(feature = "nmt-bergamot")]
fn assert_chinese(text: &str) {
    assert!(
        text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "译文里没有汉字：{text:?}"
    );
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
#[cfg(feature = "nmt-bergamot")]
fn f01_门面_local_only_产出真实译文并报告引擎与模型身份() {
    let translator = real_translator();
    let req = TranslateRequest::new("Hello world!", zh())
        .with_source(en())
        .with_policy(TranslationPolicy::LocalOnly);
    let result = pollster::block_on(translator.translate(&req)).expect("真实翻译失败");
    println!(
        "F01 译文={:?} engine={} model={:?} 耗时={}ms",
        result.text, result.engine_id, result.model_id, result.elapsed_ms
    );
    assert_chinese(&result.text);
    assert_eq!(result.engine_id, "nmt.bergamot.cli");
    assert!(result.model_id.is_some(), "必须报告模型版本");
    assert_eq!(result.source.as_str(), "en");
    assert_eq!(result.target.as_str(), "zh-Hans");
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
#[cfg(feature = "nmt-bergamot")]
fn f02_auto_local_first也选真实本地nmt() {
    let translator = real_translator();
    let req = TranslateRequest::new("Hello world!", zh()).with_source(en());
    let result = pollster::block_on(translator.translate(&req)).expect("真实翻译失败");
    println!("F02 译文={:?} engine={}", result.text, result.engine_id);
    assert_eq!(
        result.engine_id, "nmt.bergamot.cli",
        "本地优先必须选中真实引擎"
    );
    assert_chinese(&result.text);
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
#[cfg(feature = "nmt-bergamot")]
fn f07_诊断路径与普通路径译文一致且报告受保护片段() {
    let translator = real_translator();
    let text = "Hello {name}! See https://x.example/a";
    let req = TranslateRequest::new(text, zh())
        .with_source(en())
        .with_policy(TranslationPolicy::LocalOnly);
    let plain = pollster::block_on(translator.translate(&req)).expect("翻译失败");
    let out = pollster::block_on(translator.translate_with_diagnostics(&req)).expect("诊断失败");
    println!(
        "F07 译文={:?} 诊断={}",
        out.result.text,
        out.diagnostics.summary()
    );
    assert_eq!(plain.text, out.result.text);
    assert_eq!(plain.engine_id, out.result.engine_id);
    assert!(
        plain.text.contains("{name}"),
        "模板变量丢失：{:?}",
        plain.text
    );
    assert!(
        plain.text.contains("https://x.example/a"),
        "URL 丢失：{:?}",
        plain.text
    );
    assert_eq!(out.diagnostics.protected_spans.len(), 2);
}
