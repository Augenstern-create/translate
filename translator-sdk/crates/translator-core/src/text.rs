//! 文本预处理：规范化、语言识别启发式、占位符保护与还原。
//!
//! 对应设计文档 §6「流水线」与 §7.3「安全规则示例」。
//!
//! ⚠️ 这里刻意只做**低成本、可测试**的处理。完整的规则 DSL 属于 P1，
//! 但"保护占位符并在失败时报警"必须从 P0 就建立。

use translator_types::LanguageTag;

/// 折叠多余空白并去除首尾空白。不做 NFKC——那会破坏代码与符号语义。
pub fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_space = false;

    for ch in input.trim().chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }

    out
}

/// 判断是否"像单个词条"。
///
/// 用于 `AutoLocalFirst` 策略：只有单词/极短语才优先查词典，
/// 完整句子一律交给 NMT（设计文档 §6 第 1、2 条）。
///
/// # 为什么按文字系统区分阈值
///
/// 空格分隔语言（英、法等）用空白就能切词，长度上限可以放宽。
/// 但**中日韩不用空格**，`我喜欢计算机` 这种完整句子在字节层面
/// 与单词无异。实测中它会被误判为单词、白跑一次词典查询。
/// 因此 CJK 文本改用**字符数上限 4**（常见中文词为 1–4 字），
/// 超过即视为句子。
pub fn looks_like_single_word(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    // 含空白 → 更像短语或句子（空格分隔语言的主要判据）
    if trimmed.chars().any(char::is_whitespace) {
        return false;
    }

    // 含句末标点 → 句子
    if trimmed
        .chars()
        .last()
        .map(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | ';' | '；'))
        .unwrap_or(false)
    {
        return false;
    }

    let cjk = trimmed.chars().any(|c| {
        ('\u{3040}'..='\u{30FF}').contains(&c)      // 假名
            || ('\u{4E00}'..='\u{9FFF}').contains(&c) // 汉字
            || ('\u{AC00}'..='\u{D7AF}').contains(&c) // 谚文
    });

    let max_chars = if cjk { 4 } else { 32 };
    trimmed.chars().count() <= max_chars
}

/// 语言识别：委托给 `translator-analysis` 的 [`LanguageIdentifier`] 实现。
///
/// 当前为启发式实现（零模型）。接入 fastText 后只需替换注入的识别器，
/// 调用方无需改动。
///
/// [`LanguageIdentifier`]: translator_analysis::LanguageIdentifier
pub fn guess_language(text: &str) -> Option<LanguageTag> {
    translator_analysis::guess(text)
}

/// 保护后的文本。
#[derive(Clone, Debug)]
pub struct ProtectedText {
    /// 占位符替换后的文本。
    pub text: String,
    /// 原始片段，下标与占位符编号对应。
    pub segments: Vec<String>,
}

impl ProtectedText {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    fn marker(index: usize) -> String {
        format!("\u{27E6}P{index}\u{27E7}")
    }
}

/// 受保护内容的种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedKind {
    Url,
    Email,
    /// `{name}` 形式的模板变量。
    TemplateVar,
    /// 反引号代码片段。
    Code,
    /// 关键数字格式：百分比、十六进制、时间、日期、版本号（T2/W2）。
    ///
    /// 只保护这些“丢失即错”的格式，不保护所有数字：把普通数量词全部摘出来
    /// 会把句子切碎，反而破坏语序与自然度。
    Number,
}

/// 原文里的一段受保护区间。
///
/// `start` / `end` 是**字节偏移**，作用域是**传入 `protected_spans()` 的那份文本**。
///
/// # 偏移作用域（重要）
///
/// 流水线里先做 `normalize()`（折叠空白）再切分跨度，因此偏移是相对**规范化之后**的文本，
/// 不能直接用于用户原始输入。目前**不提供**回到原文的偏移映射：
/// 该映射需要规范化保留字符级对应关系（`normalize_with_map`），只有"高亮用户原文"这类需求才需要它。
/// 如果将来要加，规则是：先 `normalize_with_map()` 得到映射表，再把跨度换算回原文；
/// 不允许在未映射的情况下把规范化偏移当作原文偏移使用。
///
/// # 不暴露给 NMT 插件
///
/// 这个类型只在 SDK 内部使用。P3 接入真实 Marain/Bergamot 时，才根据 tokenizer 行为
/// 决定用文本标记、分段翻译还是别的机制跨过模型边界（ADR-009）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtectedSpan {
    pub start: usize,
    pub end: usize,
    pub kind: ProtectedKind,
}

