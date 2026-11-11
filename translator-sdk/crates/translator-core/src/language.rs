//! 语言识别与形态分析的**产品侧**最小实现。
//!
//! # 为什么不再依赖 `translator-analysis`
//!
//! 旧架构里 `translator-core` 必须依赖 `translator-analysis` 才能识别语言，
//! 于是研究型 crate 变成了产品构建的强制依赖（§3.2）。重构后：
//!
//! - 产品侧只保留**零模型**的启发式识别器与两个 trait；
//! - `translator-analysis` 作为研究线独立存在，不再被产品引用。
//!
//! # 局限（必须知道）
//!
//! 基于 Unicode 文字系统的启发式无法区分中文/日文（共用汉字），也无法区分
//! 具体拉丁语言。因此：
//!
//! - 短文本**必须**返回多候选，不允许强行判定；
//! - 翻译请求应优先显式指定 `source`，自动识别只作兜底。

use translator_types::{LanguageGuess, LanguageTag};

/// 语言识别器。
pub trait LanguageIdentifier: Send + Sync {
    /// 返回按置信度降序排列的候选，最多 `limit` 个。
    fn identify(&self, text: &str, limit: usize) -> Vec<LanguageGuess>;

    /// 人类可读的实现名，用于诊断。
    fn name(&self) -> &'static str;
}

/// 形态分析器（词形还原）。只用于**查词未命中后的候选**，不改写翻译输入。
pub trait MorphologyAnalyzer: Send + Sync {
    fn lemmas(&self, word: &str) -> Vec<String>;
    fn name(&self) -> &'static str;
}

/// 基于 Unicode 文字系统的启发式识别器（零模型、零依赖）。
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicIdentifier;

impl HeuristicIdentifier {
    pub fn new() -> Self {
        Self
    }
}

fn is_fullwidth_latin(c: char) -> bool {
    ('\u{FF21}'..='\u{FF3A}').contains(&c) || ('\u{FF41}'..='\u{FF5A}').contains(&c)
}

impl LanguageIdentifier for HeuristicIdentifier {
    fn identify(&self, text: &str, limit: usize) -> Vec<LanguageGuess> {
        if limit == 0 {
            return Vec::new();
        }

        let has_kana = text.chars().any(|c| ('\u{3040}'..='\u{30FF}').contains(&c));
        let has_hangul = text.chars().any(|c| ('\u{AC00}'..='\u{D7AF}').contains(&c));
        let has_han = text.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));
        let has_cyrillic = text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c));
        let has_arabic = text.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c));
        let has_latin = text
            .chars()
            .any(|c| c.is_ascii_alphabetic() || is_fullwidth_latin(c));

        let mut guesses: Vec<LanguageGuess> = Vec::new();
        let mut push = |tag: &str, confidence: f32| {
            if let Ok(language) = LanguageTag::new(tag) {
                guesses.push(LanguageGuess {
                    language,
                    confidence,
                });
            }
        };

        if has_kana {
            push("ja", 0.95);
            push("zh-Hans", 0.03);
        } else if has_hangul {
            push("ko", 0.95);
        } else if has_han {
            push("zh-Hans", 0.60);
            push("ja", 0.25);
        } else if has_cyrillic {
            push("ru", 0.70);
            push("uk", 0.15);
        } else if has_arabic {
            push("ar", 0.75);
            push("fa", 0.10);
        } else if has_latin {
            push("en", 0.50);
            push("fr", 0.10);
            push("de", 0.10);
        }

        guesses.truncate(limit);
        guesses
    }

    fn name(&self) -> &'static str {
        "heuristic-unicode-script"
    }
}

/// 便捷函数：返回置信度最高的语言。
pub fn guess(text: &str) -> Option<LanguageTag> {
    HeuristicIdentifier
        .identify(text, 1)
        .into_iter()
        .next()
        .map(|g| g.language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 假名判定为日语() {
        let g = HeuristicIdentifier.identify("こんにちは", 5);
        assert_eq!(g[0].language.as_str(), "ja");
    }

    #[test]
    fn 汉字返回多候选() {
        let g = HeuristicIdentifier.identify("你好世界", 5);
        assert!(g.len() >= 2);
        assert_eq!(g[0].language.as_str(), "zh-Hans");
    }

    #[test]
    fn 拉丁字母给低置信度() {
        let g = HeuristicIdentifier.identify("hello", 5);
        assert_eq!(g[0].language.as_str(), "en");
        assert!(g[0].confidence <= 0.6);
    }

    #[test]
    fn 纯数字无候选() {
        assert!(HeuristicIdentifier.identify("12345", 5).is_empty());
    }

    #[test]
    fn 全角拉丁也识别() {
        let g = HeuristicIdentifier.identify("ｃａｔ", 3);
        assert_eq!(g[0].language.as_str(), "en");
    }

    #[test]
    fn guess_返回最高置信度() {
        assert_eq!(guess("hello").unwrap().as_str(), "en");
        assert!(guess("12345").is_none());
        assert!(guess("").is_none());
    }
}
