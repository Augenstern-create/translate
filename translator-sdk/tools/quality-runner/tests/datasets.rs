//! 评测数据集的结构检查。
//!
//! 数据集是"标尺"：标尺本身错了，后面的数字全部没有意义。
//! 这里只做**结构**校验（字段、编号、重复、保留项类型），不评价译文。

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

fn dataset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/eval")
        .join(name)
}

fn load(name: &str) -> Vec<Value> {
    let path = dataset_path(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{} 里有非法 JSON：{e}", path.display()))
        })
        .collect()
}

#[test]
fn 所有数据集字段完整且编号唯一() {
    for name in [
        "smoke.jsonl",
        "dev.jsonl",
        "blind.jsonl",
        "regression.jsonl",
    ] {
        let cases = load(name);
        assert!(!cases.is_empty(), "{name} 不应为空");

        let mut ids = BTreeSet::new();
        let mut texts = BTreeSet::new();
        for case in &cases {
            let id = case["id"]
                .as_str()
                .unwrap_or_else(|| panic!("{name} 缺少 id"));
            assert!(ids.insert(id.to_string()), "{name} 里 id 重复：{id}");

            let text = case["text"]
                .as_str()
                .unwrap_or_else(|| panic!("{id} 缺少 text"));
            assert!(!text.trim().is_empty(), "{id} 的 text 为空");
            assert!(
                texts.insert(text.trim().to_lowercase()),
                "{name} 里 text 重复：{id}"
            );

            assert_eq!(case["source_lang"], "en", "{id} 的源语言应为 en");
            assert_eq!(case["target_lang"], "zh-CN", "{id} 的目标语言应为 zh-CN");
            assert!(
                case["category"].as_str().is_some_and(|c| !c.is_empty()),
                "{id} 缺少 category"
            );
            for field in ["must_preserve", "must_keep_numbers", "glossary"] {
                assert!(
                    case[field].as_array().is_some(),
                    "{id} 的 {field} 必须是数组"
                );
            }
            // 自建用例的参考译文尚未人工撰写：必须是 null（PENDING_HUMAN）；
            // 回归集自带人工核对过的参考译文，单独校验。
            if name != "regression.jsonl" {
                assert!(
                    case["reference_zh"].is_null(),
                    "{id} 的 reference_zh 应为 null（人工参考译文未完成）"
                );
            }
        }
    }
}

#[test]
fn 金额不应放进逐字保留项() {
    // 中文把 `$12.50` 写成 `12.50 美元` 是正确的：金额属于语义校验，
    // 放进 must_preserve（逐字匹配）会把正确译文判成失败。
    for name in ["smoke.jsonl", "dev.jsonl", "blind.jsonl"] {
        for case in load(name) {
            let id = case["id"].as_str().unwrap_or("?");
            for needle in case["must_preserve"].as_array().into_iter().flatten() {
                let text = needle.as_str().unwrap_or("");
                assert!(
                    !text.starts_with(['$', '€', '£', '¥', '₹', '₽', '₩']),
                    "{id} 把金额 {text} 放进了 must_preserve，应放进 must_keep_numbers"
                );
            }
        }
    }
}

#[test]
fn 回归集字段与严重级别合法() {
    let allowed_checks = [
        "no_residual_english",
        "entity_consistency",
        "context_word_sense",
        "detail_fidelity",
        "reference_quality",
    ];
    for case in load("regression.jsonl") {
        let id = case["id"].as_str().unwrap_or("?");
        let severity = case["severity"].as_str().unwrap_or("");
        assert!(
            matches!(severity, "critical" | "major" | "reference"),
            "{id} 的 severity 非法：{severity}"
        );
        let checks = case["expected_checks"]
            .as_array()
            .expect("expected_checks 必须是数组");
        assert!(!checks.is_empty(), "{id} 至少要声明一个检查项");
        for check in checks {
            let name = check.as_str().unwrap_or("");
            assert!(allowed_checks.contains(&name), "{id} 未知检查项：{name}");
        }
        // 禁用短语必须是字符串数组（数据驱动的漏译检查）
        assert!(
            case["forbidden_phrases"].as_array().is_some(),
            "{id} 的 forbidden_phrases 必须是数组"
        );
        // 参考译文有问题的用例必须写明问题，否则会被误判成模型失败
        if severity == "reference" {
            assert!(
                case["reference_issue"].is_string(),
                "{id} 缺少 reference_issue"
            );
        }
        assert!(
            case["reference_zh"].is_string()
                && !case["reference_zh"].as_str().unwrap_or("").is_empty(),
            "{id} 必须有参考译文（回归集不是自建空参考用例）"
        );
    }
}

#[test]
fn blind_集与开发集不重叠() {
    let dev: BTreeSet<String> = load("dev.jsonl")
        .iter()
        .chain(load("smoke.jsonl").iter())
        .map(|c| c["text"].as_str().unwrap_or("").trim().to_lowercase())
        .collect();
    for case in load("blind.jsonl") {
        let id = case["id"].as_str().unwrap_or("?");
        let text = case["text"].as_str().unwrap_or("").trim().to_lowercase();
        assert!(!dev.contains(&text), "blind 用例 {id} 与 dev/smoke 重复");
    }
}

#[test]
fn 已提交的逐句译文与数据集逐条对齐() {
    // docs/eval/runs/ 里保存了 bench/test 的完整译文；它们必须与数据集一一对应，
    // 否则人工评审会看到"缺条目/多条目"的假象。
    let runs = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("docs/eval/runs");
    let merged = runs.join("bench-test-both-models.jsonl");
    if !merged.exists() {
        eprintln!("跳过：{} 不存在", merged.display());
        return;
    }

    let dataset: BTreeSet<String> = load("bench/test.jsonl")
        .iter()
        .map(|c| c["id"].as_str().unwrap_or("").to_string())
        .collect();
    let text = std::fs::read_to_string(&merged).expect("读取合并译文失败");
    let mut seen = BTreeSet::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row: Value = serde_json::from_str(line).expect("合并译文里有非法 JSON");
        let id = row["id"].as_str().expect("缺少 id").to_string();
        for field in ["dataset_id", "source", "reference", "ds", "glm"] {
            assert!(!row[field].is_null(), "{id} 缺少字段 {field}");
        }
        assert!(
            row["ds"]["output"].is_string() && row["glm"]["output"].is_string(),
            "{id} 的模型输出必须是字符串（允许空串）"
        );
        seen.insert(id);
    }
    assert_eq!(seen, dataset, "逐句译文的 id 集合必须与数据集完全一致");
}

#[test]
fn blind_集覆盖全部类别() {
    let categories: BTreeSet<String> = load("blind.jsonl")
        .iter()
        .filter_map(|c| c["category"].as_str().map(str::to_string))
        .collect();
    for required in [
        "statement",
        "question",
        "negation",
        "conditional",
        "passive",
        "tense",
        "clause",
        "reference",
        "polysemy",
        "dialogue",
        "ui",
        "tech-doc",
        "money",
        "url",
        "paragraph",
        "glossary",
    ] {
        assert!(categories.contains(required), "blind 集缺少类别 {required}");
    }
}
