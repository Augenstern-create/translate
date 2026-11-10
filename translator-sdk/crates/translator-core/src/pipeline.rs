//! docs/80：首个纵向样本的真实数据流。
//!
//! Source → Lossless TokenStream → forward SyntaxGraph → Semantic Transfer /
//! MeaningGraph → Target Transfer → ChineseSyntaxTree → Surface Realizer →
//! structured validation. 该模块不调用 `parse_*`，也不从 MeaningFrame 反向
//! 制造句法图。当前只接管经过词法、句法证据确认的简单陈述/否定/时间结构；
//! 未覆盖输入返回 None，由旧兼容路径或 NMT 处理。

use std::collections::HashMap;

use translator_types::DictionaryEntry;

use crate::composer::CompositionEngine;
use crate::evidence::{LanguageEvidence, PatternKind, PatternMatch, TextSpan, UnitKind};
use crate::semantic::SemanticOutcome;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Word,
    Number,
    Punctuation,
    Symbol,
    Whitespace,
    Protected,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceToken {
    pub id: usize,
    pub span: TextSpan,
    pub surface: String,
    pub kind: TokenKind,
    pub derived_from: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverageState {
    Covered,
    Protected,
    Ignorable,
    Unresolved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageReport {
    pub complete: bool,
    pub states: Vec<(TextSpan, CoverageState)>,
}

#[derive(Clone, Debug)]
pub struct TokenStream {
    pub source: String,
    pub tokens: Vec<SourceToken>,
    pub coverage: CoverageReport,
}

pub fn tokenize_lossless(source: &str) -> TokenStream {
    let mut tokens = Vec::new();
    let mut states = Vec::new();
    let mut i = 0usize;
    while i < source.len() {
        let ch = source[i..].chars().next().expect("valid char boundary");
        let start = i;
        i += ch.len_utf8();
        if ch.is_whitespace() {
            tokens.push(SourceToken {
                id: tokens.len(),
                span: TextSpan::new(start, i),
                surface: ch.to_string(),
                kind: TokenKind::Whitespace,
                derived_from: None,
            });
            states.push((TextSpan::new(start, i), CoverageState::Ignorable));
            continue;
        }
        if ch.is_ascii_alphabetic() {
            while i < source.len() {
                let c = source[i..].chars().next().expect("valid char boundary");
                if c.is_ascii_alphabetic() || c == '\'' || c == '-' {
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
            let raw = &source[start..i];
            let raw_id = tokens.len();
            if let Some(expanded) = expand_contraction(raw) {
                for word in expanded {
                    tokens.push(SourceToken {
                        id: tokens.len(),
                        span: TextSpan::new(start, i),
                        surface: word,
                        kind: TokenKind::Word,
                        derived_from: Some(raw_id),
                    });
                }
            } else {
                tokens.push(SourceToken {
                    id: raw_id,
                    span: TextSpan::new(start, i),
                    surface: raw.to_string(),
                    kind: TokenKind::Word,
                    derived_from: None,
                });
            }
            states.push((TextSpan::new(start, i), CoverageState::Covered));
            continue;
        }
        if ch.is_ascii_digit() {
            while i < source.len() {
                let c = source[i..].chars().next().expect("valid char boundary");
                if c.is_ascii_digit() || c == '.' || c == ',' {
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
            tokens.push(SourceToken {
                id: tokens.len(),
                span: TextSpan::new(start, i),
                surface: source[start..i].to_string(),
                kind: TokenKind::Number,
                derived_from: None,
            });
            states.push((TextSpan::new(start, i), CoverageState::Unresolved));
            continue;
        }
        if matches!(
            ch,
            '.' | ','
                | '?'
                | '!'
                | ';'
                | ':'
                | '\''
                | '"'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '-'
        ) {
            tokens.push(SourceToken {
                id: tokens.len(),
                span: TextSpan::new(start, i),
                surface: ch.to_string(),
                kind: TokenKind::Punctuation,
                derived_from: None,
            });
            states.push((TextSpan::new(start, i), CoverageState::Ignorable));
            continue;
        }
        tokens.push(SourceToken {
            id: tokens.len(),
            span: TextSpan::new(start, i),
            surface: ch.to_string(),
            kind: TokenKind::Symbol,
            derived_from: None,
        });
        states.push((TextSpan::new(start, i), CoverageState::Unresolved));
    }
    let complete = !states
        .iter()
        .any(|(_, state)| matches!(state, CoverageState::Unresolved));
    TokenStream {
        source: source.to_string(),
        tokens,
        coverage: CoverageReport { complete, states },
    }
}

fn expand_contraction(raw: &str) -> Option<Vec<String>> {
    let lower = raw.to_ascii_lowercase();
    let pairs = match lower.as_str() {
        "didn't" => Some(("did", "not")),
        "doesn't" => Some(("does", "not")),
        "don't" => Some(("do", "not")),
        "isn't" => Some(("is", "not")),
        "aren't" => Some(("are", "not")),
        "wasn't" => Some(("was", "not")),
        "weren't" => Some(("were", "not")),
        "can't" => Some(("can", "not")),
        "won't" => Some(("will", "not")),
        "i'm" => Some(("i", "am")),
        "i'll" => Some(("i", "will")),
        "i've" => Some(("i", "have")),
        "i'd" => Some(("i", "would")),
        "you're" => Some(("you", "are")),
        "we're" => Some(("we", "are")),
        "they're" => Some(("they", "are")),
        "he's" => Some(("he", "is")),
        "she's" => Some(("she", "is")),
        "it's" => Some(("it", "is")),
        _ => None,
    };
    pairs.map(|(a, b)| vec![a.to_string(), b.to_string()])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyntaxNodeKind {
    Clause,
    NounPhrase,
    VerbPhrase,
    Token,
    Quantity,
    Modifier,
    Adverbial,
    Negation,
    PrepositionPhrase,
    Coordination,
    PpModifier,
    Relative,
    ClauseCoordination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyntaxRelation {
    Subject,
    Object,
    Predicate,
    Modifier,
    Quantity,
    Adverbial,
    Negation,
    Auxiliary,
    Agent,
    Complement,
    Head,
    Determiner,
    Preposition,
    PpModifier,
    Conjunct,
    Conjunction,
    Relative,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxNode {
    pub id: usize,
    pub span: TextSpan,
    pub kind: SyntaxNodeKind,
    pub surface: String,
    pub lemma: String,
    pub pos: String,
    pub past: bool,
    pub coord_heads: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxEdge {
    pub head: usize,
    pub dependent: usize,
    pub relation: SyntaxRelation,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyntaxGraph {
    pub nodes: Vec<SyntaxNode>,
    pub edges: Vec<SyntaxEdge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Np {
    Pronoun {
        token: usize,
        span: TextSpan,
        surface: String,
        consumed: usize,
    },
    Noun {
        token: usize,
        span: TextSpan,
        surface: String,
        quantity: Option<(usize, u32, TextSpan)>,
        adjective: Option<(usize, TextSpan, String)>,
        classifier: Option<String>,
        plural: bool,
        consumed: usize,
    },
}

#[derive(Clone, Debug)]
pub struct SyntaxAnalysis {
    pub stream: TokenStream,
    pub graph: SyntaxGraph,
    pub subject: Np,
    pub predicate: (usize, TextSpan, String),
    pub object: Option<Np>,
    pub negated: bool,
    pub neg_span: Option<TextSpan>,
    pub past: bool,
    pub progressive: bool,
    pub modal: Option<String>,
    pub complement: Option<(usize, TextSpan, String)>,
    pub place: Option<(TextSpan, String)>,
    pub future: bool,
    pub passive: bool,
    pub by_agent: Option<Np>,
    pub time: Option<(usize, TextSpan, String, String)>,
}

#[derive(Clone, Debug)]
pub struct SyntaxAnalyzer<'a> {
    pub lexicon: &'a CompositionEngine,
}

impl<'a> SyntaxAnalyzer<'a> {
    pub fn analyze(&self, source: &str) -> Option<SyntaxAnalysis> {
        let stream = tokenize_lossless(source);
        if !stream.coverage.complete {
            return None;
        }
        let words: Vec<&SourceToken> = stream
            .tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Word | TokenKind::Number))
            .collect();
        if words.is_empty() {
            return None;
        }
        if words.iter().any(|word| {
            let surface = word.surface.to_ascii_lowercase();
            self.lexicon.lexicon_has_pos(&surface, "adverb")
                && !matches!(
                    surface.as_str(),
                    "yesterday" | "today" | "tomorrow" | "not" | "home" | "here" | "there"
                )
        }) {
            return None;
        }
        let subject = self.np(&words, 0)?;
        let mut i = subject_len(&subject);
        let mut negated = false;
        let mut neg_span = None;
        let mut past = false;
        let mut progressive = false;
        let mut future = false;
        let mut passive = false;
        let mut modal = None;
        let mut aux_span = None;
        let mut aux_surface = String::new();
        if let Some(aux) = words.get(i) {
            if matches!(aux.surface.as_str(), "was" | "were") {
                passive = true;
                aux_span = Some(aux.span);
                aux_surface = aux.surface.to_ascii_lowercase();
                i += 1;
            } else if matches!(aux.surface.as_str(), "is" | "are" | "am") {
                progressive = true;
                aux_span = Some(aux.span);
                aux_surface = aux.surface.to_ascii_lowercase();
                i += 1;
            } else if aux.surface == "will" {
                future = true;
                aux_span = Some(aux.span);
                aux_surface = "will".to_string();
                i += 1;
            } else if aux.surface == "can" {
                modal = Some("can".to_string());
                aux_span = Some(aux.span);
                aux_surface = "can".to_string();
                i += 1;
            } else if matches!(aux.surface.as_str(), "did" | "does" | "do") {
                i += 1;
                if words.get(i).map(|t| t.surface.as_str()) == Some("not") {
                    negated = true;
                    neg_span = words.get(i).map(|t| t.span);
                    i += 1;
                }
                past = aux.surface == "did";
            }
        }
        let verb = words.get(i)?;
        let verb_surface = verb.surface.to_ascii_lowercase();
        let lemma = self.lexicon.verb_lemma(&verb_surface)?;
        if !self.lexicon.lexicon_has_pos(&verb_surface, "verb")
            && !self.lexicon.lexicon_has_pos(&lemma, "verb")
        {
            return None;
        }
        self.lexicon.zh(&lemma, "verb")?;
        if progressive && !verb_surface.ends_with("ing") {
            return None;
        }
        past = past || self.lexicon.is_past_form(&verb_surface, &lemma);
        let mut complement = None;
        if lemma == "want" && words.get(i + 1).map(|w| w.surface.as_str()) == Some("to") {
            let target = words.get(i + 2)?;
            let target_lower = target.surface.to_ascii_lowercase();
            let target_lemma = self.lexicon.verb_lemma(&target_lower)?;
            if !self.lexicon.lexicon_has_pos(&target_lower, "verb")
                && !self.lexicon.lexicon_has_pos(&target_lemma, "verb")
            {
                return None;
            }
            self.lexicon.zh(&target_lemma, "verb")?;
            complement = Some((i + 2, target.span, target_lemma));
            i += 2;
        }
        let predicate = (
            i.saturating_sub(if complement.is_some() { 2 } else { 0 }),
            verb.span,
            lemma.clone(),
        );
        i += 1;
        let mut by_agent = None;
        let mut place = None;
        let object = if passive {
            if words.get(i).map(|w| w.surface.as_str()) == Some("by") {
                let agent = self.np(&words, i + 1)?;
                i = i + 1 + subject_len(&agent);
                by_agent = Some(agent);
            }
            None
        } else if let Some((span, surface)) = self.place(&words, i) {
            place = Some((span, surface));
            i += 1;
            None
        } else {
            self.np(&words, i)
        };
        if let Some(object) = &object {
            i += subject_len(object);
        }
        let mut time = None;
        if let Some(token) = words.get(i) {
            let surface = token.surface.to_ascii_lowercase();
            if self.lexicon.lexicon_has_pos(&surface, "adverb") {
                let zh = self.lexicon.zh(&surface, "adverb")?.to_string();
                time = Some((i, token.span, surface, zh));
                i += 1;
            }
        }
        if i != words.len() {
            return None;
        }
        let mut graph = SyntaxGraph::default();
        let clause = push(
            &mut graph,
            SyntaxNodeKind::Clause,
            TextSpan::new(0, source.len()),
            "clause",
        );
        let subj = push_np(&mut graph, &subject);
        graph.edges.push(SyntaxEdge {
            head: clause,
            dependent: subj,
            relation: SyntaxRelation::Subject,
        });
        let pred = push(&mut graph, SyntaxNodeKind::VerbPhrase, verb.span, lemma);
        graph.edges.push(SyntaxEdge {
            head: clause,
            dependent: pred,
            relation: SyntaxRelation::Predicate,
        });
        if let Some((_, span, surface)) = &complement {
            let c = push(
                &mut graph,
                SyntaxNodeKind::VerbPhrase,
                *span,
                surface.clone(),
            );
            graph.edges.push(SyntaxEdge {
                head: pred,
                dependent: c,
                relation: SyntaxRelation::Complement,
            });
        }
        if progressive || modal.is_some() {
            let aux = push(
                &mut graph,
                SyntaxNodeKind::VerbPhrase,
                aux_span.unwrap_or_default(),
                aux_surface,
            );
            graph.edges.push(SyntaxEdge {
                head: pred,
                dependent: aux,
                relation: SyntaxRelation::Auxiliary,
            });
        }
        if negated {
            let neg = push(
                &mut graph,
                SyntaxNodeKind::Negation,
                neg_span.unwrap_or_default(),
                "not",
            );
            graph.edges.push(SyntaxEdge {
                head: pred,
                dependent: neg,
                relation: SyntaxRelation::Negation,
            });
        }
        if let Some(object) = &object {
            let obj = push_np(&mut graph, object);
            graph.edges.push(SyntaxEdge {
                head: pred,
                dependent: obj,
                relation: SyntaxRelation::Object,
            });
        }
        if let Some(agent) = &by_agent {
            let node = push_np(&mut graph, agent);
            graph.edges.push(SyntaxEdge {
                head: pred,
                dependent: node,
                relation: SyntaxRelation::Agent,
            });
        }
        if let Some((span, surface)) = &place {
            let node = push(
                &mut graph,
                SyntaxNodeKind::Adverbial,
                *span,
                surface.clone(),
            );
            graph.edges.push(SyntaxEdge {
                head: pred,
                dependent: node,
                relation: SyntaxRelation::Adverbial,
            });
        }
        if let Some((_, span, surface, _)) = &time {
            let adv = push(
                &mut graph,
                SyntaxNodeKind::Adverbial,
                *span,
                surface.clone(),
            );
            graph.edges.push(SyntaxEdge {
                head: pred,
                dependent: adv,
                relation: SyntaxRelation::Adverbial,
            });
        }
        Some(SyntaxAnalysis {
            stream,
            graph,
            subject,
            predicate,
            object,
            negated,
            neg_span,
            past,
            progressive,
            modal,
            complement,
            place,
            future,
            passive,
            by_agent,
            time,
        })
    }

    fn place(&self, words: &[&SourceToken], i: usize) -> Option<(TextSpan, String)> {
        let surface = words.get(i)?.surface.to_ascii_lowercase();
        if matches!(surface.as_str(), "home" | "here" | "there") {
            Some((words[i].span, surface))
        } else {
            None
        }
    }

    fn np(&self, words: &[&SourceToken], start: usize) -> Option<Np> {
        let token = words.get(start)?;
        let surface = token.surface.to_ascii_lowercase();
        if self.lexicon.lexicon_has_pos(&surface, "pronoun") {
            return Some(Np::Pronoun {
                token: start,
                span: token.span,
                surface,
                consumed: 1,
            });
        }
        let mut i = start;
        if matches!(
            surface.as_str(),
            "a" | "an"
                | "the"
                | "my"
                | "your"
                | "his"
                | "her"
                | "our"
                | "their"
                | "this"
                | "that"
                | "these"
                | "those"
        ) {
            let article_quantity =
                matches!(surface.as_str(), "a" | "an").then_some((start, 1, token.span));
            i += 1;
            let mut quantity = article_quantity;
            if let Some(token) = words.get(i) {
                if let Some(value) = numeral(&token.surface) {
                    quantity = Some((i, value, token.span));
                    i += 1;
                }
            }
            let mut adjective = None;
            if let Some(token) = words.get(i) {
                if self
                    .lexicon
                    .adjective_lemma(&token.surface.to_ascii_lowercase())
                    .is_some()
                {
                    adjective = Some((i, token.span, token.surface.to_ascii_lowercase()));
                    i += 1;
                }
            }
            let token = words.get(i)?;
            let surface = token.surface.to_ascii_lowercase();
            let lemma = self.lexicon.noun_lemma(&surface)?;
            if !self.lexicon.lexicon_has_pos(&surface, "noun")
                && !self.lexicon.lexicon_has_pos(&lemma, "noun")
            {
                return None;
            }
            self.lexicon.zh(&lemma, "noun")?;
            return Some(Np::Noun {
                token: i,
                span: TextSpan::new(words[start].span.start, token.span.end),
                surface: lemma.clone(),
                quantity,
                adjective,
                classifier: self.lexicon.classifier(&lemma).map(str::to_string),
                plural: token.surface.ends_with('s') && token.surface.to_ascii_lowercase() != lemma,
                consumed: i - start + 1,
            });
        }
        let mut quantity = None;
        if let Some(token) = words.get(i) {
            if let Some(value) = numeral(&token.surface) {
                quantity = Some((i, value, token.span));
                i += 1;
            }
        }
        let mut adjective = None;
        if let Some(token) = words.get(i) {
            if self
                .lexicon
                .adjective_lemma(&token.surface.to_ascii_lowercase())
                .is_some()
            {
                adjective = Some((i, token.span, token.surface.to_ascii_lowercase()));
                i += 1;
            }
        }
        let token = words.get(i)?;
        let surface = token.surface.to_ascii_lowercase();
        let lemma = self.lexicon.noun_lemma(&surface)?;
        if !self.lexicon.lexicon_has_pos(&surface, "noun")
            && !self.lexicon.lexicon_has_pos(&lemma, "noun")
        {
            return None;
        }
        self.lexicon.zh(&lemma, "noun")?;
        Some(Np::Noun {
            token: i,
            span: TextSpan::new(words[start].span.start, token.span.end),
            surface: lemma.clone(),
            quantity,
            adjective,
            classifier: self.lexicon.classifier(&lemma).map(str::to_string),
            plural: token.surface.ends_with('s') && token.surface.to_ascii_lowercase() != lemma,
            consumed: i - start + 1,
        })
    }
}

fn subject_len(np: &Np) -> usize {
    match np {
        Np::Pronoun { consumed, .. } | Np::Noun { consumed, .. } => *consumed,
    }
}

fn numeral(word: &str) -> Option<u32> {
    match word {
        "one" | "a" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        "2" => Some(2),
        "3" => Some(3),
        _ => None,
    }
}

fn push(
    graph: &mut SyntaxGraph,
    kind: SyntaxNodeKind,
    span: TextSpan,
    surface: impl Into<String>,
) -> usize {
    let id = graph.nodes.len();
    let surface = surface.into();
    graph.nodes.push(SyntaxNode {
        id,
        span,
        kind,
        lemma: surface.clone(),
        pos: String::new(),
        past: false,
        coord_heads: Vec::new(),
        surface,
    });
    id
}

fn push_np(graph: &mut SyntaxGraph, np: &Np) -> usize {
    match np {
        Np::Pronoun { span, surface, .. } => {
            push(graph, SyntaxNodeKind::NounPhrase, *span, surface.clone())
        }
        Np::Noun {
            span,
            surface,
            quantity,
            adjective,
            ..
        } => {
            let id = push(graph, SyntaxNodeKind::NounPhrase, *span, surface.clone());
            if let Some((_, _, qspan)) = quantity {
                let q = push(graph, SyntaxNodeKind::Quantity, *qspan, "quantity");
                graph.edges.push(SyntaxEdge {
                    head: id,
                    dependent: q,
                    relation: SyntaxRelation::Quantity,
                });
            }
            if let Some((_, aspan, _)) = adjective {
                let a = push(graph, SyntaxNodeKind::Modifier, *aspan, "modifier");
                graph.edges.push(SyntaxEdge {
                    head: id,
                    dependent: a,
                    relation: SyntaxRelation::Modifier,
                });
            }
            id
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SenseResolution {
    Resolved(String),
    Ambiguous(Vec<String>),
    Unresolved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeaningNodeKind {
    Event,
    Entity,
    Property,
    Operator,
    Time,
    Preposition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ChartCategory {
    Clause,
    Np,
    Vp,
    N,
    V,
    Adj,
    Adv,
    Det,
    Num,
    Aux,
    Neg,
    P,
    Pp,
    Conj,
    Coord,
    Inf,
    Rel,
}

#[derive(Clone, Debug)]
pub struct ChartItem {
    pub id: usize,
    pub start: usize,
    pub end: usize,
    pub category: ChartCategory,
    pub rule: &'static str,
    pub children: Vec<usize>,
    pub head: String,
    pub lemma: String,
    pub pos: String,
    pub negated: bool,
    pub past: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Chart {
    pub items: Vec<ChartItem>,
    by_span: HashMap<(usize, usize, ChartCategory), Vec<usize>>,
}

impl Chart {
    fn add(&mut self, item: ChartItem) -> bool {
        let key = (item.start, item.end, item.category);
        let candidates = self.by_span.entry(key).or_default();
        if candidates.iter().any(|id| {
            let existing = &self.items[*id];
            existing.rule == item.rule
                && existing.children == item.children
                && existing.head == item.head
                && existing.lemma == item.lemma
                && existing.pos == item.pos
                && existing.negated == item.negated
                && existing.past == item.past
        }) {
            return false;
        }
        let id = self.items.len();
        self.items.push(ChartItem { id, ..item });
        candidates.push(id);
        true
    }

    fn get(&self, start: usize, end: usize, category: ChartCategory) -> Option<&ChartItem> {
        self.by_span
            .get(&(start, end, category))
            .and_then(|ids| ids.first())
            .and_then(|id| self.items.get(*id))
    }

    pub fn candidates(&self, start: usize, end: usize, category: ChartCategory) -> Vec<&ChartItem> {
        self.by_span
            .get(&(start, end, category))
            .map(|ids| ids.iter().filter_map(|id| self.items.get(*id)).collect())
            .unwrap_or_default()
    }
}

/// 基于跨度组合的 chart parser，而不是线性句型扫描。
pub struct ChartParser<'a> {
    pub lexicon: &'a CompositionEngine,
}

impl<'a> ChartParser<'a> {
    pub fn parse(&self, stream: &TokenStream) -> Option<SyntaxGraph> {
        self.parse_candidates(stream).into_iter().next()
    }

    pub fn parse_candidates(&self, stream: &TokenStream) -> Vec<SyntaxGraph> {
        let Some(chart) = self.build(stream) else {
            return Vec::new();
        };
        let words: Vec<&SourceToken> = stream
            .tokens
            .iter()
            .filter(|token| matches!(token.kind, TokenKind::Word | TokenKind::Number))
            .collect();
        let mut clauses = chart.candidates(0, words.len(), ChartCategory::Clause);
        clauses.sort_by_key(|item| std::cmp::Reverse(chart_rule_score(item.rule)));
        clauses
            .into_iter()
            .map(|clause| {
                let mut graph = SyntaxGraph::default();
                chart_item_to_graph(&chart, clause.id, &words, &mut graph);
                // If the parser produced a coordinated NP but the Clause subject
                // still points at an unmerged child, reconnect the subject edge.
                if let Some(coord) = graph
                    .nodes
                    .iter()
                    .find(|node| !node.coord_heads.is_empty() && node.span.start == 0)
                    .map(|node| node.id)
                {
                    if let Some(clause_node) = graph
                        .nodes
                        .iter()
                        .find(|node| node.kind == SyntaxNodeKind::Clause)
                        .map(|node| node.id)
                    {
                        if let Some(edge) = graph.edges.iter_mut().find(|edge| {
                            edge.head == clause_node && edge.relation == SyntaxRelation::Subject
                        }) {
                            edge.dependent = coord;
                        }
                    }
                }
                graph
            })
            .collect()
    }

    pub fn parse_chart(&self, stream: &TokenStream) -> Option<Chart> {
        self.build(stream)
    }

    fn build(&self, stream: &TokenStream) -> Option<Chart> {
        if !stream.coverage.complete {
            return None;
        }
        let words: Vec<&SourceToken> = stream
            .tokens
            .iter()
            .filter(|token| matches!(token.kind, TokenKind::Word | TokenKind::Number))
            .collect();
        if words.is_empty() {
            return None;
        }
        for i in 0..words.len().saturating_sub(1) {
            let aux = words[i].surface.to_ascii_lowercase();
            let next = words[i + 1].surface.to_ascii_lowercase();
            if matches!(aux.as_str(), "is" | "are" | "am")
                && self.lexicon.lexicon_has_pos(&next, "adjective")
            {
                return None;
            }
        }
        let mut chart = Chart::default();
        for (i, token) in words.iter().enumerate() {
            let surface = token.surface.to_ascii_lowercase();
            let noun_surface = surface.clone();
            let is_time = matches!(surface.as_str(), "yesterday" | "today" | "tomorrow");
            if matches!(
                surface.as_str(),
                "did" | "does" | "do" | "is" | "are" | "am" | "was" | "were" | "will" | "can"
            ) {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Aux,
                    rule: "lex.aux",
                    children: vec![],
                    head: surface.clone(),
                    lemma: surface.clone(),
                    pos: "aux".to_string(),
                    negated: false,
                    past: matches!(surface.as_str(), "did" | "was" | "were"),
                });
            }
            if matches!(surface.as_str(), "not" | "n't") {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Neg,
                    rule: "lex.neg",
                    children: vec![],
                    head: surface,
                    lemma: "not".to_string(),
                    pos: "neg".to_string(),
                    negated: true,
                    past: false,
                });
            } else if self.lexicon.lexicon_has_pos(&surface, "pronoun") {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Np,
                    rule: "np.pronoun",
                    children: vec![],
                    head: surface.clone(),
                    lemma: surface,
                    pos: "pronoun".to_string(),
                    negated: false,
                    past: false,
                });
            } else if self.lexicon.lexicon_has_pos(&surface, "determiner")
                || self.lexicon.lexicon_has_pos(&surface, "article")
            {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Det,
                    rule: "lex.det",
                    children: vec![],
                    head: surface.clone(),
                    lemma: surface,
                    pos: "determiner".to_string(),
                    negated: false,
                    past: false,
                });
            } else if let Some(value) = numeral(&surface) {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Num,
                    rule: "lex.num",
                    children: vec![],
                    head: value.to_string(),
                    lemma: value.to_string(),
                    pos: "numeral".to_string(),
                    negated: false,
                    past: false,
                });
            } else if self.lexicon.lexicon_has_pos(&surface, "verb")
                || self.lexicon.verb_lemma(&surface).is_some()
            {
                if let Some(lemma) = self.lexicon.verb_lemma(&surface) {
                    let past = self.lexicon.is_past_form(&surface, &lemma);
                    chart.add(ChartItem {
                        id: 0,
                        start: i,
                        end: i + 1,
                        category: ChartCategory::V,
                        rule: "lex.verb",
                        children: vec![],
                        head: surface.clone(),
                        lemma,
                        pos: "verb".to_string(),
                        negated: false,
                        past,
                    });
                }
            } else if self.lexicon.lexicon_has_pos(&surface, "adjective") {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Adj,
                    rule: "lex.adjective",
                    children: vec![],
                    head: surface.clone(),
                    lemma: surface,
                    pos: "adjective".to_string(),
                    negated: false,
                    past: false,
                });
            } else if is_time {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Adv,
                    rule: "lex.time",
                    children: vec![],
                    head: surface.clone(),
                    lemma: surface,
                    pos: "adverb".to_string(),
                    negated: false,
                    past: false,
                });
            }
            if noun_surface == "to" {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Inf,
                    rule: "lex.inf",
                    children: vec![],
                    head: "to".to_string(),
                    lemma: "to".to_string(),
                    pos: "inf".to_string(),
                    negated: false,
                    past: false,
                });
            }
            if matches!(noun_surface.as_str(), "who" | "that" | "which") {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Rel,
                    rule: "lex.rel",
                    children: vec![],
                    head: noun_surface.clone(),
                    lemma: noun_surface.clone(),
                    pos: "rel".to_string(),
                    negated: false,
                    past: false,
                });
            }
            if self.lexicon.lexicon_has_pos(&noun_surface, "conjunction") {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::Conj,
                    rule: "lex.conjunction",
                    children: vec![],
                    head: noun_surface.clone(),
                    lemma: noun_surface.clone(),
                    pos: "conjunction".to_string(),
                    negated: false,
                    past: false,
                });
            }
            if self.lexicon.lexicon_has_pos(&noun_surface, "preposition") {
                chart.add(ChartItem {
                    id: 0,
                    start: i,
                    end: i + 1,
                    category: ChartCategory::P,
                    rule: "lex.preposition",
                    children: vec![],
                    head: noun_surface.clone(),
                    lemma: noun_surface.clone(),
                    pos: "preposition".to_string(),
                    negated: false,
                    past: false,
                });
            }
            if let Some(lemma) = self.lexicon.noun_lemma(&noun_surface) {
                if self.lexicon.lexicon_has_pos(&noun_surface, "noun")
                    || self.lexicon.lexicon_has_pos(&lemma, "noun")
                {
                    chart.add(ChartItem {
                        id: 0,
                        start: i,
                        end: i + 1,
                        category: ChartCategory::N,
                        rule: "lex.noun",
                        children: vec![],
                        head: noun_surface.clone(),
                        lemma,
                        pos: "noun".to_string(),
                        negated: false,
                        past: false,
                    });
                }
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            // unary: N -> NP, V -> VP
            for start in 0..words.len() {
                for end in (start + 1)..=words.len() {
                    if let Some(n) = chart.get(start, end, ChartCategory::N).cloned() {
                        changed |= chart.add(ChartItem {
                            id: 0,
                            start,
                            end,
                            category: ChartCategory::Np,
                            rule: "np.noun",
                            children: vec![n.id],
                            head: n.head.clone(),
                            lemma: n.lemma.clone(),
                            pos: n.pos.clone(),
                            negated: n.negated,
                            past: n.past,
                        });
                    }
                    if let Some(v) = chart.get(start, end, ChartCategory::V).cloned() {
                        changed |= chart.add(ChartItem {
                            id: 0,
                            start,
                            end,
                            category: ChartCategory::Vp,
                            rule: "vp.verb",
                            children: vec![v.id],
                            head: v.head.clone(),
                            lemma: v.lemma.clone(),
                            pos: v.pos.clone(),
                            negated: v.negated,
                            past: v.past,
                        });
                    }
                }
            }
            if chart.items.len() > 2048 {
                return None;
            }
            // V INF VP -> VP (control complement).
            for start in 0..words.len() {
                for mid1 in (start + 1)..words.len() {
                    for mid2 in (mid1 + 1)..words.len() {
                        for end in (mid2 + 1)..=words.len() {
                            let (Some(verb), Some(inf), Some(vp)) = (
                                chart.get(start, mid1, ChartCategory::V).cloned(),
                                chart.get(mid1, mid2, ChartCategory::Inf).cloned(),
                                chart.get(mid2, end, ChartCategory::Vp).cloned(),
                            ) else {
                                continue;
                            };
                            changed |= chart.add(ChartItem {
                                id: 0,
                                start,
                                end,
                                category: ChartCategory::Vp,
                                rule: "vp.inf",
                                children: vec![verb.id, inf.id, vp.id],
                                head: verb.head.clone(),
                                lemma: verb.lemma.clone(),
                                pos: verb.pos.clone(),
                                negated: verb.negated || vp.negated,
                                past: verb.past || vp.past,
                            });
                        }
                    }
                }
            }
            // binary combination over adjacent spans.
            for start in 0..words.len() {
                for mid in (start + 1)..words.len() {
                    for end in (mid + 1)..=words.len() {
                        let add = |chart: &mut Chart,
                                   left: ChartCategory,
                                   right: ChartCategory,
                                   category: ChartCategory,
                                   rule: &'static str|
                         -> bool {
                            let lefts: Vec<ChartItem> = chart
                                .candidates(start, mid, left)
                                .into_iter()
                                .cloned()
                                .collect();
                            let rights: Vec<ChartItem> = chart
                                .candidates(mid, end, right)
                                .into_iter()
                                .cloned()
                                .collect();
                            let mut changed = false;
                            for l in &lefts {
                                for r in &rights {
                                    let mut children = vec![l.id, r.id];
                                    let mut head = r.head.clone();
                                    let mut lemma = r.lemma.clone();
                                    let mut pos = r.pos.clone();
                                    let mut negated = l.negated || r.negated;
                                    let past = l.past || r.past;
                                    if matches!(
                                        category,
                                        ChartCategory::Np
                                            | ChartCategory::Vp
                                            | ChartCategory::Clause
                                    ) {
                                        if left == ChartCategory::Np || left == ChartCategory::Vp {
                                            head = l.head.clone();
                                            lemma = l.lemma.clone();
                                            pos = l.pos.clone();
                                        }
                                        if left == ChartCategory::Det
                                            || left == ChartCategory::Num
                                            || left == ChartCategory::Adj
                                        {
                                            head = r.head.clone();
                                            lemma = r.lemma.clone();
                                            pos = r.pos.clone();
                                        }
                                    }
                                    if left == ChartCategory::Aux {
                                        negated = r.negated;
                                        head = r.head.clone();
                                        lemma = r.lemma.clone();
                                        pos = r.pos.clone();
                                    }
                                    if left == ChartCategory::Neg {
                                        negated = true;
                                        head = r.head.clone();
                                        lemma = r.lemma.clone();
                                        pos = r.pos.clone();
                                    }
                                    children.shrink_to_fit();
                                    changed |= chart.add(ChartItem {
                                        id: 0,
                                        start,
                                        end,
                                        category,
                                        rule,
                                        children,
                                        head,
                                        lemma,
                                        pos,
                                        negated,
                                        past,
                                    });
                                }
                            }
                            changed
                        };
                        changed |= add(
                            &mut chart,
                            ChartCategory::Det,
                            ChartCategory::Np,
                            ChartCategory::Np,
                            "np.det",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::Num,
                            ChartCategory::Np,
                            ChartCategory::Np,
                            "np.num",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::Adj,
                            ChartCategory::Np,
                            ChartCategory::Np,
                            "np.adj",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::Np,
                            ChartCategory::Pp,
                            ChartCategory::Np,
                            "np.pp",
                        );

                        changed |= add(
                            &mut chart,
                            ChartCategory::V,
                            ChartCategory::Np,
                            ChartCategory::Vp,
                            "vp.verb_np",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::Aux,
                            ChartCategory::Vp,
                            ChartCategory::Vp,
                            "vp.aux",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::Neg,
                            ChartCategory::Vp,
                            ChartCategory::Vp,
                            "vp.neg",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::Vp,
                            ChartCategory::Adv,
                            ChartCategory::Vp,
                            "vp.adv",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::Vp,
                            ChartCategory::Pp,
                            ChartCategory::Vp,
                            "vp.pp",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::P,
                            ChartCategory::Np,
                            ChartCategory::Pp,
                            "pp.p_np",
                        );
                        changed |= add(
                            &mut chart,
                            ChartCategory::Np,
                            ChartCategory::Vp,
                            ChartCategory::Clause,
                            "clause.np_vp",
                        );
                    }
                }
            }
        }

        for start in 0..words.len() {
            for mid1 in (start + 1)..words.len() {
                for mid2 in (mid1 + 1)..words.len() {
                    for end in (mid2 + 1)..=words.len() {
                        let (Some(left), Some(conj), Some(right)) = (
                            chart.get(start, mid1, ChartCategory::Np).cloned(),
                            chart.get(mid1, mid2, ChartCategory::Conj).cloned(),
                            chart.get(mid2, end, ChartCategory::Np).cloned(),
                        ) else {
                            continue;
                        };
                        chart.add(ChartItem {
                            id: 0,
                            start,
                            end,
                            category: ChartCategory::Np,
                            rule: "np.coord3",
                            children: vec![left.id, conj.id, right.id],
                            head: left.head.clone(),
                            lemma: left.lemma.clone(),
                            pos: left.pos.clone(),
                            negated: left.negated || right.negated,
                            past: left.past || right.past,
                        });
                    }
                }
            }
        }
        let coord_ids: Vec<usize> = chart
            .items
            .iter()
            .filter(|item| item.rule == "np.coord3")
            .map(|item| item.id)
            .collect();
        for coord_id in coord_ids {
            let coord = chart.items[coord_id].clone();
            if let Some(vp) = chart
                .get(coord.end, words.len(), ChartCategory::Vp)
                .cloned()
            {
                chart.add(ChartItem {
                    id: 0,
                    start: coord.start,
                    end: words.len(),
                    category: ChartCategory::Clause,
                    rule: "clause.np_vp",
                    children: vec![coord.id, vp.id],
                    head: coord.head.clone(),
                    lemma: coord.lemma.clone(),
                    pos: coord.pos.clone(),
                    negated: coord.negated || vp.negated,
                    past: coord.past || vp.past,
                });
            }
        }

        // Object coordination: V + NP CONJ NP -> VP, then Clause.
        let object_coord_ids: Vec<usize> = chart
            .items
            .iter()
            .filter(|item| item.rule == "np.coord3")
            .map(|item| item.id)
            .collect();
        for coord_id in object_coord_ids {
            let coord = chart.items[coord_id].clone();
            let verb_ids: Vec<usize> = chart
                .items
                .iter()
                .filter(|item| item.category == ChartCategory::V && item.end == coord.start)
                .map(|item| item.id)
                .collect();
            for verb_id in verb_ids {
                let verb = chart.items[verb_id].clone();
                chart.add(ChartItem {
                    id: 0,
                    start: verb.start,
                    end: coord.end,
                    category: ChartCategory::Vp,
                    rule: "vp.verb_np",
                    children: vec![verb.id, coord.id],
                    head: verb.head.clone(),
                    lemma: verb.lemma.clone(),
                    pos: verb.pos.clone(),
                    negated: verb.negated || coord.negated,
                    past: verb.past || coord.past,
                });
                if let Some(vp) = chart
                    .items
                    .iter()
                    .find(|item| {
                        item.start == verb.start
                            && item.end == coord.end
                            && item.category == ChartCategory::Vp
                            && item.rule == "vp.verb_np"
                    })
                    .cloned()
                {
                    if let Some(subject) = chart.get(0, verb.start, ChartCategory::Np).cloned() {
                        chart.add(ChartItem {
                            id: 0,
                            start: 0,
                            end: coord.end,
                            category: ChartCategory::Clause,
                            rule: "clause.np_vp",
                            children: vec![subject.id, vp.id],
                            head: subject.head.clone(),
                            lemma: subject.lemma.clone(),
                            pos: subject.pos.clone(),
                            negated: subject.negated || vp.negated,
                            past: subject.past || vp.past,
                        });
                    }
                }
            }
        }

        // Relative clause skeleton: NP REL VP -> NP, then combine with a main VP.
        for start in 0..words.len() {
            for mid1 in (start + 1)..words.len() {
                for mid2 in (mid1 + 1)..words.len() {
                    for end in (mid2 + 1)..=words.len() {
                        let (Some(noun), Some(rel), Some(vp)) = (
                            chart.get(start, mid1, ChartCategory::Np).cloned(),
                            chart.get(mid1, mid2, ChartCategory::Rel).cloned(),
                            chart.get(mid2, end, ChartCategory::Vp).cloned(),
                        ) else {
                            continue;
                        };
                        chart.add(ChartItem {
                            id: 0,
                            start,
                            end,
                            category: ChartCategory::Np,
                            rule: "np.rel",
                            children: vec![noun.id, rel.id, vp.id],
                            head: noun.head.clone(),
                            lemma: noun.lemma.clone(),
                            pos: noun.pos.clone(),
                            negated: noun.negated || vp.negated,
                            past: noun.past || vp.past,
                        });
                    }
                }
            }
        }
        let rel_ids: Vec<usize> = chart
            .items
            .iter()
            .filter(|item| item.rule == "np.rel")
            .map(|item| item.id)
            .collect();
        for rel_id in rel_ids {
            let noun = chart.items[rel_id].clone();
            if let Some(vp) = chart.get(noun.end, words.len(), ChartCategory::Vp).cloned() {
                chart.add(ChartItem {
                    id: 0,
                    start: noun.start,
                    end: words.len(),
                    category: ChartCategory::Clause,
                    rule: "clause.np_vp",
                    children: vec![noun.id, vp.id],
                    head: noun.head.clone(),
                    lemma: noun.lemma.clone(),
                    pos: noun.pos.clone(),
                    negated: noun.negated || vp.negated,
                    past: noun.past || vp.past,
                });
            }
        }

        // Clause coordination syntax: CLAUSE CONJ CLAUSE -> CLAUSE.
        for start in 0..words.len() {
            for mid1 in (start + 1)..words.len() {
                for mid2 in (mid1 + 1)..words.len() {
                    for end in (mid2 + 1)..=words.len() {
                        let (Some(left), Some(conj), Some(right)) = (
                            chart.get(start, mid1, ChartCategory::Clause).cloned(),
                            chart.get(mid1, mid2, ChartCategory::Conj).cloned(),
                            chart.get(mid2, end, ChartCategory::Clause).cloned(),
                        ) else {
                            continue;
                        };
                        chart.add(ChartItem {
                            id: 0,
                            start,
                            end,
                            category: ChartCategory::Clause,
                            rule: "clause.coord",
                            children: vec![left.id, conj.id, right.id],
                            head: left.head.clone(),
                            lemma: left.lemma.clone(),
                            pos: left.pos.clone(),
                            negated: left.negated || right.negated,
                            past: left.past || right.past,
                        });
                    }
                }
            }
        }

        Some(chart)
    }
}

