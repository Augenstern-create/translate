//! 评测数据导入器：把外部语料转换成项目统一的评测格式。
//!
//! # 为什么单独一个工具
//!
//! 原始数据（WMT24++ 的 JSONL、NTREX 的三个文本文件）与评测数据必须**分开保存**：
//! 前者是别人的发布物，后者是我们筛过、去重、划好 split 的派生数据。
//! 这个工具只做"确定性转换 + 过滤 + 记账"，不参与翻译、不改引擎。
//!
//! # 输出
//!
//! ```text
//! fixtures/eval/bench/dev.jsonl     开发集（公开，可提交）
//! fixtures/eval/bench/test.jsonl    公开基准（公开，可提交）
//! fixtures/eval/private/blind.jsonl 私有盲测（**不提交**，见 .gitignore）
//! fixtures/eval/bench/SOURCES.json  来源/许可证/哈希/样本数汇总
//! fixtures/eval/bench/STATS.md      长度与领域分布
//! ```
//!
//! # 过滤规则（全部确定性）
//!
//! - 丢弃 WMT24++ 的 `canary` 行与 `is_bad_source=true` 的行；
//! - 丢弃空源文/空参考、源文与参考相同、参考里没有汉字（中英错位）的行；
//! - 丢弃源文没有拉丁字母的行（纯数字/符号）；
//! - 按**归一化源文**全局去重（跨数据集也去重），保留先出现的（数据集顺序固定）；
//! - NTREX 按**文档**切分 dev/blind/test（同一个文档不会跨 split），避免泄漏。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 一条评测样本。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub id: String,
    pub dataset_id: String,
    pub source: String,
    pub reference: String,
    pub domain: String,
    pub document_id: String,
    pub segment_id: u32,
    pub split: String,
    pub source_lang: String,
    pub target_lang: String,
    pub script: String,
    pub license: String,
    pub char_len: u32,
    pub difficulty: String,
    pub multi_sentence: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct Wmt24ppRow {
    #[serde(default)]
    domain: String,
    #[serde(default)]
    document_id: String,
    #[serde(default)]
    segment_id: u32,
    #[serde(default)]
    is_bad_source: bool,
    #[serde(default)]
    source: String,
    #[serde(default)]
    target: String,
}

/// 归一化：小写 + 压缩空白 + 去掉零宽字符。用于去重与"源=参考"判断。
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.chars() {
        if ch == '\u{200b}' || ch == '\u{feff}' {
            continue;
        }
        if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        last_space = false;
        out.extend(ch.to_lowercase());
    }
    out.trim_end().to_string()
}

/// 是否含有汉字（用来判断参考译文是不是中文）。
pub fn has_han(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c))
}

fn has_latin(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_alphabetic())
}

/// 长度分级（按源文字符数，用于报告与难度分层）。
pub fn difficulty(chars: usize) -> &'static str {
    match chars {
        0..=60 => "short",
        61..=160 => "medium",
        161..=400 => "long",
        _ => "very-long",
    }
}

fn multi_sentence(text: &str) -> bool {
    text.matches(['.', '!', '?', '。', '！', '？']).count() >= 2
}

/// 文档切分：稳定的哈希，保证同一份语料任何时候都切出相同的 split。
pub fn split_for_document(document_id: &str) -> &'static str {
    let mut hasher = Sha256::new();
    hasher.update(document_id.as_bytes());
    let digest = hasher.finalize();
    let bucket = digest[0] % 100;
    match bucket {
        0..=14 => "dev",
        15..=29 => "blind",
        _ => "test",
    }
}

struct Builder {
    samples: Vec<Sample>,
    seen: BTreeSet<String>,
    dropped: BTreeMap<&'static str, u32>,
}

impl Builder {
    fn new() -> Self {
        Self {
            samples: Vec::new(),
            seen: BTreeSet::new(),
            dropped: BTreeMap::new(),
        }
    }

    fn drop_reason(&mut self, reason: &'static str) {
        *self.dropped.entry(reason).or_insert(0) += 1;
    }

    /// 通用过滤：返回 `None` 表示应丢弃（并记账）。
    fn accept(&mut self, source: &str, reference: &str) -> Option<()> {
        let src = source.trim();
        let refr = reference.trim();
        if src.is_empty() {
            self.drop_reason("空源文");
            return None;
        }
        if refr.is_empty() {
            self.drop_reason("空参考译文");
            return None;
        }
        if normalize(src) == normalize(refr) {
            self.drop_reason("源文与参考相同");
            return None;
        }
        if !has_han(refr) {
            self.drop_reason("参考译文不含中文");
            return None;
        }
        if !has_latin(src) {
            self.drop_reason("源文没有拉丁字母");
            return None;
        }
        if !self.seen.insert(normalize(src)) {
            self.drop_reason("源文重复");
            return None;
        }
        Some(())
    }

