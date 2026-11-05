//! 规则应用开销基准。
//!
//! 规则在**每一次翻译请求**上都会跑一遍，所以它必须是"免费"的：
//! 如果规则把 p50 从 17 μs 拖到 30 μs，那么无论规则多正确都该被质疑。
//!
//! ```bash
//! cargo test --release -p translator-rules --test bench_rules -- --ignored --nocapture
//! ```

use std::time::Instant;

use translator_rules::RuleEngine;
use translator_types::LanguageTag;

const RULES: &str = r#"{
    "schema_version": 1,
    "id": "rules.bench",
    "version": 1,
    "sample_pair": ["zh-Hans", "en"],
    "rules": [
        { "id": "norm.nfkc", "priority": 10, "language_pairs": [["*", "*"]],
          "action": { "kind": "nfkc" },
          "samples": [{ "input": "Ａ", "expect": "A" }] },
        { "id": "term.robot", "priority": 50, "language_pairs": [["zh-Hans", "en"]],
          "action": { "kind": "replace", "from": "机器人", "to": "robot" },
          "samples": [{ "input": "机器人", "expect": "robot" }] },
        { "id": "term.llm", "priority": 60, "language_pairs": [["en", "zh-Hans"]],
          "action": { "kind": "replace", "from": "LLM", "to": "large language model", "whole_word": true },
          "samples": [{ "input": "the LLM", "expect": "the large language model", "source": "en", "target": "zh-Hans" }] }
    ]
}"#;

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

fn bench(
    name: &str,
    text: &str,
    rounds: usize,
    engine: &RuleEngine,
    zh: &LanguageTag,
    en: &LanguageTag,
) {
    for _ in 0..200 {
        let _ = engine.apply(text, zh, en);
    }
    let mut lat = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let t = Instant::now();
        let out = engine.apply(text, zh, en);
        lat.push(t.elapsed().as_nanos());
        assert!(!out.text.is_empty() || text.is_empty());
    }
    lat.sort_unstable();
    println!(
        "{name:<18} 字节 {:>6}  p50 {:>8}  p95 {:>8}  max {:>9} ns",
        text.len(),
        percentile(&lat, 0.50),
        percentile(&lat, 0.95),
        lat.last().copied().unwrap_or(0)
    );
}

#[test]
#[ignore = "基准测试，需显式运行 --ignored"]
fn 规则应用开销() {
    let engine = RuleEngine::from_json(RULES).expect("规则应能编译");
    let zh = LanguageTag::new("zh-Hans").unwrap();
    let en = LanguageTag::new("en").unwrap();

    println!(
        "\n===== 规则应用开销（{} 条规则，{}）=====",
        engine.rule_count(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );

    // 真实输入分布：短词、短句、段落
    bench("单词", "机器人", 20_000, &engine, &zh, &en);
    bench(
        "短句",
        "我需要一个机器人来帮我做饭",
        20_000,
        &engine,
        &zh,
        &en,
    );
    let paragraph = "人工智能的发展速度很快。".repeat(100);
    bench("长段落", &paragraph, 2_000, &engine, &zh, &en);

    // 最坏情况：全角文本（NFKC 会重建整个字符串）
    let fullwidth = "ａｂｃｄｅｆｇｈｉｊ".repeat(100);
    bench("全角长文", &fullwidth, 2_000, &engine, &zh, &en);

    // 阈值：单次规则应用必须远小于一次 SQLite 点查（本机约 7 μs）
    let t = Instant::now();
    for _ in 0..1_000 {
        let _ = engine.apply("机器人", &zh, &en);
    }
    let per_call = t.elapsed().as_nanos() / 1_000;
    println!("\n单词平均耗时 {per_call} ns");
    assert!(
        per_call < 20_000,
        "单次规则应用超过 20μs，已经和词典查询同量级，需要重新设计"
    );
}
