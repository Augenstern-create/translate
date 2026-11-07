//! P3.3 验收工具：把"性能"与"质量"量成可复现的 JSON。
//!
//! 每个数字都必须带**口径**：是否含模型加载、是否命中缓存、是否含子进程、RSS 还是 PSS。
//! 不带口径的延迟数字在这个阶段没有意义（缓存上线之后尤其如此）。
//!
//! ```text
//! p33-bench empty                       # 空 SDK：初始化 + 自身内存
//! p33-bench dictionary --dir resources/zh-en
//! p33-bench worker --bin target/debug/nmt-worker
//! p33-bench load                        # 真实模型：冷加载 + 首句
//! p33-bench nmt --n 100                 # 真实模型：20/100/500 字热翻译（缓存关闭）
//! p33-bench concurrency --levels 1,2,10
//! p33-bench unload                      # 卸载前后 RSS/PSS
//! p33-bench sustain --seconds 600       # 长稳
//! p33-bench quality --golden fixtures/nmt/golden-v1.jsonl
//! ```

mod corpus;
mod metrics;
mod quality;
mod stats;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use translator_api::mocks::{
    MockBackend, MockDictionaryEngine, MockDictionaryProvider, MockNmtEngine, MockRemoteEngine,
};
use translator_api::{LanguageTag, LookupRequest, TranslateRequest, TranslationPolicy, Translator};
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::WorkerConfig;

use crate::stats::Sample;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!("{USAGE}");
        return;
    }
    let cmd = args[0].clone();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let report = match cmd.as_str() {
        "empty" => cmd_empty(),
        "dictionary" => cmd_dictionary(
            flag("--dir").unwrap_or_else(|| "resources/zh-en".into()),
            flag("--term").unwrap_or_else(|| "计算机".into()),
            flag("--lang").unwrap_or_else(|| "zh-Hans".into()),
        ),
        "worker" => cmd_worker(
            flag("--bin").unwrap_or_else(|| "target/debug/nmt-worker".into()),
            flag("--n").and_then(|v| v.parse().ok()).unwrap_or(200),
        ),
        "load" => cmd_load(),
        "nmt" => cmd_nmt(flag("--n").and_then(|v| v.parse().ok()).unwrap_or(100)),
        "concurrency" => cmd_concurrency(
            flag("--levels").unwrap_or_else(|| "1,2,10".into()),
            flag("--n").and_then(|v| v.parse().ok()).unwrap_or(20),
        ),
        "unload" => cmd_unload(),
        "sustain" => cmd_sustain(
            flag("--seconds")
                .and_then(|v| v.parse().ok())
                .unwrap_or(600),
            flag("--interval")
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
        ),
        "quality" => cmd_quality(
            flag("--golden")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("fixtures/nmt/golden-v1.jsonl")),
        ),
        other => {
            eprintln!("未知子命令：{other}\n{USAGE}");
            std::process::exit(2);
        }
    };

    let text = serde_json::to_string_pretty(&report).expect("序列化报告失败");
    if let Some(path) = flag("--out") {
        if let Err(e) = std::fs::write(&path, &text) {
            eprintln!("写 {} 失败：{e}", path);
        } else {
            eprintln!("已写入 {path}");
        }
    }
    println!("{text}");
}

const USAGE: &str = "\
p33-bench —— P3.3 性能与质量验收工具（Linux）

子命令（每个都输出 JSON，可用 --out <文件> 另存）：
  empty                         空 SDK（只有 Mock）：初始化耗时 + 自身 RSS/PSS
  dictionary [--dir <包>] [--term <词>] [--lang <tag>]  挂真实词典包：初始化 + 查询 p50/p95
  worker [--bin <nmt-worker>]   只用 worker（--mock）：IPC 往返 p50/p95
  load                          真实模型：快照/加载/首句，native RSS/PSS/VmHWM
  nmt [--n <n>]                 真实模型：20/100/500 字热翻译（缓存关闭）+ 缓存命中延迟
  concurrency [--levels 1,2,10] [--n <每层请求数>]
  unload                        卸载前后 RSS/PSS 与残留目录
  sustain [--seconds 600] [--interval 10]
  quality [--golden <jsonl>]    结构精确 + 事实保留 + 代理指标（≠ 质量分）

