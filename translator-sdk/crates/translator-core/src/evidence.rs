//! 共享语言证据（P17-A / A1）。
//!
//! 这个模块刻意保持很小：它只承载**词典、规则、Planner 和执行层都需要看到**的
//! 事实，不试图变成自然语言 AST。事实分为四类：
//!
//! - 原文位置（[`TextSpan`]）；
//! - 词义候选（[`SenseCandidate`]，来自词典 provider）；
//! - 模式命中（[`PatternMatch`]，模板/短语/术语）；
//! - 翻译约束（[`TranslationConstraint`]，数字、占位符、术语等）。
//!
//! 约束是“必须满足”的条件，不是提示。它们可以进入缓存身份之外的诊断，
//! 但不会改变译文；真正执行约束的仍是保护/术语/质量检查层。

use std::fmt;

use translator_types::TextKind;

/// 规范化原文中的字节区间。`end` 为开区间。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
}

impl TextSpan {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }

    /// 从原文切片；越界/非 UTF-8 边界返回 `None`，绝不 panic。
    pub fn slice(self, text: &str) -> Option<&str> {
        if self.start > self.end || self.end > text.len() || !text.is_char_boundary(self.start) {
            return None;
        }
        if !text.is_char_boundary(self.end) {
            return None;
        }
        Some(&text[self.start..self.end])
    }
}

impl fmt::Display for TextSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// 规划层的单元类型。比 [`TextKind`] 更细：一个 Sentence 单元可以是 Clause。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnitKind {
    Word,
    Phrase,
    Clause,
    Sentence,
    Paragraph,
    Literal,
}

impl UnitKind {
    pub const fn from_text_kind(kind: TextKind, is_literal: bool) -> Self {
        if is_literal {
            return Self::Literal;
        }
        match kind {
            TextKind::Word => Self::Word,
            TextKind::Sentence => Self::Sentence,
            TextKind::Paragraph => Self::Paragraph,
            TextKind::Document | TextKind::Auto => Self::Sentence,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Phrase => "phrase",
            Self::Clause => "clause",
            Self::Sentence => "sentence",
            Self::Paragraph => "paragraph",
            Self::Literal => "literal",
        }
    }
}

impl fmt::Display for UnitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 词典词义候选。只保留做决策需要的字段，不复制完整 `DictionaryEntry`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SenseCandidate {
    pub headword: String,
    pub definition: String,
    pub part_of_speech: Option<String>,
    pub source_id: String,
    /// 0–100 的启发式置信度；没有可靠依据时保持较低值。
    pub confidence: u8,
}

impl SenseCandidate {
    pub fn summary(&self) -> String {
        match &self.part_of_speech {
            Some(pos) => format!("{} ({pos}) {}", self.headword, self.definition),
            None => format!("{} {}", self.headword, self.definition),
        }
    }
}

/// 模式命中类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PatternKind {
    /// 完整句子模板。
    Template,
    /// 固定短语/搭配。
    Phrase,
    /// 术语约束。
    Term,
    /// 规则包中的应用规则。
    Rule,
}

impl PatternKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::Phrase => "phrase",
            Self::Term => "term",
            Self::Rule => "rule",
        }
    }
}

/// 已命中的语言模式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternMatch {
    pub kind: PatternKind,
    pub id: String,
    pub span: TextSpan,
    /// 是否完整覆盖该单元。只有完整覆盖才允许规则直接产出。
    pub complete: bool,
}

/// 必须保留的翻译约束。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranslationConstraint {
    PreserveNumbers,
    PreservePlaceholders,
    PreserveUrl,
    PreserveCode,
    /// 术语表身份；执行层必须让术语检查看到它。
    Terminology {
        id: String,
    },
    /// 调用方提供的 glossary 身份。
    Glossary {
        id: String,
    },
    /// 规则/模板要求完整覆盖，不允许部分匹配。
    CompleteCoverage {
        pattern_id: String,
    },
}

impl TranslationConstraint {
    pub fn summary(&self) -> String {
        match self {
            Self::PreserveNumbers => "preserve:numbers".to_string(),
            Self::PreservePlaceholders => "preserve:placeholders".to_string(),
            Self::PreserveUrl => "preserve:url".to_string(),
            Self::PreserveCode => "preserve:code".to_string(),
            Self::Terminology { id } => format!("terminology:{id}"),
            Self::Glossary { id } => format!("glossary:{id}"),
            Self::CompleteCoverage { pattern_id } => {
                format!("complete-coverage:{pattern_id}")
            }
        }
    }
}

/// 上下文引用。原文本身不进证据对象（诊断可能被记录），只保留身份与长度。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextRef {
    pub id: String,
    pub chars: usize,
}

