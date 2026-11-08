//! 文本结构分析与翻译规划（docs/62 §3）。
//!
//! 这一层**不执行翻译**，只回答两个问题：
//!
//! 1. 这段输入是 Word / Sentence / Paragraph / Document 中的哪一类？
//! 2. 应该切成哪些翻译单元、按什么策略组织、如何按原顺序还原？
//!
//! 实际翻译仍然交给现有 `TranslationEngine`（词典 / NMT / Remote）。四级策略不是
//! 四套引擎，而是四种**组织方式**。

use translator_types::{ParagraphStrategy, TextKind, TranslateOptions};

use crate::text::{protected_spans, ProtectedKind};

/// 结构分析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextAnalysis {
    pub kind: TextKind,
    pub sentences: usize,
    pub paragraphs: usize,
    pub chars: usize,
    pub words: usize,
    pub has_markdown: bool,
    pub has_code_block: bool,
}

impl TextAnalysis {
    pub fn summary(&self) -> String {
        format!(
            "kind={} sentences={} paragraphs={} chars={} words={} markdown={} code={}",
            self.kind,
            self.sentences,
            self.paragraphs,
            self.chars,
            self.words,
            self.has_markdown,
            self.has_code_block
        )
    }
}

/// 一个翻译单元。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationUnit {
    /// 单元前的原样分隔符（空白/换行/Markdown 前缀）。
    pub separator: String,
    /// 需要翻译的文本。`None` 表示原样保留（代码块、表格等）。
    pub text: Option<String>,
    /// 原样保留的内容（`text=None` 时使用）。
    pub literal: Option<String>,
    /// 单元后的原样后缀（换行等）。
    pub suffix: String,
    /// 该单元应按什么类型翻译。
    pub kind: TextKind,
}

impl TranslationUnit {
    pub fn literal(separator: String, text: String, suffix: String) -> Self {
        Self {
            separator,
            text: None,
            literal: Some(text),
            suffix,
            kind: TextKind::Document,
        }
    }

    pub fn translatable(separator: String, text: String, suffix: String, kind: TextKind) -> Self {
        Self {
            separator,
            text: Some(text),
            literal: None,
            suffix,
            kind,
        }
    }
}

/// 翻译计划。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationPlan {
    pub kind: TextKind,
    /// 稳定策略名，进入缓存身份与诊断：`word` / `sentence` / `paragraph-whole`
    /// / `paragraph-sentence` / `paragraph-group` / `document`。
    pub strategy: &'static str,
    pub units: Vec<TranslationUnit>,
}

impl TranslationPlan {
    pub fn unit_count(&self) -> usize {
        self.units.len()
    }
}

/// 一个句子片段：`separator` 是它前面的空白，`text` 不含首尾空白。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentenceUnit {
    pub separator: String,
    pub text: String,
}

/// 结构分析 + 规划。
pub fn plan(text: &str, options: &TranslateOptions) -> TranslationPlan {
    let analysis = analyze(text, options.text_kind);
    match analysis.kind {
        TextKind::Word => TranslationPlan {
            kind: TextKind::Word,
            strategy: "word",
            units: vec![TranslationUnit::translatable(
                String::new(),
                text.to_string(),
                String::new(),
                TextKind::Word,
            )],
        },
        TextKind::Sentence => plan_sentence(text),
        TextKind::Paragraph => plan_paragraph(text, options.paragraph_strategy),
        TextKind::Document => plan_document(text),
        TextKind::Auto => unreachable!("analyze 必须解析 Auto"),
    }
}