真实模型需要环境变量：NMT_BERGAMOT_BIN / NMT_MODEL_DIR / NMT_WORKER_BIN（+ LD_LIBRARY_PATH）
";

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn mem_json(mem: metrics::Mem, pss_available: bool) -> serde_json::Value {
    json!({
        "rss_mib": stats::round2(mem.rss_mib()),
        "hwm_mib": stats::round2(mem.hwm_mib()),
        "pss_mib": if pss_available { json!(stats::round2(mem.pss_mib())) } else { json!(null) },
        "pss_available": pss_available,
        "note": "RSS 含共享页，多进程不能直接相加；PSS 才是跨进程可加的近似",
    })
}

// ------------------------------------------------------------------ empty

fn cmd_empty() -> serde_json::Value {
    let started = Instant::now();
    let translator = Translator::builder()
        .engine(MockDictionaryEngine::new())
        .engine(MockNmtEngine::new())
        .engine(MockRemoteEngine::new(false))
        .provider(MockDictionaryProvider::new())
        .backend(MockBackend::new())
        .build()
        .expect("构建失败");
    let init_ms = started.elapsed().as_micros() as f64 / 1000.0;

    let req = TranslateRequest::new("hello", zh()).with_source(en());
    let t = Instant::now();
    let _ = pollster::block_on(translator.translate(&req)).expect("mock 翻译失败");
    let first_ms = t.elapsed().as_micros() as f64 / 1000.0;

    let mem = metrics::self_mem();
    json!({
        "case": "empty_sdk",
        "caliber": "Mock 插件、无资源、无子进程；init 只含 builder+build",
        "init_ms": stats::round2(init_ms),
        "first_translate_ms": stats::round2(first_ms),
        "self": mem_json(mem, mem.pss_kib > 0),
        "processes": 1,
    })
}

// ------------------------------------------------------------- dictionary

fn cmd_dictionary(dir: String, term: String, lang: String) -> serde_json::Value {
    let started = Instant::now();
    let translator = Translator::builder()
        .dictionary_package(&dir)
        .build()
        .expect("挂载词典包失败");
    let init_ms = started.elapsed().as_micros() as f64 / 1000.0;

    // 口径 1：SDK 门面（路由 + 规范化 + 语言校验 + 缓存查找）
    let mut facade = Sample::new();
    for _ in 0..200 {
        let req = LookupRequest::new(term.as_str())
            .with_language(LanguageTag::new(lang.as_str()).unwrap());
        let t = Instant::now();
        let hits = pollster::block_on(translator.lookup(&req)).expect("查询失败");
        facade.push(t.elapsed().as_micros() as f64 / 1000.0);
        assert!(!hits.is_empty(), "词条应当能查到");
    }

    // 口径 2：词典 crate 的直接查询（P2.5 的 8.98 µs 就是这个口径）
    let package =
        translator_dictionary::DictionaryPackage::open(&dir, false).expect("直接打开词典包失败");
    let mut raw = Sample::new();
    for _ in 0..200 {
        let t = Instant::now();
        let hit = package.lookup_exact(term.as_str()).expect("直接查询失败");
        raw.push(t.elapsed().as_micros() as f64 / 1000.0);
        assert!(hit.is_some(), "直接查询应当命中");
    }

    let mem = metrics::self_mem();
    json!({
        "case": "dictionary_ready",
        "caliber": format!(
            "词典包 {dir}；init 含打开索引与数据库；lookup_facade = 经 Translator 路由一次，\
             lookup_crate = DictionaryPackage::lookup_exact；两者单位均为毫秒（n=200，热页缓存）"
        ),
        "init_ms": stats::round2(init_ms),
        "lookup_facade_ms": facade.to_json("ms"),
        "lookup_crate_ms": raw.to_json("ms"),
        "note": "P2.5 报告的 8.98 µs 是 crate 级口径；门面级多出来的是路由/规范化/语言校验的成本",
        "self": mem_json(mem, mem.pss_kib > 0),
        "processes": 1,
    })
}

// ----------------------------------------------------------------- worker

