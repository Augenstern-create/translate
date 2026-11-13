//! 启发式检查：**只报告，不修改译文**。
//!
//! 这些判断都不是确定性的：否定可能用反问表达，长句可能精简得更短，
//! 目标语言判断也可能被代码片段误导。因此它们只产生 `Major`/`Minor` 诊断，
//! 永远不单独拒绝一份译文（§5.4 最后一条）。

use translator_types::LanguageTag;

use super::report::{QualityIssue, QualityIssueKind, QualityReport};

/// 跑一遍启发式检查并并入 `report`。
pub fn check(source: &str, output: &str, target: &LanguageTag, report: &mut QualityReport) {
    let lower_source = source.to_lowercase();

    let negation_cues = count_negation_cues(&lower_source);
    if negation_cues > 0 && !has_target_negation(output) {
        report.issues.push(QualityIssue::major(
            QualityIssueKind::NegationDropped,
            format!("源文有 {negation_cues} 个否定线索，译文没有否定表达"),
        ));
    }

    let source_words = source.split_whitespace().count();
    let ratio = output.chars().count() as f32 / source.chars().count().max(1) as f32;
    if source_words >= 10 && ratio < 0.18 {
        report.issues.push(QualityIssue::major(
            QualityIssueKind::PossibleOmission,
            format!("译文/源文字符比仅 {ratio:.2}，可能遗漏内容"),
        ));
    }
    if source_words >= 5 && ratio > 2.5 {
        report.issues.push(QualityIssue::major(
            QualityIssueKind::PossibleHallucination,
            format!("译文/源文字符比 {ratio:.2}，可能追加了内容"),
        ));
    }

    if has_conditional_cue(&lower_source) && !has_target_conditional(output) {
        report.issues.push(QualityIssue::major(
            QualityIssueKind::ConditionalDropped,
            "源文有条件关系，译文没有条件标记",
        ));
    }

    if let Some(segment) = duplicated_segment(output) {
        report.issues.push(QualityIssue::major(
            QualityIssueKind::DuplicateSegment,
            format!("连续重复句段：{segment}"),
        ));
    }

    if target.as_str().starts_with("zh") && !target_looks_chinese(output) {
        let severity = if output.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 8 {
            super::report::QualitySeverity::Critical
        } else {
            super::report::QualitySeverity::Major
        };
        report.issues.push(QualityIssue::new(
            QualityIssueKind::TargetLanguageMismatch,
            severity,
            "目标语言为中文，但译文没有足够的汉字",
        ));
    } else if target.as_str().starts_with("en") && !output.chars().any(|c| c.is_ascii_alphabetic())
    {
        report.issues.push(QualityIssue::major(
            QualityIssueKind::TargetLanguageMismatch,
            "目标语言为英文，但译文没有拉丁字母",
        ));
    }

    if source.trim().eq_ignore_ascii_case(output.trim()) && source.split_whitespace().count() >= 2 {
        report.issues.push(QualityIssue::major(
            QualityIssueKind::Untranslated,
            "译文与原文完全相同",
        ));
    }
}

/// 找出"整段没翻译"的英文残留（如 `geared to being a weapon`）。
///
/// 判定条件（刻意保守，宁可漏报也不误报）：
///
/// - 连续 ≥5 个纯英文单词（不含 URL/代码，含标点的 token 会打断连续段）；
/// - 其中至少 2 个是功能词（the/of/to/and/being…）——排除 `iPhone 15 Pro` 这类品牌；
/// - 至少 1 个**小写**内容词（长度 ≥4）——排除 `The Lord of the Rings` 这类专名标题。
///
/// 命中时只报 Major（不判致命）：这是启发式，任务是"让人看见"，不是自动拒稿。
/// 分段策略下对**单个片段**做保守的合理性检查。
///
/// 分段回退的结构完整性由保护模块保证，但"模型对这一段答非所问"没有任何
/// 结构信号（返回的是非空、合法语言的内容）。这里只拦截**明显不可能**的情况：
///
/// - 目标语言是中文，片段有实义英文，却一个汉字都没有；
/// - 片段译文相对源片段过短（<15%）或过长（>4×）。
///
/// 宁可漏判（交给人看），也不要把正常译文误判成错误。
pub fn implausible_segment(source: &str, output: &str, target: &LanguageTag) -> Option<String> {
    let source_letters = source.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let source_chars = source.chars().count();
    let output_chars = output.chars().count();
    let has_han = output
        .chars()
        .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));

    if target.as_str().starts_with("zh") && source_letters >= 4 && !has_han {
        return Some("片段译文里没有中文（模型可能答非所问）".to_string());
    }
    if source_letters >= 8 {
        let ratio = output_chars as f32 / source_chars.max(1) as f32;
        if ratio < 0.15 {
            return Some(format!("片段译文过短（比例 {ratio:.2}）"));
        }
        if ratio > 4.0 {
            return Some(format!("片段译文过长（比例 {ratio:.2}）"));
        }
    }
    None
}

