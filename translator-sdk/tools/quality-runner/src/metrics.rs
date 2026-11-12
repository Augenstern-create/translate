//! 自动质量指标：chrF++ 与 BLEU。
//!
//! # 为什么是这两个
//!
//! - **chrF++**（字符 1–6 gram + 词 1–2 gram，β=2）对中文最稳：中文没有空格分词，
//!   BLEU 的"词"切分对中文本身就不公平；chrF 在字符层面比较，跨语言都成立。
//! - **BLEU-4** 仍然提供，但只作为参考数字：本项目用**空白分词**，
//!   与 sacrebleu 的 13a tokenizer 结果不同（中文尤其明显），文档里已注明。
//! - COMET 需要神经网络模型（PyTorch + 权重下载），属于可选扩展，本仓库不内置。
//!
//! # 实现口径（写清楚，避免"看起来像 sacrebleu"）
//!
//! chrF++：
//!
//! - 预处理：Unicode 小写化 + 压缩连续空白；
//! - 字符 n-gram：n = 1..6（按字符计，**不去标点**）；
//! - 词 n-gram：n = 1..2（按空白切分）；
//! - 每个 n 阶分别算 precision/recall/F_β（β=2），再对 8 个阶取算术平均；
//! - 平滑：`(matches + eps) / (total + eps)`，eps = 1e-16。
//!
//! BLEU-4：
//!
//! - 空白分词 + 小写化；
//! - 1–4 gram 精度，几何平均，指数平滑（smooth_method = exp）；
//! - brevity penalty：`exp(1 - ref_len/hyp_len)`（hyp 更短时）。

use std::collections::HashMap;

const EPS: f64 = 1e-16;
/// chrF 的 β（召回权重更高，2 是论文默认）。
const CHRF_BETA: f64 = 2.0;
const CHAR_ORDER: usize = 6;
const WORD_ORDER: usize = 2;

fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.chars() {
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
    out.trim().to_string()
}

fn char_ngrams(text: &str, n: usize) -> HashMap<Vec<char>, usize> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut map = HashMap::new();
    if chars.len() < n {
        return map;
    }
    for window in chars.windows(n) {
        *map.entry(window.to_vec()).or_insert(0) += 1;
    }
    map
}

fn word_ngrams(text: &str, n: usize) -> HashMap<Vec<String>, usize> {
    let words: Vec<String> = text.split_whitespace().map(|w| w.to_string()).collect();
    let mut map = HashMap::new();
    if words.len() < n {
        return map;
    }
    for window in words.windows(n) {
        *map.entry(window.to_vec()).or_insert(0) += 1;
    }
    map
}

fn f_beta<K: Eq + std::hash::Hash + Clone>(
    hyp: &HashMap<K, usize>,
    refr: &HashMap<K, usize>,
    beta: f64,
) -> f64 {
    let matched: usize = hyp
        .iter()
        .map(|(gram, count)| refr.get(gram).copied().unwrap_or(0).min(*count))
        .sum();
    let hyp_total: usize = hyp.values().sum();
    let ref_total: usize = refr.values().sum();

    let precision = (matched as f64 + EPS) / (hyp_total as f64 + EPS);
    let recall = (matched as f64 + EPS) / (ref_total as f64 + EPS);
    let beta_sq = beta * beta;
    if precision + recall == 0.0 {
        return 0.0;
    }
    (1.0 + beta_sq) * precision * recall / (beta_sq * precision + recall)
}

/// chrF++ 分数（0–100）。
pub fn chrf_pp(hypothesis: &str, reference: &str) -> f64 {
    let hyp = normalize(hypothesis);
    let refr = normalize(reference);
    if hyp.is_empty() || refr.is_empty() {
        return 0.0;
    }

    let mut total = 0.0;
    let mut orders = 0;
    // 参考里没有该阶 n-gram 时**跳过**（effective order）：
    // 否则两个毫无共同字符的短句会因为"空集合 precision=1"拿到虚高分。
    for n in 1..=CHAR_ORDER {
        let ref_grams = char_ngrams(&refr, n);
        if ref_grams.is_empty() {
            continue;
        }
        total += f_beta(&char_ngrams(&hyp, n), &ref_grams, CHRF_BETA);
        orders += 1;
    }
    for n in 1..=WORD_ORDER {
        let ref_grams = word_ngrams(&refr, n);
        if ref_grams.is_empty() {
            continue;
        }
        total += f_beta(&word_ngrams(&hyp, n), &ref_grams, CHRF_BETA);
        orders += 1;
    }
    if orders == 0 {
        return 0.0;
    }
    100.0 * total / orders as f64
}