fn plan_sentence(text: &str) -> TranslationPlan {
    let clauses = split_sentence_clauses(text);
    if clauses.len() <= 1 {
        return TranslationPlan {
            kind: TextKind::Sentence,
            strategy: "sentence",
            units: vec![TranslationUnit::translatable(
                String::new(),
                text.to_string(),
                String::new(),
                TextKind::Sentence,
            )],
        };
    }
    let final_punct = text
        .trim_end()
        .chars()
        .last()
        .filter(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？'))
        .map(|c| c.to_string())
        .unwrap_or_default();
    let mut units = Vec::new();
    for (idx, clause) in clauses.iter().enumerate() {
        units.push(TranslationUnit::translatable(
            if idx == 0 {
                String::new()
            } else {
                "，".to_string()
            },
            clause.clone(),
            if idx + 1 == clauses.len() {
                final_punct.clone()
            } else {
                String::new()
            },
            TextKind::Sentence,
        ));
    }
    TranslationPlan {
        kind: TextKind::Sentence,
        strategy: "sentence-clauses",
        units,
    }
}

/// 只对**较长且有两个并列/转折分句**的句子做切分。
///
/// 这是 W2 的句级切分：不是按 `.` 硬切，而是只在明确的连接词处切，
/// 并保留转折词；`and` 后的无主语动词补 `It`，避免“ends at 11 PM”被译成
/// 没有主语的片段。短句不切，避免把原本流畅的译文切碎。
fn split_sentence_clauses(text: &str) -> Vec<String> {
    if text.split_whitespace().count() < 10 {
        return vec![text.to_string()];
    }
    let protected = protected_spans(text);
    // 只处理单一连接词：多连接词（and + but / because + so / although + because）
    // 的句子结构复杂，切开后容易丢主语或逻辑词，保留整句更好。
    // because/while/although 引导的从句尤其容易在中文里丢主语，先不切。
    let conjunctions = [" and ", " but "];
    let occurrences: usize = conjunctions.iter().map(|c| text.matches(c).count()).sum();
    if occurrences != 1 {
        return vec![text.to_string()];
    }
    for conjunction in conjunctions {
        let Some(pos) = text.find(conjunction) else {
            continue;
        };
        if protected
            .iter()
            .any(|span| pos < span.end && pos + conjunction.len() > span.start)
        {
            continue;
        }
        let first = text[..pos]
            .trim()
            .trim_end_matches([',', ';', '，', '；'])
            .trim();
        let mut second = text[pos + conjunction.len()..].trim();
        if second.is_empty() {
            continue;
        }
        second = second
            .trim_end_matches(['.', '!', '?', '。', '！', '？'])
            .trim();
        if second.is_empty() {
            continue;
        }
        let mut second = second.to_string();
        let bare_verb = starts_with_bare_verb(&second);
        let subject_clause = starts_with_subject_clause(&second);
        // `and` 可能连接两个名词短语（"64 cores and 512 GB"）；`while making`
        // 是分词短语，也不是分句。只有第二段像“谓语/主语+谓语”时才切。
        if !subject_clause && !bare_verb {
            continue;
        }
        if conjunction == " and " {
            if bare_verb {
                second = format!("It {second}");
            }
        } else {
            let prefix = match conjunction.trim() {
                "but" => "但",
                "so" => "所以",
                "because" => "因为",
                "while" => "而",
                "although" => "虽然",
                "or" => "或者",
                other => other,
            };
            second = format!("{prefix}{second}");
        }
        return vec![first.to_string(), second];
    }
    vec![text.to_string()]
}

fn starts_with_subject_clause(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    if matches!(first, "he" | "she" | "it" | "they" | "we" | "you" | "i") {
        return true;
    }
    if matches!(first, "the" | "a" | "an") {
        let second = lower.split_whitespace().nth(1).unwrap_or("");
        return matches!(
            second,
            "is" | "are" | "was" | "were" | "has" | "have" | "will" | "can" | "should"
        );
    }
    false
}

fn starts_with_bare_verb(text: &str) -> bool {
    let first = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "ends"
            | "starts"
            | "begins"
            | "closes"
            | "opens"
            | "runs"
            | "fails"
            | "stops"
            | "continues"
            | "returns"
            | "contains"
            | "works"
            | "changes"
    )
}

/// 只做结构分析，不切分。
pub fn analyze(text: &str, requested: TextKind) -> TextAnalysis {
    let sentences = split_sentences(text);
    let paragraphs = count_paragraphs(text);
    let chars = text.chars().count();
    let words = text.split_whitespace().count();
    let has_code_block = text
        .lines()
        .any(|line| line.trim_start().starts_with("```"));
    let has_markdown = has_code_block
        || text.lines().any(|line| {
            let t = line.trim_start();
            t.starts_with('#')
                || t.starts_with("- ")
                || t.starts_with("* ")
                || t.starts_with("> ")
                || t.starts_with("|")
                || is_ordered_list_item(t)
        });

    let kind = match requested {
        TextKind::Auto => {
            if has_markdown || paragraphs >= 3 || sentences.len() >= 6 {
                TextKind::Document
            } else if paragraphs >= 2 || sentences.len() >= 2 {
                TextKind::Paragraph
            } else if sentences.len() == 1
                && (has_terminal_punctuation(text) || words >= 3 || chars > 32)
            {
                TextKind::Sentence
            } else {
                TextKind::Word
            }
        }
        other => other,
    };

    TextAnalysis {
        kind,
        sentences: sentences.len(),
        paragraphs,
        chars,
        words,
        has_markdown,
        has_code_block,
    }
}