    fn push(&mut self, mut sample: Sample) {
        sample.char_len = sample.source.chars().count() as u32;
        sample.difficulty = difficulty(sample.char_len as usize).to_string();
        sample.multi_sentence = multi_sentence(&sample.source);
        self.samples.push(sample);
    }
}

fn read_wmt24pp(path: &Path) -> Result<Vec<Wmt24ppRow>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: Wmt24ppRow = serde_json::from_str(line)
            .map_err(|e| format!("{}:{} 解析失败：{e}", path.display(), index + 1))?;
        rows.push(row);
    }
    Ok(rows)
}

fn read_lines(path: &Path) -> Result<Vec<String>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    Ok(text.lines().map(|l| l.to_string()).collect())
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!(
            "eval-import --external <dir> --out <dir> [--private <dir>]\n\
             \n  --external  原始数据目录（fixtures/eval/external）\n  --out       转换输出目录（fixtures/eval/bench）\n  --private   私有盲测目录（fixtures/eval/private，默认 <out>/../private）"
        );
        return Ok(());
    }

    let mut external = PathBuf::from("fixtures/eval/external");
    let mut out_dir = PathBuf::from("fixtures/eval/bench");
    let mut private_dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--external" => {
                external = PathBuf::from(args.get(i + 1).ok_or("--external 需要值")?);
                i += 2;
            }
            "--out" => {
                out_dir = PathBuf::from(args.get(i + 1).ok_or("--out 需要值")?);
                i += 2;
            }
            "--private" => {
                private_dir = Some(PathBuf::from(args.get(i + 1).ok_or("--private 需要值")?));
                i += 2;
            }
            other => return Err(format!("未知参数：{other}")),
        }
    }
    let private_dir =
        private_dir.unwrap_or_else(|| out_dir.parent().unwrap_or(Path::new(".")).join("private"));

    let mut builder = Builder::new();

    // ---------- WMT24++ ----------
    let wmt_path = external.join("wmt24pp/en-zh_CN.jsonl");
    if wmt_path.exists() {
        let mut wmt_rows: Vec<Wmt24ppRow> = read_wmt24pp(&wmt_path)?;
        wmt_rows.sort_by(|a, b| {
            (a.document_id.as_str(), a.segment_id).cmp(&(b.document_id.as_str(), b.segment_id))
        });
        for row in &wmt_rows {
            if row.domain == "canary" {
                builder.drop_reason("WMT24++ canary 行");
                continue;
            }
            if row.is_bad_source {
                builder.drop_reason("WMT24++ 标记为 bad_source");
                continue;
            }
            if builder.accept(&row.source, &row.target).is_none() {
                continue;
            }
            builder.push(Sample {
                id: format!("wmt24pp-{}", builder.samples.len() + 1),
                dataset_id: "wmt24pp".to_string(),
                source: row.source.trim().to_string(),
                reference: row.target.trim().to_string(),
                domain: row.domain.clone(),
                document_id: row.document_id.clone(),
                segment_id: row.segment_id,
                // WMT24++ 是公开测试集，整份都作为公开基准
                split: "test".to_string(),
                source_lang: "en".to_string(),
                target_lang: "zh-CN".to_string(),
                script: "simplified".to_string(),
                license: "Apache-2.0".to_string(),
                char_len: 0,
                difficulty: String::new(),
                multi_sentence: false,
            });
        }
    } else {
        eprintln!("跳过 WMT24++（缺少 {}）", wmt_path.display());
    }

    // ---------- NTREX-128 ----------
    let src_path = external.join("ntrex/newstest2019-src.eng.txt");
    let ref_path = external.join("ntrex/newstest2019-ref.zho-CN.txt");
    let doc_path = external.join("ntrex/DOCUMENT_IDS.tsv");
    if src_path.exists() && ref_path.exists() && doc_path.exists() {
        let sources = read_lines(&src_path)?;
        let refs = read_lines(&ref_path)?;
        let docs = read_lines(&doc_path)?;
        if sources.len() != refs.len() || sources.len() != docs.len() {
            return Err(format!(
                "NTREX 行数不一致：src={} ref={} docs={}",
                sources.len(),
                refs.len(),
                docs.len()
            ));
        }
        for (index, ((src, refr), doc)) in
            sources.iter().zip(refs.iter()).zip(docs.iter()).enumerate()
        {
            if builder.accept(src, refr).is_none() {
                continue;
            }
            builder.push(Sample {
                id: format!("ntrex128-{}", index + 1),
                dataset_id: "ntrex128".to_string(),
                source: src.trim().to_string(),
                reference: refr.trim().to_string(),
                domain: "news".to_string(),
                document_id: doc.trim().to_string(),
                segment_id: index as u32,
                split: split_for_document(doc.trim()).to_string(),
                source_lang: "en".to_string(),
                target_lang: "zh-CN".to_string(),
                script: "simplified".to_string(),
                license: "CC-BY-SA-4.0".to_string(),
                char_len: 0,
                difficulty: String::new(),
                multi_sentence: false,
            });
        }
    } else {
        eprintln!("跳过 NTREX（缺少源文/参考/文档边界）");
    }

    if builder.samples.is_empty() {
        return Err("没有导入任何样本：请先运行 scripts/fetch-eval-data.sh".to_string());
    }

    // ---------- 写文件 ----------
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&private_dir).map_err(|e| e.to_string())?;

    let mut by_split: BTreeMap<&str, Vec<&Sample>> = BTreeMap::new();
    for sample in &builder.samples {
        by_split
            .entry(sample.split.as_str())
            .or_default()
            .push(sample);
    }

    for (split, samples) in &by_split {
        let path = match *split {
            "blind" => private_dir.join("blind.jsonl"),
            "dev" => out_dir.join("dev.jsonl"),
            _ => out_dir.join("test.jsonl"),
        };
        write_jsonl(&path, samples)?;
        println!("{:>6} 条 → {}", samples.len(), path.display());
    }

    let stats = Stats::compute(&builder.samples, &builder.dropped);
    std::fs::write(out_dir.join("STATS.md"), stats.to_markdown())
        .map_err(|e| format!("写入 STATS.md 失败：{e}"))?;
    merge_manifest(&external, &out_dir, &builder)?;
    println!("丢弃统计：{:?}", builder.dropped);
    Ok(())
}