fn chart_rule_score(rule: &str) -> u32 {
    match rule {
        "clause.np_vp" => 100,
        "clause.coord" => 110,
        "vp.verb_np" => 60,
        "vp.aux" => 55,
        "vp.neg" => 55,
        "np.det" => 40,
        "np.num" => 40,
        "np.adj" => 30,
        "vp.adv" => 20,
        "pp.p_np" => 35,
        "np.pp" => 25,
        "vp.pp" => 30,
        "coord.left" => 25,
        "np.coord" => 30,
        "np.coord3" => 35,
        "vp.inf" => 50,
        "np.rel" => 45,
        _ => 10,
    }
}

fn chart_item_to_graph(
    chart: &Chart,
    id: usize,
    words: &[&SourceToken],
    graph: &mut SyntaxGraph,
) -> usize {
    let item = &chart.items[id];
    let kind = match item.category {
        ChartCategory::Clause => SyntaxNodeKind::Clause,
        ChartCategory::Np => SyntaxNodeKind::NounPhrase,
        ChartCategory::Vp => SyntaxNodeKind::VerbPhrase,
        ChartCategory::Pp => SyntaxNodeKind::PrepositionPhrase,
        ChartCategory::Coord => SyntaxNodeKind::Coordination,
        _ => SyntaxNodeKind::Token,
    };
    let span = if item.start < item.end && item.end > 0 {
        TextSpan::new(words[item.start].span.start, words[item.end - 1].span.end)
    } else {
        TextSpan::default()
    };
    let node = push(graph, kind, span, item.head.clone());
    graph.nodes[node].lemma = item.lemma.clone();
    graph.nodes[node].pos = item.pos.clone();
    graph.nodes[node].past = item.past;
    let child_relation = |left: ChartCategory, right: ChartCategory| -> Option<SyntaxRelation> {
        match (left, right) {
            (ChartCategory::Np, ChartCategory::Vp) => Some(SyntaxRelation::Subject),
            (ChartCategory::Vp, ChartCategory::Vp) => None,
            _ => None,
        }
    };
    let _ = child_relation;
    let mut child_nodes = Vec::new();
    for (idx, child_id) in item.children.iter().enumerate() {
        let child = chart_item_to_graph(chart, *child_id, words, graph);
        child_nodes.push(child);
        let child_item = &chart.items[*child_id];
        let relation = match (item.category, idx) {
            (ChartCategory::Clause, 0) if item.rule == "clause.coord" => SyntaxRelation::Conjunct,
            (ChartCategory::Clause, 1) if item.rule == "clause.coord" => {
                SyntaxRelation::Conjunction
            }
            (ChartCategory::Clause, 2) if item.rule == "clause.coord" => SyntaxRelation::Conjunct,
            (ChartCategory::Clause, 0) => SyntaxRelation::Subject,
            (ChartCategory::Clause, 1) => SyntaxRelation::Predicate,
            (ChartCategory::Vp, 0) if child_item.category == ChartCategory::V => {
                SyntaxRelation::Head
            }
            (ChartCategory::Vp, 1) if item.rule == "vp.inf" => SyntaxRelation::Head,
            (ChartCategory::Vp, 2) if item.rule == "vp.inf" => SyntaxRelation::Complement,
            (ChartCategory::Vp, 1) if child_item.category == ChartCategory::Np => {
                SyntaxRelation::Object
            }
            (ChartCategory::Vp, 0) if child_item.category == ChartCategory::Aux => {
                SyntaxRelation::Auxiliary
            }
            (ChartCategory::Vp, 0) if child_item.category == ChartCategory::Neg => {
                SyntaxRelation::Negation
            }
            (ChartCategory::Vp, 1) if child_item.category == ChartCategory::Adv => {
                SyntaxRelation::Adverbial
            }
            (ChartCategory::Np, 0) if child_item.category == ChartCategory::Det => {
                SyntaxRelation::Determiner
            }
            (ChartCategory::Np, 0) if child_item.category == ChartCategory::Num => {
                SyntaxRelation::Quantity
            }
            (ChartCategory::Np, 0) if child_item.category == ChartCategory::Adj => {
                SyntaxRelation::Modifier
            }
            (ChartCategory::Np, 0) if child_item.category == ChartCategory::Coord => {
                SyntaxRelation::Conjunct
            }
            (ChartCategory::Np, _)
                if item.rule == "np.coord" && child_item.category == ChartCategory::Np =>
            {
                SyntaxRelation::Conjunct
            }
            (ChartCategory::Coord, 0) if child_item.category == ChartCategory::Np => {
                SyntaxRelation::Conjunct
            }
            (ChartCategory::Coord, 1) if child_item.category == ChartCategory::Conj => {
                SyntaxRelation::Conjunction
            }
            (ChartCategory::Np, 0) if item.rule == "np.rel" => SyntaxRelation::Head,
            (ChartCategory::Np, 1) if item.rule == "np.rel" => SyntaxRelation::Relative,
            (ChartCategory::Np, 2) if item.rule == "np.rel" => SyntaxRelation::Relative,
            (ChartCategory::Np, 0) if item.rule == "np.coord3" => SyntaxRelation::Conjunct,
            (ChartCategory::Np, 1) if item.rule == "np.coord3" => SyntaxRelation::Conjunction,
            (ChartCategory::Np, 2) if item.rule == "np.coord3" => SyntaxRelation::Conjunct,
            (ChartCategory::Np, 0) => SyntaxRelation::Head,
            (ChartCategory::Pp, 0) if child_item.category == ChartCategory::P => {
                SyntaxRelation::Preposition
            }
            (ChartCategory::Pp, 1) if child_item.category == ChartCategory::Np => {
                SyntaxRelation::Object
            }
            (ChartCategory::Np, _) if child_item.category == ChartCategory::Pp => {
                SyntaxRelation::PpModifier
            }
            (ChartCategory::Vp, _) if child_item.category == ChartCategory::Pp => {
                SyntaxRelation::Adverbial
            }
            _ => SyntaxRelation::Head,
        };
        graph.edges.push(SyntaxEdge {
            head: node,
            dependent: child,
            relation,
        });
    }
    if item.rule == "np.coord3" && child_nodes.len() == 3 {
        graph.nodes[node].coord_heads = vec![child_nodes[0], child_nodes[2]];
    }
    node
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeaningNode {
    pub id: usize,
    pub kind: MeaningNodeKind,
    pub lemma: String,
    pub pos: String,
    pub plural: bool,
    pub sense: SenseResolution,
    pub span: TextSpan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeaningRelationKind {
    Agent,
    Patient,
    Quantity,
    Modifier,
    Time,
    Scope,
    Complement,
    Location,
    Instrument,
    Preposition,
    Condition,
    Coordination,
    PpModifier,
    Relative,
    ClauseCoordination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeaningRelation {
    pub head: usize,
    pub dependent: usize,
    pub kind: MeaningRelationKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeaningGraph {
    pub nodes: Vec<MeaningNode>,
    pub relations: Vec<MeaningRelation>,
    pub tense_past: bool,
    pub progressive: bool,
    pub modal: Option<String>,
    pub passive: bool,
    pub future: bool,
    pub conditional: bool,
}

struct NpFacts {
    head: usize,
    quantity: Option<u32>,
    modifiers: Vec<usize>,
    pps: Vec<usize>,
    conjuncts: Vec<usize>,
    relative: Option<usize>,
}

impl NpFacts {
    fn collect(graph: &SyntaxGraph, np: usize) -> Option<Self> {
        let node = graph.nodes.get(np)?;
        let mut facts = Self {
            head: np,
            quantity: None,
            modifiers: Vec::new(),
            pps: Vec::new(),
            conjuncts: Vec::new(),
            relative: None,
        };
        for edge in graph.edges.iter().filter(|edge| edge.head == np) {
            match edge.relation {
                SyntaxRelation::Head => {
                    if let Some(child) = graph.nodes.get(edge.dependent) {
                        if child.kind == SyntaxNodeKind::NounPhrase {
                            let nested = Self::collect(graph, edge.dependent)?;
                            facts.head = nested.head;
                            facts.quantity = facts.quantity.or(nested.quantity);
                            facts.modifiers.extend(nested.modifiers);
                            facts.pps.extend(nested.pps);
                            facts.conjuncts.extend(nested.conjuncts);
                        } else {
                            facts.head = edge.dependent;
                        }
                    }
                }
                SyntaxRelation::Quantity => {
                    if let Some(child) = graph.nodes.get(edge.dependent) {
                        facts.quantity = child.lemma.parse::<u32>().ok();
                    }
                }
                SyntaxRelation::Determiner => {
                    if let Some(child) = graph.nodes.get(edge.dependent) {
                        if matches!(child.lemma.as_str(), "a" | "an") {
                            facts.quantity = Some(1);
                        }
                    }
                }
                SyntaxRelation::Modifier => facts.modifiers.push(edge.dependent),
                SyntaxRelation::PpModifier => facts.pps.push(edge.dependent),
                SyntaxRelation::Relative => {
                    if let Some(child) = graph.nodes.get(edge.dependent) {
                        if child.kind == SyntaxNodeKind::VerbPhrase {
                            facts.relative = Some(edge.dependent);
                        }
                    }
                }
                SyntaxRelation::Conjunct => {
                    if let Some(child) = graph.nodes.get(edge.dependent) {
                        if child.kind == SyntaxNodeKind::Coordination {
                            for coord_edge in graph.edges.iter().filter(|candidate| {
                                candidate.head == edge.dependent
                                    && candidate.relation == SyntaxRelation::Conjunct
                            }) {
                                if let Some(nested) = NpFacts::collect(graph, coord_edge.dependent)
                                {
                                    facts.conjuncts.push(nested.head);
                                }
                            }
                        } else if child.kind == SyntaxNodeKind::NounPhrase {
                            if let Some(nested) = NpFacts::collect(graph, edge.dependent) {
                                facts.conjuncts.push(nested.head);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let _ = node;
        Some(facts)
    }
}

fn collect_np_heads(graph: &SyntaxGraph, np: usize, out: &mut Vec<usize>, depth: usize) {
    if depth > 8 {
        return;
    }
    if let Some(node) = graph.nodes.get(np) {
        if node.kind != SyntaxNodeKind::NounPhrase {
            out.push(np);
            return;
        }
    }
    for edge in graph.edges.iter().filter(|edge| edge.head == np) {
        match edge.relation {
            SyntaxRelation::Head => {
                if let Some(child) = graph.nodes.get(edge.dependent) {
                    if child.kind == SyntaxNodeKind::NounPhrase {
                        collect_np_heads(graph, edge.dependent, out, depth + 1);
                    } else {
                        out.push(edge.dependent);
                    }
                }
            }
            SyntaxRelation::Conjunct => {
                collect_np_heads(graph, edge.dependent, out, depth + 1);
            }
            _ => {}
        }
    }
}

struct PpFacts {
    preposition: String,
    object: usize,
}

impl PpFacts {
    fn collect(graph: &SyntaxGraph, pp: usize) -> Option<Self> {
        let prep = graph_edge(graph, pp, SyntaxRelation::Preposition)?;
        let object = graph_edge(graph, pp, SyntaxRelation::Object)?;
        Some(Self {
            preposition: graph.nodes.get(prep)?.lemma.clone(),
            object: NpFacts::collect(graph, object)?.head,
        })
    }
}

#[derive(Default)]
struct VpFacts {
    verb: Option<usize>,
    object: Option<usize>,
    negation: Option<usize>,
    auxiliary: Option<usize>,
    adverbials: Vec<usize>,
    pps: Vec<usize>,
    complement: Option<usize>,
    modal: Option<String>,
    past: bool,
    progressive: bool,
    future: bool,
    passive: bool,
}

impl VpFacts {
    fn collect(graph: &SyntaxGraph, vp: usize) -> Option<Self> {
        let mut facts = Self::default();
        facts.visit(graph, vp, 0);
        Some(facts)
    }

    fn visit(&mut self, graph: &SyntaxGraph, vp: usize, depth: usize) {
        if depth > 8 {
            return;
        }
        let node = match graph.nodes.get(vp) {
            Some(node) => node,
            None => return,
        };
        if node.kind == SyntaxNodeKind::Token && node.pos == "verb" {
            if self.verb.is_none() {
                self.verb = Some(vp);
            }
            self.past |= node.past;
            return;
        }
        for edge in graph.edges.iter().filter(|edge| edge.head == vp) {
            match edge.relation {
                SyntaxRelation::Head => {
                    if let Some(child) = graph.nodes.get(edge.dependent) {
                        if child.kind == SyntaxNodeKind::VerbPhrase {
                            self.visit(graph, edge.dependent, depth + 1);
                        } else if child.pos == "verb" {
                            self.verb.get_or_insert(edge.dependent);
                        }
                    }
                }
                SyntaxRelation::Object => self.object = Some(edge.dependent),
                SyntaxRelation::Negation => self.negation = Some(edge.dependent),
                SyntaxRelation::Auxiliary => {
                    self.auxiliary = Some(edge.dependent);
                    if let Some(aux) = graph.nodes.get(edge.dependent) {
                        match aux.surface.as_str() {
                            "did" => self.past = true,
                            "was" | "were" => {
                                self.past = true;
                                self.passive = true;
                            }
                            "will" => self.future = true,
                            "can" => self.modal = Some("can".to_string()),
                            "is" | "are" | "am" => self.progressive = true,
                            _ => {}
                        }
                    }
                }
                SyntaxRelation::Complement => self.complement = Some(edge.dependent),
                SyntaxRelation::Adverbial => {
                    if graph
                        .nodes
                        .get(edge.dependent)
                        .is_some_and(|node| node.kind == SyntaxNodeKind::PrepositionPhrase)
                    {
                        self.pps.push(edge.dependent);
                    } else {
                        self.adverbials.push(edge.dependent);
                    }
                }
                _ => {}
            }
        }
    }
}

pub struct SemanticTransfer<'a> {
    pub lexicon: &'a CompositionEngine,
}

impl<'a> SemanticTransfer<'a> {
    /// 新路径：只从 SyntaxGraph 的节点和关系推导语义。
    pub fn transfer_graph(&self, syntax: &SyntaxGraph) -> Option<MeaningGraph> {
        if syntax.edges.iter().any(|edge| {
            edge.relation == SyntaxRelation::Conjunction
                && syntax
                    .nodes
                    .get(edge.head)
                    .is_some_and(|node| node.kind == SyntaxNodeKind::Clause)
        }) {
            return None;
        }
        let clause = syntax
            .nodes
            .iter()
            .find(|node| node.kind == SyntaxNodeKind::Clause)?;
        let subject = graph_edge(syntax, clause.id, SyntaxRelation::Subject)?;
        let predicate = graph_edge(syntax, clause.id, SyntaxRelation::Predicate)?;
        let facts = VpFacts::collect(syntax, predicate)?;
        let verb = facts.verb?;
        let object = facts.object;
        let neg = facts.negation;
        let _aux = facts.auxiliary;
        let advs = facts.adverbials;
        let patient_np = if facts.passive { Some(subject) } else { object };
        let mut graph = MeaningGraph {
            tense_past: facts.past || syntax.nodes.get(verb).is_some_and(|node| node.past),
            progressive: facts.progressive,
            modal: facts.modal.clone(),
            passive: facts.passive,
            future: facts.future,
            ..Default::default()
        };
        let event = add(
            &mut graph,
            MeaningNodeKind::Event,
            syntax.nodes.get(verb)?.lemma.clone(),
            "verb",
            syntax.nodes.get(verb)?.span,
            self.sense(
                &syntax.nodes.get(verb)?.lemma,
                "verb",
                patient_np
                    .and_then(|id| NpFacts::collect(syntax, id))
                    .and_then(|np| syntax.nodes.get(np.head))
                    .map(|node| node.lemma.as_str()),
            ),
        );
        if !facts.passive {
            let subject_np = NpFacts::collect(syntax, subject)?;
            let agent = self.graph_entity(syntax, &mut graph, subject)?;
            graph.relations.push(MeaningRelation {
                head: event,
                dependent: agent,
                kind: MeaningRelationKind::Agent,
            });
            if let Some(relative_vp) = subject_np.relative {
                self.add_relative(syntax, &mut graph, agent, relative_vp)?;
            }
        }
        if let Some(object) = patient_np {
            let np = NpFacts::collect(syntax, object)?;
            let patient = self.graph_entity(syntax, &mut graph, np.head)?;
            graph.relations.push(MeaningRelation {
                head: event,
                dependent: patient,
                kind: MeaningRelationKind::Patient,
            });
            if let Some(relative_vp) = np.relative {
                self.add_relative(syntax, &mut graph, patient, relative_vp)?;
            }
            if let Some(value) = np.quantity {
                let node = syntax.nodes.get(np.head)?;
                let quantity = add(
                    &mut graph,
                    MeaningNodeKind::Property,
                    value.to_string(),
                    "numeral",
                    node.span,
                    SenseResolution::Resolved(format!("quantity:{value}")),
                );
                graph.relations.push(MeaningRelation {
                    head: patient,
                    dependent: quantity,
                    kind: MeaningRelationKind::Quantity,
                });
            }
            for modifier in np.modifiers {
                let node = syntax.nodes.get(modifier)?;
                let modifier = add(
                    &mut graph,
                    MeaningNodeKind::Property,
                    node.lemma.clone(),
                    "adjective",
                    node.span,
                    self.sense(&node.lemma, "adjective", None),
                );
                graph.relations.push(MeaningRelation {
                    head: patient,
                    dependent: modifier,
                    kind: MeaningRelationKind::Modifier,
                });
            }
            for pp_id in np.pps {
                let pp = PpFacts::collect(syntax, pp_id)?;
                let object = self.graph_entity(syntax, &mut graph, pp.object)?;
                let prep = add(
                    &mut graph,
                    MeaningNodeKind::Preposition,
                    pp.preposition.clone(),
                    "preposition",
                    syntax.nodes.get(pp_id)?.span,
                    SenseResolution::Resolved(format!("prep:{}", pp.preposition)),
                );
                graph.relations.push(MeaningRelation {
                    head: object,
                    dependent: prep,
                    kind: MeaningRelationKind::Preposition,
                });
                graph.relations.push(MeaningRelation {
                    head: patient,
                    dependent: object,
                    kind: MeaningRelationKind::PpModifier,
                });
            }
        }
        if let Some(complement_id) = facts.complement {
            let complement_verb =
                graph_edge(syntax, complement_id, SyntaxRelation::Head).unwrap_or(complement_id);
            let node = syntax.nodes.get(complement_verb)?;
            let complement_event = add(
                &mut graph,
                MeaningNodeKind::Event,
                node.lemma.clone(),
                "verb",
                node.span,
                self.sense(&node.lemma, "verb", None),
            );
            graph.relations.push(MeaningRelation {
                head: event,
                dependent: complement_event,
                kind: MeaningRelationKind::Complement,
            });
            if let Some(cfacts) = VpFacts::collect(syntax, complement_id) {
                if let Some(object) = cfacts.object {
                    let np = NpFacts::collect(syntax, object)?;
                    let patient = self.graph_entity(syntax, &mut graph, np.head)?;
                    graph.relations.push(MeaningRelation {
                        head: complement_event,
                        dependent: patient,
                        kind: MeaningRelationKind::Patient,
                    });
                    if let Some(value) = np.quantity {
                        let node = syntax.nodes.get(np.head)?;
                        let quantity = add(
                            &mut graph,
                            MeaningNodeKind::Property,
                            value.to_string(),
                            "numeral",
                            node.span,
                            SenseResolution::Resolved(format!("quantity:{value}")),
                        );
                        graph.relations.push(MeaningRelation {
                            head: patient,
                            dependent: quantity,
                            kind: MeaningRelationKind::Quantity,
                        });
                    }
                    for modifier in np.modifiers {
                        let node = syntax.nodes.get(modifier)?;
                        let modifier = add(
                            &mut graph,
                            MeaningNodeKind::Property,
                            node.lemma.clone(),
                            "adjective",
                            node.span,
                            self.sense(&node.lemma, "adjective", None),
                        );
                        graph.relations.push(MeaningRelation {
                            head: patient,
                            dependent: modifier,
                            kind: MeaningRelationKind::Modifier,
                        });
                    }
                }
            }
        }
        for pp_id in facts.pps {
            let pp = PpFacts::collect(syntax, pp_id)?;
            let entity = self.graph_entity(syntax, &mut graph, pp.object)?;
            let prep = add(
                &mut graph,
                MeaningNodeKind::Preposition,
                pp.preposition.clone(),
                "preposition",
                syntax.nodes.get(pp_id)?.span,
                SenseResolution::Resolved(format!("prep:{}", pp.preposition)),
            );
            graph.relations.push(MeaningRelation {
                head: entity,
                dependent: prep,
                kind: MeaningRelationKind::Preposition,
            });
            let kind = if pp.preposition == "with" {
                MeaningRelationKind::Instrument
            } else {
                MeaningRelationKind::Location
            };
            graph.relations.push(MeaningRelation {
                head: event,
                dependent: entity,
                kind,
            });
        }
        for adv in advs {
            let node = syntax.nodes.get(adv)?;
            let time = add(
                &mut graph,
                MeaningNodeKind::Time,
                node.lemma.clone(),
                "adverb",
                node.span,
                self.sense(&node.lemma, "adverb", None),
            );
            graph.relations.push(MeaningRelation {
                head: event,
                dependent: time,
                kind: MeaningRelationKind::Time,
            });
        }
        if let Some(neg) = neg {
            let node = syntax.nodes.get(neg)?;
            let operator = add(
                &mut graph,
                MeaningNodeKind::Operator,
                "not".to_string(),
                "adverb",
                node.span,
                SenseResolution::Resolved("negation".to_string()),
            );
            graph.relations.push(MeaningRelation {
                head: operator,
                dependent: event,
                kind: MeaningRelationKind::Scope,
            });
        }
        Some(graph)
    }

    fn add_relative(
        &self,
        syntax: &SyntaxGraph,
        graph: &mut MeaningGraph,
        entity: usize,
        relative_vp: usize,
    ) -> Option<()> {
        let facts = VpFacts::collect(syntax, relative_vp)?;
        let verb = facts.verb?;
        let verb_node = syntax.nodes.get(verb)?;
        let event = add(
            graph,
            MeaningNodeKind::Event,
            verb_node.lemma.clone(),
            "verb",
            verb_node.span,
            self.sense(&verb_node.lemma, "verb", None),
        );
        graph.relations.push(MeaningRelation {
            head: entity,
            dependent: event,
            kind: MeaningRelationKind::Relative,
        });
        if let Some(object) = facts.object {
            let np = NpFacts::collect(syntax, object)?;
            let patient = self.graph_entity(syntax, graph, np.head)?;
            graph.relations.push(MeaningRelation {
                head: event,
                dependent: patient,
                kind: MeaningRelationKind::Patient,
            });
        }
        Some(())
    }

    fn graph_entity(
        &self,
        syntax: &SyntaxGraph,
        graph: &mut MeaningGraph,
        id: usize,
    ) -> Option<usize> {
        let np = NpFacts::collect(syntax, id)?;
        let mut heads: Vec<usize> = syntax
            .nodes
            .get(id)
            .map(|node| node.coord_heads.clone())
            .unwrap_or_default();
        if heads.is_empty() {
            heads = syntax
                .edges
                .iter()
                .filter(|edge| edge.head == id && edge.relation == SyntaxRelation::Conjunct)
                .map(|edge| edge.dependent)
                .collect();
        }
        if heads.is_empty() {
            collect_np_heads(syntax, id, &mut heads, 0);
        }
        if heads.is_empty() {
            heads.push(np.head);
        }
        let mut entities = Vec::new();
        for head in heads {
            let node = syntax.nodes.get(head)?;
            let entity = add(
                graph,
                MeaningNodeKind::Entity,
                node.lemma.clone(),
                if node.pos.is_empty() {
                    "noun"
                } else {
                    &node.pos
                },
                node.span,
                self.sense(
                    &node.lemma,
                    if node.pos.is_empty() {
                        "noun"
                    } else {
                        &node.pos
                    },
                    None,
                ),
            );
            graph.nodes[entity].plural = node.surface.ends_with('s') && node.lemma != "books";
            if let Some(facts) = NpFacts::collect(syntax, head) {
                if let Some(value) = facts.quantity {
                    let quantity = add(
                        graph,
                        MeaningNodeKind::Property,
                        value.to_string(),
                        "numeral",
                        node.span,
                        SenseResolution::Resolved(format!("quantity:{value}")),
                    );
                    graph.relations.push(MeaningRelation {
                        head: entity,
                        dependent: quantity,
                        kind: MeaningRelationKind::Quantity,
                    });
                }
                for modifier_id in facts.modifiers {
                    if let Some(modifier_node) = syntax.nodes.get(modifier_id) {
                        let modifier = add(
                            graph,
                            MeaningNodeKind::Property,
                            modifier_node.lemma.clone(),
                            "adjective",
                            modifier_node.span,
                            self.sense(&modifier_node.lemma, "adjective", None),
                        );
                        graph.relations.push(MeaningRelation {
                            head: entity,
                            dependent: modifier,
                            kind: MeaningRelationKind::Modifier,
                        });
                    }
                }
            }
            entities.push(entity);
        }
        if entities.len() > 1 {
            graph.relations.push(MeaningRelation {
                head: entities[0],
                dependent: entities[1],
                kind: MeaningRelationKind::Coordination,
            });
        }
        entities.first().copied()
    }

    pub fn transfer(&self, syntax: &SyntaxAnalysis) -> Option<MeaningGraph> {
        let mut graph = MeaningGraph {
            tense_past: syntax.past,
            progressive: syntax.progressive,
            modal: syntax.modal.clone(),
            passive: syntax.passive,
            future: syntax.future,
            conditional: false,
            ..Default::default()
        };
        let agent_np = if syntax.passive {
            syntax.by_agent.as_ref()
        } else {
            Some(&syntax.subject)
        };
        let patient_np = if syntax.passive {
            Some(&syntax.subject)
        } else {
            syntax.object.as_ref()
        };
        let predicate = add(
            &mut graph,
            MeaningNodeKind::Event,
            syntax.predicate.2.clone(),
            "verb",
            syntax.predicate.1,
            self.sense(
                syntax.predicate.2.as_str(),
                "verb",
                patient_np.map(np_lemma),
            ),
        );
        if let Some(agent_np) = agent_np {
            let agent = self.entity_node(&mut graph, agent_np)?;
            graph.relations.push(MeaningRelation {
                head: predicate,
                dependent: agent,
                kind: MeaningRelationKind::Agent,
            });
        }
        let mut patient_head = predicate;
        if let Some((_, span, lemma)) = &syntax.complement {
            let complement = add(
                &mut graph,
                MeaningNodeKind::Event,
                lemma.clone(),
                "verb",
                *span,
                self.sense(lemma, "verb", patient_np.map(np_lemma)),
            );
            graph.relations.push(MeaningRelation {
                head: predicate,
                dependent: complement,
                kind: MeaningRelationKind::Complement,
            });
            patient_head = complement;
        }
        let patient = patient_np.and_then(|object| self.entity_node(&mut graph, object));
        if let Some(patient) = patient {
            graph.relations.push(MeaningRelation {
                head: patient_head,
                dependent: patient,
                kind: MeaningRelationKind::Patient,
            });
        }
        if let Some(Np::Noun {
            quantity,
            adjective,
            ..
        }) = &syntax.object
        {
            let patient = patient?;
            if let Some((_, q, span)) = quantity {
                if *q > 0 {
                    let quantity = add(
                        &mut graph,
                        MeaningNodeKind::Property,
                        format!("{q}"),
                        "numeral",
                        *span,
                        SenseResolution::Resolved(format!("quantity:{q}")),
                    );
                    graph.relations.push(MeaningRelation {
                        head: patient,
                        dependent: quantity,
                        kind: MeaningRelationKind::Quantity,
                    });
                }
            }
            if let Some((_, span, surface)) = adjective {
                let modifier = add(
                    &mut graph,
                    MeaningNodeKind::Property,
                    surface.clone(),
                    "adjective",
                    *span,
                    self.sense(surface, "adjective", None),
                );
                graph.relations.push(MeaningRelation {
                    head: patient,
                    dependent: modifier,
                    kind: MeaningRelationKind::Modifier,
                });
            }
        }
        if let Some((_, span, surface, _)) = &syntax.time {
            let time = add(
                &mut graph,
                MeaningNodeKind::Time,
                surface.clone(),
                "adverb",
                *span,
                self.sense(surface, "adverb", None),
            );
            graph.relations.push(MeaningRelation {
                head: predicate,
                dependent: time,
                kind: MeaningRelationKind::Time,
            });
        }
        if let Some((span, surface)) = &syntax.place {
            let location = add(
                &mut graph,
                MeaningNodeKind::Time,
                surface.clone(),
                "adverb",
                *span,
                self.sense(surface, "adverb", None),
            );
            graph.relations.push(MeaningRelation {
                head: predicate,
                dependent: location,
                kind: MeaningRelationKind::Location,
            });
        }
        if syntax.negated {
            let neg = add(
                &mut graph,
                MeaningNodeKind::Operator,
                "not".to_string(),
                "adverb",
                syntax.neg_span.unwrap_or(syntax.predicate.1),
                SenseResolution::Resolved("negation".to_string()),
            );
            graph.relations.push(MeaningRelation {
                head: neg,
                dependent: predicate,
                kind: MeaningRelationKind::Scope,
            });
        }
        Some(graph)
    }

    fn entity_node(&self, graph: &mut MeaningGraph, np: &Np) -> Option<usize> {
        match np {
            Np::Pronoun { span, surface, .. } => Some(add(
                graph,
                MeaningNodeKind::Entity,
                surface.clone(),
                "pronoun",
                *span,
                self.sense(surface, "pronoun", None),
            )),
            Np::Noun {
                span,
                surface,
                plural,
                ..
            } => {
                let id = add(
                    graph,
                    MeaningNodeKind::Entity,
                    surface.clone(),
                    "noun",
                    *span,
                    self.sense(surface, "noun", None),
                );
                graph.nodes[id].plural = *plural;
                Some(id)
            }
        }
    }

    fn sense(&self, lemma: &str, pos: &str, object: Option<&str>) -> SenseResolution {
        let candidates = self.lexicon.sense_targets(lemma, pos, object);
        match candidates.len() {
            0 => {
                if self.lexicon.zh(lemma, pos).is_some() {
                    SenseResolution::Resolved(format!("lex:{lemma}:{pos}"))
                } else {
                    SenseResolution::Unresolved
                }
            }
            1 => SenseResolution::Resolved(candidates[0].1.clone()),
            _ => SenseResolution::Ambiguous(candidates.into_iter().map(|(_, id)| id).collect()),
        }
    }
}

fn graph_edge(graph: &SyntaxGraph, head: usize, relation: SyntaxRelation) -> Option<usize> {
    graph
        .edges
        .iter()
        .find(|edge| edge.head == head && edge.relation == relation)
        .map(|edge| edge.dependent)
}

fn add(
    graph: &mut MeaningGraph,
    kind: MeaningNodeKind,
    lemma: String,
    pos: &str,
    span: TextSpan,
    sense: SenseResolution,
) -> usize {
    let id = graph.nodes.len();
    graph.nodes.push(MeaningNode {
        id,
        kind,
        lemma,
        pos: pos.to_string(),
        plural: false,
        sense,
        span,
    });
    id
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChineseNodeKind {
    Clause,
    NounPhrase,
    VerbPhrase,
    Noun,
    Verb,
    Aspect,
    Modal,
    Quantity,
    Classifier,
    Modifier,
    Time,
    Negation,
    Passive,
    Place,
    Preposition,
    Conjunction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChineseNode {
    pub id: usize,
    pub kind: ChineseNodeKind,
    pub text: String,
    pub children: Vec<usize>,
    pub source: Option<usize>,
    pub role: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChineseSyntaxTree {
    pub nodes: Vec<ChineseNode>,
    pub root: usize,
    pub condition: Option<Box<ChineseSyntaxTree>>,
    pub coordination: Option<Vec<ChineseSyntaxTree>>,
}

pub struct TargetTransfer<'a> {
    pub lexicon: &'a CompositionEngine,
}

impl<'a> TargetTransfer<'a> {
    fn target<'b>(&'b self, node: &'b MeaningNode) -> Option<&'b str> {
        if let SenseResolution::Resolved(sense_id) = &node.sense {
            if let Some(target) = self.lexicon.target_for_sense(sense_id) {
                return Some(target);
            }
        }
        self.lexicon.zh(&node.lemma, &node.pos)
    }

    pub fn transfer(&self, meaning: &MeaningGraph) -> Option<ChineseSyntaxTree> {
        let event = meaning
            .nodes
            .iter()
            .find(|n| n.kind == MeaningNodeKind::Event)?;
        let agent = relation_node(meaning, event.id, MeaningRelationKind::Agent)?.id;
        let complement = relation_node(meaning, event.id, MeaningRelationKind::Complement);
        let patient = relation_node(
            meaning,
            complement.map_or(event.id, |node| node.id),
            MeaningRelationKind::Patient,
        )
        .map(|node| node.id);
        let mut tree = ChineseSyntaxTree::default();
        let clause = add_ch(&mut tree, ChineseNodeKind::Clause, String::new());
        tree.root = clause;
        if meaning.passive {
            let patient = self.entity(&mut tree, meaning, patient?)?;
            mark_role(&mut tree, patient, "Patient");
            tree.nodes[clause].children.push(patient);
            let passive = add_ch(&mut tree, ChineseNodeKind::Passive, "被".to_string());
            tree.nodes[clause].children.push(passive);
            if let Some(agent) = relation_node(meaning, event.id, MeaningRelationKind::Agent) {
                let agent = self.entity(&mut tree, meaning, agent.id)?;
                mark_role(&mut tree, agent, "Agent");
                tree.nodes[clause].children.push(agent);
            }
            let verb = add_ch(
                &mut tree,
                ChineseNodeKind::Verb,
                self.target(event)?.to_string(),
            );
            tree.nodes[verb].source = Some(event.id);
            tree.nodes[clause].children.push(verb);
            let aspect = add_ch(&mut tree, ChineseNodeKind::Aspect, "了".to_string());
            tree.nodes[clause].children.push(aspect);
            return Some(tree);
        }
        let subject = self.entity(&mut tree, meaning, agent)?;
        mark_role(&mut tree, subject, "Agent");
        tree.nodes[clause].children.push(subject);
        for relation in meaning.relations.iter().filter(|relation| {
            relation.head == event.id && relation.kind == MeaningRelationKind::Time
        }) {
            let time = meaning.nodes.get(relation.dependent)?;
            let text = self.target(time)?.to_string();
            let node = add_ch(&mut tree, ChineseNodeKind::Time, text);
            tree.nodes[node].source = Some(time.id);
            tree.nodes[clause].children.push(node);
        }
        let negated = meaning
            .relations
            .iter()
            .any(|r| r.kind == MeaningRelationKind::Scope && r.dependent == event.id);
        if negated {
            let text = if meaning.progressive {
                "没在"
            } else if event.lemma == "have" || meaning.tense_past {
                "没"
            } else {
                "不"
            };
            let node = add_ch(&mut tree, ChineseNodeKind::Negation, text.to_string());
            tree.nodes[node].source = meaning
                .relations
                .iter()
                .find(|relation| {
                    relation.kind == MeaningRelationKind::Scope && relation.dependent == event.id
                })
                .map(|relation| relation.head);
            tree.nodes[clause].children.push(node);
        }
        if let Some(modal) = &meaning.modal {
            let m = add_ch(
                &mut tree,
                ChineseNodeKind::Modal,
                if modal == "can" { "会" } else { "" }.to_string(),
            );
            tree.nodes[clause].children.push(m);
        }
        if meaning.future {
            let m = add_ch(
                &mut tree,
                ChineseNodeKind::Modal,
                if meaning.conditional { "就" } else { "会" }.to_string(),
            );
            tree.nodes[clause].children.push(m);
        }
        if meaning.progressive && !negated {
            let asp = add_ch(&mut tree, ChineseNodeKind::Aspect, "正在".to_string());
            tree.nodes[clause].children.push(asp);
        }
        let go_home = matches!(event.lemma.as_str(), "go" | "come")
            && relation_node(meaning, event.id, MeaningRelationKind::Location)
                .is_some_and(|location| location.lemma == "home");
        let verb = add_ch(
            &mut tree,
            ChineseNodeKind::Verb,
            if go_home { "回" } else { self.target(event)? }.to_string(),
        );
        tree.nodes[verb].source = Some(event.id);
        tree.nodes[clause].children.push(verb);
        if let Some(complement) = relation_node(meaning, event.id, MeaningRelationKind::Complement)
        {
            let c = add_ch(
                &mut tree,
                ChineseNodeKind::Verb,
                self.target(complement)?.to_string(),
            );
            tree.nodes[c].source = Some(complement.id);
            tree.nodes[clause].children.push(c);
        }
        if meaning.tense_past && !negated && !go_home {
            let aspect = add_ch(&mut tree, ChineseNodeKind::Aspect, "了".to_string());
            tree.nodes[clause].children.push(aspect);
        }
        for relation in meaning.relations.iter().filter(|relation| {
            relation.head == event.id
                && matches!(
                    relation.kind,
                    MeaningRelationKind::Location | MeaningRelationKind::Instrument
                )
        }) {
            let object = meaning.nodes.get(relation.dependent)?;
            let Some(prep_relation) = meaning.relations.iter().find(|candidate| {
                candidate.head == object.id && candidate.kind == MeaningRelationKind::Preposition
            }) else {
                continue;
            };
            let prep = meaning.nodes.get(prep_relation.dependent)?;
            let object_np = self.entity(&mut tree, meaning, object.id)?;
            let go_to = event.lemma == "go" && prep.lemma == "to";
            let prefix = if go_to {
                ""
            } else {
                match prep.lemma.as_str() {
                    "with" => "用",
                    "to" => "到",
                    "in" | "at" | "on" => "在",
                    other => self.lexicon.zh(other, "preposition").unwrap_or(other),
                }
            };
            let prep_node = add_ch(&mut tree, ChineseNodeKind::Preposition, prefix.to_string());
            tree.nodes[prep_node].source = Some(prep.id);
            tree.nodes[clause].children.push(prep_node);
            tree.nodes[clause].children.push(object_np);
        }
        if let Some(location) = relation_node(meaning, event.id, MeaningRelationKind::Location) {
            let text = if event.lemma == "stay" && location.lemma == "home" {
                "在家里".to_string()
            } else {
                self.target(location)?.to_string()
            };
            let p = add_ch(&mut tree, ChineseNodeKind::Place, text);
            tree.nodes[p].source = Some(location.id);
            tree.nodes[clause].children.push(p);
            if go_home && meaning.tense_past && !negated {
                let aspect = add_ch(&mut tree, ChineseNodeKind::Aspect, "了".to_string());
                tree.nodes[clause].children.push(aspect);
            }
        }
        if let Some(patient) = patient {
            let object = self.entity(&mut tree, meaning, patient)?;
            mark_role(&mut tree, object, "Patient");
            tree.nodes[clause].children.push(object);
        }
        Some(tree)
    }

    fn entity(
        &self,
        tree: &mut ChineseSyntaxTree,
        meaning: &MeaningGraph,
        id: usize,
    ) -> Option<usize> {
        if let Some(relation) = meaning.relations.iter().find(|relation| {
            relation.kind == MeaningRelationKind::Coordination
                && (relation.head == id || relation.dependent == id)
        }) {
            let ids = if relation.head == id {
                [relation.head, relation.dependent]
            } else {
                [relation.dependent, relation.head]
            };
            let np = add_ch(tree, ChineseNodeKind::NounPhrase, String::new());
            for (index, entity_id) in ids.into_iter().enumerate() {
                if index > 0 {
                    let conjunction = add_ch(tree, ChineseNodeKind::Conjunction, "和".to_string());
                    tree.nodes[np].children.push(conjunction);
                }
                let child = self.entity_single(tree, meaning, entity_id)?;
                tree.nodes[np].children.push(child);
            }
            return Some(np);
        }
        self.entity_single(tree, meaning, id)
    }

    fn entity_single(
        &self,
        tree: &mut ChineseSyntaxTree,
        meaning: &MeaningGraph,
        id: usize,
    ) -> Option<usize> {
        let node = meaning.nodes.get(id)?;
        if node.kind != MeaningNodeKind::Entity {
            return None;
        }
        let np = add_ch(tree, ChineseNodeKind::NounPhrase, String::new());
        if let Some(quantity) = relation_node(meaning, id, MeaningRelationKind::Quantity) {
            if let Ok(value) = quantity.lemma.parse::<u32>() {
                let q = add_ch(tree, ChineseNodeKind::Quantity, numeral_zh(value));
                tree.nodes[q].source = Some(quantity.id);
                tree.nodes[np].children.push(q);
                if let Some(classifier) = self.lexicon.classifier(&node.lemma) {
                    let c = add_ch(tree, ChineseNodeKind::Classifier, classifier.to_string());
                    tree.nodes[np].children.push(c);
                }
            }
        }
        for relation in meaning.relations.iter().filter(|relation| {
            relation.head == id && relation.kind == MeaningRelationKind::Modifier
        }) {
            let modifier = meaning.nodes.get(relation.dependent)?;
            let mut modifier_text = self.lexicon.zh(&modifier.lemma, &modifier.pos)?.to_string();
            if modifier_text.chars().count() > 1 {
                modifier_text.push('的');
            }
            let m = add_ch(tree, ChineseNodeKind::Modifier, modifier_text);
            tree.nodes[m].source = Some(modifier.id);
            tree.nodes[np].children.push(m);
        }
        for relation in meaning.relations.iter().filter(|relation| {
            relation.head == id && relation.kind == MeaningRelationKind::Relative
        }) {
            let event = meaning.nodes.get(relation.dependent)?;
            let patient = meaning
                .relations
                .iter()
                .find(|candidate| {
                    candidate.head == event.id && candidate.kind == MeaningRelationKind::Patient
                })
                .and_then(|candidate| meaning.nodes.get(candidate.dependent));
            let verb_text = self.target(event)?;
            let text = if let Some(patient) = patient {
                format!("{}{}的", verb_text, self.target(patient)?)
            } else {
                format!("{}的", verb_text)
            };
            let modifier = add_ch(tree, ChineseNodeKind::Modifier, text);
            tree.nodes[modifier].source = Some(event.id);
            tree.nodes[np].children.push(modifier);
            let hidden_verb = add_ch(tree, ChineseNodeKind::Verb, String::new());
            tree.nodes[hidden_verb].source = Some(event.id);
            tree.nodes[np].children.push(hidden_verb);
            if let Some(patient) = patient {
                let hidden_noun = add_ch(tree, ChineseNodeKind::Noun, String::new());
                tree.nodes[hidden_noun].source = Some(patient.id);
                tree.nodes[np].children.push(hidden_noun);
            }
        }
        for relation in meaning.relations.iter().filter(|relation| {
            relation.head == id && relation.kind == MeaningRelationKind::PpModifier
        }) {
            let object = meaning.nodes.get(relation.dependent)?;
            let text = format!("有{}的", self.target(object)?);
            let modifier = add_ch(tree, ChineseNodeKind::Modifier, text);
            tree.nodes[modifier].source = Some(object.id);
            tree.nodes[np].children.push(modifier);
            if let Some(prep_relation) = meaning.relations.iter().find(|candidate| {
                candidate.head == object.id && candidate.kind == MeaningRelationKind::Preposition
            }) {
                let hidden = add_ch(tree, ChineseNodeKind::Preposition, String::new());
                tree.nodes[hidden].source = Some(prep_relation.dependent);
                tree.nodes[np].children.push(hidden);
            }
        }
        let noun_text = if node.plural && node.lemma == "brother" {
            "兄弟".to_string()
        } else {
            self.target(node)?.to_string()
        };
        let noun = add_ch(tree, ChineseNodeKind::Noun, noun_text);
        tree.nodes[noun].source = Some(node.id);
        tree.nodes[np].children.push(noun);
        Some(np)
    }
}

fn np_lemma(np: &Np) -> &str {
    match np {
        Np::Pronoun { surface, .. } | Np::Noun { surface, .. } => surface,
    }
}

fn relation_node(
    meaning: &MeaningGraph,
    head: usize,
    kind: MeaningRelationKind,
) -> Option<&MeaningNode> {
    let relation = meaning
        .relations
        .iter()
        .find(|r| r.head == head && r.kind == kind)?;
    meaning.nodes.get(relation.dependent)
}

fn add_ch(tree: &mut ChineseSyntaxTree, kind: ChineseNodeKind, text: String) -> usize {
    let id = tree.nodes.len();
    tree.nodes.push(ChineseNode {
        id,
        kind,
        text,
        children: Vec::new(),
        source: None,
        role: None,
    });
    id
}

fn numeral_zh(value: u32) -> String {
    match value {
        1 => "一".to_string(),
        2 => "两".to_string(),
        3 => "三".to_string(),
        4 => "四".to_string(),
        5 => "五".to_string(),
        6 => "六".to_string(),
        7 => "七".to_string(),
        8 => "八".to_string(),
        9 => "九".to_string(),
        10 => "十".to_string(),
        n => n.to_string(),
    }
}

pub fn render(tree: &ChineseSyntaxTree) -> String {
    if let Some(parts) = &tree.coordination {
        let text = parts
            .iter()
            .map(|part| render(part).trim_end_matches(['.', '。']).to_string())
            .collect::<Vec<_>>()
            .join("，");
        return format!("{}。", text);
    }
    if let Some(condition) = &tree.condition {
        let cond = render(condition).trim_end_matches(['.', '。']).to_string();
        let main = render_base(tree);
        return format!("如果{}，{}。", cond, main.trim_end_matches(['.', '。']));
    }
    render_base(tree)
}

fn render_base(tree: &ChineseSyntaxTree) -> String {
    let mut out = String::new();
    render_node(tree, tree.root, &mut out);
    out.trim_end_matches('。').to_string() + "。"
}

fn mark_role(tree: &mut ChineseSyntaxTree, np: usize, role: &str) {
    let children = tree.nodes[np].children.clone();
    for child in children {
        if tree.nodes[child].kind == ChineseNodeKind::Noun {
            tree.nodes[child].role = Some(role.to_string());
        } else if tree.nodes[child].kind == ChineseNodeKind::NounPhrase {
            mark_role(tree, child, role);
        }
    }
}

fn render_node(tree: &ChineseSyntaxTree, id: usize, out: &mut String) {
    let node = &tree.nodes[id];
    if matches!(
        node.kind,
        ChineseNodeKind::Clause | ChineseNodeKind::NounPhrase
    ) {
        for child in &node.children {
            render_node(tree, *child, out);
        }
    } else {
        out.push_str(&node.text);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub meaning: usize,
    pub chinese: usize,
    pub relation: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationMapping {
    pub meaning_relation: String,
    pub target_role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationReport {
    pub valid: bool,
    pub mapped_meanings: usize,
    pub total_meanings: usize,
    pub mappings: Vec<Mapping>,
    pub relation_mappings: Vec<RelationMapping>,
}

fn noun_in_np(tree: &ChineseSyntaxTree, np: usize, source: usize) -> bool {
    tree.nodes.get(np).is_some_and(|node| {
        node.children.iter().any(|child| {
            tree.nodes.get(*child).is_some_and(|child| {
                child.kind == ChineseNodeKind::Noun && child.source == Some(source)
            })
        })
    })
}

pub fn validate(meaning: &MeaningGraph, tree: &ChineseSyntaxTree) -> ValidationReport {
    let mut target_nodes: Vec<&ChineseNode> = tree.nodes.iter().collect();
    if let Some(condition) = &tree.condition {
        target_nodes.extend(condition.nodes.iter());
    }
    let mut mappings = Vec::new();
    let mut used = vec![false; target_nodes.len()];
    for (idx, node) in meaning.nodes.iter().enumerate() {
        let expected = match node.kind {
            MeaningNodeKind::Event => ChineseNodeKind::Verb,
            MeaningNodeKind::Entity => ChineseNodeKind::Noun,
            MeaningNodeKind::Time if node.lemma == "home" => ChineseNodeKind::Place,
            MeaningNodeKind::Time => ChineseNodeKind::Time,
            MeaningNodeKind::Operator => ChineseNodeKind::Negation,
            MeaningNodeKind::Property if node.lemma.chars().all(|c| c.is_ascii_digit()) => {
                ChineseNodeKind::Quantity
            }
            MeaningNodeKind::Property => ChineseNodeKind::Modifier,
            MeaningNodeKind::Preposition => ChineseNodeKind::Preposition,
        };
        if let Some(chinese) = target_nodes.iter().enumerate().find(|(i, n)| {
            !used[*i] && (n.source == Some(idx) || (n.source.is_none() && n.kind == expected))
        }) {
            used[chinese.0] = true;
            mappings.push(Mapping {
                meaning: idx,
                chinese: chinese.1.id,
                relation: format!("{:?}", expected),
            });
        }
    }
    let mut relation_mappings = Vec::new();
    for relation in &meaning.relations {
        let has = |kind: ChineseNodeKind, source: usize| {
            target_nodes
                .iter()
                .any(|node| node.kind == kind && node.source == Some(source))
        };
        let clause_nps: Vec<usize> = tree
            .nodes
            .get(tree.root)
            .map(|root| {
                root.children
                    .iter()
                    .copied()
                    .filter(|id| tree.nodes[*id].kind == ChineseNodeKind::NounPhrase)
                    .collect()
            })
            .unwrap_or_default();
        let subject_np = clause_nps.first().copied();
        let object_np = clause_nps.get(1).copied();
        let agent_in_role = |source: usize| {
            if meaning.passive {
                clause_nps
                    .get(1)
                    .is_some_and(|np| noun_in_np(tree, *np, source))
            } else {
                subject_np.is_some_and(|np| noun_in_np(tree, np, source))
            }
        };
        let patient_in_role = |source: usize| {
            if meaning.passive {
                subject_np.is_some_and(|np| noun_in_np(tree, np, source))
            } else {
                object_np.is_some_and(|np| noun_in_np(tree, np, source))
            }
        };
        let role = match relation.kind {
            MeaningRelationKind::Agent => {
                (meaning
                    .relations
                    .iter()
                    .any(|candidate| candidate.kind == MeaningRelationKind::Coordination)
                    || agent_in_role(relation.dependent)
                    || tree.condition.as_ref().is_some_and(|condition| {
                        condition.nodes.iter().any(|node| {
                            node.kind == ChineseNodeKind::Noun
                                && node.source == Some(relation.dependent)
                                && node.role.as_deref() == Some("Agent")
                        })
                    }))
                    && target_nodes.iter().any(|node| {
                        node.kind == ChineseNodeKind::Noun
                            && node.source == Some(relation.dependent)
                            && node.role.as_deref() == Some("Agent")
                    })
                    && has(ChineseNodeKind::Verb, relation.head)
            }
            MeaningRelationKind::Patient => {
                let relative_patient = meaning.relations.iter().any(|candidate| {
                    candidate.kind == MeaningRelationKind::Relative
                        && candidate.dependent == relation.head
                });
                let coordinated_patient = meaning.relations.iter().any(|candidate| {
                    candidate.kind == MeaningRelationKind::Coordination
                        && candidate.head == relation.dependent
                });
                if relative_patient || coordinated_patient {
                    target_nodes.iter().any(|node| {
                        node.kind == ChineseNodeKind::Noun
                            && node.source == Some(relation.dependent)
                    })
                } else {
                    patient_in_role(relation.dependent)
                        && target_nodes.iter().any(|node| {
                            node.kind == ChineseNodeKind::Noun
                                && node.source == Some(relation.dependent)
                                && node.role.as_deref() == Some("Patient")
                        })
                }
            }
            MeaningRelationKind::Quantity => has(ChineseNodeKind::Quantity, relation.dependent),
            MeaningRelationKind::Modifier => has(ChineseNodeKind::Modifier, relation.dependent),
            MeaningRelationKind::Time => has(ChineseNodeKind::Time, relation.dependent),
            MeaningRelationKind::Location => {
                has(ChineseNodeKind::Place, relation.dependent)
                    || has(ChineseNodeKind::Noun, relation.dependent)
            }
            MeaningRelationKind::Instrument => has(ChineseNodeKind::Noun, relation.dependent),
            MeaningRelationKind::Preposition => {
                has(ChineseNodeKind::Preposition, relation.dependent)
            }
            MeaningRelationKind::Scope => has(ChineseNodeKind::Negation, relation.head),
            MeaningRelationKind::Complement => has(ChineseNodeKind::Verb, relation.dependent),
            MeaningRelationKind::Condition => tree.condition.is_some(),
            MeaningRelationKind::Coordination => {
                tree.nodes
                    .iter()
                    .filter(|node| node.kind == ChineseNodeKind::Noun)
                    .count()
                    >= 2
            }
            MeaningRelationKind::PpModifier => has(ChineseNodeKind::Modifier, relation.dependent),
            MeaningRelationKind::Relative => has(ChineseNodeKind::Modifier, relation.dependent),
            MeaningRelationKind::ClauseCoordination => tree.coordination.is_some(),
        };
        if role {
            relation_mappings.push(RelationMapping {
                meaning_relation: format!("{:?}", relation.kind),
                target_role: format!("{:?}", relation.kind),
            });
        }
    }
    let valid = !meaning.nodes.is_empty()
        && tree.root < tree.nodes.len()
        && mappings.len() == meaning.nodes.len()
        && relation_mappings.len() == meaning.relations.len();
    ValidationReport {
        valid,
        mapped_meanings: mappings.len(),
        total_meanings: meaning.nodes.len(),
        mappings,
        relation_mappings,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParserRoute {
    Chart,
    LegacyLinear,
}

pub struct PipelineOutput {
    pub tokens: TokenStream,
    pub route: ParserRoute,
    pub candidates: usize,
    pub candidate_score: u32,
    pub legacy_reason: Option<String>,
    pub condition: Option<Box<PipelineOutput>>,
    pub syntax: SyntaxGraph,
    pub meaning: MeaningGraph,
    pub chinese: ChineseSyntaxTree,
    pub text: String,
    pub validation: ValidationReport,
}

fn meaning_signature(meaning: &MeaningGraph) -> Vec<String> {
    let mut signature: Vec<String> = meaning
        .relations
        .iter()
        .map(|relation| {
            format!(
                "{:?}:{}->{}",
                relation.kind,
                meaning.nodes[relation.head].lemma,
                meaning.nodes[relation.dependent].lemma
            )
        })
        .collect();
    signature.sort();
    signature
}

pub fn analyze(source: &str, lexicon: &CompositionEngine) -> Option<PipelineOutput> {
    let trimmed = source.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("if ") {
        if let Some((condition, main)) = trimmed.split_once(',') {
            let condition = condition
                .trim_start_matches("If ")
                .trim_start_matches("if ")
                .trim();
            return analyze_conditional(condition, main.trim(), lexicon);
        }
    }
    analyze_single(source, lexicon)
}

fn analyze_conditional(
    condition_source: &str,
    main_source: &str,
    lexicon: &CompositionEngine,
) -> Option<PipelineOutput> {
    let condition = analyze_single(condition_source, lexicon)?;
    let mut main = analyze_single(main_source, lexicon)?;
    main.meaning.conditional = true;
    for node in &mut main.chinese.nodes {
        if node.kind == ChineseNodeKind::Modal && node.text == "会" {
            node.text = "就".to_string();
        }
    }
    let offset = main.meaning.nodes.len();
    let condition_event = condition
        .meaning
        .nodes
        .iter()
        .find(|node| node.kind == MeaningNodeKind::Event)
        .map(|node| node.id)?;
    for node in &condition.meaning.nodes {
        let mut node = node.clone();
        node.id += offset;
        main.meaning.nodes.push(node);
    }
    for relation in &condition.meaning.relations {
        let mut relation = relation.clone();
        relation.head += offset;
        relation.dependent += offset;
        main.meaning.relations.push(relation);
    }
    let main_event = main
        .meaning
        .nodes
        .iter()
        .find(|node| node.kind == MeaningNodeKind::Event && node.id < offset)
        .map(|node| node.id)?;
    main.meaning.relations.push(MeaningRelation {
        head: condition_event + offset,
        dependent: main_event,
        kind: MeaningRelationKind::Condition,
    });
    // Keep the condition as a real target subtree, not a string prefix.
    let mut condition_chinese = condition.chinese.clone();
    let subject_id = condition_chinese.nodes.iter().find_map(|node| {
        (node.kind == ChineseNodeKind::NounPhrase
            && node
                .children
                .iter()
                .any(|id| condition_chinese.nodes[*id].text == "它"))
        .then_some(node.id)
    });
    if let Some(subject) = subject_id {
        let children = condition_chinese.nodes[subject].children.clone();
        for child in children {
            if condition_chinese.nodes[child].kind == ChineseNodeKind::Noun {
                condition_chinese.nodes[child].text.clear();
            }
        }
    }
    for node in &mut condition_chinese.nodes {
        if let Some(source) = node.source.as_mut() {
            *source += offset;
        }
    }
    main.chinese.condition = Some(Box::new(condition_chinese));
    let validation = validate(&main.meaning, &main.chinese);
    if !validation.valid {
        return None;
    }
    main.validation = validation;
    main.text = render(&main.chinese);
    main.condition = Some(Box::new(condition));
    Some(main)
}

fn clause_coordination_parts(graph: &SyntaxGraph) -> Option<Vec<SyntaxGraph>> {
    let clause = graph
        .nodes
        .iter()
        .find(|node| node.kind == SyntaxNodeKind::Clause)?;
    let conjuncts: Vec<usize> = graph
        .edges
        .iter()
        .filter(|edge| edge.head == clause.id && edge.relation == SyntaxRelation::Conjunct)
        .map(|edge| edge.dependent)
        .filter(|id| {
            graph
                .nodes
                .get(*id)
                .is_some_and(|node| node.kind == SyntaxNodeKind::Clause)
        })
        .collect();
    if conjuncts.len() < 2 {
        return None;
    }
    let mut parts = Vec::new();
    for root in conjuncts.into_iter().take(2) {
        let mut nodes = Vec::new();
        let mut stack = vec![root];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(node) = graph.nodes.get(id) {
                nodes.push(node.clone());
                for edge in graph.edges.iter().filter(|edge| edge.head == id) {
                    stack.push(edge.dependent);
                }
            }
        }
        nodes.sort_by_key(|node| node.id);
        let remap: HashMap<usize, usize> = nodes
            .iter()
            .enumerate()
            .map(|(new, node)| (node.id, new))
            .collect();
        let mut part = SyntaxGraph::default();
        for (new, mut node) in nodes.into_iter().enumerate() {
            node.id = new;
            part.nodes.push(node);
        }
        for edge in &graph.edges {
            if let (Some(head), Some(dependent)) =
                (remap.get(&edge.head), remap.get(&edge.dependent))
            {
                part.edges.push(SyntaxEdge {
                    head: *head,
                    dependent: *dependent,
                    relation: edge.relation.clone(),
                });
            }
        }
        parts.push(part);
    }
    Some(parts)
}

fn analyze_single(source: &str, lexicon: &CompositionEngine) -> Option<PipelineOutput> {
    let key = source
        .trim_end_matches(['.', '?', '!', '。', '？'])
        .trim()
        .to_ascii_lowercase();
    if lexicon.phrase(&key).is_some() {
        return None;
    }
    let stream = tokenize_lossless(source);
    if stream.coverage.complete {
        let mut candidates = Vec::new();
        for graph in (ChartParser { lexicon }).parse_candidates(&stream) {
            if let Some(parts) = clause_coordination_parts(&graph) {
                let mut outputs = Vec::new();
                for part in parts {
                    let meaning = (SemanticTransfer { lexicon }).transfer_graph(&part)?;
                    let chinese = (TargetTransfer { lexicon }).transfer(&meaning)?;
                    let validation = validate(&meaning, &chinese);
                    if !validation.valid {
                        return None;
                    }
                    let text = render(&chinese);
                    outputs.push(PipelineOutput {
                        tokens: stream.clone(),
                        route: ParserRoute::Chart,
                        candidates: 0,
                        candidate_score: meaning.relations.len() as u32 * 10
                            + part.edges.len() as u32,
                        legacy_reason: None,
                        condition: None,
                        syntax: part,
                        meaning,
                        chinese,
                        text,
                        validation,
                    });
                }
                if outputs.len() == 2 {
                    let right = outputs.pop()?;
                    let mut left = outputs.pop()?;
                    let offset = left.meaning.nodes.len();
                    let mut right_nodes = right.meaning.nodes.clone();
                    for node in &mut right_nodes {
                        node.id += offset;
                    }
                    left.meaning.nodes.extend(right_nodes);
                    for relation in &right.meaning.relations {
                        let mut relation = relation.clone();
                        relation.head += offset;
                        relation.dependent += offset;
                        left.meaning.relations.push(relation);
                    }
                    let left_event = left
                        .meaning
                        .nodes
                        .iter()
                        .find(|node| node.kind == MeaningNodeKind::Event)
                        .map(|node| node.id)?;
                    let right_event = left
                        .meaning
                        .nodes
                        .iter()
                        .rev()
                        .find(|node| node.kind == MeaningNodeKind::Event)
                        .map(|node| node.id)?;
                    left.meaning.relations.push(MeaningRelation {
                        head: left_event,
                        dependent: right_event,
                        kind: MeaningRelationKind::ClauseCoordination,
                    });
                    let base = left.chinese.clone();
                    let mut chinese = left.chinese.clone();
                    chinese.coordination = Some(vec![base, right.chinese.clone()]);
                    left.chinese = chinese;
                    left.text = render(&left.chinese);
                    left.validation.valid = left.validation.valid && right.validation.valid;
                    left.syntax = graph;
                    left.candidates = 2;
                    return Some(left);
                }
            }
            if let Some(meaning) = (SemanticTransfer { lexicon }).transfer_graph(&graph) {
                if let Some(chinese) = (TargetTransfer { lexicon }).transfer(&meaning) {
                    let validation = validate(&meaning, &chinese);
                    if validation.valid {
                        let text = render(&chinese);
                        candidates.push(PipelineOutput {
                            tokens: stream.clone(),
                            route: ParserRoute::Chart,
                            candidates: 0,
                            candidate_score: meaning.relations.len() as u32 * 10
                                + graph.edges.len() as u32,
                            legacy_reason: None,
                            condition: None,
                            syntax: graph,
                            meaning,
                            chinese,
                            text,
                            validation,
                        });
                    }
                }
            }
        }
        if !candidates.is_empty() {
            let best_index = candidates
                .iter()
                .enumerate()
                .max_by_key(|(_, candidate)| candidate.candidate_score)
                .map(|(index, _)| index)
                .unwrap_or(0);
            let first = candidates.remove(best_index);
            let signature = meaning_signature(&first.meaning);
            if candidates.iter().any(|candidate| {
                candidate.text != first.text || meaning_signature(&candidate.meaning) != signature
            }) {
                // Ambiguous analyses with different semantics must not win by order.
                return None;
            }
            let mut output = first;
            output.candidates = candidates.len() + 1;
            return Some(output);
        }
    }
    let syntax = SyntaxAnalyzer { lexicon }.analyze(source)?;
    let meaning = SemanticTransfer { lexicon }.transfer(&syntax)?;
    if meaning.nodes.iter().any(|node| {
        matches!(
            node.sense,
            SenseResolution::Unresolved | SenseResolution::Ambiguous(_)
        )
    }) {
        return None;
    }
    let chinese = (TargetTransfer { lexicon }).transfer(&meaning)?;
    let validation = validate(&meaning, &chinese);
    if !validation.valid {
        return None;
    }
    let text = render(&chinese);
    Some(PipelineOutput {
        tokens: syntax.stream.clone(),
        route: ParserRoute::LegacyLinear,
        candidates: 1,
        candidate_score: 0,
        legacy_reason: Some("chart-unsupported".to_string()),
        condition: None,
        syntax: syntax.graph.clone(),
        meaning,
        chinese,
        text,
        validation,
    })
}

pub fn compose(
    source: &str,
    lexicon: &CompositionEngine,
    _entries: &HashMap<String, Vec<DictionaryEntry>>,
) -> Option<SemanticOutcome> {
    let output = analyze(source, lexicon)?;
    let mut evidence =
        LanguageEvidence::for_text(TextSpan::new(0, source.len()), UnitKind::Sentence, source);
    evidence.add_pattern(PatternMatch {
        kind: PatternKind::Rule,
        id: "pipeline.structured.svo".to_string(),
        span: TextSpan::new(0, source.len()),
        complete: output.tokens.coverage.complete,
    });
    Some(SemanticOutcome {
        text: output.text,
        rule_id: "pipeline.structured.svo".to_string(),
        pattern: "structured-svo".to_string(),
        evidence,
        alternatives: Vec::new(),
    })
}