/// 文本片段。
///
/// 规则引擎**只能**改写 `Plain` 片段：URL、邮箱、模板变量、代码里的内容
/// 必须字节级保持不变（设计文档 §6/§7.3，ADR-009）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    /// 普通文本，允许规则改写。
    Plain(&'a str),
    /// 受保护片段，规则不得改动。
    Protected(&'a str),
}

/// 把输入切成「纯文本 / 受保护」片段。
///
/// 这是 `protect()` 与「规则只在纯文本上生效」共用的**唯一**扫描器，
/// 避免两套正则各自演化、最终对不上。
pub fn split_protected(input: &str) -> Vec<Segment<'_>> {
    let spans = protected_spans(input);
    let mut out = Vec::with_capacity(spans.len() * 2 + 1);
    let mut cursor = 0usize;

    for span in spans {
        if span.start > cursor {
            out.push(Segment::Plain(&input[cursor..span.start]));
        }
        out.push(Segment::Protected(&input[span.start..span.end]));
        cursor = span.end;
    }

    if cursor < input.len() {
        out.push(Segment::Plain(&input[cursor..]));
    }
    out
}

/// 扫描受保护区间。这是 URL / 邮箱 / `{模板变量}` / 反引号代码的**唯一**识别入口。
pub fn protected_spans(input: &str) -> Vec<ProtectedSpan> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let byte_at = |idx: usize| chars.get(idx).map_or(input.len(), |(b, _)| *b);

    let mut out = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        let (byte_pos, c) = chars[i];

        let found = if starts_with_url(&chars, i) {
            url_end(&chars, i).map(|end| (end, ProtectedKind::Url))
        } else if c == '{' {
            brace_end(&chars, i).map(|end| (end, ProtectedKind::TemplateVar))
        } else if c == '`' {
            backtick_end(&chars, i).map(|end| (end, ProtectedKind::Code))
        } else if is_email_char(c) && is_token_start(&chars, i) {
            // 邮箱扫描失败时必须继续尝试数字，否则 `50%` 会被当成
            // “像邮箱但没找到 @” 而整个分支吞掉。
            email_end(&chars, i)
                .map(|end| (end, ProtectedKind::Email))
                .or_else(|| {
                    if c.is_ascii_digit() {
                        numeric_end(&chars, i).map(|end| (end, ProtectedKind::Number))
                    } else {
                        None
                    }
                })
        } else if c.is_ascii_digit() {
            numeric_end(&chars, i).map(|end| (end, ProtectedKind::Number))
        } else {
            None
        };

        match found {
            Some((end_idx, kind)) => {
                out.push(ProtectedSpan {
                    start: byte_pos,
                    end: byte_at(end_idx),
                    kind,
                });
                i = end_idx;
            }
            None => i += 1,
        }
    }

    out
}

/// 保护 URL、邮箱、`{占位符}` 与反引号代码块。
///
/// 设计文档 §6 明确要求：未能确保占位符恢复时必须返回警告或错误，
/// 不得静默丢失。
pub fn protect(input: &str) -> ProtectedText {
    let mut segments: Vec<String> = Vec::new();
    let mut out = String::with_capacity(input.len());

    for segment in split_protected(input) {
        match segment {
            Segment::Plain(text) => out.push_str(text),
            Segment::Protected(text) => {
                out.push_str(&ProtectedText::marker(segments.len()));
                segments.push(text.to_string());
            }
        }
    }

    ProtectedText {
        text: out,
        segments,
    }
}