fn write_jsonl(path: &Path, samples: &[&Sample]) -> Result<(), String> {
    let mut text = String::new();
    for sample in samples {
        text.push_str(&serde_json::to_string(sample).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    std::fs::write(path, text).map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

/// 把转换结果与原始清单合并成一份可审计的 SOURCES.json。
fn merge_manifest(external: &Path, out_dir: &Path, builder: &Builder) -> Result<(), String> {
    let raw =
        std::fs::read_to_string(external.join("SOURCES.json")).unwrap_or_else(|_| "{}".to_string());
    let mut manifest: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}));

    let mut counts = serde_json::Map::new();
    for sample in &builder.samples {
        let entry = counts
            .entry(sample.dataset_id.clone())
            .or_insert_with(|| serde_json::json!({ "total": 0, "by_split": {}, "by_domain": {} }));
        if let Some(total) = entry["total"].as_u64() {
            entry["total"] = serde_json::json!(total + 1);
        }
        let split = entry["by_split"][sample.split.clone()]
            .as_u64()
            .unwrap_or(0);
        entry["by_split"][sample.split.clone()] = serde_json::json!(split + 1);
        let domain = entry["by_domain"][sample.domain.clone()]
            .as_u64()
            .unwrap_or(0);
        entry["by_domain"][sample.domain.clone()] = serde_json::json!(domain + 1);
    }

    manifest["converted"] = serde_json::json!({
        "samples": builder.samples.len(),
        "by_dataset": counts,
        "dropped": builder.dropped,
        "filters": [
            "丢弃 WMT24++ canary 与 is_bad_source",
            "丢弃空源文/空参考、源文与参考归一化后相同",
            "丢弃参考译文不含汉字（中英错位）的行",
            "丢弃源文没有拉丁字母的行",
            "按归一化源文全局去重（跨数据集）",
            "NTREX 按 document_id 哈希切分 dev/blind/test，同一文档不跨 split"
        ],
    });

    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(out_dir.join("SOURCES.json"), text)
        .map_err(|e| format!("写入 SOURCES.json 失败：{e}"))
}

struct Stats {
    total: usize,
    by_split: BTreeMap<String, usize>,
    by_dataset: BTreeMap<String, usize>,
    by_domain: BTreeMap<String, usize>,
    by_difficulty: BTreeMap<String, usize>,
    multi_sentence: usize,
    documents: usize,
    len_p50: usize,
    len_p90: usize,
    len_max: usize,
    dropped: BTreeMap<&'static str, u32>,
}

