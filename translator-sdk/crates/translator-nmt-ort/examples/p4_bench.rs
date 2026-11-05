//! P4-G03：ORT 后端的 Rust 侧延迟与内存实测（不参与 CI，需真实模型资产）。
//!
//! ```bash
//! export P4_ORT_MODEL_DIR=/path/to/ort-model
//! cargo run -p translator-nmt-ort --features ort-runtime --release \
//!     --example p4_bench -- 5 1 > fixtures/nmt/baseline-p4-ort-linux-x86_64.json
//! ```
//!
//! 参数：`<重复次数> <线程数> [max_new_tokens]`。
//!
//! 口径（必须随数字一起引用，否则数字没有意义）：
//! - **release 构建**、单线程 intra-op（P4-02 已测：加线程几乎无收益，见 `docs/24` §2.4）；
//! - 进程内、**无缓存**、每句独立编码 + 贪心解码；
//! - 内存是**本进程** RSS/PSS，含 ORT 运行库与 int8 权重，**不能**与 Python 侧数字直接比。

use std::time::Instant;

use translator_nmt_ort::{OrtNmtConfig, OrtNmtEngine};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest};

fn rss_kib() -> (u64, Option<u64>) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let rss = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(0);
    let smaps = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    let pss = smaps
        .lines()
        .find(|l| l.starts_with("Pss:"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse().ok());
    (rss, pss)
}

fn p50(mut v: Vec<u128>) -> u128 {
    v.sort_unstable();
    v[(v.len() - 1) / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let reps: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(5);
    let threads: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(1);
    let max_new: Option<usize> = args.get(2).and_then(|a| a.parse().ok());
    let dir = std::env::var("P4_ORT_MODEL_DIR").expect("需要 P4_ORT_MODEL_DIR");

    let en = LanguageTag::new("en").unwrap();
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let mut config = OrtNmtConfig::new(&dir, en.clone(), zh.clone()).with_threads(threads);
    if let Some(n) = max_new {
        config = config.with_max_new_tokens(n);
    }
    let engine = OrtNmtEngine::new(config).expect("构造失败");

    let before = rss_kib();
    let load_started = Instant::now();
    let identity = engine.load().expect("加载失败");
    let load_ms = load_started.elapsed().as_millis();
    let after = rss_kib();

    // 三个长度：17 / 94 / 499 个字符。
    // **文本与 `docs/24` 的 Python 侧 B 类测量逐字相同**，否则两边的 p50 不能放在一张表里。
    let cases: [(&str, &str); 3] = [
        ("17", "The library opens at 9."),
        (
            "94",
            "Translation quality depends on training data, tokenizer and decoding parameters, so every \
             release must be checked against a fixed golden set before shipping.",
        ),
        (
            "499",
            "A translation engine that runs entirely on the device must balance three things: memory, \
             latency and quality. Memory is dominated by the model weights, which are fixed at load \
             time and released when the engine is unloaded. Latency depends on the number of decoder \
             steps, and every step re-reads the same weights, so cache behaviour matters. Quality \
             depends on the training data and on how faithfully the tokenizer round-trips the source \
             text. When a sentence contains protected spans such as URLs or template variables, the \
             engine only sees plain segments, which costs one extra decoder run per segment but keeps \
             the structure exact. ",
        ),
    ];

    let mut rows = Vec::new();
    for (label, text) in &cases {
        let mut samples = Vec::new();
        // 首次调用含图优化/arena 预热，单独记一笔，不混进 p50
        let first = Instant::now();
        let out = pollster::block_on(engine.translate(
            &TranslateRequest::new(text.to_string(), zh.clone()).with_source(en.clone()),
        ))
        .expect("翻译失败");
        let first_ms = first.elapsed().as_millis();
        for _ in 0..reps {
            let t0 = Instant::now();
            let r = pollster::block_on(engine.translate(
                &TranslateRequest::new(text.to_string(), zh.clone()).with_source(en.clone()),
            ))
            .expect("翻译失败");
            assert!(!r.text.is_empty());
            samples.push(t0.elapsed().as_millis());
        }
        rows.push(format!(
            "    {{\"chars\": {label}, \"first_ms\": {first_ms}, \"p50_ms\": {}, \"reps\": {},\
             \"output\": {:?}}}",
            p50(samples.clone()),
            reps,
            out.text
        ));
    }

    let peak = rss_kib();
    println!("{{");
    println!("  \"schema\": \"p4-ort-bench/v1\",");
    println!("  \"caliber\": {{");
    println!("    \"build\": \"release\",");
    println!("    \"intra_op_threads\": {threads},");
    println!("    \"cache\": \"none\",");
    println!("    \"decoding\": \"greedy + no-repeat-3gram + 3-run stop\",");
    println!(
        "    \"note\": \"进程内单线程；内存含 ORT 运行库与 int8 权重；不是 Python 侧数字的对照\""
    );
    println!("  }},");
    println!("  \"identity\": {{");
    println!("    \"engine_id\": {:?},", identity.engine_id);
    println!("    \"model_revision\": {:?},", identity.model_revision);
    println!("    \"runtime_build\": {:?},", identity.runtime_build);
    println!("    \"load_ms\": {load_ms}");
    println!("  }},");
    println!("  \"memory_mib\": {{");
    println!("    \"rss_before_load\": {:.1},", before.0 as f64 / 1024.0);
    println!("    \"rss_after_load\": {:.1},", after.0 as f64 / 1024.0);
    println!("    \"rss_after_bench\": {:.1},", peak.0 as f64 / 1024.0);
    match peak.1 {
        Some(p) => println!("    \"pss_after_bench\": {:.1}", p as f64 / 1024.0),
        None => println!("    \"pss_after_bench\": null"),
    }
    println!("  }},");
    println!("  \"rows\": [");
    println!("{}", rows.join(",\n"));
    println!("  ]");
    println!("}}");
}