fn cmd_worker(bin: String, n: usize) -> serde_json::Value {
    let transport = ProcessTransport::new(WorkerConfig::new(&bin).with_args(["--mock"]), 1);
    let started = Instant::now();
    let mut sample = Sample::new();
    for _ in 0..n {
        let t = Instant::now();
        transport.health().expect("health 失败");
        sample.push(t.elapsed().as_micros() as f64 / 1000.0);
    }
    let warmup_ms = started.elapsed().as_millis() as u64;
    let metrics = transport.metrics();
    let (tree, kids) = metrics::tree_total();
    let kids_info: Vec<_> = metrics::descendants()
        .into_iter()
        .map(|p| {
            json!({
                "pid": p.pid,
                "name": p.name,
                "rss_mib": stats::round2(p.mem.rss_mib()),
                "pss_mib": if p.mem.pss_kib > 0 { json!(stats::round2(p.mem.pss_mib())) } else { json!(null) },
            })
        })
        .collect();
    transport.shutdown().ok();
    json!({
        "case": "worker_no_model",
        "caliber": "nmt-worker --mock，单 worker、单 in-flight；不含权重，因此这些内存数字不能当预算",
        "ipc_rtt_ms": sample.to_json("ms"),
        "health_total_ms": warmup_ms,
        "worker_spawns": metrics.spawns,
        "tree": mem_json(tree, tree.pss_kib > 0),
        "descendants": kids_info,
        "descendant_count": kids,
    })
}

// ------------------------------------------------------------------- load

fn real_config(cache_capacity: usize) -> translator_api::nmt_bergamot::BergamotNmtConfig {
    let mut config = translator_api::nmt_bergamot::BergamotNmtConfig::from_env()
        .expect("需要 NMT_BERGAMOT_BIN / NMT_MODEL_DIR / NMT_WORKER_BIN");
    config.cache_capacity = cache_capacity;
    config
}

fn cmd_load() -> serde_json::Value {
    let config = real_config(0);
    let manifest =
        translator_api::nmt_bergamot::build_manifest(&config.model_dir).expect("模型文件不全");
    let engine =
        translator_api::nmt_bergamot::BergamotNmtEngine::new(config.clone()).expect("构造引擎失败");

    // 快照准备在引擎内部；这里用整体时间拆成"加载"与"首句"两段
    let t = Instant::now();
    engine.load().expect("加载失败");
    let load_ms = t.elapsed().as_micros() as f64 / 1000.0;

    let req = TranslateRequest::new("Hello world!", zh())
        .with_source(en())
        .with_policy(TranslationPolicy::LocalOnly);
    let t = Instant::now();
    use translator_plugin_api::TranslationEngine;
    let _ = pollster::block_on(engine.translate(&req)).expect("首句失败");
    let first_sentence_ms = t.elapsed().as_micros() as f64 / 1000.0;

    let (tree, _) = metrics::tree_total();
    let kids: Vec<_> = metrics::descendants()
        .into_iter()
        .map(|p| {
            json!({
                "pid": p.pid,
                "name": p.name,
                "rss_mib": stats::round2(p.mem.rss_mib()),
                "hwm_mib": stats::round2(p.mem.hwm_mib()),
                "pss_mib": if p.mem.pss_kib > 0 { json!(stats::round2(p.mem.pss_mib())) } else { json!(null) },
            })
        })
        .collect();
    let model_bytes: u64 = manifest.files.values().map(|f| f.bytes).sum();
    let identity = engine.identity();
    engine.unload().ok();
    json!({
        "case": "model_load",
        "caliber": "进程内首次加载；页缓存未清（真冷盘需要 root 清 page cache，本机未做）；first_sentence 含首次 native 推理",
        "model_bytes": model_bytes,
        "model_revision": identity.as_ref().map(|i| i.model_revision.clone()),
        "runtime": identity.as_ref().and_then(|i| i.runtime.clone()),
        "load_ms": stats::round2(load_ms),
        "first_sentence_ms": stats::round2(first_sentence_ms),
        "tree": mem_json(tree, tree.pss_kib > 0),
        "descendants": kids,
    })
}

// -------------------------------------------------------------------- nmt