fn has_conditional_cue(lower_source: &str) -> bool {
    let padded = format!(" {lower_source} ");
    [
        " if ",
        " unless ",
        " when ",
        " once ",
        " provided ",
        " as long as ",
    ]
    .iter()
    .any(|cue| padded.contains(cue))
}

fn has_target_conditional(output: &str) -> bool {
    [
        "如果", "若", "假如", "要是", "除非", "当", "一旦", "只要", "只有", "否则", "即使", "时",
        "后", "一经", "完成", "的话", "则", "便", "就",
    ]
    .iter()
    .any(|cue| output.contains(cue))
}

fn count_negation_cues(lower_source: &str) -> usize {
    const CUES: &[&str] = &[
        "not",
        "n't",
        "never",
        "none",
        "nobody",
        "nothing",
        "neither",
        "nor",
        "without",
        "cannot",
        "can't",
        "don't",
        "doesn't",
        "didn't",
        "isn't",
        "aren't",
        "wasn't",
        "weren't",
        "won't",
        "wouldn't",
        "shouldn't",
        "couldn't",
        "haven't",
        "hasn't",
        "hadn't",
        "no",
        "hardly",
        "barely",
        "rarely",
        "seldom",
    ];
    let mut count = 0usize;
    let bytes = lower_source.as_bytes();
    for cue in CUES {
        let mut from = 0usize;
        while let Some(pos) = lower_source[from..].find(cue) {
            let at = from + pos;
            let end = at + cue.len();
            let left_ok =
                at == 0 || !bytes[at - 1].is_ascii_alphanumeric() || cue.starts_with("n't");
            let right_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if left_ok && right_ok {
                count += 1;
                // "not only" 不算否定线索
                if *cue == "not" && lower_source[end..].trim_start().starts_with("only") {
                    count = count.saturating_sub(1);
                }
            }
            from = end;
        }
    }
    count
}

fn has_target_negation(output: &str) -> bool {
    const NEGATORS: [char; 14] = [
        '不', '没', '無', '无', '未', '别', '別', '非', '否', '勿', '莫', '甭', '休', '毋',
    ];
    output.chars().any(|c| NEGATORS.contains(&c))
}

fn target_looks_chinese(output: &str) -> bool {
    let total = output.chars().filter(|c| !c.is_whitespace()).count();
    let han = output
        .chars()
        .filter(|c| ('\u{4E00}'..='\u{9FFF}').contains(c))
        .count();
    // 很短的输出（例如 "OK"）允许没有汉字
    han > 0 || total < 8
}

fn duplicated_segment(output: &str) -> Option<String> {
    let segments: Vec<&str> = output
        .split(['。', '！', '？', '!', '?', ';', '；', ',', '，', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 2 {
        let chars: Vec<char> = output.chars().filter(|c| !c.is_whitespace()).collect();
        for period in 2..=chars.len() / 3 {
            let block: String = chars[..period].iter().collect();
            let repeats = chars
                .chunks(period)
                .filter(|c| c.iter().collect::<String>() == block)
                .count();
            if repeats >= 3 {
                return Some(block);
            }
        }
        return None;
    }
    for idx in 1..segments.len() {
        if segments[idx] == segments[idx - 1] && segments[idx].chars().count() >= 4 {
            return Some(segments[idx].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_types::TranslationMode;

    fn report(source: &str, output: &str) -> QualityReport {
        let mut r = QualityReport::default();
        check(
            source,
            output,
            &LanguageTag::new("zh-Hans").unwrap(),
            &mut r,
        );
        r
    }

    #[test]
    fn 否定丢失被报告但不致命() {
        let r = report("The service is not available.", "服务可用。");
        assert!(r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NegationDropped));
        assert!(!r.has_fatal());
        assert_eq!(r.mode, TranslationMode::Precise);
    }

    #[test]
    fn 正常的否定译文不误报() {
        let r = report("The service is not available.", "服务不可用。");
        assert!(r.issues.is_empty(), "{:?}", r.issues);
    }

    #[test]
    fn 重复句段被报告() {
        let r = report(
            "This is a long sentence about the system.",
            "这是系统的一句话。这是系统的一句话。",
        );
        assert!(r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::DuplicateSegment));
    }

    #[test]
    fn 目标语言明显不对时报告() {
        let r = report("Hello world.", "Hello world this is still English.");
        assert!(r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::TargetLanguageMismatch));
    }

    #[test]
    fn 原文原样返回被报告() {
        let r = report(
            "Please restart the server now.",
            "Please restart the server now.",
        );
        assert!(r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::Untranslated));
    }

    #[test]
    fn 条件句丢失被报告() {
        let r = report("If the file exists, delete it.", "删除该文件。");
        assert!(r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::ConditionalDropped));
    }

    #[test]
    fn 片段合理性检查只拦明显异常() {
        let zh = LanguageTag::new("zh-Hans").unwrap();
        // 正常译文不拦
        assert!(implausible_segment("Visit example.com", "访问 example.com", &zh).is_none());
        // 完全没有中文 → 拦
        assert!(implausible_segment("Visit example.com now", "nonsense", &zh).is_some());
        // 明显过短 → 拦
        assert!(implausible_segment("and enter the secret code", "好", &zh).is_some());
        // 纯符号片段不判（没有实义字母）
        assert!(implausible_segment(" - ", " - ", &zh).is_none());
    }

    #[test]
    fn 译文过短被报告为可能遗漏() {
        let r = report(
            "The quick brown fox jumps over the lazy dog every single morning.",
            "狐狸跳。",
        );
        assert!(r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::PossibleOmission));
    }
}

