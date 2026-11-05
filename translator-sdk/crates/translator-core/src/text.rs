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
pub fn looks_like_single_word(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    // 含空白 → 更像短语或句子
    if trimmed.chars().any(char::is_whitespace) {
        // 例外：两三个词组成的固定短语可能命中短语表，但 P0 不猜
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

    // 过长的"词"多半不是词条
    trimmed.chars().count() <= 32
}

/// 极简语言识别启发式：优先看文字系统，无法判断时按 ASCII 猜测。
///
/// ⚠️ 这不是完整的 LID。真正的实现应接 fastText `lid.176.ftz`
/// （资源文档 §4），且短文本必须返回多候选而非强行判定。
pub fn guess_language(text: &str) -> Option<LanguageTag> {
    let has_hiragana_or_katakana = text.chars().any(|c| ('\u{3040}'..='\u{30FF}').contains(&c));
    let has_hangul = text.chars().any(|c| ('\u{AC00}'..='\u{D7AF}').contains(&c));
    let has_han = text.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));

    let tag = if has_hiragana_or_katakana {
        "ja"
    } else if has_hangul {
        "ko"
    } else if has_han {
        // 汉字无法区分中日，按简体中文处理（可被显式 source 覆盖）
        "zh-Hans"
    } else if text.chars().any(|c| c.is_ascii_alphabetic()) {
        "en"
    } else {
        return None;
    };

    LanguageTag::new(tag).ok()
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

/// 保护 URL、邮箱、`{占位符}` 与反引号代码块。
///
/// 设计文档 §6 明确要求：未能确保占位符恢复时必须返回警告或错误，
/// 不得静默丢失。
pub fn protect(input: &str) -> ProtectedText {
    let mut segments: Vec<String> = Vec::new();
    let mut out = String::with_capacity(input.len());

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // URL：http:// 或 https:// 直到空白
        let rest: String = chars[i..chars.len().min(i + 8)].iter().collect();
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let mut j = i;
            while j < chars.len() && !chars[j].is_whitespace() {
                j += 1;
            }
            let seg: String = chars[i..j].iter().collect();
            out.push_str(&ProtectedText::marker(segments.len()));
            segments.push(seg);
            i = j;
            continue;
        }

        // {占位符}
        if chars[i] == '{' {
            if let Some(offset) = chars[i..].iter().position(|c| *c == '}') {
                let j = i + offset;
                let seg: String = chars[i..=j].iter().collect();
                out.push_str(&ProtectedText::marker(segments.len()));
                segments.push(seg);
                i = j + 1;
                continue;
            }
        }

        // `code`
        if chars[i] == '`' {
            if let Some(offset) = chars[i + 1..].iter().position(|c| *c == '`') {
                let j = i + 1 + offset;
                let seg: String = chars[i..=j].iter().collect();
                out.push_str(&ProtectedText::marker(segments.len()));
                segments.push(seg);
                i = j + 1;
                continue;
            }
        }

        out.push(chars[i]);
        i += 1;
    }

    ProtectedText {
        text: out,
        segments,
    }
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
}
