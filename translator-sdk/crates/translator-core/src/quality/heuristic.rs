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

    if target.as_str().starts_with("zh") {
        if let Some(fragment) = residual_english_fragment(output) {
            report.issues.push(QualityIssue::major(
                QualityIssueKind::Untranslated,
                format!("译文中残留未翻译的英文片段：{fragment}"),
            ));
        }
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
fn residual_english_fragment(output: &str) -> Option<String> {
    const FUNCTION_WORDS: [&str; 34] = [
        "the", "a", "an", "of", "to", "and", "or", "is", "are", "was", "were", "be", "being",
        "been", "in", "on", "at", "for", "with", "that", "this", "it", "its", "as", "by", "from",
        "not", "you", "we", "they", "he", "she", "but", "if",
    ];

    // 把文本切成 (单词, 与前一个单词之间是否只隔空格)。
    // 中文与标点都会打断"连续英文段"——但被打断前的单词要保留，
    // 否则 `geared to being a weapon，在…` 里的 weapon 会把整段吞掉。
    let mut tokens: Vec<(String, bool)> = Vec::new();
    let mut current = String::new();
    let mut spacer_only = true;
    for ch in output.chars() {
        let is_word_char = ch.is_ascii_alphanumeric() || ch == '\'' || ch == '-';
        if is_word_char {
            current.push(ch);
            continue;
        }
        if !current.is_empty() {
            tokens.push((std::mem::take(&mut current), spacer_only));
        }
        spacer_only = ch.is_whitespace();
    }
    if !current.is_empty() {
        tokens.push((current, spacer_only));
    }

    let mut run: Vec<String> = Vec::new();
    let mut candidate: Option<String> = None;
    let evaluate = |run: &mut Vec<String>, candidate: &mut Option<String>| {
        if candidate.is_none() && run.len() >= 5 {
            let funcs = run
                .iter()
                .filter(|w| FUNCTION_WORDS.contains(&w.to_lowercase().as_str()))
                .count();
            let lower_content = run.iter().any(|w| {
                w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                    && w.len() >= 4
                    && !FUNCTION_WORDS.contains(&w.to_lowercase().as_str())
            });
            if funcs >= 2 && lower_content {
                *candidate = Some(run.join(" "));
            }
        }
        run.clear();
    };

    for (word, spaced) in tokens {
        if word
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '\'' || c == '-')
        {
            if !spaced {
                evaluate(&mut run, &mut candidate);
            }
            run.push(word);
        } else {
            // 含数字的 token（iPhone 15、PA-28）打断连续段但不参与判断
            evaluate(&mut run, &mut candidate);
        }
    }
    evaluate(&mut run, &mut candidate);
    candidate
}

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
    fn 未翻译的英文片段会被报告() {
        let zh = LanguageTag::new("zh-Hans").unwrap();

        // 真实案例（wmt24pp-440）：模型留下了整段英文
        let mut report = QualityReport::default();
        check(
            "Everything is geared to being a weapon, displayed in accompanying media with gratuitous effect.",
            "一切都 geared to being a weapon，在 accompanying media 中以 gratuitous effect 展示。",
            &zh,
            &mut report,
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.kind == QualityIssueKind::Untranslated),
            "{:?}",
            report.issues
        );

        // 品牌/人名不算残留
        let mut ok = QualityReport::default();
        check(
            "Buy the iPhone 15 Pro Max now.",
            "现在就购买 iPhone 15 Pro Max。",
            &zh,
            &mut ok,
        );
        assert!(
            !ok.issues
                .iter()
                .any(|i| i.kind == QualityIssueKind::Untranslated),
            "{:?}",
            ok.issues
        );

        // 专名标题不算残留（全是功能词或大写开头）
        let mut title = QualityReport::default();
        check(
            "The film is The Lord of the Rings.",
            "这部电影是 The Lord of the Rings。",
            &zh,
            &mut title,
        );
        assert!(
            !title
                .issues
                .iter()
                .any(|i| i.kind == QualityIssueKind::Untranslated),
            "{:?}",
            title.issues
        );
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
