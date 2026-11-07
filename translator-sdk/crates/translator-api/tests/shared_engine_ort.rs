//! P8.2：同一 ORT 模型/配置的多个 `Translator` 共享一个会话。
#![cfg(feature = "nmt-ort")]

use std::sync::Arc;

use translator_api::nmt_ort::{OrtNmtConfig, OrtNmtEngine};
use translator_api::Translator;

struct Fixture {
    _dir: tempfile::TempDir,
    config: OrtNmtConfig,
}

fn fixture(model: &[u8]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [
        ("encoder_model_quantized.onnx", model),
        ("decoder_model_merged_quantized.onnx", b"decoder"),
        ("tokenizer.json", b"{}"),
    ] {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    let config = OrtNmtConfig::new(
        dir.path(),
        translator_api::LanguageTag::new("en").unwrap(),
        translator_api::LanguageTag::new("zh-Hans").unwrap(),
    );
    Fixture { _dir: dir, config }
}

fn engine(translator: &Translator) -> Arc<OrtNmtEngine> {
    translator.nmt_ort_engines()[0].clone()
}

#[test]
fn sm01_ort_相同配置共享同一个会话() {
    let fx = fixture(b"encoder");
    let a = Translator::builder()
        .nmt_ort(fx.config.clone())
        .build()
        .unwrap();
    let b = Translator::builder()
        .nmt_ort(fx.config.clone())
        .build()
        .unwrap();
    assert!(Arc::ptr_eq(&engine(&a), &engine(&b)));
}

#[test]
fn sm04_ort_不同解码配置不误共享() {
    let fx = fixture(b"encoder-2");
    let a = Translator::builder()
        .nmt_ort(fx.config.clone())
        .build()
        .unwrap();
    let mut other = fx.config.clone();
    other.max_new_tokens = 64;
    let b = Translator::builder().nmt_ort(other).build().unwrap();
    assert!(!Arc::ptr_eq(&engine(&a), &engine(&b)));
}

#[test]
fn sm03_ort_最后一个实例释放后池只留弱引用() {
    let fx = fixture(b"encoder-3");
    let a = Translator::builder()
        .nmt_ort(fx.config.clone())
        .build()
        .unwrap();
    let weak = Arc::downgrade(&engine(&a));
    drop(a);
    assert!(weak.upgrade().is_none());
}
