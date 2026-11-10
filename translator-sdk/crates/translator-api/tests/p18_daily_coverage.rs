//! P18-L：冻结日常知识集命中率必须 >=80%。
#![cfg(feature = "dictionary")]

use std::path::PathBuf;

use translator_analysis::{EnglishLemmatizer, MorphologyAnalyzer};
use translator_api::dictionary::DictionaryPackage;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_dataset() -> Vec<serde_json::Value> {
    let path = root().join("fixtures/quality/daily-knowledge-v1.jsonl");
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn 冻结日常知识集命中率达到80() {
    let package = DictionaryPackage::open(root().join("resources/en-zh-ecdict"), true).unwrap();
    let lemmatizer = EnglishLemmatizer::new();
    let dataset = load_dataset();
    assert_eq!(dataset.len(), 10_000);

    let mut total = 0usize;
    let mut hit = 0usize;
    let mut frozen_total = 0usize;
    let mut frozen_hit = 0usize;
    let mut lemma_hits = 0usize;
    let mut category_hits: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();

    for item in &dataset {
        let unit = item["unit"].as_str().unwrap();
        let category = item["category"].as_str().unwrap();
        let split = item["split"].as_str().unwrap();
        let min_senses = item["min_senses"].as_u64().unwrap_or(1) as usize;

        let mut entries = package.lookup_exact_all(unit).unwrap();
        let exact = !entries.is_empty();
        if !exact {
            for lemma in lemmatizer.lemmas(unit) {
                entries = package.lookup_exact_all(&lemma).unwrap();
                if !entries.is_empty() {
                    lemma_hits += 1;
                    break;
                }
            }
        }
        let senses: usize = entries.iter().map(|e| e.senses.len()).sum();
        let found = !entries.is_empty() && senses >= min_senses;

        total += 1;
        if found {
            hit += 1;
        }
        if split == "frozen" {
            frozen_total += 1;
            if found {
                frozen_hit += 1;
            }
        }
        let stat = category_hits.entry(category.to_string()).or_insert((0, 0));
        stat.0 += 1;
        if found {
            stat.1 += 1;
        }
    }

    let coverage = hit as f64 / total as f64;
    let frozen = frozen_hit as f64 / frozen_total.max(1) as f64;
    assert!(coverage >= 0.80, "总命中率 {coverage:.4} < 0.80");
    assert!(frozen >= 0.80, "冻结集 {frozen:.4} < 0.80");
    for (category, (total, hit)) in &category_hits {
        assert!(
            *hit as f64 / *total as f64 >= 0.80,
            "{category} 命中率 {}/{} < 80%",
            hit,
            total
        );
    }
    let _ = lemma_hits;

    // 词形回退独立验证（测试集本身以原形为主，不应伪造 lemma 命中）。
    let forms = [
        "running", "went", "children", "better", "happier", "studies", "written", "taking",
        "bigger", "knives", "leaves", "taught", "thought", "bought", "flew", "sang", "swum",
        "women", "mice", "feet",
    ];
    let mut form_hit = 0usize;
    let mut form_lemma = 0usize;
    for form in forms {
        let entries = package.lookup_exact_all(form).unwrap();
        if !entries.is_empty() {
            form_hit += 1;
            continue;
        }
        if lemmatizer
            .lemmas(form)
            .iter()
            .any(|lemma| !package.lookup_exact_all(lemma).unwrap().is_empty())
        {
            form_hit += 1;
            form_lemma += 1;
        }
    }
    assert_eq!(
        form_hit,
        forms.len(),
        "常见词形应有 {}/{} 命中",
        form_hit,
        forms.len()
    );
    assert!(form_lemma >= 2, "至少应验证到词形回退，实际 {form_lemma}");
}

#[test]
fn 日常多义词保留多个义项() {
    let package = DictionaryPackage::open(root().join("resources/en-zh-ecdict"), true).unwrap();
    for (word, min) in [
        ("bank", 2),
        ("light", 3),
        ("get", 3),
        ("take", 3),
        ("make", 3),
        ("have", 2),
        ("go", 3),
        ("stack", 2),
        ("heap", 2),
        ("spring", 3),
    ] {
        let entries = package.lookup_exact_all(word).unwrap();
        let senses: usize = entries.iter().map(|e| e.senses.len()).sum();
        assert!(senses >= min, "{word} 只有 {senses} 个义项，期望至少 {min}");
    }
}