/// 识别必须原样保留的数字格式。
///
/// 只匹配：
/// - `50%` / `0.05%`
/// - `0x80070005`
/// - `07:15` / `7:15:30`
/// - `2026-12-01` / `2026/12/01`
/// - 版本号 `2.4.1`（至少两段点号）
///
/// 普通整数/小数不在这里保护；它们由译后数字检查负责。
fn numeric_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;

    // 0x...
    if chars[i].1 == '0' && i + 1 < chars.len() && matches!(chars[i + 1].1, 'x' | 'X') {
        j = i + 2;
        let start = j;
        while j < chars.len() && chars[j].1.is_ascii_hexdigit() {
            j += 1;
        }
        if j > start {
            return Some(j);
        }
        return None;
    }

    // 时间 / 日期 / 版本 / 百分比：先吃掉数字与分隔符
    let mut saw_colon = false;
    let mut saw_dash = false;
    let mut saw_slash = false;
    let mut dots = 0usize;
    while j < chars.len() {
        let c = chars[j].1;
        if c.is_ascii_digit() {
            j += 1;
        } else if c == ':' && !saw_colon && j + 1 < chars.len() && chars[j + 1].1.is_ascii_digit() {
            saw_colon = true;
            j += 1;
        } else if c == '-' && j + 1 < chars.len() && chars[j + 1].1.is_ascii_digit() {
            saw_dash = true;
            j += 1;
        } else if c == '/' && j + 1 < chars.len() && chars[j + 1].1.is_ascii_digit() {
            saw_slash = true;
            j += 1;
        } else if c == '.' && j + 1 < chars.len() && chars[j + 1].1.is_ascii_digit() {
            dots += 1;
            j += 1;
        } else {
            break;
        }
    }

    // 百分比（允许数字与 % 之间没有空格；有空格时只保护数字部分）
    if j < chars.len() && chars[j].1 == '%' {
        return Some(j + 1);
    }
    // 冒号时间（07:15）不单独保护：模型通常会把 07:15 写成 7: 15，
    // 数字检查已能识别；强行分段反而会让 “10:30 AM on March 5” 这类
    // 日期时间短语被拆坏。日期/版本/百分比仍然保护。
    let _ = saw_colon;
    if saw_dash || saw_slash || dots >= 2 {
        return Some(j);
    }
    None
}

fn starts_with_url(chars: &[(usize, char)], i: usize) -> bool {
    let head: String = chars[i..chars.len().min(i + 8)]
        .iter()
        .map(|(_, c)| *c)
        .collect();
    head.starts_with("http://") || head.starts_with("https://")
}

fn url_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;
    while j < chars.len() && !chars[j].1.is_whitespace() {
        j += 1;
    }
    Some(j)
}

fn brace_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    chars[i..]
        .iter()
        .position(|(_, c)| *c == '}')
        .map(|offset| i + offset + 1)
}

fn backtick_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    chars[i + 1..]
        .iter()
        .position(|(_, c)| *c == '`')
        .map(|offset| i + 1 + offset + 1)
}

fn is_email_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

/// 只在"词的起点"尝试匹配邮箱，避免把 `a@b.c` 里的片段重复匹配。
fn is_token_start(chars: &[(usize, char)], i: usize) -> bool {
    match i.checked_sub(1).and_then(|prev| chars.get(prev)) {
        Some((_, c)) => !is_email_char(*c),
        None => true,
    }
}

/// 匹配 `local@domain.tld`。域名至少一个点、TLD 至少两个字母。
fn email_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;
    while j < chars.len() && is_email_char(chars[j].1) {
        j += 1;
    }
    if j >= chars.len() || chars[j].1 != '@' {
        return None;
    }
    let at = j;
    j += 1;

    let mut seen_dot = false;
    while j < chars.len() {
        let c = chars[j].1;
        if c == '.' {
            seen_dot = true;
            j += 1;
        } else if c.is_ascii_alphanumeric() || c == '-' {
            j += 1;
        } else {
            break;
        }
    }
    if !seen_dot || j == at + 1 {
        return None;
    }

    // TLD 必须是至少两个字母（否则 `a@b.c` 这种不像邮箱，保持原样）
    let tld_start = chars[..j]
        .iter()
        .rposition(|(_, c)| *c == '.')
        .map(|pos| pos + 1)?;
    let tld: String = chars[tld_start..j].iter().map(|(_, c)| *c).collect();
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(j)
}

