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

use translator_nmt_ort::{token_cap, OrtNmtConfig, OrtNmtEngine, DECODE_POLICY, TOKENIZER_FILE};
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

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 把"这一档数字到底是什么负载"钉进基线文件（`docs/27` R-04）：
/// 文本摘要、源 token 数、token 上界、分词器摘要、解码策略、benchmark 版本。
/// 没有这些，跨阶段的两个 p50 根本不能比——P4 阶段就吃过这个亏。
fn workload_meta(
    tokenizer: &Option<tokenizers::Tokenizer>,
    tokenizer_sha: &str,
    text: &str,
    max_new: usize,
) -> serde_json_lite::Value {
    // 注意：分词器要**复用**，不能每档新建一份——3.8 MB 的词表 ×3 会直接把内存数字抬高，
    // 那是测量工具自己造成的偏差，不是被测量的后端。
    let src_tokens = tokenizer
        .as_ref()
        .and_then(|tk| tk.encode(text, false).ok())
        .map(|e| e.get_ids().len())
        .unwrap_or(0);
    let src_len = src_tokens + 1; // + EOS，与引擎内部口径一致
    serde_json_lite::Value::object(vec![
        (
            "text_sha256",
            serde_json_lite::Value::str(&sha256_hex(text.as_bytes())),
        ),
        (
            "text_chars",
            serde_json_lite::Value::num(text.chars().count()),
        ),
        ("src_tokens", serde_json_lite::Value::num(src_tokens)),
        ("src_len_with_eos", serde_json_lite::Value::num(src_len)),
        (
            "token_cap",
            serde_json_lite::Value::num(token_cap(src_len, max_new)),
        ),
        ("max_new_tokens", serde_json_lite::Value::num(max_new)),
        (
            "tokenizer_sha256",
            serde_json_lite::Value::str(tokenizer_sha),
        ),
        ("decode_policy", serde_json_lite::Value::str(DECODE_POLICY)),
        (
            "benchmark_revision",
            serde_json_lite::Value::str("p4-bench/v2（含 workload 元数据）"),
        ),
    ])
}

/// 只为在这个示例里拼 JSON，不额外引入 serde 依赖。
mod serde_json_lite {
    pub enum Value {
        Str(String),
        Num(usize),
        Obj(Vec<(&'static str, Value)>),
    }

    impl Value {
        pub fn str(s: &str) -> Self {
            Value::Str(s.to_string())
        }
        pub fn num(n: usize) -> Self {
            Value::Num(n)
        }
        pub fn object(v: Vec<(&'static str, Value)>) -> Self {
            Value::Obj(v)
        }
        pub fn render(&self) -> String {
            match self {
                Value::Str(s) => format!("{:?}", s),
                Value::Num(n) => n.to_string(),
                Value::Obj(fields) => {
                    let body: Vec<String> = fields
                        .iter()
                        .map(|(k, v)| format!("{:?}: {}", k, v.render()))
                        .collect();
                    format!("{{{}}}", body.join(", "))
                }
            }
        }
    }
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

    let en = LanguageTag::new("en").unwrap();
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let mut config = OrtNmtConfig::new(&dir, en.clone(), zh.clone()).with_threads(threads);
    if let Some(n) = max_new {
        config = config.with_max_new_tokens(n);
    }
    let engine = OrtNmtEngine::new(config).expect("构造失败");

    // 在**加载权重之前**算好每档的 workload 元数据，然后丢掉分词器：
    // 它对内存读数是纯污染（3.8 MB 的 JSON 词表展开后常驻几十 MiB），
    // 而我们量的是后端，不是测量工具（上一版因此把 PSS 抬高了约 70 MiB）。
    let tokenizer_path = std::path::Path::new(&dir).join(TOKENIZER_FILE);
    let tokenizer_sha = std::fs::read(&tokenizer_path)
        .map(|b| sha256_hex(&b))
        .unwrap_or_else(|_| "missing".into());
    let case_list: [(&str, &str); 3] = cases;
    let metas: Vec<String> = {
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path).ok();
        case_list
            .iter()
            .map(|(_, text)| {
                workload_meta(&tokenizer, &tokenizer_sha, text, max_new.unwrap_or(192)).render()
            })
            .collect()
    };

    let before = rss_kib();
    let load_started = Instant::now();
    let identity = engine.load().expect("加载失败");
    let load_ms = load_started.elapsed().as_millis();
    let after = rss_kib();

    let mut rows = Vec::new();
    for (idx, (label, text)) in cases.iter().enumerate() {
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
        let meta = metas[idx].clone();
        rows.push(format!(
            "    {{\"chars\": {label}, \"workload\": {meta}, \"first_ms\": {first_ms}, \
             \"p50_ms\": {}, \"reps\": {}, \"output\": {:?}}}",
            p50(samples.clone()),
            reps,
            out.text
        ));
    }

    let peak = rss_kib();
    println!("{{");
    println!("  \"caliber\": {{");
    println!("    \"build\": \"release\",");
    println!("    \"intra_op_threads\": {threads},");
    println!("    \"cache\": \"none\",");
    println!("    \"decoding\": \"greedy + no-repeat-3gram + 3-run stop\",");
    println!(
        "    \"note\": \"进程内单线程；内存含 ORT 运行库与 int8 权重；不是 Python 侧数字的对照\""
    );
    println!("  }},");
    println!("  \"schema\": \"p4-ort-bench/v2\",");
    println!("  \"identity\": {{");
    println!("    \"engine_id\": {:?},", identity.engine_id);
    println!("    \"decode_policy\": {:?},", identity.decode_policy);
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