/// 中英混排分词：CJK 按**字**切开，其余按空白切分。
///
/// 纯空白分词对中文没有任何 2/3/4-gram 命中，BLEU 恒为 0，毫无信息量；
/// 这个折中方案让 BLEU 在 zh 上可用，但它**不是** sacrebleu 的 13a tokenizer，
/// 结果不能与 sacrebleu 直接比较（chrF++ 才是本项目的口径基准）。
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    for ch in text.chars() {
        let is_cjk = ('\u{3400}'..='\u{4DBF}').contains(&ch)
            || ('\u{4E00}'..='\u{9FFF}').contains(&ch)
            || ('\u{3040}'..='\u{30FF}').contains(&ch)
            || ('\u{AC00}'..='\u{D7AF}').contains(&ch);
        if is_cjk {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            tokens.push(ch.to_string());
        } else if ch.is_whitespace() {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    tokens
}

fn ngram_counts(gram: &[&str], order: usize) -> HashMap<Vec<String>, usize> {
    let mut map = HashMap::new();
    if gram.len() < order {
        return map;
    }
    for window in gram.windows(order) {
        let key: Vec<String> = window.iter().map(|w| (*w).to_string()).collect();
        *map.entry(key).or_insert(0) += 1;
    }
    map
}

/// BLEU-4（0–100），中英混排分词（CJK 按字）+ 有效阶。
pub fn bleu4(hypothesis: &str, reference: &str) -> f64 {
    let hyp_owned = tokenize(&normalize(hypothesis));
    let refr_owned = tokenize(&normalize(reference));
    let hyp: Vec<&str> = hyp_owned.iter().map(|s| s.as_str()).collect();
    let refr: Vec<&str> = refr_owned.iter().map(|s| s.as_str()).collect();
    if hyp.is_empty() || refr.is_empty() {
        return 0.0;
    }

    let mut log_sum = 0.0;
    let mut effective = 0usize;
    for order in 1..=4 {
        let hyp_counts = ngram_counts(&hyp, order);
        let ref_counts = ngram_counts(&refr, order);
        if hyp_counts.is_empty() || ref_counts.is_empty() {
            // effective order：这句话根本没有这一阶 n-gram（例如 2 词句子没有 4-gram）
            continue;
        }
        let matched: usize = hyp_counts
            .iter()
            .map(|(gram, count)| ref_counts.get(gram).copied().unwrap_or(0).min(*count))
            .sum();
        if matched == 0 {
            // 标准 BLEU：任何一阶零匹配就是 0（不靠平滑救回来）
            return 0.0;
        }
        let total: usize = hyp_counts.values().sum();
        log_sum += (matched as f64 / total as f64).ln();
        effective += 1;
    }
    if effective == 0 {
        return 0.0;
    }
    let geo_mean = (log_sum / effective as f64).exp();

    let hyp_len = hyp.len() as f64;
    let ref_len = refr.len() as f64;
    let brevity = if hyp_len >= ref_len {
        1.0
    } else if hyp_len == 0.0 {
        0.0
    } else {
        (1.0 - ref_len / hyp_len).exp()
    };
    100.0 * brevity * geo_mean
}

/// 语料级平均（逐句计算再平均，与 sacrebleu 的 corpus 级不同，文档已注明）。
pub fn corpus_average(pairs: &[(String, String)]) -> (f64, f64) {
    if pairs.is_empty() {
        return (0.0, 0.0);
    }
    let mut chrf = 0.0;
    let mut bleu = 0.0;
    for (hyp, refr) in pairs {
        chrf += chrf_pp(hyp, refr);
        bleu += bleu4(hyp, refr);
    }
    let n = pairs.len() as f64;
    (chrf / n, bleu / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 完全相同的译文得满分() {
        assert!((chrf_pp("你好，世界。", "你好，世界。") - 100.0).abs() < 1e-6);
        assert!((bleu4("hello world today", "hello world today") - 100.0).abs() < 1e-6);
    }

    #[test]
    fn 完全不同的译文得零分() {
        assert!(chrf_pp("abc", "xyz") < 1.0, "{}", chrf_pp("abc", "xyz"));
        assert!(bleu4("abc", "xyz") < 1.0);
    }

    #[test]
    fn 部分匹配的字符指标介于两者之间() {
        let score = chrf_pp("ab", "abc");
        // 手工验算：n=1 F2=0.714、n=2 F2=0.556、n=3 与词 1-gram 为 0 → 均值 31.7
        assert!((score - 31.75).abs() < 0.5, "{score}");
        assert!(score < chrf_pp("abc", "abc"));
    }

    #[test]
    fn 词序不同会降低_bleu_但_chrf_更宽容() {
        let a = "the cat sat on the mat";
        let b = "the mat sat on the cat";
        let bleu = bleu4(a, b);
        let chrf = chrf_pp(a, b);
        assert!(bleu < 100.0);
        assert!(chrf < 100.0);
        assert!(
            chrf > bleu,
            "chrF 对词序变化应比 BLEU 宽容：chrf={chrf} bleu={bleu}"
        );
    }

    #[test]
    fn 短译文受简洁惩罚() {
        let full = bleu4("hello world today", "hello world today");
        let short = bleu4("hello world", "hello world today");
        assert!(short < full, "{short} vs {full}");
    }

    #[test]
    fn 短句子用有效阶而不是直接归零() {
        // 2 个词没有 3/4-gram：有效阶下仍应给出 100
        assert!((bleu4("hello world", "hello world") - 100.0).abs() < 1e-6);
    }

    #[test]
    fn 语料平均与逐句一致() {
        let pairs = vec![
            ("你好".to_string(), "你好".to_string()),
            ("再见".to_string(), "再见".to_string()),
        ];
        let (chrf, bleu) = corpus_average(&pairs);
        assert!((chrf - 100.0).abs() < 1e-6);
        assert!((bleu - 100.0).abs() < 1e-6);
    }

    #[test]
    fn 中文_bleu_不再恒为零() {
        let hyp = "博物馆工作日下午六点闭馆。";
        let refr = "博物馆在工作日下午六点闭馆。";
        let score = bleu4(hyp, refr);
        assert!(score > 0.0, "中文混排分词后 BLEU 应有非零值：{score}");
        assert!(score < 100.0);
    }

    #[test]
    fn 空输入不崩溃() {
        assert_eq!(chrf_pp("", "你好"), 0.0);
        assert_eq!(bleu4("", ""), 0.0);
        assert_eq!(corpus_average(&[]), (0.0, 0.0));
    }
}
