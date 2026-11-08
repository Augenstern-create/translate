//! P10.1-A4-1：两个实例共享 ORT，A/B 并发推理期间 unload/重载。
//!
//! ```text
//! P4_ORT_MODEL_DIR=... cargo run -p translator-nmt-ort --features ort-runtime --release \
//!   --example p10_shared
//! ```
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use translator_nmt_ort::{OrtNmtConfig, OrtNmtEngine};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest};

fn req(text: &str, en: &LanguageTag, zh: &LanguageTag) -> TranslateRequest {
    TranslateRequest {
        text: text.into(),
        source: Some(en.clone()),
        target: zh.clone(),
        policy: Default::default(),
        glossary_id: None,
        timeout_ms: None,
    }
}

fn main() {
    let dir = std::env::var("P4_ORT_MODEL_DIR").expect("需要 P4_ORT_MODEL_DIR");
    let en = LanguageTag::new("en").unwrap();
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let config = OrtNmtConfig::new(&dir, en.clone(), zh.clone()).with_threads(1);
    let a = OrtNmtEngine::shared(config.clone()).expect("A");
    let b = OrtNmtEngine::shared(config).expect("B");
    assert!(Arc::ptr_eq(&a, &b));
    let t = Instant::now();
    let a2 = Arc::clone(&a);
    let b2 = Arc::clone(&b);
    let en_a = en.clone();
    let zh_a = zh.clone();
    let ha = thread::spawn(move || {
        let mut ok = 0;
        let mut err = 0;
        for i in 0..20 {
            match pollster::block_on(a2.translate(&req(
                &format!("Thread A sentence {i}."),
                &en_a,
                &zh_a,
            ))) {
                Ok(o) if !o.text.is_empty() => ok += 1,
                _ => err += 1,
            }
        }
        (ok, err)
    });
    let en_b = en.clone();
    let zh_b = zh.clone();
    let hb = thread::spawn(move || {
        let mut ok = 0;
        let mut err = 0;
        for i in 0..20 {
            match pollster::block_on(b2.translate(&req(
                &format!("Thread B sentence {i}."),
                &en_b,
                &zh_b,
            ))) {
                Ok(o) if !o.text.is_empty() => ok += 1,
                _ => err += 1,
            }
        }
        (ok, err)
    });
    thread::sleep(std::time::Duration::from_millis(100));
    // A 发起卸载；B 仍在并发推理。不能假设 B 一定成功，但必须得到受控结果/重载。
    let unload = a.unload();
    let (a_ok, a_err) = ha.join().unwrap();
    let (b_ok, b_err) = hb.join().unwrap();
    let reload_t = Instant::now();
    let after = pollster::block_on(b.translate(&req("After unload.", &en, &zh)));
    let reload_ms = reload_t.elapsed().as_secs_f64() * 1000.0;
    let json = serde_json::json!({
        "schema": "p10.1-ort-shared/v1",
        "shared_arc": Arc::ptr_eq(&a, &b),
        "pool_count": OrtNmtEngine::shared_engine_count(),
        "a_ok": a_ok, "a_err": a_err, "b_ok": b_ok, "b_err": b_err,
        "unload_ok": unload.is_ok(),
        "reload_ok": after.map(|o| !o.text.is_empty()).unwrap_or(false),
        "reload_ms": reload_ms,
        "elapsed_ms": t.elapsed().as_secs_f64() * 1000.0,
    });
    println!("{}", serde_json::to_string_pretty(&json).unwrap());
}
