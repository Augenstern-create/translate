//! P9.2：fastText LID vs 启发式，同集比较。
//!
//! ```text
//! P9_LID_MODEL=.build/p9-data/lid.176.ftz \
//! cargo test -p translator-analysis --features fasttext --test p9_lid_compare -- --ignored --nocapture
//! ```
#![cfg(feature = "fasttext")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use translator_analysis::{FastTextIdentifier, HeuristicIdentifier, LanguageIdentifier};
use translator_types::LanguageGuess;

#[derive(serde::Deserialize)]
struct Row {
    text: String,
    lang: String,
}

fn corpus() -> Vec<Row> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/p9/lid-corpus.jsonl");
    std::fs::read_to_string(path)
        .expect("缺少 fixtures/p9/lid-corpus.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn top(guesses: &[LanguageGuess]) -> Option<&str> {
    guesses.first().map(|g| g.language.as_str())
}

fn metrics(
    rows: &[Row],
    ident: &dyn LanguageIdentifier,
) -> (f64, f64, BTreeMap<(String, String), usize>) {
    let labels = ["en", "zh-Hans", "ja", "ko"];
    let mut correct = 0;
    let mut confusion = BTreeMap::new();
    for row in rows {
        let pred = top(&ident.identify(&row.text, 3)).unwrap_or("").to_string();
        *confusion
            .entry((row.lang.clone(), pred.clone()))
            .or_insert(0) += 1;
        if pred == row.lang {
            correct += 1;
        }
    }
    let mut f1_sum = 0.0;
    for label in labels {
        let tp = confusion
            .get(&(label.to_string(), label.to_string()))
            .copied()
            .unwrap_or(0) as f64;
        let fp: f64 = confusion
            .iter()
            .filter(|((gold, pred), _)| gold != label && pred == label)
            .map(|(_, n)| *n as f64)
            .sum();
        let fnn: f64 = confusion
            .iter()
            .filter(|((gold, pred), _)| gold == label && pred != label)
            .map(|(_, n)| *n as f64)
            .sum();
        let precision = if tp + fp == 0.0 { 0.0 } else { tp / (tp + fp) };
        let recall = if tp + fnn == 0.0 {
            0.0
        } else {
            tp / (tp + fnn)
        };
        let f1 = if precision + recall == 0.0 {
            0.0
        } else {
            2.0 * precision * recall / (precision + recall)
        };
        f1_sum += f1;
    }
    (
        correct as f64 / rows.len() as f64,
        f1_sum / labels.len() as f64,
        confusion,
    )
}

fn rss_kib() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4)
}

#[test]
#[ignore = "需要 P9_LID_MODEL 指向 lid.176.ftz"]
fn p9_2_fasttext与启发式同集对照() {
    let model = std::env::var("P9_LID_MODEL").expect("需要 P9_LID_MODEL");
    let rows = corpus();
    let rss_before = rss_kib();
    let t0 = Instant::now();
    let ft = FastTextIdentifier::from_path(&model).expect("加载 fastText 模型失败");
    let cold_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let rss_loaded = rss_kib();
    let heuristic = HeuristicIdentifier::new();

    let (h_acc, h_f1, h_conf) = metrics(&rows, &heuristic);
    let (f_acc, f_f1, f_conf) = metrics(&rows, &ft);

    let mut samples = Vec::new();
    for _ in 0..20 {
        for row in &rows {
            let t = Instant::now();
            let _ = ft.identify(&row.text, 1);
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() as f64 * 0.95) as usize];
    let rss_hot = rss_kib();
    drop(ft);
    let rss_unloaded = rss_kib();

    println!(
        "{}",
        serde_json::json!({
            "stage": "P9.2",
            "model": model,
            "corpus": rows.len(),
            "heuristic": {"accuracy": h_acc, "macro_f1": h_f1, "confusion": format!("{h_conf:?}")},
            "fasttext": {"accuracy": f_acc, "macro_f1": f_f1, "confusion": format!("{f_conf:?}")},
            "cold_load_ms": cold_ms,
            "hot_p50_ms": p50,
            "hot_p95_ms": p95,
            "rss_kib": {"before": rss_before, "loaded": rss_loaded, "hot": rss_hot, "unloaded": rss_unloaded},
        })
    );
}

#[test]
fn p94_04_模型缺失返回明确错误() {
    let err = FastTextIdentifier::from_path("/nonexistent/lid.176.ftz").unwrap_err();
    assert!(err.contains("读取"), "{err}");
}
