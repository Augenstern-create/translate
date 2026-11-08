//! P10.1-A：Windows/Linux ORT 真实模型 Soak（共享会话、内存、生命周期）。
//!
//! ```text
//! P4_ORT_MODEL_DIR=... cargo run -p translator-nmt-ort --features ort-runtime --release \
//!   --example p10_soak -- 1000
//! ```
//!
//! 只使用本地模型；不下载。输出 JSON；每 500 次采样内存。

use std::sync::Arc;
use std::time::Instant;

use translator_nmt_ort::{OrtNmtConfig, OrtNmtEngine};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest};

#[derive(Clone, Copy, Debug)]
struct Mem {
    rss_mib: f64,
    private_mib: Option<f64>,
    peak_mib: Option<f64>,
}

#[cfg(windows)]
fn mem() -> Mem {
    use std::ffi::c_void;
    #[repr(C)]
    struct Pmc {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(process: *mut c_void, counters: *mut Pmc, cb: u32) -> i32;
    }
    let mut pmc = Pmc {
        cb: std::mem::size_of::<Pmc>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) } != 0;
    if !ok {
        return Mem {
            rss_mib: 0.0,
            private_mib: None,
            peak_mib: None,
        };
    }
    Mem {
        rss_mib: pmc.working_set_size as f64 / 1_048_576.0,
        private_mib: Some(pmc.pagefile_usage as f64 / 1_048_576.0),
        peak_mib: Some(pmc.peak_working_set_size as f64 / 1_048_576.0),
    }
}