// ============================================================
// 未翻译片段：基于"保护计划 + 原文对应"的判断（不使用英文词表）
// ============================================================

/// 译文中"整段英文没有翻译"的可疑片段。
///
/// # 判定依据（全部来自前处理与原文，不用功能词表、不设词数阈值）
///
/// 1. 先把**受保护内容**（URL/代码/模板变量/路径/编号……）从译文里遮掉：
///    这些内容本来就该原样出现，不能算"未翻译"；
/// 2. 按形态识别**标识符**：全大写缩写、含数字、CamelCase、含 `_`/`-` → 合理保留；
/// 3. 剩下的连续普通英文词（≥2 个）如果**在原文里以同样的顺序连续出现**，
///    说明模型把这一段原文照搬了 → 报可疑；
/// 4. 但若该短语在原文里含大写开头的实词（`The Lord of the Rings`）
///    或全大写（`Machine Learning`），按专名/术语处理，不报。
///
/// 命中只报 Major（疑似问题）：无法确定语义时不得修改译文，交给人或重试流程。
pub fn untranslated_fragments(source: &str, protected: &[String], restored: &str) -> Vec<String> {
    let scrubbed = mask_protected(restored, protected);
    suspicious_phrases(&scrubbed, source)
}

/// 把受保护内容的**每一次出现**替换成空格，保留字节长度以便后续定位。
fn mask_protected(text: &str, protected: &[String]) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for original in protected {
        if original.trim().is_empty() {
            continue;
        }
        let needle: Vec<char> = original.chars().collect();
        if needle.len() > chars.len() {
            continue;
        }
        let mut start = 0usize;
        while start + needle.len() <= chars.len() {
            if chars[start..start + needle.len()] == needle[..] {
                for slot in &mut chars[start..start + needle.len()] {
                    *slot = ' ';
                }
                start += needle.len();
            } else {
                start += 1;
            }
        }
    }
    chars.into_iter().collect()
}

/// 找出"连续 ≥2 个普通英文词，且在原文里按同样顺序出现"的短语。
fn suspicious_phrases(scrubbed: &str, source: &str) -> Vec<String> {
    // 先把文本切成"词"，再按**仅由空白分隔**聚成连续短语。
    let mut runs: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut word = String::new();

    for ch in scrubbed.chars() {
        let is_word_char = ch.is_ascii_alphanumeric() || ch == '\'' || ch == '-' || ch == '_';
        if is_word_char {
            word.push(ch);
            continue;
        }
        if !word.is_empty() {
            current.push(std::mem::take(&mut word));
        }
        if !ch.is_whitespace() && !current.is_empty() {
            // 标点/中文打断连续短语
            runs.push(std::mem::take(&mut current));
        }
    }
    if !word.is_empty() {
        current.push(word);
    }
    if !current.is_empty() {
        runs.push(current);
    }

    let lower_source = source.to_lowercase();
    let mut found: Vec<String> = Vec::new();
    for words in runs {
        if words.len() < 2 {
            continue;
        }
        if words.iter().any(|w| is_identifier(w)) {
            continue;
        }
        let phrase = words.join(" ");
        if !lower_source.contains(&phrase.to_lowercase()) {
            continue;
        }
        if phrase_is_proper_noun(source, &words) {
            continue;
        }
        if !found.contains(&phrase) {
            found.push(phrase);
        }
        if found.len() >= 3 {
            break;
        }
    }
    found
}

