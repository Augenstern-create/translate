//! P4-04：**插件替换验证** —— 同一个 `Translator` 契约下，把 native 运行时整个换掉
//! （Bergamot → ONNX Runtime），宿主调用代码一行不改。
//!
//! 默认 `#[ignore]`：需要真实 ORT 模型资产。
//!
//! ```bash
//! export P4_ORT_MODEL_DIR=/path/to/ort-model
//! cargo test -p translator-api --features nmt-ort \
//!     --test nmt_ort_facade -- --ignored --test-threads=1
//! ```

#![cfg(feature = "nmt-ort")]

use std::sync::atomic::{AtomicUsize, Ordering};

use translator_api::{nmt_ort::OrtNmtConfig, Translator, TranslatorBuilder};
use translator_types::{
    LanguageTag, LookupRequest, TranslateRequest, TranslationErrorCode, TranslationResult,
};

fn model_dir() -> String {
    std::env::var("P4_ORT_MODEL_DIR")
        .expect("需要 P4_ORT_MODEL_DIR（含 encoder/decoder/tokenizer.json）")
}

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn config() -> OrtNmtConfig {
    // P8.2 默认共享相同 ORT 配置的引擎；测试并行时，显式给每个测试不同
    // max_new_tokens，避免一个测试的 unload 影响另一个测试（共享故障域）。
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    OrtNmtConfig::new(model_dir(), en(), zh())
        .with_threads(1)
        .with_max_new_tokens(192 + n)
}

fn req(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, zh()).with_source(en())
}

fn translator() -> Translator {
    TranslatorBuilder::new()
        .nmt_ort(config())
        .build()
        .expect("构建失败")
}

/// P4-G02：**只挂 ORT 后端**的 `Translator` 能独立完成翻译。
///
/// 这是替换验证的核心：没有词典、没有规则、没有 Bergamot，
/// 仅靠 `nmt_ort()` 注入的引擎就走完整条 `translate()` 链路。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn g02_仅挂ort后端也能完成翻译() {
    let t = translator();
    let out: TranslationResult =
        pollster::block_on(t.translate(&req("The cat is on the table."))).expect("翻译失败");
    assert_eq!(out.engine_id, translator_api::nmt_ort::ENGINE_ID);
    assert_eq!(out.source, en());
    assert_eq!(out.target, zh());
    assert!(
        out.text
            .chars()
            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "译文应当含汉字：{:?}",
        out.text
    );
    println!("engine_id={} text={}", out.engine_id, out.text);
}

/// 路由可解释：引擎注册进 registry 后，`explain_route` 必须指向它，
/// 而不是"没人处理于是返回原文"。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn 路由指向ort引擎且能力可枚举() {
    let t = translator();
    let engines = t.nmt_ort_engines();
    assert_eq!(engines.len(), 1, "应当挂上 1 个 ORT 引擎");
    assert!(!engines[0].is_loaded(), "构造期不该加载权重");

    let decision = t.explain_route(&req("Good morning")).expect("路由失败");
    assert_eq!(decision.engine_id, translator_api::nmt_ort::ENGINE_ID);
    println!("route = {decision:?}");

    let caps = t.capabilities();
    assert!(
        caps.iter()
            .any(|c| c.engine_id == translator_api::nmt_ort::ENGINE_ID),
        "能力表里应当出现 ORT 引擎：{caps:?}"
    );
}

/// 加载/卸载由宿主观测与驱动（与 Bergamot 后端同一套操作语义）。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn 宿主可观测并可卸载() {
    let t = translator();
    pollster::block_on(t.translate(&req("Hello world!"))).expect("翻译失败");
    let engine = &t.nmt_ort_engines()[0];
    assert!(engine.is_loaded(), "翻译之后引擎应当是已加载");
    let id = engine.identity().expect("加载后应当有身份");
    println!("identity = {id:?}");
    assert_eq!(id.runtime, "onnxruntime");

    t.unload_nmt_ort().expect("卸载失败");
    assert!(!engine.is_loaded(), "卸载后应当回到未加载");

    // 卸载后再翻译：必须自己能重新加载，而不是报错
    let out = pollster::block_on(t.translate(&req("Hello world!"))).expect("重载后翻译失败");
    assert!(!out.text.is_empty());
}

/// 查词入口与整句翻译分开建模（ADR-003）：只挂 NMT 后端时，`lookup` 不会
/// 被路由到 NMT 上（NMT 引擎没有 `Capability::Dictionary`），查词结果是空表，
/// 而**同一个 `Translator` 的 `translate()` 仍然正常出译文**。
///
/// 这里记录的是 P1 既有行为（`Coordinator::lookup` 遍历 provider、没有 provider 就
/// 返回空表），P4 不改它；`docs/25` 把它列为观察项而不是"已解决"。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn 查词与翻译互不影响() {
    let t = translator();
    let hits = pollster::block_on(t.lookup(&LookupRequest {
        term: "cat".into(),
        language: Some(en()),
        limit: 10,
    }))
    .expect("查词本身不该失败");
    assert!(
        hits.is_empty(),
        "没有词典 provider 时不该凭空造词条：{hits:?}"
    );

    let out = pollster::block_on(t.translate(&req("The cat is on the table."))).expect("翻译失败");
    assert_eq!(out.engine_id, translator_api::nmt_ort::ENGINE_ID);
}

/// 重复挂载同一引擎必须被**明确拒绝**（引擎 id 冲突），
/// 不能让两份会话悄悄吃双份内存。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn 重复挂载同一引擎明确拒绝() {
    let c = config();
    let err = TranslatorBuilder::new()
        .nmt_ort(c.clone())
        .nmt_ort(c)
        .build()
        .expect_err("重复挂载同一 engine_id 必须失败");
    assert_eq!(err.code, TranslationErrorCode::InvalidRequest);
    assert!(err.message.contains("重复"), "错误信息应说明原因：{err}");
}
