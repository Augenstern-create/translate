//! P9.3-A：轻量英语词形还原（规则 + 例外词表）。
//!
//! 只产生**候选 lemma**，不修改用户原文；调用方应把候选交给 `DictionaryProvider`
//! 查询，而不是用 lemma 替换整句。覆盖常见规则与高频不规则词，不做完整形态学。
use std::collections::HashMap;

/// 可选形态分析器契约。
pub trait MorphologyAnalyzer: Send + Sync {
    /// 返回候选 lemma（去重、稳定顺序）。
    fn lemmas(&self, word: &str) -> Vec<String>;
    fn name(&self) -> &'static str;
}

/// 英语轻量 lemmatizer。
#[derive(Debug, Default, Clone)]
pub struct EnglishLemmatizer {
    irregular: HashMap<&'static str, &'static str>,
}

impl EnglishLemmatizer {
    pub fn new() -> Self {
        let mut irregular = HashMap::new();
        for (form, lemma) in [
            ("went", "go"),
            ("gone", "go"),
            ("children", "child"),
            ("mice", "mouse"),
            ("men", "man"),
            ("women", "woman"),
            ("teeth", "tooth"),
            ("feet", "foot"),
            ("better", "good"),
            ("best", "good"),
            ("worse", "bad"),
            ("worst", "bad"),
            ("was", "be"),
            ("were", "be"),
            ("been", "be"),
            ("has", "have"),
            ("had", "have"),
            ("did", "do"),
            ("done", "do"),
            ("saw", "see"),
            ("seen", "see"),
            ("made", "make"),
            ("took", "take"),
            ("taken", "take"),
            ("gave", "give"),
            ("given", "give"),
            ("found", "find"),
            ("thought", "think"),
            ("bought", "buy"),
            ("brought", "bring"),
            ("caught", "catch"),
            ("taught", "teach"),
        ] {
            irregular.insert(form, lemma);
        }
        Self { irregular }
    }

    fn rule_candidates(lower: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut push = |s: String| {
            if !s.is_empty() && !out.contains(&s) {
                out.push(s);
            }
        };

        if lower.ends_with("ies") && lower.len() > 4 {
            push(format!("{}y", &lower[..lower.len() - 3]));
        }
        if lower.ends_with("ves") && lower.len() > 4 {
            // wolves -> wolf / knives -> knife（两类都作为候选）
            push(format!("{}f", &lower[..lower.len() - 3]));
            push(format!("{}fe", &lower[..lower.len() - 3]));
        }
        if lower.ends_with("es") && lower.len() > 3 {
            push(lower[..lower.len() - 2].to_string());
            push(lower[..lower.len() - 1].to_string());
        }
        if lower.ends_with('s')
            && !lower.ends_with("ss")
            && !lower.ends_with("us")
            && !lower.ends_with("is")
            && lower.len() > 2
        {
            push(lower[..lower.len() - 1].to_string());
        }

        for suffix in ["ing", "ed"] {
            if lower.ends_with(suffix) && lower.len() > suffix.len() + 1 {
                let stem = &lower[..lower.len() - suffix.len()];
                push(stem.to_string());
                push(format!("{stem}e"));
                // running -> run, stopped -> stop（双写辅音）
                let chars: Vec<char> = stem.chars().collect();
                if chars.len() >= 2 {
                    let last = chars[chars.len() - 1];
                    let prev = chars[chars.len() - 2];
                    if last == prev && !matches!(last, 'a' | 'e' | 'i' | 'o' | 'u') {
                        push(chars[..chars.len() - 1].iter().collect());
                    }
                }
            }
        }

        out
    }
}

impl MorphologyAnalyzer for EnglishLemmatizer {
    fn lemmas(&self, word: &str) -> Vec<String> {
        let lower = word.trim().to_lowercase();
        if lower.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(lemma) = self.irregular.get(lower.as_str()) {
            out.push((*lemma).to_string());
        }
        for candidate in Self::rule_candidates(&lower) {
            if candidate != lower && !out.contains(&candidate) {
                out.push(candidate);
            }
        }
        out
    }

    fn name(&self) -> &'static str {
        "english-rule-lemmatizer-v1"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 规则与不规则词形() {
        let l = EnglishLemmatizer::new();
        for (word, expected) in [
            ("running", "run"),
            ("walked", "walk"),
            ("children", "child"),
            ("mice", "mouse"),
            ("went", "go"),
            ("studies", "study"),
            ("boxes", "box"),
            ("making", "make"),
            ("stopped", "stop"),
        ] {
            assert!(
                l.lemmas(word).iter().any(|x| x == expected),
                "{word} -> {:?}，缺 {expected}",
                l.lemmas(word)
            );
        }
        assert!(l.lemmas("better").contains(&"good".to_string()));
        // 不做过度的去 s：glass / bus 不应变成 glas/bu
        assert!(!l.lemmas("glass").iter().any(|x| x == "glas"));
        assert!(!l.lemmas("bus").iter().any(|x| x == "bu"));
        assert!(l.lemmas("").is_empty());
    }
}
