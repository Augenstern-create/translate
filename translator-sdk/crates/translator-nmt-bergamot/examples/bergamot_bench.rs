//! P4 审查（`docs/27` R-04）要求的**同文本**对照：用和 `p4_bench` 逐字相同的三句，
//! 测 P3 基线后端（Bergamot）的延迟与进程树内存。
//!
//! 为什么必须补这一条：`docs/24` §2.4 里 Bergamot 的 6.78 / 25.05 / 175.67 ms 来自
//! `tools/p33-bench` 的固定语料（`corpus.rs` 那段"图书馆"长文按词边界截断），
//! 与 ORT 候选那三句**不是同一批文本**。拿它们并排算"慢 15～26 倍"是把文本差异算进了框架差异里。
//! 这个例子把两边拉回同一批输入、同一口径（热模型、结果缓存关闭、单线程、p50）。
//!
//! ```bash
//! export NMT_BERGAMOT_BIN=/path/to/bergamot
//! export NMT_MODEL_DIR=/path/to/models
//! export NMT_WORKER_BIN=$PWD/target/release/nmt-worker
//! export LD_LIBRARY_PATH=/path/to/native-libs
//! cargo run -p translator-nmt-bergamot --release --example bergamot_bench -- 10 1
//! ```
//!
//! 参数：`<重复次数> <线程数>`。内存读的是**本进程 + 所有 `nmt-worker` 子进程**的合计
//! （Bergamot 是进程隔离后端，只读 `/proc/self` 会严重低估）。

use std::time::Instant;

use translator_nmt_bergamot::{BergamotNmtConfig, BergamotNmtEngine};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest};

/// 与 `crates/translator-nmt-ort/examples/p4_bench.rs` **逐字相同**的三句。
fn texts() -> [(&'static str, &'static str); 3] {
    [
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
    ]
}

fn read_mem(pid: u32) -> Option<(f64, f64)> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let rss_kib: u64 = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())?;
    let pss_kib: u64 = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("Pss:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0);
    Some((rss_kib as f64 / 1024.0, pss_kib as f64 / 1024.0))
}

/// 某个 pid 的父进程（读 `/proc/<pid>/stat` 的第 4 个字段）。
fn ppid_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm 里可能有空格和括号，取最后一个 ')' 之后再切
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// 本进程 + **整棵子树**（worker 是 Rust 进程，真正吃内存的是它拉起的 `bergamot` native，
/// 所以必须按父子关系走，不能按进程名认）。
fn process_tree_mem() -> (f64, f64, usize) {
    let me = std::process::id();
    let mut rss = 0.0;
    let mut pss = 0.0;
    let mut n = 0usize;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return (rss, pss, n);
    };
    let mut pids: Vec<u32> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
        .collect();
    pids.sort_unstable();
    for pid in pids {
        let mut cur = pid;
        let mut is_descendant = false;
        for _ in 0..8 {
            match ppid_of(cur) {
                Some(p) if p == me => {
                    is_descendant = true;
                    break;
                }
                Some(p) if p > 1 && p != cur => cur = p,
                _ => break,
            }
        }
        if pid != me && !is_descendant {
            continue;
        }
        if let Some((r, p)) = read_mem(pid) {
            rss += r;
            pss += p;
            n += 1;
        }
    }
    (rss, pss, n)
}

fn p50(mut v: Vec<u128>) -> u128 {
    v.sort_unstable();
    v[(v.len() - 1) / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let reps: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(10);
    let threads: u16 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(1);

    let mut config = BergamotNmtConfig::from_env().expect(
        "需要 NMT_BERGAMOT_BIN / NMT_MODEL_DIR（以及可执行的 NMT_WORKER_BIN）；见文件头注释",
    );
    config.cpu_threads = threads;
    // 与 ORT 那侧口径对齐：结果缓存关闭（0），否则命中会掩盖真实解码成本
    config.cache_capacity = 0;
    let engine = BergamotNmtEngine::new(config).expect("构造失败");

    let en = LanguageTag::new("en").unwrap();
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let (rss_before, _, _) = process_tree_mem();
    let load_started = Instant::now();
    let identity = engine.load().expect("加载真实模型失败");
    let load_ms = load_started.elapsed().as_millis();
    let (rss_after, pss_after, procs_after) = process_tree_mem();

    let mut rows = Vec::new();
    for (label, text) in texts() {
        let req = TranslateRequest::new(text, zh.clone()).with_source(en.clone());
        let first = Instant::now();
        let out = pollster::block_on(engine.translate(&req)).expect("翻译失败");
        let first_ms = first.elapsed().as_millis();
        let mut samples = Vec::new();
        for _ in 0..reps {
            let t0 = Instant::now();
            let r = pollster::block_on(engine.translate(&req)).expect("翻译失败");
            assert!(!r.text.is_empty());
            samples.push(t0.elapsed().as_millis());
        }
        rows.push(format!(
            "    {{\"chars\": {label}, \"first_ms\": {first_ms}, \"p50_ms\": {}, \"reps\": {reps}, \
             \"output\": {:?}}}",
            p50(samples),
            out.text
        ));
    }

    let (rss_end, pss_end, procs_end) = process_tree_mem();
    eprintln!("worker 进程数：加载后 {procs_after}，结束 {procs_end}");

    println!("{{");
    println!("  \"schema\": \"bergamot-bench/v1\",");
    println!("  \"caliber\": {{");
    println!("    \"build\": \"release\",");
    println!("    \"intra_op_threads\": {threads},");
    println!("    \"cache\": \"none（每次请求都关缓存：cache_capacity=0 等价的冷请求语义）\",");
    println!("    \"note\": \"**热模型、结果缓存关闭**；文本与 p4_bench 逐字相同；进程隔离后端，内存为本进程 + 全部 nmt-worker 子进程合计\"");
    println!("  }},");
    println!("  \"identity\": {{");
    println!("    \"engine_id\": {:?},", identity.engine_id);
    println!("    \"model_revision\": {:?},", identity.model_revision);
    // Option<String> 要自己渲染：`{:?}` 会写出 `Some("…")`，那不是合法 JSON
    let as_json = |v: &Option<String>| {
        v.as_deref()
            .map(|x| format!("{:?}", x))
            .unwrap_or_else(|| "null".into())
    };
    println!("    \"runtime\": {},", as_json(&identity.runtime));
    println!(
        "    \"runtime_build\": {},",
        as_json(&identity.runtime_build)
    );
    println!("    \"load_ms\": {load_ms}");
    println!("  }},");
    println!("  \"memory_mib\": {{");
    println!("    \"rss_before_load\": {rss_before:.1},");
    println!("    \"rss_after_load\": {rss_after:.1},");
    println!("    \"pss_after_load\": {pss_after:.1},");
    println!("    \"rss_after_bench\": {rss_end:.1},");
    println!("    \"pss_after_bench\": {pss_end:.1},");
    println!("    \"processes_after_load\": {procs_after},");
    println!("    \"processes_after_bench\": {procs_end}");
    println!("  }},");
    println!("  \"rows\": [");
    println!("{}", rows.join(",\n"));
    println!("  ]");
    println!("}}");
}
