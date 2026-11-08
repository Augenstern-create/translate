//! P10.1-A4-2：模型内容变化时共享池身份必须不同（使用独立副本，不改原模型）。
//!
//! ```text
//! P4_ORT_MODEL_DIR=... cargo run -p translator-nmt-ort --features ort-runtime --release \
//!   --example p10_identity
//! ```
use std::path::Path;
use std::sync::Arc;

use translator_nmt_ort::{OrtNmtConfig, OrtNmtEngine};
use translator_plugin_api::TranslationEngine;
use translator_types::LanguageTag;

fn copy_model(src: &Path, dst: &Path) {
    let _ = std::fs::remove_dir_all(dst);
    std::fs::create_dir_all(dst).unwrap();
    for name in [
        "encoder_model_quantized.onnx",
        "decoder_model_merged_quantized.onnx",
        "tokenizer.json",
    ] {
        std::fs::copy(src.join(name), dst.join(name)).unwrap();
    }
}

fn main() {
    let src =
        std::path::PathBuf::from(std::env::var("P4_ORT_MODEL_DIR").expect("需要 P4_ORT_MODEL_DIR"));
    let dst =
        std::path::PathBuf::from(std::env::var("P4_IDENTITY_MODEL_DIR").unwrap_or_else(|_| {
            src.parent()
                .unwrap()
                .join("p10-identity-copy")
                .to_string_lossy()
                .into_owned()
        }));
    copy_model(&src, &dst);
    let en = LanguageTag::new("en").unwrap();
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let make = || OrtNmtConfig::new(&dst, en.clone(), zh.clone()).with_threads(1);
    let a = OrtNmtEngine::shared(make()).expect("A");
    let a_id = a.cache_identity();
    // 只改副本 tokenizer，模拟“同路径/revision 描述不变但内容变化”
    let tok = dst.join("tokenizer.json");
    let mut bytes = std::fs::read(&tok).unwrap();
    bytes.extend_from_slice(b"\n");
    std::fs::write(&tok, bytes).unwrap();
    let b = OrtNmtEngine::shared(make()).expect("B");
    let b_id = b.cache_identity();
    let out = serde_json::json!({
        "schema": "p10.1-ort-identity/v1",
        "same_arc": Arc::ptr_eq(&a, &b),
        "a_cache_identity": a_id,
        "b_cache_identity": b_id,
        "identity_changed": a_id != b_id,
        "original_model_dir": src,
        "experiment_copy": dst,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