fn plan_paragraph(text: &str, requested: ParagraphStrategy) -> TranslationPlan {
    let sentences = split_sentences(text);
    let strategy = match requested {
        ParagraphStrategy::Whole => "paragraph-whole",
        ParagraphStrategy::Sentence => "paragraph-sentence",
        ParagraphStrategy::Group => "paragraph-group",
        ParagraphStrategy::Best => "paragraph-sentence",
        ParagraphStrategy::Context => "paragraph-context",
        ParagraphStrategy::Auto => {
            // 50 段（3 句/段）对照：整段 0.3917 / 逐句 0.3987 / 句群 0.3909；
            // 逐句与整段候选再比较（Best）0.4205、覆盖 0.3848→0.4111。
            // 因此多句段落在 API 层走 Best（整段+逐句候选）；这里保留逐句作为
            // 规划标签，单句段落仍整段。
            if sentences.len() <= 1 {
                "paragraph-whole"
            } else {
                "paragraph-sentence"
            }
        }
    };

    let mut units = Vec::new();
    match strategy {
        "paragraph-whole" => units.push(TranslationUnit::translatable(
            String::new(),
            text.to_string(),
            String::new(),
            TextKind::Paragraph,
        )),
        "paragraph-sentence" => {
            for sentence in sentences {
                units.push(TranslationUnit::translatable(
                    sentence.separator,
                    sentence.text,
                    String::new(),
                    TextKind::Sentence,
                ));
            }
        }
        _ => {
            // 句群：每 2 句一组，组内保留原文空白；末组可为 1 句。
            let mut idx = 0usize;
            while idx < sentences.len() {
                let end = (idx + 2).min(sentences.len());
                let separator = sentences[idx].separator.clone();
                let mut group = String::new();
                for (offset, sentence) in sentences[idx..end].iter().enumerate() {
                    if offset > 0 {
                        group.push_str(&sentence.separator);
                    }
                    group.push_str(&sentence.text);
                }
                units.push(TranslationUnit::translatable(
                    separator,
                    group,
                    String::new(),
                    TextKind::Sentence,
                ));
                idx = end;
            }
        }
    }

    TranslationPlan {
        kind: TextKind::Paragraph,
        strategy,
        units,
    }
}

fn plan_document(text: &str) -> TranslationPlan {
    let blocks = parse_markdown_blocks(text);
    TranslationPlan {
        kind: TextKind::Document,
        strategy: "document",
        units: blocks,
    }
}