fn cmd_nmt(n: usize) -> serde_json::Value {
    use translator_plugin_api::TranslationEngine;
    let engine =
        translator_api::nmt_bergamot::BergamotNmtEngine::new(real_config(0)).expect("构造引擎失败");
    engine.load().expect("加载失败");

    let mut cases = Vec::new();
    for chars in [20usize, 100, 500] {
        let text = corpus::text_of_chars(chars);
        // 预热一次（不计入统计）
        let warm = TranslateRequest::new(text.clone(), zh()).with_source(en());
        let _ = pollster::block_on(engine.translate(&warm)).expect("预热失败");
        let mut sample = Sample::new();
        for _ in 0..n {
            let req = TranslateRequest::new(text.clone(), zh()).with_source(en());
            let t = Instant::now();
            let out = pollster::block_on(engine.translate(&req)).expect("翻译失败");
            sample.push(t.elapsed().as_micros() as f64 / 1000.0);
            std::hint::black_box(out);
        }
        cases.push(json!({
            "chars": text.chars().count(),
            "latency": sample.to_json("ms"),
        }));
    }

    // 分段模式的开销：带 2 个受保护跨度
    let protected = corpus::protected_sentence();
    let mut sample = Sample::new();
    for _ in 0..n {
        let req = TranslateRequest::new(protected.clone(), zh()).with_source(en());
        let t = Instant::now();
        let out = pollster::block_on(engine.translate(&req)).expect("受保护句失败");
        sample.push(t.elapsed().as_micros() as f64 / 1000.0);
        assert!(out.text.contains("https://example.com/docs"));
    }
    let protected_case = json!({
        "chars": protected.chars().count(),
        "protected_spans": 2,
        "latency": sample.to_json("ms"),
        "note": "分段模式：每个纯文本段一次 native 调用（2 个跨度 → 3 次）",
    });

    let (tree, _) = metrics::tree_total();
    let engine_cache = engine.cache_stats();
    engine.unload().ok();

    // 缓存命中延迟：单独一个开了缓存的引擎
    let cached_engine = translator_api::nmt_bergamot::BergamotNmtEngine::new(real_config(256))
        .expect("构造缓存引擎失败");
    cached_engine.load().expect("加载失败");
    let text = corpus::text_of_chars(100);
    let req = TranslateRequest::new(text.clone(), zh()).with_source(en());
    let _ = pollster::block_on(cached_engine.translate(&req)).expect("首次失败");
    let mut hit = Sample::new();
    for _ in 0..n {
        let req = TranslateRequest::new(text.clone(), zh()).with_source(en());
        let t = Instant::now();
        let _ = pollster::block_on(cached_engine.translate(&req)).expect("命中失败");
        hit.push(t.elapsed().as_micros() as f64 / 1000.0);
    }
    let hit_stats = cached_engine.cache_stats();
    cached_engine.unload().ok();

    json!({
        "case": "nmt_warm_translate",
        "caliber": "单 worker、单 in-flight、beam=1、threads=1、cache_capacity=0（除命中延迟一组）；单位毫秒",
        "n": n,
        "by_length": cases,
        "protected_sentence": protected_case,
        "cache_hit_ms": hit.to_json("ms"),
        "cache_hit_stats": {
            "hits": hit_stats.hits,
            "misses": hit_stats.misses,
            "entries": hit_stats.entries,
        },
        "engine_cache_before": {
            "hits": engine_cache.hits,
            "misses": engine_cache.misses,
            "entries": engine_cache.entries,
        },
        "tree": mem_json(tree, tree.pss_kib > 0),
    })
}

// ----------------------------------------------------------- concurrency