/// 一个单元的共享语言证据。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanguageEvidence {
    pub span: TextSpan,
    pub kind: Option<UnitKind>,
    pub lexical_candidates: Vec<SenseCandidate>,
    pub pattern_matches: Vec<PatternMatch>,
    pub constraints: Vec<TranslationConstraint>,
    pub context: Option<ContextRef>,
    /// 词典 provider 是否对该语言对提供了候选（不代表一定命中词条）。
    pub dictionary_available: bool,
}

impl LanguageEvidence {
    pub fn for_text(span: TextSpan, kind: UnitKind, text: &str) -> Self {
        let mut evidence = Self {
            span,
            kind: Some(kind),
            ..Self::default()
        };
        evidence.add_text_constraints(text);
        evidence
    }

    /// 从文本本身可确定的约束。这里使用与保护层相同的 `protected_spans`，
    /// 因此词典/规则/执行层看到的是同一组事实。
    pub fn add_text_constraints(&mut self, text: &str) {
        use crate::text::{protected_spans, ProtectedKind};

        if text.chars().any(|c| c.is_ascii_digit()) {
            push_unique(
                &mut self.constraints,
                TranslationConstraint::PreserveNumbers,
            );
        }
        for span in protected_spans(text) {
            let constraint = match span.kind {
                ProtectedKind::Url => TranslationConstraint::PreserveUrl,
                ProtectedKind::Email => TranslationConstraint::PreservePlaceholders,
                ProtectedKind::TemplateVar => TranslationConstraint::PreservePlaceholders,
                ProtectedKind::Code => TranslationConstraint::PreserveCode,
                ProtectedKind::Number => TranslationConstraint::PreserveNumbers,
            };
            push_unique(&mut self.constraints, constraint);
        }
    }

    pub fn add_senses(&mut self, senses: impl IntoIterator<Item = SenseCandidate>) {
        for sense in senses {
            if self
                .lexical_candidates
                .iter()
                .any(|s| s.headword == sense.headword && s.definition == sense.definition)
            {
                continue;
            }
            self.lexical_candidates.push(sense);
            if self.lexical_candidates.len() >= 8 {
                break;
            }
        }
    }

    pub fn add_pattern(&mut self, pattern: PatternMatch) {
        if pattern.complete {
            push_unique(
                &mut self.constraints,
                TranslationConstraint::CompleteCoverage {
                    pattern_id: pattern.id.clone(),
                },
            );
        }
        self.pattern_matches.push(pattern);
    }

    pub fn has_pattern(&self, kind: PatternKind) -> bool {
        self.pattern_matches.iter().any(|p| p.kind == kind)
    }

    pub fn has_complete_pattern(&self) -> bool {
        self.pattern_matches.iter().any(|p| p.complete)
    }

    pub fn has_constraint(&self, wanted: &TranslationConstraint) -> bool {
        self.constraints.iter().any(|c| c == wanted)
    }

    /// 供诊断/CLI 使用的人类可读摘要。**不包含原文**。
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "span={} kind={} senses={} patterns={} constraints={} dictionary={}",
            self.span,
            self.kind.map(|k| k.as_str()).unwrap_or("none"),
            self.lexical_candidates.len(),
            self.pattern_matches.len(),
            self.constraints.len(),
            self.dictionary_available
        )];
        if let Some(context) = &self.context {
            parts.push(format!("context={} chars={}", context.id, context.chars));
        }
        parts.join(" ")
    }
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, value: T) {
    if !items.contains(&value) {
        items.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span越界与字符边界都安全() {
        let text = "你好 world";
        assert_eq!(TextSpan::new(0, 6).slice(text), Some("你好"));
        assert_eq!(TextSpan::new(0, 1).slice(text), None, "必须落在 UTF-8 边界");
        assert_eq!(TextSpan::new(1, 99).slice(text), None);
    }

    #[test]
    fn 文本约束来自保护层() {
        let ev = LanguageEvidence::for_text(
            TextSpan::new(0, 30),
            UnitKind::Sentence,
            "see https://example.com and 42",
        );
        assert!(ev.has_constraint(&TranslationConstraint::PreserveUrl));
        assert!(ev.has_constraint(&TranslationConstraint::PreserveNumbers));
    }

    #[test]
    fn 完整模式才要求完整覆盖() {
        let mut ev = LanguageEvidence::default();
        ev.add_pattern(PatternMatch {
            kind: PatternKind::Template,
            id: "t".into(),
            span: TextSpan::new(0, 3),
            complete: false,
        });
        assert!(!ev.has_complete_pattern());
        assert!(ev.constraints.is_empty());
    }
}