/// 把 Markdown/纯文本切成结构块。
///
/// 代码块、表格行原样保留；标题/列表/引用只翻译内容，前缀与换行原样恢复。
fn parse_markdown_blocks(text: &str) -> Vec<TranslationUnit> {
    let mut units = Vec::new();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // fenced code block：整块原样保留
        if trimmed.starts_with("```") {
            let mut block = String::new();
            let mut j = i;
            while j < lines.len() {
                block.push_str(lines[j]);
                let is_close = j > i && lines[j].trim_start().starts_with("```");
                j += 1;
                if is_close {
                    break;
                }
            }
            units.push(TranslationUnit::literal(
                String::new(),
                block,
                String::new(),
            ));
            i = j;
            continue;
        }

        if trimmed.trim().is_empty() {
            units.push(TranslationUnit::literal(
                String::new(),
                line.to_string(),
                String::new(),
            ));
            i += 1;
            continue;
        }

        // 标题
        if let Some((prefix, body, suffix)) = strip_heading(line) {
            units.push(TranslationUnit::translatable(
                String::new(),
                body,
                suffix,
                TextKind::Sentence,
            ));
            // prefix 不能丢：塞进 separator（在译文前原样输出）
            if let Some(last) = units.last_mut() {
                last.separator = prefix;
            }
            i += 1;
            continue;
        }

        // 列表项 / 引用
        if let Some((prefix, body, suffix)) = strip_markup_prefix(line) {
            let kind = if body.chars().count() > 400 {
                TextKind::Paragraph
            } else {
                TextKind::Sentence
            };
            units.push(TranslationUnit::translatable(
                String::new(),
                body,
                suffix,
                kind,
            ));
            if let Some(last) = units.last_mut() {
                last.separator = prefix;
            }
            i += 1;
            continue;
        }

        // 表格行：原样保留（简单表格翻译留待后续）
        if trimmed.starts_with('|') {
            units.push(TranslationUnit::literal(
                String::new(),
                line.to_string(),
                String::new(),
            ));
            i += 1;
            continue;
        }

        // 普通段落：连续非空、非特殊行
        let mut paragraph = String::new();
        let mut j = i;
        while j < lines.len() {
            let candidate = lines[j];
            let t = candidate.trim_start();
            if t.trim().is_empty()
                || t.starts_with("```")
                || t.starts_with('#')
                || t.starts_with('>')
                || t.starts_with('|')
                || strip_markup_prefix(candidate).is_some()
            {
                break;
            }
            paragraph.push_str(candidate);
            j += 1;
        }
        if paragraph.is_empty() {
            // 兜底：未知结构原样保留
            units.push(TranslationUnit::literal(
                String::new(),
                line.to_string(),
                String::new(),
            ));
            i += 1;
        } else {
            let has_newline = paragraph.ends_with('\n');
            let body = paragraph.trim_end_matches('\n').to_string();
            units.push(TranslationUnit::translatable(
                String::new(),
                body,
                if has_newline {
                    "\n".to_string()
                } else {
                    String::new()
                },
                TextKind::Paragraph,
            ));
            i = j;
        }
    }
    units
}

fn strip_heading(line: &str) -> Option<(String, String, String)> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = &t[hashes..];
    if !after.starts_with(' ') && !after.starts_with('\t') {
        return None;
    }
    let leading = &line[..line.len() - t.len()];
    let body = after.trim_start().trim_end_matches('\n').to_string();
    let suffix = if line.ends_with('\n') {
        "\n".to_string()
    } else {
        String::new()
    };
    let prefix_end = hashes + 1; // 连同标题后的空格
    Some((format!("{leading}{}", &t[..prefix_end]), body, suffix))
}

fn strip_markup_prefix(line: &str) -> Option<(String, String, String)> {
    let t = line.trim_start();
    let leading = &line[..line.len() - t.len()];
    let suffix = if line.ends_with('\n') {
        "\n".to_string()
    } else {
        String::new()
    };
    let body = line.trim_end_matches('\n');

    for marker in ["> ", "- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(marker) {
            return Some((
                format!("{leading}{}", &marker[0..2]),
                rest.trim_end().to_string(),
                suffix,
            ));
        }
    }
    if let Some(rest) = t.strip_prefix('>') {
        return Some((format!("{leading}>"), rest.trim_start().to_string(), suffix));
    }
    if is_ordered_list_item(t) {
        let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
        if let Some(rest) = t[digits..].strip_prefix(". ") {
            return Some((
                format!("{leading}{}. ", &t[..digits]),
                rest.trim_end().to_string(),
                suffix,
            ));
        }
    }
    let _ = body;
    None
}

fn is_ordered_list_item(t: &str) -> bool {
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && t[digits..].starts_with(". ")
}