#[cfg(target_os = "linux")]
fn mem() -> Mem {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let get = |key: &str| -> f64 {
        status
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map(|kib| kib as f64 / 1024.0)
            .unwrap_or(0.0)
    };
    let private = std::fs::read_to_string("/proc/self/smaps_rollup")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("Private_Clean:") || l.starts_with("Private_Dirty:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map(|kib| kib as f64 / 1024.0);
    Mem {
        rss_mib: get("VmRSS:"),
        private_mib: private,
        peak_mib: Some(get("VmHWM:")),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn mem() -> Mem {
    Mem {
        rss_mib: 0.0,
        private_mib: None,
        peak_mib: None,
    }
}

fn mem_json(m: Mem) -> serde_json::Value {
    serde_json::json!({"rss": m.rss_mib, "private": m.private_mib, "peak": m.peak_mib})
}

fn req(text: &str, en: &LanguageTag, zh: &LanguageTag) -> TranslateRequest {
    TranslateRequest {
        text: text.to_string(),
        source: Some(en.clone()),
        target: zh.clone(),
        policy: Default::default(),
        glossary_id: None,
        timeout_ms: None,
    }
}

fn main() {
    let rounds: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let dir = std::env::var("P4_ORT_MODEL_DIR").expect("需要 P4_ORT_MODEL_DIR（本地模型）");
    let checkpoint = std::env::var("P10_CHECKPOINT")
        .unwrap_or_else(|_| format!("p10-ort-{rounds}.checkpoint.jsonl"));
    let run_id = std::env::var("P10_RUN_ID").unwrap_or_else(|_| {
        format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        )
    });
    let mut cp = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&checkpoint)
        .expect("无法创建 checkpoint");
    use std::io::Write;
    let started_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let en = LanguageTag::new("en").unwrap();
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let config = OrtNmtConfig::new(&dir, en.clone(), zh.clone()).with_threads(1);

    let before = mem();
    let a = OrtNmtEngine::shared(config.clone()).expect("共享构造 A 失败");
    let b = OrtNmtEngine::shared(config).expect("共享构造 B 失败");
    let shared = Arc::ptr_eq(&a, &b);
    let pool_count = OrtNmtEngine::shared_engine_count();
    let t_load = Instant::now();
    let identity = a.load().expect("加载失败");
    let cold_ms = t_load.elapsed().as_secs_f64() * 1000.0;
    let loaded = mem();

    let texts = [
        "Hello world.",
        "I like this book.",
        "Please open the door.",
        "I do not agree.",
        "The price is 125 dollars.",
        "Hello, {name}.",
        "Visit https://example.com.",
        "The library opens at nine tomorrow and closes at six in the evening.",
        "Translation quality depends on the training data, tokenizer and decoding parameters.",
        "This is a short sentence.",
    ];
    let mut samples = Vec::with_capacity(rounds);
    let mut errors = 0usize;
    let mut mem_marks = Vec::new();
    for i in 0..rounds {
        let t = Instant::now();
        match pollster::block_on(a.translate(&req(texts[i % texts.len()], &en, &zh))) {
            Ok(out) if !out.text.is_empty() => samples.push(t.elapsed().as_secs_f64() * 1000.0),
            _ => errors += 1,
        }
        if (i + 1) % 500 == 0 {
            let m = mem();
            mem_marks.push((i + 1, m.rss_mib, m.private_mib, m.peak_mib));
            let mut sorted = samples.clone();
            sorted.sort_by(|x, y| x.partial_cmp(y).unwrap());
            let p50 = if sorted.is_empty() {
                0.0
            } else {
                sorted[sorted.len() / 2]
            };
            let p95 = if sorted.is_empty() {
                0.0
            } else {
                sorted[(sorted.len() as f64 * 0.95) as usize]
            };
            let line = serde_json::json!({
                "seq": i + 1, "run_id": run_id, "pid": std::process::id(),
                "round": i + 1, "completed": i + 1, "errors": errors,
                "p50_ms": p50, "p95_ms": p95,
                "rss_mib": m.rss_mib, "private_mib": m.private_mib, "peak_mib": m.peak_mib,
                "started_epoch": started_epoch, "continuous": true,
            });
            writeln!(cp, "{}", line).unwrap();
            cp.flush().unwrap();
            eprintln!(
                "after {}: rss {:.1} MiB private {:?}",
                i + 1,
                m.rss_mib,
                m.private_mib
            );
        }
    }
    let hot = mem();
    a.unload().expect("卸载失败");
    let unloaded = mem();
    // B 共享同一个会话；unload 后重新翻译会重新加载（共享生命周期语义）
    let reload_t = Instant::now();
    let b_out = pollster::block_on(b.translate(&req("The cat is on the table.", &en, &zh)));
    let reload_ms = reload_t.elapsed().as_secs_f64() * 1000.0;
    let after_b = mem();
    drop(a);
    drop(b);
    let after_drop = mem();

    samples.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() as f64 * 0.95) as usize];
    let json = serde_json::json!({
        "schema": "p10.1-ort-soak/v1",
        "platform": std::env::consts::OS,
        "rounds": rounds,
        "completed": samples.len(),
        "run_id": run_id,
        "checkpoint": checkpoint,
        "continuous": true,
        "model_dir": dir,
        "model_revision": identity.model_revision,
        "runtime": identity.runtime,
        "runtime_build": identity.runtime_build,
        "shared_arc": shared,
        "shared_pool_count": pool_count,
        "cold_load_ms": cold_ms,
        "p50_ms": p50,
        "p95_ms": p95,
        "errors": errors,
        "b_reload_ok": b_out.map(|o| !o.text.is_empty()).unwrap_or(false),
        "b_reload_ms": reload_ms,
        "mem_mib": {
            "before": mem_json(before),
            "loaded": mem_json(loaded),
            "hot": mem_json(hot),
            "unloaded": mem_json(unloaded),
            "after_b": mem_json(after_b),
            "after_drop": mem_json(after_drop),
            "marks": mem_marks.iter().map(|(n,r,p,peak)| serde_json::json!({"round":n,"rss":r,"private":p,"peak":peak})).collect::<Vec<_>>()
        }
    });
    println!("{}", serde_json::to_string_pretty(&json).unwrap());
}