impl Stats {
    fn compute(samples: &[Sample], dropped: &BTreeMap<&'static str, u32>) -> Self {
        let mut lengths: Vec<usize> = samples.iter().map(|s| s.char_len as usize).collect();
        lengths.sort_unstable();
        let pick = |q: f64| -> usize {
            if lengths.is_empty() {
                return 0;
            }
            let index = ((lengths.len() as f64 - 1.0) * q).round() as usize;
            lengths[index.min(lengths.len() - 1)]
        };
        let mut by_split = BTreeMap::new();
        let mut by_dataset = BTreeMap::new();
        let mut by_domain = BTreeMap::new();
        let mut by_difficulty = BTreeMap::new();
        let mut docs = std::collections::BTreeSet::new();
        let mut multi = 0;
        for sample in samples {
            *by_split.entry(sample.split.clone()).or_insert(0) += 1;
            *by_dataset.entry(sample.dataset_id.clone()).or_insert(0) += 1;
            *by_domain.entry(sample.domain.clone()).or_insert(0) += 1;
            *by_difficulty.entry(sample.difficulty.clone()).or_insert(0) += 1;
            docs.insert((sample.dataset_id.clone(), sample.document_id.clone()));
            if sample.multi_sentence {
                multi += 1;
            }
        }
        Self {
            total: samples.len(),
            by_split,
            by_dataset,
            by_domain,
            by_difficulty,
            multi_sentence: multi,
            documents: docs.len(),
            len_p50: pick(0.5),
            len_p90: pick(0.9),
            len_max: *lengths.last().unwrap_or(&0),
            dropped: dropped.clone(),
        }
    }

    fn to_markdown(&self) -> String {
        let mut out = String::from("# 外部评测集统计\n\n");
        out.push_str(&format!("- 样本总数：{}\n", self.total));
        out.push_str(&format!("- 文档数：{}\n", self.documents));
        out.push_str(&format!("- 多句样本：{}\n", self.multi_sentence));
        out.push_str(&format!(
            "- 源文长度（字符）：p50 {} / p90 {} / max {}\n\n",
            self.len_p50, self.len_p90, self.len_max
        ));
        for (title, map) in [
            ("按 split", &self.by_split),
            ("按数据集", &self.by_dataset),
            ("按领域", &self.by_domain),
            ("按长度分级", &self.by_difficulty),
        ] {
            out.push_str(&format!("## {title}\n\n| 键 | 条数 |\n|---|---|\n"));
            for (key, value) in map {
                out.push_str(&format!("| {key} | {value} |\n"));
            }
            out.push('\n');
        }
        out.push_str("## 丢弃统计\n\n| 原因 | 条数 |\n|---|---|\n");
        for (reason, count) in &self.dropped {
            out.push_str(&format!("| {reason} | {count} |\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 归一化压缩空白并小写() {
        assert_eq!(normalize("  Hello    World \n"), "hello world");
        assert_eq!(normalize("a\u{200b}b"), "ab");
    }

    #[test]
    fn 中文判断() {
        assert!(has_han("你好，世界。"));
        assert!(!has_han("Hello world"));
        assert!(!has_han("12345 — !!!"));
    }

    #[test]
    fn 长度分级边界() {
        assert_eq!(difficulty(10), "short");
        assert_eq!(difficulty(60), "short");
        assert_eq!(difficulty(61), "medium");
        assert_eq!(difficulty(161), "long");
        assert_eq!(difficulty(401), "very-long");
    }

    #[test]
    fn 文档切分稳定且覆盖三个划分() {
        let splits: BTreeSet<&str> = (0..500)
            .map(|i| split_for_document(&format!("doc-{i}")))
            .collect();
        assert!(splits.contains("dev"));
        assert!(splits.contains("test"));
        assert!(splits.contains("blind"));
        // 同一文档任何时候都一样
        assert_eq!(split_for_document("bbc.1"), split_for_document("bbc.1"));
    }

    #[test]
    fn 过滤规则() {
        let mut b = Builder::new();
        assert!(b.accept("Hello.", "你好。").is_some());
        // 重复
        assert!(b.accept("hello.", "你好啊。").is_none());
        // 空
        assert!(b.accept("", "你好。").is_none());
        // 参考不是中文
        assert!(b.accept("Another.", "Another.").is_none());
        // 源文没有字母
        assert!(b.accept("12345", "一万二千三百四十五").is_none());
        assert_eq!(b.dropped.get("源文重复"), Some(&1));
        assert_eq!(b.dropped.get("空源文"), Some(&1));
    }
}