/// 句子边界切分。**禁止简单按 `.` 分割**。
///
/// 处理：URL/邮箱/模板变量/代码（跳过）、小数、常见缩写、首字母缩写、括号、
/// 连续标点、收尾引号。返回的 `separator` 是句子前的空白，按原顺序拼接即可还原。
pub fn split_sentences(text: &str) -> Vec<SentenceUnit> {
    let protected = protected_spans(text);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    let mut paren = 0usize;

    while i < chars.len() {
        let (byte, c) = chars[i];

        // 受保护跨度整体跳过
        if let Some(span) = protected.iter().find(|s| s.start == byte) {
            i = chars
                .iter()
                .position(|(b, _)| *b == span.end)
                .unwrap_or(chars.len());
            continue;
        }

        if c == '(' || c == '[' {
            paren += 1;
        } else if c == ')' || c == ']' {
            paren = paren.saturating_sub(1);
        }

        if c == '\n' {
            let next = chars.get(i + 1).map(|(_, c)| *c);
            if paren == 0 && next.map(|c| !c.is_whitespace()).unwrap_or(false) {
                push_sentence(text, start, byte, &mut out);
                start = byte;
                cursor = start;
            }
            i += 1;
            continue;
        }

        if matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '…') {
            if c == '.' && is_decimal_or_abbreviation(text, &chars, i) {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < chars.len()
                && matches!(chars[j].1, '.' | '!' | '?' | '。' | '！' | '？' | '…')
            {
                j += 1;
            }
            while j < chars.len()
                && matches!(
                    chars[j].1,
                    '"' | '\'' | '”' | '’' | ')' | ']' | '}' | '」' | '』'
                )
            {
                j += 1;
            }
            let next = chars.get(j).map(|(_, c)| *c);
            let boundary = next
                .map(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '“' | '‘' | '(' | '['))
                .unwrap_or(true);
            if boundary && paren == 0 {
                let end_byte = chars.get(j).map(|(b, _)| *b).unwrap_or(text.len());
                push_sentence(text, start, end_byte, &mut out);
                start = end_byte;
                cursor = start;
            }
            i = j;
            continue;
        }

        i += 1;
        let _ = cursor;
    }

    if start < text.len() {
        push_sentence(text, start, text.len(), &mut out);
    }
    if out.is_empty() && !text.trim().is_empty() {
        out.push(SentenceUnit {
            separator: String::new(),
            text: text.trim().to_string(),
        });
    }
    out
}

fn push_sentence(text: &str, start: usize, end: usize, out: &mut Vec<SentenceUnit>) {
    if start >= end {
        return;
    }
    let raw = &text[start..end];
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let leading_len = raw.len() - raw.trim_start().len();
    let separator = raw[..leading_len].to_string();
    out.push(SentenceUnit {
        separator,
        text: trimmed.to_string(),
    });
}

fn has_terminal_punctuation(text: &str) -> bool {
    text.trim_end()
        .chars()
        .last()
        .map(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '…'))
        .unwrap_or(false)
}

fn count_paragraphs(text: &str) -> usize {
    text.split("\n\n").filter(|p| !p.trim().is_empty()).count()
}

fn is_decimal_or_abbreviation(text: &str, chars: &[(usize, char)], i: usize) -> bool {
    let prev = i
        .checked_sub(1)
        .and_then(|idx| chars.get(idx))
        .map(|(_, c)| *c);
    let next = chars.get(i + 1).map(|(_, c)| *c);
    if prev.map(|c| c.is_ascii_digit()).unwrap_or(false)
        && next.map(|c| c.is_ascii_digit()).unwrap_or(false)
    {
        return true;
    }

    // 取点号前最后一个词
    let mut end = i;
    while end > 0 && chars[end - 1].1.is_whitespace() {
        end -= 1;
    }
    let mut begin = end;
    while begin > 0 && !chars[begin - 1].1.is_whitespace() {
        begin -= 1;
    }
    let token: String = chars[begin..end].iter().map(|(_, c)| *c).collect();
    let token = token
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.')
        .to_ascii_lowercase();

    const ABBREV: &[&str] = &[
        "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "u.s", "a.m",
        "p.m", "no", "fig", "approx", "dept", "inc", "ltd", "co", "corp", "jan", "feb", "mar",
        "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec", "mon", "tue", "wed", "thu",
        "fri", "sat", "sun",
    ];
    if ABBREV.contains(&token.as_str()) {
        return true;
    }
    // 单个大写首字母（J. Smith）
    if token.len() == 1 && chars[begin].1.is_ascii_uppercase() {
        return true;
    }
    // 省略号 ... 不在这里切（由连续标点处理）
    let _ = text;
    false
}