/// 形态上像标识符的英文 token（缩写/带数字/CamelCase/含下划线或连字符）。
fn is_identifier(word: &str) -> bool {
    if word.chars().any(|c| c.is_ascii_digit()) || word.contains('_') || word.contains('-') {
        return true;
    }
    let letters: Vec<char> = word.chars().collect();
    if letters.len() >= 2 && letters.iter().all(|c| c.is_ascii_uppercase()) {
        return true;
    }
    // CamelCase：小写后跟大写
    letters
        .windows(2)
        .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
}

/// 原文里这个词组是否带专名/标题特征（实词以大写开头，或整词全大写）。
fn phrase_is_proper_noun(source: &str, words: &[String]) -> bool {
    let lowered = source.to_lowercase();
    let phrase = words.join(" ").to_lowercase();
    let Some(pos) = lowered.find(&phrase) else {
        return false;
    };
    let original: Vec<char> = source.chars().collect();
    // 用字符偏移换算（原文可能含多字节字符）
    let byte_to_char = |byte: usize| source[..byte].chars().count();
    let start = byte_to_char(pos);
    let mut cursor = start;
    for word in words {
        let len = word.chars().count();
        let slice: String = original[cursor..(cursor + len).min(original.len())]
            .iter()
            .collect();
        let first = slice.chars().next().unwrap_or(' ');
        let all_upper = slice
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .all(|c| c.is_ascii_uppercase());
        let is_content = len >= 4;
        if (is_content && first.is_ascii_uppercase()) || all_upper {
            return true;
        }
        cursor += len + 1; // 空格
    }
    false
}

#[cfg(test)]
mod untranslated_tests {
    use super::*;
    use crate::protection::{ProtectionPlan, ProtectionPolicy};

    fn plan(source: &str) -> ProtectionPlan {
        ProtectionPlan::prepare(source, &ProtectionPolicy::default()).expect("保护计划")
    }

    fn fragments_with(source: &str, output: &str) -> Vec<String> {
        let plan = plan(source);
        let protected: Vec<String> = plan.spans().iter().map(|s| s.original.clone()).collect();
        untranslated_fragments(plan.source(), &protected, output)
    }

    #[test]
    fn 两词英文残留会被发现() {
        // 真实缺陷：2 个词的短语没翻译（旧的"≥5 词 + 功能词表"实现抓不到）
        let source = "Everything is geared to being a weapon, displayed in accompanying media with gratuitous effect.";
        let output = "一切都以成为武器为导向，并在 accompanying media 中以无谓的效果加以展示。";
        let found = fragments_with(source, output);
        assert!(
            found.iter().any(|f| f.contains("accompanying media")),
            "应发现 2 词残留：{found:?}"
        );
    }

    #[test]
    fn 受保护内容不算未翻译() {
        let source = "Open https://example.com/docs and run `cargo build` now.";
        let plan = plan(source);
        let protected: Vec<String> = plan.spans().iter().map(|s| s.original.clone()).collect();
        // 正常译文：URL/代码被贴回
        let output = "现在打开 https://example.com/docs 并运行 `cargo build`。";
        assert!(untranslated_fragments(plan.source(), &protected, output).is_empty());
    }

    #[test]
    fn 品牌与技术标识符不算未翻译() {
        let source = "Buy the iPhone 15 Pro Max and install Python via the API.";
        let plan = plan(source);
        let protected: Vec<String> = plan.spans().iter().map(|s| s.original.clone()).collect();
        let output = "购买 iPhone 15 Pro Max，并通过 API 安装 Python。";
        assert!(untranslated_fragments(plan.source(), &protected, output).is_empty());
    }

    #[test]
    fn 专名标题不算未翻译() {
        let source = "The film is The Lord of the Rings and the budget is large.";
        let plan = plan(source);
        let protected: Vec<String> = plan.spans().iter().map(|s| s.original.clone()).collect();
        let output = "这部电影是 The Lord of the Rings，预算很大。";
        assert!(untranslated_fragments(plan.source(), &protected, output).is_empty());
    }

    #[test]
    fn 普通短语在原文里没有时不报() {
        // 译文里出现了原文没有的英文（比如模型自己加的）→ 不属于"未翻译"
        let source = "这个系统运行稳定。";
        let plan = plan(source);
        let protected: Vec<String> = plan.spans().iter().map(|s| s.original.clone()).collect();
        let output = "这个系统运行稳定，performance is good。";
        assert!(untranslated_fragments(plan.source(), &protected, output).is_empty());
    }
}