fn cmd_concurrency(levels: String, per_level: usize) -> serde_json::Value {
    let translator = Arc::new(real_translator());
    // 预热：把模型加载与首句开销排除在并发统计之外（否则 p99 会被 1.1 s 的加载污染）
    let warm = TranslateRequest::new(corpus::distinct(usize::MAX - 1), zh())
        .with_source(en())
        .with_policy(TranslationPolicy::LocalOnly);
    let warm_started = Instant::now();
    pollster::block_on(translator.translate(&warm)).expect("预热失败");
    let warmup_ms = warm_started.elapsed().as_micros() as f64 / 1000.0;

    let mut results = Vec::new();
    let mut peak_pss = 0.0f64;

    for level in levels
        .split(',')
        .filter_map(|v| v.trim().parse::<usize>().ok())
    {
        let mut latencies = Sample::new();
        let mut errors = 0usize;
        let started = Instant::now();
        let mut handles = Vec::new();
        for worker in 0..level {
            let translator = translator.clone();
            handles.push(std::thread::spawn(move || {
                let mut local = Vec::new();
                let mut failed = 0usize;
                for i in 0..per_level {
                    // 每一层用互不重叠的文本：避免上一层的结果被这一层缓存命中
                    let text = corpus::distinct(level * 100_000 + worker * 1_000 + i);
                    let req = TranslateRequest::new(text, zh())
                        .with_source(en())
                        .with_policy(TranslationPolicy::LocalOnly);
                    let t = Instant::now();
                    match pollster::block_on(translator.translate(&req)) {
                        Ok(_) => local.push(t.elapsed().as_micros() as f64 / 1000.0),
                        Err(_) => failed += 1,
                    }
                }
                (local, failed)
            }));
        }
        for handle in handles {
            let (local, failed) = handle.join().expect("工作线程 panic");
            for v in local {
                latencies.push(v);
            }
            errors += failed;
        }
        let wall_ms = started.elapsed().as_micros() as f64 / 1000.0;
        let (tree, _) = metrics::tree_total();
        peak_pss = peak_pss.max(tree.pss_mib());
        results.push(json!({
            "concurrency": level,
            "requests": level * per_level,
            "wall_ms": stats::round2(wall_ms),
            "throughput_rps": stats::round2((level * per_level) as f64 * 1000.0 / wall_ms.max(0.1)),
            "latency": latencies.to_json("ms"),
            "errors": errors,
            "tree_pss_mib": stats::round2(tree.pss_mib()),
        }));
    }

    let states = translator.nmt_states();
    translator.unload_nmt().ok();
    json!({
        "case": "concurrency",
        "caliber": "同一个 Translator（单 worker、单 in-flight）；已预热（加载不计入）；每层文本互不重叠，因此不会互相命中缓存；并发靠排队，吞吐不会随线程数线性上升，延迟尾部会变长",
        "warmup_ms": stats::round2(warmup_ms),
        "results": results,
        "peak_tree_pss_mib": stats::round2(peak_pss),
        "load_state_seen": format!("{:?}", states.first().map(|s| s.is_loaded())),
    })
}

fn real_translator() -> Translator {
    Translator::builder()
        .nmt_bergamot(real_config(256))
        .build()
        .expect("构建真实翻译器失败")
}

// ----------------------------------------------------------------- unload

fn cmd_unload() -> serde_json::Value {
    let translator = real_translator();
    let req = TranslateRequest::new("Hello world!", zh())
        .with_source(en())
        .with_policy(TranslationPolicy::LocalOnly);
    let load_started = Instant::now();
    pollster::block_on(translator.translate(&req)).expect("翻译失败");
    let first_ms = load_started.elapsed().as_micros() as f64 / 1000.0;

    let (before, _) = metrics::tree_total();
    let kids_before = metrics::descendants().len();
    let t = Instant::now();
    translator.unload_nmt().expect("卸载失败");
    let unload_ms = t.elapsed().as_micros() as f64 / 1000.0;
    let (after, _) = metrics::tree_total();
    let kids_after = metrics::descendants().len();

    let snapshot_root = PathBuf::from(
        std::env::var("NMT_SNAPSHOT_ROOT").unwrap_or_else(|_| "/tmp/nmt-snapshots".into()),
    );
    let leftovers = std::fs::read_dir(&snapshot_root)
        .map(|it| it.flatten().count())
        .unwrap_or(0);

    json!({
        "case": "unload",
        "caliber": "同一进程内：翻译一次（含加载）→ 卸载 → 再测内存；PSS 为全树（自己 + worker + native）",
        "first_translate_ms": stats::round2(first_ms),
        "unload_ms": stats::round2(unload_ms),
        "before": {
            "rss_mib": stats::round2(before.rss_mib()),
            "pss_mib": stats::round2(before.pss_mib()),
            "processes": kids_before + 1,
        },
        "after": {
            "rss_mib": stats::round2(after.rss_mib()),
            "pss_mib": stats::round2(after.pss_mib()),
            "processes": kids_after + 1,
        },
        "snapshot_root": snapshot_root.display().to_string(),
        "snapshot_dirs_left": leftovers,
        "load_state": format!("{:?}", translator.nmt_states().first().map(|s| s.is_loaded())),
    })
}

// ---------------------------------------------------------------- sustain

