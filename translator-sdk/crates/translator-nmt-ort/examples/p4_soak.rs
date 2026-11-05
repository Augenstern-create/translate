//! P4-06 的长稳部分：**加载/卸载循环**与**持续翻译**下的 RSS/PSS 漂移。
//!
//! `p4_bench.rs` 量的是"热路径延迟"，这个例子量的是"会不会越跑越胖"——
//! 进程内跑 native 最容易翻车的地方就是 arena 没还回去。
//!
//! ```bash
//! export P4_ORT_MODEL_DIR=/path/to/ort-model
//! cargo run -p translator-nmt-ort --features ort-runtime --release \
//!     --example p4_soak -- 5 100
//! ```
//!
//! 参数：`<加载/卸载轮数> <持续翻译次数>`。

use std::time::Instant;

use translator_nmt_ort::{OrtNmtConfig, OrtNmtEngine};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest};

fn mem_mib() -> (f64, Option<f64>) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let rss: u64 = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(0);
    let smaps = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    let pss = smaps
        .lines()
        .find(|l| l.starts_with("Pss:"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        .map(|v: u64| v as f64 / 1024.0);
    (rss as f64 / 1024.0, pss)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cycles: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(5);
    let rounds: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(100);
    let dir = std::env::var("P4_ORT_MODEL_DIR").expect("需要 P4_ORT_MODEL_DIR");

    let en = LanguageTag::new("en").unwrap();
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let engine = OrtNmtEngine::new(OrtNmtConfig::new(&dir, en.clone(), zh.clone()).with_threads(1))
        .expect("构造失败");

    // ---------- 1) 加载/卸载循环 ----------
    let mut cycle_rows = Vec::new();
    for i in 0..cycles {
        engine.load().expect("加载失败");
        let out = pollster::block_on(engine.translate(&TranslateRequest {
            text: "The cat is on the table.".to_string(),
            source: Some(en.clone()),
            target: zh.clone(),
            policy: Default::default(),
            glossary_id: None,
            timeout_ms: None,
        }))
        .expect("翻译失败");
        engine.unload().expect("卸载失败");
        let (rss, pss) = mem_mib();
        cycle_rows.push(format!(
            "{{\"cycle\": {}, \"rss_mib\": {rss:.1}, \"pss_mib\": {}, \"output\": {:?}}}",
            i + 1,
            pss.map(|p| format!("{p:.1}"))
                .unwrap_or_else(|| "null".into()),
            out.text
        ));
        eprintln!("cycle {} -> rss {rss:.1} MiB, pss {pss:?}", i + 1);
    }

    // ---------- 2) 持续翻译 ----------
    engine.load().expect("加载失败");
    let texts = [
        "The library opens at 9.",
        "Translation quality depends on training data, tokenizer and decoding parameters, so every \
         release must be checked against a fixed golden set before shipping.",
        "Visit https://example.com/docs or write to hi@example.com with {plan} before Friday.",
    ];
    let (rss0, pss0) = mem_mib();
    let mut samples = Vec::new();
    let started = Instant::now();
    for i in 0..rounds {
        let text = texts[i % texts.len()];
        let t0 = Instant::now();
        let out = pollster::block_on(engine.translate(&TranslateRequest {
            text: text.to_string(),
            source: Some(en.clone()),
            target: zh.clone(),
            policy: Default::default(),
            glossary_id: None,
            timeout_ms: None,
        }))
        .expect("翻译失败");
        assert!(!out.text.is_empty());
        samples.push(t0.elapsed().as_millis());
        if (i + 1) % 20 == 0 {
            let (rss, pss) = mem_mib();
            eprintln!("after {} -> rss {rss:.1} MiB, pss {pss:?}", i + 1);
        }
    }
    let wall = started.elapsed().as_millis();
    let (rss1, pss1) = mem_mib();
    engine.unload().expect("卸载失败");
    let (rss2, pss2) = mem_mib();

    samples.sort_unstable();
    let p50 = samples[(samples.len() - 1) / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];

    println!("{{");
    println!("  \"schema\": \"p4-ort-soak/v1\",");
    println!("  \"caliber\": {{");
    println!("    \"build\": \"release\", \"intra_op_threads\": 1, \"cache\": \"none\",");
    println!("    \"note\": \"同一进程内连续加载/卸载与持续翻译；RSS 含 ORT 静态链接的运行库\"");
    println!("  }},");
    println!("  \"load_unload_cycles\": [");
    println!("    {}", cycle_rows.join(",\n    "));
    println!("  ],");
    println!("  \"sustained\": {{");
    println!("    \"rounds\": {rounds},");
    println!("    \"wall_ms\": {wall},");
    println!("    \"p50_ms\": {p50}, \"p95_ms\": {p95},");
    println!("    \"rss_start_mib\": {rss0:.1}, \"rss_end_mib\": {rss1:.1},");
    println!(
        "    \"pss_start_mib\": {}, \"pss_end_mib\": {},",
        pss0.map(|p| format!("{p:.1}"))
            .unwrap_or_else(|| "null".into()),
        pss1.map(|p| format!("{p:.1}"))
            .unwrap_or_else(|| "null".into())
    );
    println!("    \"rss_after_unload_mib\": {rss2:.1},");
    println!(
        "    \"pss_after_unload_mib\": {}",
        pss2.map(|p| format!("{p:.1}"))
            .unwrap_or_else(|| "null".into())
    );
    println!("  }}");
    println!("}}");
}
