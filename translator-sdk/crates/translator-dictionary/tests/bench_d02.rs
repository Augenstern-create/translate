//! D-02 基准：`dictionary.sqlite.v1` 与 `v2` 的查询延迟对比（P2.5）。
//!
//! 用**真实 CC-CEDICT**（不是合成数据）分别打成两种 payload，再跑同一组查询，
//! 比较 p50 / p95 / p99 与文件体积。
//!
//! ```bash
//! # 源文件默认找 /tmp/cedict（解压后的 CC-CEDICT 词表），也可用环境变量指定
//! CEDICT_SOURCE=/path/to/cedict_ts.u8 \
//!   cargo test --release -p translator-dictionary --test bench_d02 -- --ignored --nocapture
//! ```
//!
//! # 为什么必须用 release + 真机
//!
//! debug 构建下本项目代码未优化，延迟数字没有参考价值（见 `docs/04` §6 的说明）。
//! 而且这是**本机单次测量**：绝对数值不能跨机器比较，只能比较同一台机器上的 v1/v2 差异。

use std::io::BufReader;
use std::path::PathBuf;
use std::time::Instant;

use pack_dictionary::{pack, parse_cedict, PackOptions};
use translator_dictionary::{DictionaryPackage, PayloadKind, Provenance};

const QUERY_ROUNDS: usize = 5000;

fn source_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CEDICT_SOURCE") {
        return Some(PathBuf::from(p));
    }
    let default = PathBuf::from("/tmp/cedict");
    default.exists().then_some(default)
}

fn pack_options(payload: PayloadKind) -> PackOptions {
    PackOptions {
        id: "dictionary.cc-cedict.zh-en".to_string(),
        version: "0.1.0".to_string(),
        payload,
        language: "zh-Hans".to_string(),
        target_languages: vec!["en".to_string()],
        provenance: Provenance {
            name: "CC-CEDICT".to_string(),
            source_url: "https://www.mdbg.net/chinese/dictionary?page=cc-cedict".to_string(),
            revision: "bench".to_string(),
            license_spdx: "CC-BY-SA-4.0".to_string(),
            license_file: "LICENSE.txt".to_string(),
            retrieved_at: "2026-11-05".to_string(),
        },
        license_text: "# CC-CEDICT\nLicense: CC BY-SA 4.0\n".to_string(),
    }
}

fn percentile(sorted: &[u128], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx] as f64 / 1000.0
}

#[test]
#[ignore = "基准测试，需显式运行 --ignored --nocapture"]
fn d02_payload_v1_v2对比() {
    let Some(src) = source_path() else {
        println!("跳过：找不到 CC-CEDICT 源文件（设置 CEDICT_SOURCE 或放到 /tmp/cedict）");
        return;
    };

    println!("\n===== D-02 基准：payload v1 vs v2（真实 CC-CEDICT）=====");
    println!(
        "构建配置        {}",
        if cfg!(debug_assertions) {
            "debug（数字偏悲观，请用 --release 重跑）"
        } else {
            "release"
        }
    );
    println!("源文件          {}", src.display());

    // 只解析一次：两种 payload 用同一份词条，避免"输入不同"导致结论无效
    let file = std::fs::File::open(&src).expect("打开词表");
    let t0 = Instant::now();
    let parsed = parse_cedict(BufReader::new(file), None).expect("解析 CC-CEDICT");
    println!(
        "解析            {} ms，{} 条词条（跳过畸形行 {}）",
        t0.elapsed().as_millis(),
        parsed.entries.len(),
        parsed.skipped_lines
    );
    assert!(parsed.entries.len() > 100_000, "真实词表应有十万条以上");

    let mut results = Vec::new();
    for payload in [PayloadKind::V1, PayloadKind::V2] {
        let dir = tempfile::tempdir().expect("临时目录");
        let t0 = Instant::now();
        let stats = pack(parsed.entries.clone(), dir.path(), &pack_options(payload)).expect("打包");
        let pack_ms = t0.elapsed().as_millis();

        let package = DictionaryPackage::open(dir.path(), false).expect("打开");
        assert_eq!(package.payload_kind(), payload);
        assert_eq!(package.entry_count(), stats.records);

        // 查询集：均匀跨全表取真实词条，两轮都用同一组
        let step = (parsed.entries.len() / QUERY_ROUNDS).max(1) * 7 + 1;
        let queries: Vec<String> = (0..QUERY_ROUNDS)
            .map(|i| {
                parsed.entries[(i * step) % parsed.entries.len()]
                    .headword
                    .clone()
            })
            .collect();

        // 预热：把 FST 与 SQLite 页都过一遍，避免把冷启动算进延迟
        for term in queries.iter().take(200) {
            let _ = package.lookup_exact(term).expect("查询");
        }

        let mut lat = Vec::with_capacity(QUERY_ROUNDS);
        let mut hits = 0usize;
        let t0 = Instant::now();
        for term in &queries {
            let t = Instant::now();
            let hit = package.lookup_exact(term).expect("查询");
            lat.push(t.elapsed().as_nanos());
            if hit.is_some() {
                hits += 1;
            }
        }
        let total = t0.elapsed();
        lat.sort_unstable();

        println!(
            "\n--- {} ---\n打包            {} ms（{} 记录 / {} 键）\nindex.fst       {:.1} KB\nentries.sqlite  {:.1} MB\n命中            {}/{}",
            payload.as_str(),
            pack_ms,
            stats.records,
            stats.keys,
            stats.index_bytes as f64 / 1024.0,
            stats.db_bytes as f64 / 1_048_576.0,
            hits,
            QUERY_ROUNDS
        );
        println!(
            "精确查询 p50 {:.2} µs  p95 {:.2} µs  p99 {:.2} µs  均值 {:.2} µs",
            percentile(&lat, 0.50),
            percentile(&lat, 0.95),
            percentile(&lat, 0.99),
            total.as_micros() as f64 / QUERY_ROUNDS as f64
        );

        results.push((
            payload,
            stats.db_bytes,
            percentile(&lat, 0.50),
            percentile(&lat, 0.95),
            percentile(&lat, 0.99),
        ));
        assert_eq!(hits, QUERY_ROUNDS, "真实词表应全部命中");
    }

    // 对照表
    let [(v1p, v1b, v1_50, v1_95, v1_99), (v2p, v2b, v2_50, v2_95, v2_99)] =
        [results[0], results[1]];
    println!("\n===== 对比 =====");
    println!("payload          {:>12} {:>12}", v1p.as_str(), v2p.as_str());
    println!(
        "entries.sqlite   {:>10.1} MB {:>10.1} MB",
        v1b as f64 / 1_048_576.0,
        v2b as f64 / 1_048_576.0
    );
    println!("p50              {:>10.2} µs {:>10.2} µs", v1_50, v2_50);
    println!("p95              {:>10.2} µs {:>10.2} µs", v1_95, v2_95);
    println!("p99              {:>10.2} µs {:>10.2} µs", v1_99, v2_99);
    println!(
        "p50 变化         {:+.1}%（v2 相对 v1）",
        (v2_50 - v1_50) / v1_50 * 100.0
    );
    println!(
        "库体积变化       {:+.1}%",
        (v2b as f64 - v1b as f64) / v1b as f64 * 100.0
    );
}