fn cmd_sustain(seconds: u64, interval: u64) -> serde_json::Value {
    let translator = Arc::new(real_translator());
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut samples = Vec::new();
    let mut latencies = Sample::new();
    let mut errors = 0usize;
    let mut done = 0usize;
    let started = Instant::now();

    loop {
        if Instant::now() >= deadline {
            break;
        }
        let tick = Instant::now();
        for i in 0..50 {
            let text = corpus::distinct(done + i);
            let req = TranslateRequest::new(text, zh())
                .with_source(en())
                .with_policy(TranslationPolicy::LocalOnly);
            let t = Instant::now();
            match pollster::block_on(translator.translate(&req)) {
                Ok(_) => latencies.push(t.elapsed().as_micros() as f64 / 1000.0),
                Err(_) => errors += 1,
            }
        }
        done += 50;
        let (tree, _) = metrics::tree_total();
        samples.push(json!({
            "elapsed_s": started.elapsed().as_secs(),
            "requests": done,
            "tree_rss_mib": stats::round2(tree.rss_mib()),
            "tree_pss_mib": stats::round2(tree.pss_mib()),
            "self_hwm_mib": stats::round2(metrics::self_mem().hwm_mib()),
            "errors": errors,
        }));
        let spent = tick.elapsed();
        let sleep = Duration::from_secs(interval).saturating_sub(spent);
        if !sleep.is_zero() {
            std::thread::sleep(sleep);
        }
    }

    let first = samples.first().cloned().unwrap_or(json!({}));
    let last = samples.last().cloned().unwrap_or(json!({}));
    let rss_first = first["tree_rss_mib"].as_f64().unwrap_or(0.0);
    let rss_last = last["tree_rss_mib"].as_f64().unwrap_or(0.0);
    translator.unload_nmt().ok();
    json!({
        "case": "sustain",
        "caliber": "持续翻译（每轮 50 句、互不相同）；每 interval 秒采样一次全树 RSS/PSS；errors 只统计返回错误，不含超时被杀",
        "seconds": seconds,
        "interval_s": interval,
        "total_requests": done,
        "errors": errors,
        "latency": latencies.to_json("ms"),
        "rss_first_mib": rss_first,
        "rss_last_mib": rss_last,
        "rss_delta_mib": stats::round2(rss_last - rss_first),
        "samples": samples,
    })
}

// ---------------------------------------------------------------- quality

fn cmd_quality(golden: PathBuf) -> serde_json::Value {
    use translator_plugin_api::TranslationEngine;
    let engine =
        translator_api::nmt_bergamot::BergamotNmtEngine::new(real_config(0)).expect("构造引擎失败");
    engine.load().expect("加载失败");

    let text = std::fs::read_to_string(&golden).expect("读不到金样本");
    let mut rows = Vec::new();
    let mut structural_pass = 0usize;
    let mut facts_pass = 0usize;
    let mut not_applicable = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line).expect("金样本不是合法 JSON");
        let id = value["id"].as_str().unwrap_or("?").to_string();
        let source = value["text"].as_str().unwrap_or("").to_string();
        // 空/纯空白输入：引擎的既定行为是 InvalidRequest（不把空白当译文），
        // 这不是"翻译失败"，在评分里必须记为不适用，而不是算成没通过结构/事实。
        if source.trim().is_empty() {
            not_applicable += 1;
            rows.push(json!({
                "id": id, "source": source, "output": null,
                "status": "not_applicable_empty_input",
                "structural_ok": null, "facts_ok": null,
            }));
            continue;
        }
        let req = TranslateRequest::new(source.clone(), zh()).with_source(en());
        let t = Instant::now();
        let out = pollster::block_on(engine.translate(&req));
        let elapsed = t.elapsed().as_millis() as u64;
        match out {
            Ok(result) => {
                let row = quality::score_row(&id, &source, &result.text, elapsed);
                if row.structural_ok {
                    structural_pass += 1;
                }
                if row.facts_ok {
                    facts_pass += 1;
                }
                let mut json = row.to_json();
                json["status"] = json!("ok");
                rows.push(json);
            }
            Err(err) => {
                rows.push(json!({
                    "id": id, "source": source, "output": null,
                    "status": format!("error:{:?}", err.code),
                    "error": err.to_string(),
                    "structural_ok": false, "facts_ok": false,
                }));
            }
        }
    }

    let total = rows.len();
    engine.unload().ok();
    let applicable = total - not_applicable;
    json!({
        "case": "quality",
        "caliber": "结构精确 = 受保护跨度字节级；事实保留 = 数字串启发式；cjk/length/residue 是代理指标，不是质量分；可读性必须人工",
        "golden": golden.display().to_string(),
        "total": total,
        "applicable": applicable,
        "not_applicable": not_applicable,
        "structural_pass": structural_pass,
        "facts_pass": facts_pass,
        "note": "structural_pass 与 facts_pass 分开报，不合成单一分数（docs/17 §6.2）",
        "rows": rows,
    })
}
