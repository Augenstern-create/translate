//! 文本分析：语言识别与后续的形态分析。
//!
//! 对应设计文档 §7「字典、语言检测、规则系统」与资源文档 §4「语言识别候选」。
//!
//! # 设计原则
//!
//! 语言识别被抽象为 [`LanguageIdentifier`] trait，便于**替换实现**：
//!
//! - P1：`HeuristicIdentifier`（基于 Unicode 文字系统，无模型，零内存）
//! - 后续：fastText `lid.176.ftz`（917 KB，CC BY-SA 3.0）实现同一 trait
//!
//! 资源文档 §4 明确要求：**短文本与低置信度必须返回多候选，不能强行判定**。
//! 因此本 trait 返回 `Vec<LanguageGuess>` 而非单个结果。

use translator_types::{LanguageGuess, LanguageTag};

/// 语言识别器。
pub trait LanguageIdentifier: Send + Sync {
    /// 返回按置信度降序排列的候选，最多 `limit` 个。
    fn identify(&self, text: &str, limit: usize) -> Vec<LanguageGuess>;

    /// 人类可读的实现名，用于诊断。
    fn name(&self) -> &'static str;
}

/// 基于 Unicode 文字系统的启发式识别器。
///
/// 优点：零模型、零依赖、零内存。
/// 缺点：汉字为中/日共享，无法区分简体/繁体与中文/日文；拉丁字母只能猜 `en`。
///
/// **它不是产品级的 LID**，仅用于 P1 打通链路；接 fastText 后由配置切换。
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicIdentifier;

impl HeuristicIdentifier {
    pub fn new() -> Self {
        Self
    }
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
        // 全角拉丁字母（Ａ-Ｚ ａ-ｚ）也算拉丁：用户从中文输入法切过来时常留下全角，
        // 若这里不认，就会在语言识别阶段直接失败，规则包连纠正的机会都没有。
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
            // 有假名 → 基本可确定日语
            push("ja", 0.95);
            push("zh-Hans", 0.03);
        } else if has_hangul {
            push("ko", 0.95);
        } else if has_han {
            // 汉字共享：中文为主，日文为候选（满足"多候选"要求）
            push("zh-Hans", 0.60);
            push("ja", 0.25);
        } else if has_cyrillic {
            push("ru", 0.70);
            push("uk", 0.15);
        } else if has_arabic {
            push("ar", 0.75);
            push("fa", 0.10);
        } else if has_latin {
            // 拉丁字母无法区分具体语言，只能给低置信度候选
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

/// 兼容便捷函数：返回置信度最高的语言。
pub fn guess(text: &str) -> Option<LanguageTag> {
    HeuristicIdentifier
        .identify(text, 1)
        .into_iter()
        .next()
        .map(|g| g.language)
}

fn is_fullwidth_latin(c: char) -> bool {
    ('\u{FF21}'..='\u{FF3A}').contains(&c) || ('\u{FF41}'..='\u{FF5A}').contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 假名应判定为日语() {
        let g = HeuristicIdentifier.identify("こんにちは", 5);
        assert_eq!(g[0].language.as_str(), "ja");
        assert!(g[0].confidence > 0.9);
    }

    #[test]
    fn 汉字应返回多个候选() {
        let g = HeuristicIdentifier.identify("你好世界", 5);
        assert!(g.len() >= 2, "短文本不得只给一个答案");
        assert_eq!(g[0].language.as_str(), "zh-Hans");
        assert!(g.iter().any(|x| x.language.as_str() == "ja"));
    }

    #[test]
    fn 韩文应判定为韩语() {
        let g = HeuristicIdentifier.identify("안녕하세요", 3);
        assert_eq!(g[0].language.as_str(), "ko");
    }

    #[test]
    fn 拉丁字母给低置信度候选() {
        let g = HeuristicIdentifier.identify("hello", 5);
        assert_eq!(g[0].language.as_str(), "en");
        assert!(g[0].confidence <= 0.6, "拉丁字母不应给出高置信度");
    }

    #[test]
    fn 纯数字无候选() {
        assert!(HeuristicIdentifier.identify("12345", 5).is_empty());
    }

    #[test]
    fn 全角拉丁字母也算拉丁() {
        // 用户从中文输入法切回英文时常见全角残留，必须仍能识别出拉丁语言，
        // 否则规则包的 NFKC 规则连执行机会都没有（见 docs/04 §5.1）。
        let id = HeuristicIdentifier;
        let guesses = id.identify("ｃａｔ", 3);
        assert_eq!(guesses[0].language.as_str(), "en");
    }

    #[test]
    fn limit参数生效() {
        assert_eq!(HeuristicIdentifier.identify("你好", 1).len(), 1);
        assert_eq!(HeuristicIdentifier.identify("你好", 0).len(), 0);
    }
}
