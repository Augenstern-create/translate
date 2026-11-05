//! P4-03：ORT 后端的真实模型测试（默认 `#[ignore]`，因为需要用户自己准备的模型资产）。
//!
//! 运行方式：
//!
//! ```bash
//! export P4_ORT_MODEL_DIR=/path/to/ort-model   # encoder + decoder + tokenizer.json
//! cargo test -p translator-nmt-ort --features ort-runtime -- --ignored --test-threads=1
//! ```
//!
//! 资产准备见 `docs/24` §4 与 `tools/p4/build-tokenizer.py`。

use translator_nmt_ort::{OrtIdentity, OrtNmtConfig, OrtNmtEngine, ENGINE_ID};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest, TranslationErrorCode};

fn engine() -> OrtNmtEngine {
    let dir = std::env::var("P4_ORT_MODEL_DIR")
        .expect("需要 P4_ORT_MODEL_DIR（含 encoder/decoder/tokenizer.json）");
    let config = OrtNmtConfig::new(
        dir,
        LanguageTag::new("en").unwrap(),
        LanguageTag::new("zh-Hans").unwrap(),
    )
    .with_threads(1);
    OrtNmtEngine::new(config).expect("构造引擎失败")
}

fn req(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, LanguageTag::new("zh-Hans").unwrap())
        .with_source(LanguageTag::new("en").unwrap())
}

fn translate(engine: &OrtNmtEngine, text: &str) -> String {
    pollster::block_on(engine.translate(&req(text)))
        .unwrap_or_else(|e| panic!("翻译失败（{text}）：{e}"))
        .text
}

/// P4-G01：真实备选模型经插件契约产出译文。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn g01_真实模型产出译文() {
    let engine = engine();
    let out = translate(&engine, "The cat is on the table.");
    assert!(!out.is_empty(), "译文不能为空");
    assert!(
        out.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "译文里应当有汉字：{out:?}"
    );
    println!("The cat is on the table. -> {out}");
}

/// P4-G04：结构保护由 core 负责，换成 ORT 后端后受保护跨度仍必须逐字节保留。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn g04_受保护跨度逐字节保留() {
    let engine = engine();
    let source = "Hello {name}! See https://example.com/a for `cargo test` options.";
    let out = translate(&engine, source);
    for must in ["{name}", "https://example.com/a", "`cargo test`"] {
        assert!(out.contains(must), "受保护跨度 {must:?} 丢失：{out:?}");
    }
    println!("{source}\n  -> {out}");
}

/// 同一引擎重复翻译同一句：greedy 解码必须稳定（否则"可复现"无从谈起）。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn 同一输入输出稳定() {
    let engine = engine();
    let a = translate(&engine, "Good morning, how are you today?");
    let b = translate(&engine, "Good morning, how are you today?");
    assert_eq!(a, b, "greedy 解码在同一进程内必须稳定");
}

/// 加载身份与卸载：身份要能说明"哪个模型 + 哪个运行时"，卸载后状态要真的回到未加载。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn 身份可观测且卸载后回到未加载() {
    let engine = engine();
    assert!(!engine.is_loaded(), "构造后不应加载（构造只查文件）");
    let identity: OrtIdentity = engine.load().expect("加载失败");
    assert_eq!(identity.engine_id, ENGINE_ID);
    assert_eq!(identity.runtime, "onnxruntime");
    assert!(identity.model_revision.starts_with("nmt.ort.opus-mt#"));
    assert_eq!(identity.threads, 1);
    translate(&engine, "Hello world!");
    engine.unload().expect("卸载失败");
    assert!(!engine.is_loaded());
    assert!(engine.identity().is_none(), "卸载后不该还留着身份");
}

/// P4-02 第 1 条坑的**回归测试**：模型的 id 空间是 `vocab.json` 的联合词表，
/// 不是 `source.spm` 的 piece id。用错不会报错，只会输出通顺但错误的译文。
///
/// `[3828, 431, 50]` 是门禁阶段用 Python 参考实现核对过的答案；
/// 生成/替换 `tokenizer.json` 必须重跑这一条（`tools/p4/build-tokenizer.py --check` 同源）。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn 分词器id空间对齐联合词表() {
    let dir = std::env::var("P4_ORT_MODEL_DIR").expect("需要 P4_ORT_MODEL_DIR");
    let tk = tokenizers::Tokenizer::from_file(std::path::Path::new(&dir).join("tokenizer.json"))
        .expect("tokenizer.json 加载失败");
    let ids = tk
        .encode("Hello world!", false)
        .expect("分词失败")
        .get_ids()
        .to_vec();
    assert_eq!(
        ids,
        vec![3828, 431, 50],
        "id 空间不对：这是联合词表 vs piece id 的坑"
    );
    // 特殊 token 的位置也是契约的一部分（EOS=0、PAD=65000）
    let vocab_size = tk.get_vocab_size(true);
    assert_eq!(vocab_size, 65001, "联合词表大小应为 65001");
}

/// 不支持的语言对必须明确拒绝，不能"看起来成功"。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn 不支持的语言对明确拒绝() {
    let engine = engine();
    let ja = LanguageTag::new("ja").unwrap();
    let zh = LanguageTag::new("zh-Hans").unwrap();
    assert!(!engine.supports(&ja, &zh));
    let err = pollster::block_on(
        engine.translate(&TranslateRequest::new("こんにちは", zh).with_source(ja)),
    )
    .expect_err("不支持的语言对必须报错");
    assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
}
