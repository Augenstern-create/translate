//! 质量评分器（`docs/17` §6.2）：**结构精确**与**语言质量**分开算，不混成一个分数。
//!
//! 机器能判的只有结构：
//! - 受保护跨度是否逐字节保留、次数正确；
//! - 数字/单位/否定词这些"事实"是否还在（启发式：数字必须出现在译文里）。
//!
//! 机器**不能**判的：译文是否自然、可读。这里只给**代理指标**（汉字占比、长度比、
//! 源文残留率），并明确标注"不等于质量分"。人工评分表在 `docs/20` §5。

use serde_json::json;
use translator_core::protection::ProtectionPlan;

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub id: String,
    pub source: String,
    pub output: String,
    pub structural_ok: bool,
    pub structural_notes: Vec<String>,
    pub facts_ok: bool,
    pub facts_notes: Vec<String>,
    pub cjk_ratio: f64,
    pub length_ratio: f64,
    pub source_residue: f64,
    pub elapsed_ms: u64,
}

impl Row {
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "id": self.id,
            "source": self.source,
            "output": self.output,
            "structural_ok": self.structural_ok,
            "structural_notes": self.structural_notes,
            "facts_ok": self.facts_ok,
            "facts_notes": self.facts_notes,
            "cjk_ratio": round3(self.cjk_ratio),
            "length_ratio": round3(self.length_ratio),
            "source_residue": round3(self.source_residue),
            "elapsed_ms": self.elapsed_ms,
        })
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// 结构评分：受保护跨度逐字节、按顺序、次数正确。
pub fn score_structure(source: &str, output: &str) -> (bool, Vec<String>) {
    let plan = ProtectionPlan::build(source);
    let mut notes = Vec::new();
    let mut ok = true;
    let mut cursor = 0usize;
    for item in plan.items() {
        let expected = if item.kind_str() == "template_var" {
            source.matches(&item.original).count()
        } else {
            1
        };
        let got = output.matches(&item.original).count();
        if got != expected {
            ok = false;
            notes.push(format!(
                "{} {:?} 出现 {got} 次（期望 {expected}）",
                item.kind_str(),
                item.original
            ));
        }
        match output[cursor..].find(&item.original) {
            Some(offset) => cursor += offset + item.original.len(),
            None => {
                ok = false;
                notes.push(format!(
                    "{} {:?} 丢失或乱序",
                    item.kind_str(),
                    item.original
                ));
            }
        }
    }
    (ok, notes)
}

/// 事实保留的启发式：源文里的数字串必须出现在译文里。
///
/// 这是**启发式**，不是完备的事实校验：数字换个写法（"nine" vs "9"）会误判。
/// 所以 `facts_ok` 只用来发现"整段事实被丢掉"，不用来判定质量。
pub fn score_facts(source: &str, output: &str) -> (bool, Vec<String>) {
    let mut notes = Vec::new();
    let mut ok = true;
    let mut number = String::new();
    for ch in source.chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
        } else if !number.is_empty() {
            if !output.contains(&number) {
                ok = false;
                notes.push(format!("数字 {number} 未出现在译文里"));
            }
            number.clear();
        }
    }
    if !number.is_empty() && !output.contains(&number) {
        ok = false;
        notes.push(format!("数字 {number} 未出现在译文里"));
    }
    (ok, notes)
}

fn cjk_ratio(text: &str) -> f64 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    text.chars().filter(|c| is_cjk(*c)).count() as f64 / total as f64
}

fn length_ratio(source: &str, output: &str) -> f64 {
    let s = source.chars().count();
    if s == 0 {
        return 0.0;
    }
    output.chars().count() as f64 / s as f64
}

/// 源文残留率：源文里的实词有多少原样出现在译文里。
///
/// 高残留通常意味着"没翻"或"半翻"，但也可能是专有名词/URL 的正常保留——
/// 所以这是**观察指标**，不单独作为失败判据（受保护跨度另算）。
pub fn source_residue(source: &str, output: &str) -> f64 {
    let words: Vec<&str> = source
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .collect();
    if words.is_empty() {
        return 0.0;
    }
    let kept = words.iter().filter(|w| output.contains(**w)).count();
    kept as f64 / words.len() as f64
}

pub fn score_row(id: &str, source: &str, output: &str, elapsed_ms: u64) -> Row {
    let (structural_ok, structural_notes) = score_structure(source, output);
    let (facts_ok, facts_notes) = score_facts(source, output);
    Row {
        id: id.to_string(),
        source: source.to_string(),
        output: output.to_string(),
        structural_ok,
        structural_notes,
        facts_ok,
        facts_notes,
        cjk_ratio: cjk_ratio(output),
        length_ratio: length_ratio(source, output),
        source_residue: source_residue(source, output),
        elapsed_ms,
    }
}
