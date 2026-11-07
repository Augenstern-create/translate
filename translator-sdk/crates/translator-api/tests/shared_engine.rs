//! P8.2：同一模型/配置的多个 `Translator` 共享一个 Bergamot engine/worker。
#![cfg(feature = "nmt-bergamot")]

use std::sync::Arc;

use translator_api::nmt_bergamot::BergamotNmtConfig;
use translator_api::Translator;
use translator_nmt_bergamot::BergamotNmtEngine;

struct Fixture {
    _dir: tempfile::TempDir,
    config: BergamotNmtConfig,
}

fn fixture(model_body: &[u8]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [
        ("model.enzh.intgemm.alphas.bin", model_body),
        ("srcvocab.enzh.spm", b"src-vocab"),
        ("trgvocab.enzh.spm", b"trg-vocab"),
        ("lex.50.50.enzh.s2t.bin", b"lex"),
        ("nonbreaking_prefix.en", b"prefix"),
    ] {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    let config = BergamotNmtConfig::new("/bin/true", "/bin/true", dir.path());
    Fixture { _dir: dir, config }
}

fn engine(translator: &Translator) -> Arc<BergamotNmtEngine> {
    translator.nmt_engines()[0].clone()
}

#[test]
fn sm01_相同配置拿到同一个引擎与worker() {
    let fx = fixture(b"model-body");
    let a = Translator::builder()
        .nmt_bergamot(fx.config.clone())
        .build()
        .unwrap();
    let b = Translator::builder()
        .nmt_bergamot(fx.config.clone())
        .build()
        .unwrap();
    assert!(
        Arc::ptr_eq(&engine(&a), &engine(&b)),
        "相同模型/配置必须共享同一个 BergamotNmtEngine（同一个 worker）"
    );
}

#[test]
fn sm02_关闭一个实例不影响另一个() {
    let fx = fixture(b"model-body-2");
    let a = Translator::builder()
        .nmt_bergamot(fx.config.clone())
        .build()
        .unwrap();
    let b = Translator::builder()
        .nmt_bergamot(fx.config.clone())
        .build()
        .unwrap();
    let shared = engine(&b);
    let before = Arc::strong_count(&shared);
    assert!(before >= 2, "两个实例应各持有一份 Arc：{before}");
    drop(a);
    assert!(Arc::strong_count(&shared) >= 1, "关闭 A 后 B 仍持有引擎");
    assert_eq!(
        engine(&b).state(),
        translator_nmt_bergamot::LoadState::Unloaded
    );
}

#[test]
fn sm04_不同模型内容或解码参数不得误共享() {
    let fx_a = fixture(b"model-A");
    let fx_b = fixture(b"model-B");
    let a = Translator::builder()
        .nmt_bergamot(fx_a.config.clone())
        .build()
        .unwrap();
    let b = Translator::builder()
        .nmt_bergamot(fx_b.config.clone())
        .build()
        .unwrap();
    assert!(
        !Arc::ptr_eq(&engine(&a), &engine(&b)),
        "不同模型内容不得共享"
    );

    let mut beam = fx_a.config.clone();
    beam.beam_size = 4;
    let c = Translator::builder().nmt_bergamot(beam).build().unwrap();
    assert!(!Arc::ptr_eq(&engine(&a), &engine(&c)), "不同 beam 不得共享");
}

#[test]
fn sm03_最后一个实例释放后池不持有强引用() {
    let fx = fixture(b"model-body-3");
    let a = Translator::builder()
        .nmt_bergamot(fx.config.clone())
        .build()
        .unwrap();
    let weak = Arc::downgrade(&engine(&a));
    drop(a);
    // 池只存 Weak；最后一个强引用释放后引擎应可回收。
    assert!(
        weak.upgrade().is_none(),
        "最后一个 Translator 释放后，池不得继续持有模型"
    );
}