/// 受保护跨度里是否包含数字类（供诊断/统计）。
pub fn number_protected_count(text: &str) -> usize {
    protected_spans(text)
        .iter()
        .filter(|s| s.kind == ProtectedKind::Number)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_types::{TextKind, TranslateOptions};

    #[test]
    fn 自动分类四个层级() {
        assert_eq!(analyze("bank", TextKind::Auto).kind, TextKind::Word);
        assert_eq!(
            analyze("Please restart the server.", TextKind::Auto).kind,
            TextKind::Sentence
        );
        assert_eq!(
            analyze("First sentence. Second sentence.", TextKind::Auto).kind,
            TextKind::Paragraph
        );
        assert_eq!(
            analyze(
                "# Title\n\nFirst paragraph.\n\nSecond paragraph.",
                TextKind::Auto
            )
            .kind,
            TextKind::Document
        );
    }

    #[test]
    fn 句子切分不拆小数缩写url() {
        let units = split_sentences(
            "Version 2.10 is ready. See https://example.com/a.b for details. Dr. Smith agrees.",
        );
        assert_eq!(units.len(), 3, "{units:?}");
        assert!(units[0].text.contains("2.10"));
        assert!(units[1].text.contains("https://example.com/a.b"));
        assert!(units[2].text.starts_with("Dr. Smith"));
    }

    #[test]
    fn 句子分隔符可按原样还原() {
        let text = "First sentence.  Second sentence.\nThird.";
        let units = split_sentences(text);
        let rebuilt: String = units
            .iter()
            .map(|u| format!("{}{}", u.separator, u.text))
            .collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn 长句在连接词处分句() {
        let p = plan(
            "The event starts at 8 PM and ends at 11 PM.",
            &TranslateOptions::default().with_text_kind(TextKind::Sentence),
        );
        assert_eq!(p.strategy, "sentence-clauses");
        assert_eq!(p.units.len(), 2);
        assert!(p.units[1].text.as_deref().unwrap().starts_with("It ends"));
    }

    #[test]
    fn 短句不切分() {
        let p = plan(
            "Tea and coffee are ready.",
            &TranslateOptions::default().with_text_kind(TextKind::Sentence),
        );
        assert_eq!(p.strategy, "sentence");
        assert_eq!(p.units.len(), 1);
    }

    #[test]
    fn 并列名词短语不切分() {
        let p = plan(
            "The server has 64 CPU cores and 512 GB of memory.",
            &TranslateOptions::default().with_text_kind(TextKind::Sentence),
        );
        assert_eq!(p.strategy, "sentence");
        assert_eq!(p.units.len(), 1);
    }

    #[test]
    fn 段落三种策略单元数不同() {
        let text = "One. Two. Three. Four.";
        let whole = plan(
            text,
            &TranslateOptions::default()
                .with_text_kind(TextKind::Paragraph)
                .with_paragraph_strategy(ParagraphStrategy::Whole),
        );
        assert_eq!(whole.strategy, "paragraph-whole");
        assert_eq!(whole.units.len(), 1);

        let sentence = plan(
            text,
            &TranslateOptions::default()
                .with_text_kind(TextKind::Paragraph)
                .with_paragraph_strategy(ParagraphStrategy::Sentence),
        );
        assert_eq!(sentence.strategy, "paragraph-sentence");
        assert_eq!(sentence.units.len(), 4);

        let group = plan(
            text,
            &TranslateOptions::default()
                .with_text_kind(TextKind::Paragraph)
                .with_paragraph_strategy(ParagraphStrategy::Group),
        );
        assert_eq!(group.strategy, "paragraph-group");
        assert_eq!(group.units.len(), 2);
    }

    #[test]
    fn 段落auto默认逐句() {
        let p = plan(
            "One. Two. Three.",
            &TranslateOptions::default().with_text_kind(TextKind::Paragraph),
        );
        assert_eq!(p.strategy, "paragraph-sentence");
        assert_eq!(p.units.len(), 3);
    }

    #[test]
    fn markdown保留代码块与列表前缀() {
        let text = "# Title\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```\n";
        let p = plan(
            text,
            &TranslateOptions::default().with_text_kind(TextKind::Document),
        );
        assert_eq!(p.kind, TextKind::Document);
        assert!(p.units.iter().any(|u| u.text.is_none()), "代码块应原样保留");
        let heading = p
            .units
            .iter()
            .find(|u| u.separator.starts_with('#'))
            .expect("标题前缀");
        assert_eq!(heading.text.as_deref(), Some("Title"));
        let list = p
            .units
            .iter()
            .filter(|u| u.separator.starts_with('-'))
            .count();
        assert_eq!(list, 2);
    }
}
