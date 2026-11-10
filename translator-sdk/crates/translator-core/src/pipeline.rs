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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxNode {
    pub id: usize,
    pub span: TextSpan,
    pub kind: SyntaxNodeKind,
    pub surface: String,
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
    graph.nodes.push(SyntaxNode {
        id,
        span,
        kind,
        surface: surface.into(),
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

pub struct SemanticTransfer<'a> {
    pub lexicon: &'a CompositionEngine,
}

impl<'a> SemanticTransfer<'a> {
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChineseNode {
    pub id: usize,
    pub kind: ChineseNodeKind,
    pub text: String,
    pub children: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChineseSyntaxTree {
    pub nodes: Vec<ChineseNode>,
    pub root: usize,
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
            tree.nodes[clause].children.push(patient);
            let passive = add_ch(&mut tree, ChineseNodeKind::Passive, "被".to_string());
            tree.nodes[clause].children.push(passive);
            if let Some(agent) = relation_node(meaning, event.id, MeaningRelationKind::Agent) {
                let agent = self.entity(&mut tree, meaning, agent.id)?;
                tree.nodes[clause].children.push(agent);
            }
            let verb = add_ch(
                &mut tree,
                ChineseNodeKind::Verb,
                self.target(event)?.to_string(),
            );
            tree.nodes[clause].children.push(verb);
            let aspect = add_ch(&mut tree, ChineseNodeKind::Aspect, "了".to_string());
            tree.nodes[clause].children.push(aspect);
            return Some(tree);
        }
        let subject = self.entity(&mut tree, meaning, agent)?;
        tree.nodes[clause].children.push(subject);
        if let Some(time) = relation_node(meaning, event.id, MeaningRelationKind::Time) {
            let text = self.target(time)?.to_string();
            let node = add_ch(&mut tree, ChineseNodeKind::Time, text);
            tree.nodes[clause].children.push(node);
        }
        let negated = meaning
            .relations
            .iter()
            .any(|r| r.kind == MeaningRelationKind::Scope && r.dependent == event.id);
        if negated {
            let text = if event.lemma == "have" || meaning.tense_past {
                "没"
            } else {
                "不"
            };
            let node = add_ch(&mut tree, ChineseNodeKind::Negation, text.to_string());
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
        if meaning.progressive {
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
        tree.nodes[clause].children.push(verb);
        if let Some(complement) = relation_node(meaning, event.id, MeaningRelationKind::Complement)
        {
            let c = add_ch(
                &mut tree,
                ChineseNodeKind::Verb,
                self.target(complement)?.to_string(),
            );
            tree.nodes[clause].children.push(c);
        }
        if meaning.tense_past && !negated && !go_home {
            let aspect = add_ch(&mut tree, ChineseNodeKind::Aspect, "了".to_string());
            tree.nodes[clause].children.push(aspect);
        }
        if let Some(location) = relation_node(meaning, event.id, MeaningRelationKind::Location) {
            let text = if event.lemma == "stay" && location.lemma == "home" {
                "在家里".to_string()
            } else {
                self.target(location)?.to_string()
            };
            let p = add_ch(&mut tree, ChineseNodeKind::Place, text);
            tree.nodes[clause].children.push(p);
            if go_home && meaning.tense_past && !negated {
                let aspect = add_ch(&mut tree, ChineseNodeKind::Aspect, "了".to_string());
                tree.nodes[clause].children.push(aspect);
            }
        }
        if let Some(patient) = patient {
            let object = self.entity(&mut tree, meaning, patient)?;
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
        let node = meaning.nodes.get(id)?;
        if node.kind != MeaningNodeKind::Entity {
            return None;
        }
        let np = add_ch(tree, ChineseNodeKind::NounPhrase, String::new());
        if let Some(quantity) = relation_node(meaning, id, MeaningRelationKind::Quantity) {
            if let Ok(value) = quantity.lemma.parse::<u32>() {
                let q = add_ch(tree, ChineseNodeKind::Quantity, numeral_zh(value));
                tree.nodes[np].children.push(q);
                if let Some(classifier) = self.lexicon.classifier(&node.lemma) {
                    let c = add_ch(tree, ChineseNodeKind::Classifier, classifier.to_string());
                    tree.nodes[np].children.push(c);
                }
            }
        }
        if let Some(modifier) = relation_node(meaning, id, MeaningRelationKind::Modifier) {
            let mut modifier_text = self.lexicon.zh(&modifier.lemma, &modifier.pos)?.to_string();
            if modifier_text.chars().count() > 1 {
                modifier_text.push('的');
            }
            let m = add_ch(tree, ChineseNodeKind::Modifier, modifier_text);
            tree.nodes[np].children.push(m);
        }
        let noun_text = if node.plural && node.lemma == "brother" {
            "兄弟".to_string()
        } else {
            self.target(node)?.to_string()
        };
        let noun = add_ch(tree, ChineseNodeKind::Noun, noun_text);
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
    let mut out = String::new();
    render_node(tree, tree.root, &mut out);
    out.trim_end_matches('。').to_string() + "。"
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
pub struct ValidationReport {
    pub valid: bool,
    pub mapped_meanings: usize,
    pub total_meanings: usize,
    pub mappings: Vec<Mapping>,
}

pub fn validate(meaning: &MeaningGraph, tree: &ChineseSyntaxTree) -> ValidationReport {
    let mut mappings = Vec::new();
    let mut used = vec![false; tree.nodes.len()];
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
        };
        if let Some(chinese) = tree
            .nodes
            .iter()
            .enumerate()
            .find(|(i, n)| !used[*i] && n.kind == expected)
        {
            used[chinese.0] = true;
            mappings.push(Mapping {
                meaning: idx,
                chinese: chinese.1.id,
                relation: format!("{:?}", expected),
            });
        }
    }
    let valid = !meaning.nodes.is_empty()
        && tree.root < tree.nodes.len()
        && mappings.len() == meaning.nodes.len()
        && !meaning.relations.is_empty();
    ValidationReport {
        valid,
        mapped_meanings: mappings.len(),
        total_meanings: meaning.nodes.len(),
        mappings,
    }
}

#[derive(Clone, Debug)]
pub struct PipelineOutput {
    pub tokens: TokenStream,
    pub condition: Option<Box<PipelineOutput>>,
    pub syntax: SyntaxGraph,
    pub meaning: MeaningGraph,
    pub chinese: ChineseSyntaxTree,
    pub text: String,
    pub validation: ValidationReport,
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
    main.text = render(&main.chinese);
    let condition_text = condition
        .text
        .trim_end_matches(['.', '。'])
        .trim_start_matches('它')
        .to_string();
    let main_text = main.text.trim_end_matches(['.', '。']);
    let text = format!("如果{condition_text}，{main_text}。");
    let mut output = main;
    output.condition = Some(Box::new(condition));
    output.text = text;
    Some(output)
}

fn analyze_single(source: &str, lexicon: &CompositionEngine) -> Option<PipelineOutput> {
    let key = source
        .trim_end_matches(['.', '?', '!', '。', '？'])
        .trim()
        .to_ascii_lowercase();
    if lexicon.phrase(&key).is_some() {
        return None;
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
    let chinese = TargetTransfer { lexicon }.transfer(&meaning)?;
    let validation = validate(&meaning, &chinese);
    if !validation.valid {
        return None;
    }
    let text = render(&chinese);
    Some(PipelineOutput {
        tokens: syntax.stream.clone(),
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