/// 还原占位符。
///
/// 返回 `(还原后的文本, 未能还原的占位符列表)`。
/// 后者非空时调用方**必须**生成 `PlaceholderRestoreFailed` 警告。
pub fn restore(protected: &ProtectedText, translated: &str) -> (String, Vec<String>) {
    let mut out = translated.to_string();
    let mut missing = Vec::new();

    for (idx, seg) in protected.segments.iter().enumerate() {
        let marker = ProtectedText::marker(idx);
        if out.contains(&marker) {
            out = out.replace(&marker, seg);
        } else {
            // 标记被引擎吞掉了
            missing.push(seg.clone());
        }
    }

    (out, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 规范化应折叠空白() {
        assert_eq!(normalize("  hello   world \n"), "hello world");
    }

    #[test]
    fn 单词判定() {
        assert!(looks_like_single_word("hello"));
        assert!(looks_like_single_word("猫"));
        assert!(!looks_like_single_word("hello world"));
        assert!(!looks_like_single_word("hello."));
        assert!(!looks_like_single_word(""));
    }

    #[test]
    fn cjk句子不应被误判为单词() {
        // 中文没有空格，必须靠字数区分
        assert!(looks_like_single_word("计算机"), "3 字应当作词条");
        assert!(looks_like_single_word("世界"), "2 字应当作词条");
        assert!(!looks_like_single_word("我喜欢计算机"), "6 字应视为句子");
        assert!(!looks_like_single_word("今天天气很好"), "6 字应视为句子");
    }

    #[test]
    fn 长英文单词仍算单词() {
        assert!(looks_like_single_word("internationalization"));
    }

    #[test]
    fn 切分受保护片段() {
        // URL、邮箱、模板变量、反引号代码都必须是 Protected
        let text =
            "see https://example.com/LLM and mail a.b@example.com, then `code_x` and {token}";
        let segments = split_protected(text);
        let protected: Vec<&str> = segments
            .iter()
            .filter_map(|s| match s {
                Segment::Protected(t) => Some(*t),
                Segment::Plain(_) => None,
            })
            .collect();
        assert_eq!(
            protected,
            vec![
                "https://example.com/LLM",
                "a.b@example.com",
                "`code_x`",
                "{token}"
            ]
        );

        // 拼回原文必须无损
        let rebuilt: String = segments
            .iter()
            .map(|s| match s {
                Segment::Plain(t) | Segment::Protected(t) => *t,
            })
            .collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn 受保护跨度带类型与字节偏移() {
        let text = "x https://a.example/p y a@b.com z {v} `c`";
        let spans = protected_spans(text);
        let kinds: Vec<(ProtectedKind, &str)> = spans
            .iter()
            .map(|s| (s.kind, &text[s.start..s.end]))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (ProtectedKind::Url, "https://a.example/p"),
                (ProtectedKind::Email, "a@b.com"),
                (ProtectedKind::TemplateVar, "{v}"),
                (ProtectedKind::Code, "`c`"),
            ]
        );
        // 偏移必须是字节偏移，且切片不 panic
        for span in &spans {
            assert!(span.start < span.end);
            assert!(span.end <= text.len());
            assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
        }
    }

    #[test]
    fn 邮箱识别不应误伤普通文本() {
        // 没有 TLD / TLD 太短 / 没有 @ 的情况都不该被当成邮箱
        for text in ["a@b", "a@b.c", "hello world", "价格 100@50"] {
            let segments = split_protected(text);
            assert!(
                segments.iter().all(|s| matches!(s, Segment::Plain(_))),
                "{text:?} 被误判为受保护片段：{segments:?}"
            );
        }
    }

    #[test]
    fn 语言启发式() {
        assert_eq!(guess_language("hello").unwrap().as_str(), "en");
        assert_eq!(guess_language("你好").unwrap().as_str(), "zh-Hans");
        assert_eq!(guess_language("こんにちは").unwrap().as_str(), "ja");
        assert_eq!(guess_language("123").map(|t| t.as_str().to_string()), None);
    }

    #[test]
    fn 占位符应被完整保护与还原() {
        let src = "visit https://example.com and {name} in `code`";
        let p = protect(src);
        assert_eq!(p.segments.len(), 3);
        assert!(!p.text.contains("example.com"));

        let (restored, missing) = restore(&p, &format!("zh:{}", p.text));
        assert!(missing.is_empty());
        assert!(restored.contains("https://example.com"));
        assert!(restored.contains("{name}"));
        assert!(restored.contains("`code`"));
    }

    #[test]
    fn 丢失占位符应被检出() {
        let p = protect("hello {name}");
        let (_, missing) = restore(&p, "你好"); // 引擎吞掉了标记
        assert_eq!(missing, vec!["{name}".to_string()]);
    }
    #[test]
    fn 关键数字格式被保护并可原样还原() {
        let input = "Usage 50% on 2026-12-01, code 0x80070005, version 2.4.1.";
        let protected = protect(input);
        assert_eq!(protected.segments.len(), 4, "{:?}", protected.segments);
        assert!(protected.segments.iter().any(|s| s == "50%"));
        assert!(protected.segments.iter().any(|s| s == "2026-12-01"));
        assert!(protected.segments.iter().any(|s| s == "0x80070005"));
        assert!(protected.segments.iter().any(|s| s == "2.4.1"));
        let (restored, missing) = restore(&protected, &protected.text);
        assert!(missing.is_empty());
        assert_eq!(restored, input);
    }
}
