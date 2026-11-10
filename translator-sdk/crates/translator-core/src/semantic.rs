//! P20-C：局部语义分析、MeaningFrame 与独立中文生成层（第一版）。
//!
//! 设计目标不是建立通用英语 parser，而是把 P19-G 的整句字符串规则推进到：
//!
//! ```text
//! token/span → 局部实体/谓词/语义角色 → MeaningFrame → ChineseRealizer
//!            → 候选校验/选择 → Coordinator Rule 路由
//! ```
//!
//! 第一版只接入少量**必须新增分析能力**的日常结构：
//! - 两个祈使动作的并列；
//! - 否定 want-to（否定作用域在 want，不在 to 后的动词）；
//! - 过去动作 + 时间状语（多语序候选）；
//! - 固定颜色短语作定语 + be 形容词。
//!
//! 定语从句、介词附着歧义、未解决代词、嵌套从句一律拒绝，交回 P19-G/NMT。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use translator_analysis::{EnglishLemmatizer, MorphologyAnalyzer};
use translator_types::{DictionaryEntry, LanguageTag};

use crate::composer::{tokenize, CompositionEngine, CompositionToken};
use crate::evidence::{LanguageEvidence, PatternKind, PatternMatch, TextSpan, UnitKind};
use crate::templates::normalize_key;

/// 语义分析拒绝原因（内部 Trace，不急于成为公共 API）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectionReason {
    UnsupportedClause,
    PpAttachmentAmbiguous,
    EmbeddedClause,
    UnresolvedPronoun,
    UnknownWord,
    IncompleteCoverage,
    UnsupportedPattern,
}

impl RejectionReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::UnsupportedClause => "unsupported_clause",
            Self::PpAttachmentAmbiguous => "pp_attachment_ambiguous",
            Self::EmbeddedClause => "embedded_clause",
            Self::UnresolvedPronoun => "unresolved_pronoun",
            Self::UnknownWord => "unknown_word",
            Self::IncompleteCoverage => "incomplete_coverage",
            Self::UnsupportedPattern => "unsupported_pattern",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    pub span: TextSpan,
    pub reason: RejectionReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lexeme {
    pub word: String,
    pub lemma: String,
    pub zh: String,
    pub pos: String,
    pub span: TextSpan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Determiner {
    A,
    The,
    Possessive(String),
    This,
    That,
    These,
    Those,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityRef {
    pub head: Lexeme,
    pub adjectives: Vec<Lexeme>,
    pub determiner: Option<Determiner>,
    /// 显式数量（two/three…）；`None` 表示没有数量词。
    pub quantity: Option<u32>,
    pub classifier: Option<String>,
    pub plural: bool,
    pub span: TextSpan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Polarity {
    Affirmative,
    Negative,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TenseAspect {
    Simple,
    Past,
    Progressive,
    Imperative,
    Future,
    Experiential,
    Passive,
    Modal(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionType {
    None,
    YesNo,
    Wh(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modifier {
    Time(String),
    Place(String),
    Manner(String),
    Degree(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeaningFrame {
    pub predicate: Lexeme,
    pub agent: Option<EntityRef>,
    pub patient: Option<EntityRef>,
    /// 嵌入式谓词，例如 want to **go** 的 go。
    pub complement: Option<Lexeme>,
    /// 数据化动宾搭配（如 book room → 预订一个房间）。
    pub collocation: Option<String>,
    /// 比较对象（比…），仅用于 `X is heavier than Y` 这类可靠子集。
    pub comparison: Option<EntityRef>,
    pub modifiers: Vec<Modifier>,
    pub polarity: Polarity,
    pub tense_aspect: TenseAspect,
    pub question: QuestionType,
    pub source_spans: Vec<TextSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramRelation {
    Single,
    Coordination,
    Conditional,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeaningProgram {
    pub frames: Vec<MeaningFrame>,
    pub relation: ProgramRelation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticAnalysis {
    pub program: Option<MeaningProgram>,
    pub rejections: Vec<Rejection>,
    pub question: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealizationCandidate {
    pub text: String,
    pub rule_id: String,
    /// 确定性排序：越小越优先（不是语言模型分数）。
    pub rank: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticOutcome {
    pub text: String,
    pub rule_id: String,
    pub pattern: String,
    pub evidence: LanguageEvidence,
    pub alternatives: Vec<String>,
}

/// 第一版中文实现器：只接受有明确语义角色的 MeaningFrame。
#[derive(Debug, Default, Clone, Copy)]
pub struct ChineseRealizer;

impl ChineseRealizer {
    pub fn realize(&self, program: &MeaningProgram) -> Vec<RealizationCandidate> {
        match program.relation {
            ProgramRelation::Coordination => {
                let mut parts = Vec::new();
                for (index, frame) in program.frames.iter().enumerate() {
                    let text = self.realize_imperative(frame);
                    if index == 0 {
                        parts.push(text);
                    } else {
                        parts.push(text.trim_start_matches('请').to_string());
                    }
                }
                vec![candidate(
                    format!("{}。", parts.join("，并")),
                    "realize.coordination",
                    0,
                )]
            }
            ProgramRelation::Conditional => {
                let Some(condition) = program.frames.first() else {
                    return Vec::new();
                };
                let Some(main) = program.frames.get(1) else {
                    return Vec::new();
                };
                let condition_text = if condition
                    .agent
                    .as_ref()
                    .is_some_and(|agent| agent.head.zh == "它")
                    && condition.predicate.lemma == "rain"
                {
                    "如果下雨".to_string()
                } else {
                    let text = self
                        .realize_frame(condition)
                        .first()
                        .map(|c| c.text.trim_end_matches(['.', '。']).to_string())
                        .unwrap_or_default();
                    format!("如果{text}")
                };
                let main_text = self
                    .realize_frame(main)
                    .first()
                    .map(|c| c.text.trim_end_matches(['.', '。', '?', '？']).to_string())
                    .unwrap_or_default();
                vec![candidate(
                    format!("{condition_text}，{main_text}"),
                    "realize.conditional",
                    0,
                )]
            }
            ProgramRelation::Single => {
                let Some(frame) = program.frames.first() else {
                    return Vec::new();
                };
                self.realize_frame(frame)
            }
        }
    }

    fn realize_frame(&self, frame: &MeaningFrame) -> Vec<RealizationCandidate> {
        if frame.tense_aspect == TenseAspect::Imperative {
            return vec![candidate(
                self.realize_imperative(frame),
                "realize.imperative",
                0,
            )];
        }

        let subject = frame
            .agent
            .as_ref()
            .map(|entity| self.entity_zh(entity))
            .unwrap_or_default();
        let obj = frame
            .patient
            .as_ref()
            .map(|p| self.entity_zh(p))
            .unwrap_or_default();

        if let QuestionType::Wh(word) = &frame.question {
            if word == "phrase" {
                return vec![question_candidate(
                    frame.predicate.zh.clone(),
                    "realize.fixed-phrase-question",
                )];
            }
            if word == "where" {
                if let Some(agent) = &frame.agent {
                    return vec![question_candidate(
                        format!("{}在哪里", self.entity_zh(agent)),
                        "realize.where",
                    )];
                }
            }
        }
        if frame.question == QuestionType::YesNo {
            return vec![question_candidate(
                format!("{}{}{}吗", subject, frame.predicate.zh, obj),
                "realize.yes-no-question",
            )];
        }

        if matches!(frame.tense_aspect, TenseAspect::Modal(_)) {
            let modal = if matches!(frame.polarity, Polarity::Negative) {
                "不会"
            } else {
                "会"
            };
            return vec![candidate(
                format!("{}{}{}{}", subject, modal, frame.predicate.zh, obj),
                "realize.modal",
                0,
            )];
        }
        if frame.tense_aspect == TenseAspect::Progressive {
            let aspect = if matches!(frame.polarity, Polarity::Negative) {
                "没在"
            } else {
                "正在"
            };
            return vec![candidate(
                format!("{}{}{}{}", subject, aspect, frame.predicate.zh, obj),
                "realize.progressive",
                0,
            )];
        }

        if frame.predicate.pos == "adjective" {
            if let Some(than) = &frame.comparison {
                return vec![candidate(
                    format!(
                        "{}比{}{}",
                        subject,
                        self.entity_zh(than),
                        frame.predicate.zh
                    ),
                    "realize.comparison",
                    0,
                )];
            }
            if frame.predicate.lemma == "open" {
                return vec![candidate(format!("{subject}开着"), "realize.state-open", 0)];
            }
            if frame.predicate.lemma == "afraid" {
                return vec![candidate(
                    format!("{}害怕{}", subject, obj),
                    "realize.afraid-of",
                    0,
                )];
            }
            let adv = if matches!(frame.polarity, Polarity::Negative) {
                "不"
            } else {
                "很"
            };
            return vec![candidate(
                format!("{}{}{}", subject, adv, frame.predicate.zh),
                "realize.be-adjective",
                0,
            )];
        }

        if frame.predicate.lemma == "want" {
            if let Some(complement) = &frame.complement {
                let neg = if matches!(frame.polarity, Polarity::Negative) {
                    "不"
                } else {
                    ""
                };
                return vec![candidate(
                    format!(
                        "{}{}{}{}{}",
                        subject, neg, frame.predicate.zh, complement.zh, obj
                    ),
                    "realize.want-complement",
                    0,
                )];
            }
        }

        if matches!(frame.polarity, Polarity::Negative) {
            if frame.predicate.lemma == "have" {
                return vec![candidate(
                    format!("{subject}没有{obj}"),
                    "realize.negative-have",
                    0,
                )];
            }
            let neg = if matches!(frame.tense_aspect, TenseAspect::Past) {
                "没"
            } else {
                "不"
            };
            return vec![candidate(
                format!("{}{}{}{}", subject, neg, frame.predicate.zh, obj),
                "realize.negative",
                0,
            )];
        }

        if frame.tense_aspect == TenseAspect::Passive {
            let patient = frame
                .patient
                .as_ref()
                .map(|p| self.entity_zh(p))
                .unwrap_or_default();
            let agent = frame
                .agent
                .as_ref()
                .map(|a| self.entity_zh(a))
                .unwrap_or_default();
            return vec![candidate(
                format!("{patient}被{agent}{}了", frame.predicate.zh),
                "realize.passive",
                0,
            )];
        }
        if frame.tense_aspect == TenseAspect::Experiential {
            return vec![candidate(
                format!("{subject}{}过{obj}", frame.predicate.zh),
                "realize.experiential",
                0,
            )];
        }
        if frame.tense_aspect == TenseAspect::Future {
            let place = frame.modifiers.iter().find_map(|m| match m {
                Modifier::Place(p) => Some(p.clone()),
                _ => None,
            });
            if frame.predicate.lemma == "stay" && place.as_deref() == Some("家") {
                return vec![candidate(
                    format!("{subject}就待在家里"),
                    "realize.future",
                    0,
                )];
            }
            let tail = place.unwrap_or_else(|| obj.clone());
            return vec![candidate(
                format!("{}就{}{}", subject, frame.predicate.zh, tail),
                "realize.future",
                0,
            )];
        }
        if let Some(manner) = frame.modifiers.iter().find_map(|m| match m {
            Modifier::Manner(v) => Some(v.clone()),
            _ => None,
        }) {
            return vec![candidate(
                format!("{subject}{manner}地{}{}", frame.predicate.zh, obj),
                "realize.manner",
                0,
            )];
        }
        if let Some(degree) = frame.modifiers.iter().find_map(|m| match m {
            Modifier::Degree(v) => Some(v.clone()),
            _ => None,
        }) {
            return vec![candidate(
                format!("{subject}{}得{degree}", frame.predicate.zh),
                "realize.degree",
                0,
            )];
        }
        let past = matches!(frame.tense_aspect, TenseAspect::Past);
        let time = frame
            .modifiers
            .iter()
            .find(|m| matches!(m, Modifier::Time(_)))
            .map(|m| match m {
                Modifier::Time(t) => t.clone(),
                _ => unreachable!(),
            });
        let place = frame
            .modifiers
            .iter()
            .find(|m| matches!(m, Modifier::Place(_)))
            .map(|m| match m {
                Modifier::Place(p) => p.clone(),
                _ => unreachable!(),
            });
        if past {
            let phrase = if let Some(place) = &place {
                if matches!(frame.predicate.lemma.as_str(), "go" | "come") && place == "家" {
                    "回家了".to_string()
                } else if frame.predicate.lemma == "stay" && place == "家" {
                    "待在家里了".to_string()
                } else {
                    format!("{}{}{}", frame.predicate.zh, "了", place)
                }
            } else {
                format!("{}{}{}", frame.predicate.zh, "了", obj)
            };
            match time {
                Some(time) => vec![
                    candidate(
                        format!("{subject}{time}{phrase}"),
                        "realize.past-time.subject-first",
                        0,
                    ),
                    candidate(
                        format!("{time}{subject}{phrase}"),
                        "realize.past-time.time-first",
                        1,
                    ),
                ],
                None => vec![candidate(format!("{subject}{phrase}"), "realize.past", 0)],
            }
        } else {
            let phrase = if let Some(collocation) = &frame.collocation {
                format!("{collocation}{obj}")
            } else if let Some(place) = &place {
                if matches!(frame.predicate.lemma.as_str(), "go" | "come") && place == "家" {
                    "回家".to_string()
                } else if frame.predicate.lemma == "stay" && place == "家" {
                    "待在家里".to_string()
                } else {
                    format!("{}{}", frame.predicate.zh, place)
                }
            } else {
                format!("{}{}", frame.predicate.zh, obj)
            };
            vec![candidate(format!("{subject}{phrase}"), "realize.simple", 0)]
        }
    }

    fn realize_imperative(&self, frame: &MeaningFrame) -> String {
        let obj = frame
            .patient
            .as_ref()
            .map(|p| self.entity_zh(p))
            .unwrap_or_default();
        if let Some(collocation) = &frame.collocation {
            return format!("请{collocation}{obj}");
        }
        format!("请{}{}", frame.predicate.zh, obj)
    }

    fn entity_zh(&self, entity: &EntityRef) -> String {
        let mut adjectives = String::new();
        for adj in &entity.adjectives {
            adjectives.push_str(&adj.zh);
            if adj.zh.chars().count() > 1 {
                adjectives.push('的');
            }
        }
        let noun = if entity.plural && entity.head.lemma == "brother" {
            "兄弟"
        } else {
            entity.head.zh.as_str()
        };
        if let Some(quantity) = entity.quantity {
            let classifier = entity.classifier.as_deref().unwrap_or("");
            let prefix = match &entity.determiner {
                Some(Determiner::This) | Some(Determiner::These) | Some(Determiner::The) => "这",
                Some(Determiner::That) | Some(Determiner::Those) => "那",
                Some(Determiner::Possessive(zh)) => zh.as_str(),
                _ => "",
            };
            return format!(
                "{prefix}{}{classifier}{adjectives}{noun}",
                numeral_zh(quantity)
            );
        }
        match &entity.determiner {
            Some(Determiner::A) => {
                let classifier = entity.classifier.as_deref().unwrap_or("");
                format!("一{classifier}{adjectives}{noun}")
            }
            Some(Determiner::The) => format!("{adjectives}{noun}"),
            Some(Determiner::Possessive(zh)) => format!("{zh}{adjectives}{noun}"),
            Some(Determiner::This) => format!("这个{adjectives}{noun}"),
            Some(Determiner::That) => format!("那个{adjectives}{noun}"),
            Some(Determiner::These) => format!("这些{adjectives}{noun}"),
            Some(Determiner::Those) => format!("那些{adjectives}{noun}"),
            None => format!("{adjectives}{noun}"),
        }
    }
}

fn numeral_zh(value: u32) -> String {
    match value {
        2 => "两".to_string(),
        3 => "三".to_string(),
        4 => "四".to_string(),
        5 => "五".to_string(),
        6 => "六".to_string(),
        7 => "七".to_string(),
        8 => "八".to_string(),
        9 => "九".to_string(),
        10 => "十".to_string(),
        other => other.to_string(),
    }
}

fn candidate(text: String, rule_id: &str, rank: u8) -> RealizationCandidate {
    RealizationCandidate {
        text: text.trim_end_matches(['.', '。', '?', '？']).to_string() + "。",
        rule_id: rule_id.to_string(),
        rank,
    }
}

fn question_candidate(text: String, rule_id: &str) -> RealizationCandidate {
    RealizationCandidate {
        text: text.trim_end_matches(['.', '。', '?', '？']).to_string() + "？",
        rule_id: rule_id.to_string(),
        rank: 0,
    }
}

/// 候选硬约束校验结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateValidation {
    pub valid: bool,
    pub reasons: Vec<&'static str>,
}

/// 候选选择器：先做硬约束，再做确定性排序；不做语言模型流畅度打分。
#[derive(Debug, Default, Clone, Copy)]
pub struct CandidateSelector;

impl CandidateSelector {
    pub fn validate(
        &self,
        program: &MeaningProgram,
        candidate: &RealizationCandidate,
    ) -> CandidateValidation {
        let mut reasons = Vec::new();
        if candidate.text.trim().is_empty() {
            reasons.push("empty");
        }
        if program.relation == ProgramRelation::Conditional {
            let main_imperative = program
                .frames
                .get(1)
                .is_some_and(|frame| frame.tense_aspect == TenseAspect::Imperative);
            if !candidate.text.contains("如果")
                || (!main_imperative && !candidate.text.contains("就"))
            {
                reasons.push("missing-condition");
            }
        }
        for frame in &program.frames {
            let predicate_text = if matches!(frame.predicate.lemma.as_str(), "go" | "come")
                && frame
                    .modifiers
                    .iter()
                    .any(|m| matches!(m, Modifier::Place(p) if p == "家"))
            {
                "回"
            } else if frame.predicate.lemma == "open" && frame.predicate.pos == "adjective" {
                "开着"
            } else {
                frame
                    .collocation
                    .as_deref()
                    .unwrap_or(frame.predicate.zh.as_str())
            };
            let where_question =
                matches!(&frame.question, QuestionType::Wh(word) if word == "where");
            if !where_question
                && (predicate_text.trim().is_empty() || !candidate.text.contains(predicate_text))
            {
                reasons.push("missing-predicate");
            }
            if let Some(agent) = &frame.agent {
                let weather_condition = program.relation == ProgramRelation::Conditional
                    && agent.head.zh == "它"
                    && frame.predicate.lemma == "rain";
                if !weather_condition && !candidate.text.contains(&agent.head.zh) {
                    reasons.push("missing-agent");
                }
            }
            if let Some(patient) = &frame.patient {
                if !candidate.text.contains(&patient.head.zh) {
                    reasons.push("missing-patient");
                }
            }
            if let Some(complement) = &frame.complement {
                if !candidate.text.contains(&complement.zh) {
                    reasons.push("missing-complement");
                }
            }
            if let Some(than) = &frame.comparison {
                if !candidate.text.contains('比') || !candidate.text.contains(&than.head.zh) {
                    reasons.push("missing-comparison");
                }
            }
            if matches!(frame.polarity, Polarity::Negative) {
                let expected = if frame.predicate.lemma == "want" {
                    "不想"
                } else if frame.predicate.lemma == "have" {
                    "没有"
                } else if matches!(frame.tense_aspect, TenseAspect::Modal(_)) {
                    "不会"
                } else if frame.tense_aspect == TenseAspect::Progressive {
                    "没在"
                } else if frame.tense_aspect == TenseAspect::Past {
                    "没"
                } else {
                    "不"
                };
                if !candidate.text.contains(expected) {
                    reasons.push("wrong-negation");
                }
            } else {
                if matches!(frame.tense_aspect, TenseAspect::Modal(_))
                    && !candidate.text.contains('会')
                {
                    reasons.push("wrong-modal");
                }
                if frame.tense_aspect == TenseAspect::Progressive
                    && !candidate.text.contains("正在")
                {
                    reasons.push("wrong-aspect");
                }
                if frame.tense_aspect == TenseAspect::Experiential && !candidate.text.contains('过')
                {
                    reasons.push("wrong-aspect");
                }
                if frame.tense_aspect == TenseAspect::Passive && !candidate.text.contains('被') {
                    reasons.push("wrong-aspect");
                }
                if frame.predicate.pos == "adjective"
                    && !matches!(frame.predicate.lemma.as_str(), "afraid" | "open")
                    && frame.comparison.is_none()
                    && frame.question == QuestionType::None
                    && !candidate.text.contains('很')
                {
                    reasons.push("wrong-adjective-marker");
                }
            }
            for modifier in &frame.modifiers {
                match modifier {
                    Modifier::Time(time) if !candidate.text.contains(time) => {
                        reasons.push("missing-time")
                    }
                    Modifier::Place(place) if !candidate.text.contains(place) => {
                        reasons.push("missing-place")
                    }
                    _ => {}
                }
            }
            match &frame.question {
                QuestionType::YesNo if !candidate.text.contains('吗') => {
                    reasons.push("missing-yes-no-marker")
                }
                QuestionType::Wh(word) if word == "where" && !candidate.text.contains("哪里") => {
                    reasons.push("missing-where")
                }
                _ => {}
            }
        }
        CandidateValidation {
            valid: reasons.is_empty(),
            reasons,
        }
    }

    pub fn select<'a>(
        &self,
        candidates: &'a [RealizationCandidate],
    ) -> Option<&'a RealizationCandidate> {
        candidates.iter().min_by_key(|c| c.rank)
    }

    pub fn select_validated<'a>(
        &self,
        program: &MeaningProgram,
        candidates: &'a [RealizationCandidate],
    ) -> Option<&'a RealizationCandidate> {
        candidates
            .iter()
            .filter(|c| self.validate(program, c).valid)
            .min_by_key(|c| c.rank)
    }
}

/// P20-C 语义组合引擎：复用 P19-G 的词典词表，只增加局部语义分析/中文生成。
pub struct SemanticCompositionEngine {
    lexical: CompositionEngine,
    id: String,
    version: u32,
    lemmatizer: EnglishLemmatizer,
}

impl std::fmt::Debug for SemanticCompositionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticCompositionEngine")
            .field("id", &self.id)
            .field("version", &self.version)
            .finish()
    }
}

impl SemanticCompositionEngine {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, crate::composer::CompositionError> {
        let lexical = CompositionEngine::from_file(path)?;
        Ok(Self {
            id: format!("semantic.{}", lexical.id()),
            version: lexical.version(),
            lexical,
            lemmatizer: EnglishLemmatizer::new(),
        })
    }

    pub fn as_arc(self) -> Arc<Self> {
        Arc::new(self)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn rule_count(&self) -> usize {
        4
    }

    pub fn source(&self) -> &LanguageTag {
        self.lexical.source()
    }

    pub fn target(&self) -> &LanguageTag {
        self.lexical.target()
    }

    pub fn is_candidate(&self, text: &str) -> bool {
        let t = tokenize(text);
        if t.tokens.is_empty() {
            return false;
        }
        let words: Vec<&str> = t.tokens.iter().map(|x| x.text.as_str()).collect();
        if self.lexical.is_candidate(text)
            || matches!(
                words.first().copied(),
                Some(
                    "this"
                        | "that"
                        | "my"
                        | "your"
                        | "his"
                        | "her"
                        | "our"
                        | "their"
                        | "these"
                        | "those"
                        | "two"
                        | "three"
                        | "four"
                        | "five"
                        | "six"
                        | "seven"
                        | "eight"
                        | "nine"
                        | "ten"
                )
            )
            || words
                .first()
                .is_some_and(|word| self.lexical.lexicon_has_pos(word, "verb"))
        {
            return true;
        }
        if words.contains(&"with") {
            return true; // 让 analyze 记录拒绝原因
        }
        if words.len() >= 2 && words[0] == "please" && words.contains(&"and") {
            return true;
        }
        if words.contains(&"want") && words.contains(&"to") && words.contains(&"not") {
            return true;
        }
        if words
            .iter()
            .any(|w| matches!(*w, "yesterday" | "today" | "tomorrow"))
            && words.iter().any(|w| {
                self.lexical.is_past_form(w, w)
                    || self
                        .lexical
                        .is_past_form(w, &self.lexical.verb_lemma(w).unwrap_or_default())
            })
        {
            return true;
        }
        if words.windows(2).any(|w| w == ["light", "blue"]) && words.contains(&"is") {
            return true;
        }
        false
    }

    pub fn lookup_candidates(&self, text: &str) -> Vec<String> {
        if !self.is_candidate(text) {
            return Vec::new();
        }
        let t = tokenize(text);
        let mut out = Vec::new();
        for token in t.tokens {
            if !out.contains(&token.text) {
                out.push(token.text.clone());
            }
            for lemma in self.lemmatizer.lemmas(&token.text).into_iter().take(4) {
                if !out.contains(&lemma) {
                    out.push(lemma);
                }
            }
            if out.len() >= 24 {
                break;
            }
        }
        out
    }

    pub fn analyze(
        &self,
        text: &str,
        entries: &HashMap<String, Vec<DictionaryEntry>>,
    ) -> SemanticAnalysis {
        let t = tokenize(text);
        let words = &t.tokens;
        if words.is_empty() {
            return SemanticAnalysis {
                program: None,
                rejections: Vec::new(),
                question: t.question,
            };
        }
        if let Some(span) = words.iter().find(|w| w.text == "with") {
            return rejected(
                text,
                span,
                RejectionReason::PpAttachmentAmbiguous,
                t.question,
            );
        }
        let complex = [
            "that", "which", "who", "whom", "whose", "because", "although", "though", "while",
            "unless", "whereas",
        ];
        if let Some(span) = words
            .iter()
            .enumerate()
            .find(|(i, w)| {
                if !complex.contains(&w.text.as_str()) {
                    return false;
                }
                // `that bag` 中的 that 是限定词；真正的从句标记仍拒绝。
                !(w.text == "that"
                    && words
                        .get(i + 1)
                        .is_some_and(|next| self.lexical.noun_lemma(&next.text).is_some()))
            })
            .map(|(_, w)| w)
        {
            return rejected(text, span, RejectionReason::UnsupportedClause, t.question);
        }
        // 允许句首主语、助动词后的疑问主语、限定词性物主代词；
        // 其余句中代词视为未解决指代/嵌入从句。
        let auxiliaries = ["do", "does", "did", "can", "is", "are", "am", "will", "by"];
        let possessives = ["my", "your", "his", "her", "our", "their", "this", "that"];
        if words.first().map(|w| w.text.as_str()) != Some("if") {
            if let Some(span) = words
                .iter()
                .enumerate()
                .find(|(i, w)| {
                    if *i == 0 || !self.lexical.lexicon_has_pos(&w.text, "pronoun") {
                        return false;
                    }
                    let prev = words[i - 1].text.as_str();
                    !auxiliaries.contains(&prev) && !possessives.contains(&w.text.as_str())
                })
                .map(|(_, w)| w)
            {
                return rejected(text, span, RejectionReason::UnresolvedPronoun, t.question);
            }
        }

        if let Some(chinese) = self.lexical.phrase(&t.key) {
            return SemanticAnalysis {
                program: Some(MeaningProgram {
                    frames: vec![MeaningFrame {
                        predicate: Lexeme {
                            word: text.to_string(),
                            lemma: t.key.clone(),
                            zh: chinese.to_string(),
                            pos: "phrase".to_string(),
                            span: TextSpan::new(0, text.len()),
                        },
                        agent: None,
                        patient: None,
                        complement: None,
                        collocation: None,
                        comparison: None,
                        modifiers: Vec::new(),
                        polarity: Polarity::Affirmative,
                        tense_aspect: TenseAspect::Simple,
                        question: if t.question {
                            QuestionType::Wh("phrase".to_string())
                        } else {
                            QuestionType::None
                        },
                        source_spans: vec![TextSpan::new(0, text.len())],
                    }],
                    relation: ProgramRelation::Single,
                }),
                rejections: Vec::new(),
                question: t.question,
            };
        }
        let dict = DictView { entries };
        let parser = SemanticParser {
            engine: self,
            dict: &dict,
            words,
        };
        let program = parser.parse();
        match program {
            Some(program) => SemanticAnalysis {
                program: Some(program),
                rejections: Vec::new(),
                question: t.question,
            },
            None => SemanticAnalysis {
                program: None,
                rejections: vec![Rejection {
                    span: TextSpan::new(0, text.len()),
                    reason: RejectionReason::UnsupportedPattern,
                }],
                question: t.question,
            },
        }
    }

    pub fn compose(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
        entries: &HashMap<String, Vec<DictionaryEntry>>,
    ) -> Option<SemanticOutcome> {
        if source != self.source() || target != self.target() {
            return None;
        }
        let analysis = self.analyze(text, entries);
        let program = analysis.program?;
        let candidates = ChineseRealizer.realize(&program);
        let selected = CandidateSelector
            .select_validated(&program, &candidates)?
            .clone();
        let mut evidence =
            LanguageEvidence::for_text(TextSpan::new(0, text.len()), UnitKind::Sentence, text);
        evidence.add_pattern(PatternMatch {
            kind: PatternKind::Rule,
            id: selected.rule_id.clone(),
            span: TextSpan::new(0, text.len()),
            complete: true,
        });
        let alternatives = candidates
            .iter()
            .filter(|c| c.text != selected.text)
            .map(|c| c.text.clone())
            .collect();
        Some(SemanticOutcome {
            text: selected.text,
            rule_id: selected.rule_id,
            pattern: "semantic".to_string(),
            evidence,
            alternatives,
        })
    }
}

fn rejected(
    text: &str,
    token: &CompositionToken,
    reason: RejectionReason,
    question: bool,
) -> SemanticAnalysis {
    SemanticAnalysis {
        program: None,
        rejections: vec![Rejection {
            span: TextSpan::new(token.start.min(text.len()), token.end.min(text.len())),
            reason,
        }],
        question,
    }
}

#[derive(Clone, Copy)]
struct DictView<'a> {
    entries: &'a HashMap<String, Vec<DictionaryEntry>>,
}

impl DictView<'_> {
    fn has_pos(&self, word: &str, lemma: &str, pos: &str) -> bool {
        for key in [word, lemma] {
            if let Some(entries) = self.entries.get(key) {
                if entries.iter().any(|entry| {
                    !entry.senses.is_empty()
                        && entry
                            .parts_of_speech
                            .iter()
                            .any(|p| p.eq_ignore_ascii_case(pos))
                }) {
                    return true;
                }
            }
        }
        false
    }
}

struct SemanticParser<'a> {
    engine: &'a SemanticCompositionEngine,
    dict: &'a DictView<'a>,
    words: &'a [CompositionToken],
}

impl SemanticParser<'_> {
    fn span(&self, start: usize, end: usize) -> TextSpan {
        if start >= end || start >= self.words.len() {
            return TextSpan::default();
        }
        TextSpan::new(self.words[start].start, self.words[end - 1].end)
    }

    fn pronoun_entity(&self, i: usize) -> Option<(EntityRef, usize)> {
        let token = self.words.get(i)?;
        if !self.engine.lexical.lexicon_has_pos(&token.text, "pronoun")
            || matches!(
                token.text.as_str(),
                "this"
                    | "that"
                    | "these"
                    | "those"
                    | "my"
                    | "your"
                    | "his"
                    | "her"
                    | "our"
                    | "their"
            )
        {
            return None;
        }
        let zh = self.engine.lexical.zh(&token.text, "pronoun")?.to_string();
        Some((
            EntityRef {
                head: Lexeme {
                    word: token.text.clone(),
                    lemma: token.text.clone(),
                    zh,
                    pos: "pronoun".to_string(),
                    span: TextSpan::new(token.start, token.end),
                },
                adjectives: Vec::new(),
                determiner: None,
                quantity: None,
                classifier: None,
                plural: false,
                span: TextSpan::new(token.start, token.end),
            },
            i + 1,
        ))
    }

    fn subject(&self, i: usize) -> Option<(EntityRef, usize)> {
        self.pronoun_entity(i).or_else(|| self.entity(i))
    }

    fn entity(&self, i: usize) -> Option<(EntityRef, usize)> {
        let start = i;
        let mut idx = i;
        let mut determiner = None;
        if let Some(token) = self.words.get(idx) {
            determiner = match token.text.as_str() {
                "a" | "an" => Some(Determiner::A),
                "the" => Some(Determiner::The),
                "this" => Some(Determiner::This),
                "that" => Some(Determiner::That),
                "these" => Some(Determiner::These),
                "those" => Some(Determiner::Those),
                "my" => Some(Determiner::Possessive("我的".to_string())),
                "your" => Some(Determiner::Possessive("你的".to_string())),
                "his" => Some(Determiner::Possessive("他的".to_string())),
                "her" => Some(Determiner::Possessive("她的".to_string())),
                "our" => Some(Determiner::Possessive("我们的".to_string())),
                "their" => Some(Determiner::Possessive("他们的".to_string())),
                _ => None,
            };
            if determiner.is_some() {
                idx += 1;
            }
        }
        let mut quantity = None;
        if let Some(token) = self.words.get(idx) {
            quantity = match token.text.as_str() {
                "two" => Some(2),
                "three" => Some(3),
                "four" => Some(4),
                "five" => Some(5),
                "six" => Some(6),
                "seven" => Some(7),
                "eight" => Some(8),
                "nine" => Some(9),
                "ten" => Some(10),
                _ => None,
            };
            if quantity.is_some() {
                idx += 1;
            }
        }
        let mut adjectives = Vec::new();
        while idx < self.words.len() {
            if self.words[idx].text == "light"
                && self.words.get(idx + 1).map(|w| w.text.as_str()) == Some("blue")
            {
                adjectives.push(Lexeme {
                    word: "light blue".to_string(),
                    lemma: "light blue".to_string(),
                    zh: "浅蓝色".to_string(),
                    pos: "adjective".to_string(),
                    span: self.span(idx, idx + 2),
                });
                idx += 2;
                continue;
            }
            let token = &self.words[idx];
            let Some(lemma) = self.engine.lexical.adjective_lemma(&token.text) else {
                break;
            };
            let next_is_noun = self
                .words
                .get(idx + 1)
                .and_then(|n| self.engine.lexical.noun_lemma(&n.text))
                .is_some();
            if !next_is_noun || !self.dict.has_pos(&token.text, &lemma, "adjective") {
                break;
            }
            let zh = self
                .engine
                .lexical
                .zh(&lemma, "adjective")
                .or_else(|| self.engine.lexical.zh(&token.text, "adjective"))?
                .to_string();
            adjectives.push(Lexeme {
                word: token.text.clone(),
                lemma,
                zh,
                pos: "adjective".to_string(),
                span: TextSpan::new(token.start, token.end),
            });
            idx += 1;
        }
        let token = self.words.get(idx)?;
        let lemma = self.engine.lexical.noun_lemma(&token.text)?;
        if !self.dict.has_pos(&token.text, &lemma, "noun") {
            return None;
        }
        let zh = self
            .engine
            .lexical
            .zh(&lemma, "noun")
            .or_else(|| self.engine.lexical.zh(&token.text, "noun"))?
            .to_string();
        let classifier = self.engine.lexical.classifier(&lemma).map(str::to_string);
        let plural = token.text.ends_with('s') && token.text != lemma;
        Some((
            EntityRef {
                head: Lexeme {
                    word: token.text.clone(),
                    lemma,
                    zh,
                    pos: "noun".to_string(),
                    span: TextSpan::new(token.start, token.end),
                },
                adjectives,
                determiner,
                quantity,
                classifier,
                plural,
                span: self.span(start, idx + 1),
            },
            idx + 1,
        ))
    }

    fn entity_coordinated(&self, i: usize) -> Option<(EntityRef, usize)> {
        let (first, end) = self.entity(i)?;
        if self.words.get(end).map(|w| w.text.as_str()) != Some("and") {
            return Some((first, end));
        }
        let (second, next) = self.entity(end + 1)?;
        let zh = format!("{}和{}", first.head.zh, second.head.zh);
        Some((
            EntityRef {
                head: Lexeme {
                    word: first.head.word.clone(),
                    lemma: first.head.lemma.clone(),
                    zh,
                    pos: "noun".to_string(),
                    span: self.span(i, next),
                },
                adjectives: Vec::new(),
                determiner: None,
                quantity: None,
                classifier: None,
                plural: false,
                span: self.span(i, next),
            },
            next,
        ))
    }

    fn verb(&self, i: usize) -> Option<(Lexeme, bool, usize)> {
        let token = self.words.get(i)?;
        if let Some(next) = self.words.get(i + 1) {
            let phrase = format!("{} {}", token.text, next.text);
            if let Some(zh) = self.engine.lexical.phrasal_verb(&phrase) {
                return Some((
                    Lexeme {
                        word: phrase.clone(),
                        lemma: phrase,
                        zh: zh.to_string(),
                        pos: "verb".to_string(),
                        span: self.span(i, i + 2),
                    },
                    false,
                    i + 2,
                ));
            }
        }
        let lemma = self.engine.lexical.verb_lemma(&token.text)?;
        if !self.dict.has_pos(&token.text, &lemma, "verb")
            && !self.dict.has_pos(&lemma, &lemma, "verb")
        {
            return None;
        }
        let zh = self
            .engine
            .lexical
            .zh(&lemma, "verb")
            .or_else(|| self.engine.lexical.zh(&token.text, "verb"))?
            .to_string();
        let past = self.engine.lexical.is_past_form(&token.text, &lemma);
        Some((
            Lexeme {
                word: token.text.clone(),
                lemma,
                zh,
                pos: "verb".to_string(),
                span: TextSpan::new(token.start, token.end),
            },
            past,
            i + 1,
        ))
    }

    fn adjective(&self, i: usize) -> Option<(Lexeme, usize)> {
        let token = self.words.get(i)?;
        let lemma = self.engine.lexical.adjective_lemma(&token.text)?;
        if !self.dict.has_pos(&token.text, &lemma, "adjective")
            && !self.dict.has_pos(&lemma, &lemma, "adjective")
        {
            return None;
        }
        let zh = self
            .engine
            .lexical
            .zh(&lemma, "adjective")
            .or_else(|| self.engine.lexical.zh(&token.text, "adjective"))?
            .to_string();
        Some((
            Lexeme {
                word: token.text.clone(),
                lemma,
                zh,
                pos: "adjective".to_string(),
                span: TextSpan::new(token.start, token.end),
            },
            i + 1,
        ))
    }

    fn adverb(&self, i: usize) -> Option<(String, usize)> {
        let token = self.words.get(i)?;
        if !self.engine.lexical.lexicon_has_pos(&token.text, "adverb") {
            return None;
        }
        let zh = self.engine.lexical.zh(&token.text, "adverb")?.to_string();
        Some((zh, i + 1))
    }

    fn collocation(&self, predicate: &Lexeme, patient: Option<&EntityRef>) -> Option<String> {
        let patient = patient?;
        self.engine
            .lexical
            .collocation(&format!("{} {}", predicate.lemma, patient.head.lemma))
            .map(str::to_string)
    }

    fn time(&self, i: usize) -> Option<(String, usize)> {
        let token = self.words.get(i)?;
        let quantity = match token.text.as_str() {
            "a" | "one" => Some(1),
            "two" => Some(2),
            "three" => Some(3),
            "four" => Some(4),
            "five" => Some(5),
            "six" => Some(6),
            "seven" => Some(7),
            "eight" => Some(8),
            "nine" => Some(9),
            "ten" => Some(10),
            _ => None,
        };
        if let Some(quantity) = quantity {
            if let Some(unit) = self.words.get(i + 1).map(|w| w.text.as_str()) {
                if self.words.get(i + 2).map(|w| w.text.as_str()) == Some("ago") {
                    let unit_zh = match unit {
                        "day" | "days" => "天",
                        "week" | "weeks" => "周",
                        "month" | "months" => "个月",
                        "year" | "years" => "年",
                        _ => return None,
                    };
                    return Some((format!("{}{unit_zh}前", numeral_zh(quantity)), i + 3));
                }
            }
        }
        match token.text.as_str() {
            "yesterday" => Some(("昨天".to_string(), i + 1)),
            "today" => Some(("今天".to_string(), i + 1)),
            "tomorrow" => Some(("明天".to_string(), i + 1)),
            "last" => {
                let next = self.words.get(i + 1)?.text.as_str();
                let zh = match next {
                    "week" => "上周",
                    "month" => "上个月",
                    "year" => "去年",
                    _ => return None,
                };
                Some((zh.to_string(), i + 2))
            }
            "this" => {
                let next = self.words.get(i + 1)?.text.as_str();
                let zh = match next {
                    "week" => "本周",
                    "month" => "本月",
                    "year" => "今年",
                    _ => return None,
                };
                Some((zh.to_string(), i + 2))
            }
            _ => None,
        }
    }

    fn place(&self, i: usize) -> Option<(String, usize)> {
        let token = self.words.get(i)?;
        match token.text.as_str() {
            "home" => Some(("家".to_string(), i + 1)),
            "here" => Some(("这里".to_string(), i + 1)),
            "there" => Some(("那里".to_string(), i + 1)),
            _ => None,
        }
    }

    fn parse(&self) -> Option<MeaningProgram> {
        if self.words.first()?.text == "if" {
            return self.parse_if();
        }
        if self.words.first()?.text == "please" {
            if self.words.iter().any(|w| w.text == "and") {
                return self.parse_imperative_coordination();
            }
            return self.parse_imperative(1, self.words.len());
        }
        if self.words.first()?.text == "where" {
            return self.parse_where();
        }
        if matches!(self.words.first()?.text.as_str(), "do" | "does") {
            return self.parse_do_question();
        }
        self.parse_passive()
            .or_else(|| self.parse_experiential())
            .or_else(|| self.parse_want())
            .or_else(|| self.parse_negation())
            .or_else(|| self.parse_can())
            .or_else(|| self.parse_progressive())
            .or_else(|| self.parse_afraid())
            .or_else(|| self.parse_comparison())
            .or_else(|| self.parse_be_adjective())
            .or_else(|| self.parse_past())
            .or_else(|| self.parse_simple())
            .or_else(|| self.parse_bare_imperative())
    }

    fn parse_if(&self) -> Option<MeaningProgram> {
        let (condition_agent, idx) = self.subject(1)?;
        let (condition_predicate, _, condition_end) = self.verb(idx)?;
        let Some(will_pos) = self.words[condition_end..]
            .iter()
            .position(|w| w.text == "will")
            .map(|pos| condition_end + pos)
        else {
            let main = self.parse_imperative(condition_end, self.words.len())?;
            let condition_frame = MeaningFrame {
                predicate: condition_predicate,
                agent: Some(condition_agent),
                patient: None,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::None,
                source_spans: vec![self.span(0, condition_end)],
            };
            return Some(MeaningProgram {
                frames: vec![condition_frame, main.frames[0].clone()],
                relation: ProgramRelation::Conditional,
            });
        };
        let (main_agent, idx2) = self.subject(condition_end)?;
        if idx2 != will_pos {
            return None;
        }
        let (main_predicate, _, mut main_end) = self.verb(will_pos + 1)?;
        let mut modifiers = Vec::new();
        if let Some((place, next)) = self.place(main_end) {
            modifiers.push(Modifier::Place(place));
            main_end = next;
        } else if let Some((patient, next)) = self.entity(main_end) {
            main_end = next;
            let mut main_frame = MeaningFrame {
                predicate: main_predicate.clone(),
                agent: Some(main_agent.clone()),
                patient: Some(patient),
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: modifiers.clone(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Future,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            };
            if main_end != self.words.len() {
                return None;
            }
            let condition_frame = MeaningFrame {
                predicate: condition_predicate,
                agent: Some(condition_agent),
                patient: None,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::None,
                source_spans: vec![self.span(0, condition_end)],
            };
            main_frame.source_spans = vec![self.span(will_pos, self.words.len())];
            return Some(MeaningProgram {
                frames: vec![condition_frame, main_frame],
                relation: ProgramRelation::Conditional,
            });
        }
        if main_end != self.words.len() {
            return None;
        }
        let condition_frame = MeaningFrame {
            predicate: condition_predicate,
            agent: Some(condition_agent),
            patient: None,
            complement: None,
            collocation: None,
            comparison: None,
            modifiers: Vec::new(),
            polarity: Polarity::Affirmative,
            tense_aspect: TenseAspect::Simple,
            question: QuestionType::None,
            source_spans: vec![self.span(0, condition_end)],
        };
        let main_frame = MeaningFrame {
            predicate: main_predicate,
            agent: Some(main_agent),
            patient: None,
            complement: None,
            collocation: None,
            comparison: None,
            modifiers,
            polarity: Polarity::Affirmative,
            tense_aspect: TenseAspect::Future,
            question: QuestionType::None,
            source_spans: vec![self.span(will_pos, self.words.len())],
        };
        Some(MeaningProgram {
            frames: vec![condition_frame, main_frame],
            relation: ProgramRelation::Conditional,
        })
    }

    fn parse_imperative(&self, start: usize, end: usize) -> Option<MeaningProgram> {
        if start >= end || end > self.words.len() {
            return None;
        }
        let (predicate, _, verb_end) = self.verb(start)?;
        let patient = if verb_end < end {
            let (patient, np_end) = self.entity(verb_end)?;
            if np_end != end {
                return None;
            }
            Some(patient)
        } else {
            None
        };
        let collocation = self.collocation(&predicate, patient.as_ref());
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: None,
                patient,
                complement: None,
                collocation,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Imperative,
                question: QuestionType::None,
                source_spans: vec![self.span(start, end)],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_imperative_coordination(&self) -> Option<MeaningProgram> {
        let and_pos = self.words.iter().position(|w| w.text == "and")?;
        if and_pos <= 1 || and_pos + 1 >= self.words.len() {
            return None;
        }
        let first = self.parse_imperative(1, and_pos)?;
        let second = self.parse_imperative(and_pos + 1, self.words.len())?;
        Some(MeaningProgram {
            frames: vec![first.frames[0].clone(), second.frames[0].clone()],
            relation: ProgramRelation::Coordination,
        })
    }

    fn parse_where(&self) -> Option<MeaningProgram> {
        if self.words.len() < 3 || !matches!(self.words[1].text.as_str(), "is" | "are") {
            return None;
        }
        let (agent, end) = self.entity(2)?;
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate: Lexeme {
                    word: "be".to_string(),
                    lemma: "be".to_string(),
                    zh: String::new(),
                    pos: "verb".to_string(),
                    span: TextSpan::default(),
                },
                agent: Some(agent),
                patient: None,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::Wh("where".to_string()),
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_do_question(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(1)?;
        let (predicate, _, mut end) = self.verb(idx)?;
        let mut patient = None;
        if let Some((p, next)) = self.entity(end) {
            patient = Some(p);
            end = next;
        }
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::YesNo,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_want(&self) -> Option<MeaningProgram> {
        let (agent, mut idx) = self.subject(0)?;
        let mut negative = false;
        if matches!(self.words.get(idx)?.text.as_str(), "do" | "does")
            && self.words.get(idx + 1)?.text == "not"
        {
            negative = true;
            idx += 2;
        }
        let (predicate, _, _) = self.verb(idx)?;
        if predicate.lemma != "want" || self.words.get(idx + 1)?.text != "to" {
            return None;
        }
        let (complement, _, mut end) = self.verb(idx + 2)?;
        let mut patient = None;
        if let Some((p, next)) = self.entity(end) {
            patient = Some(p);
            end = next;
        }
        if end != self.words.len() {
            return None;
        }
        let collocation = self.collocation(&complement, patient.as_ref());
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient,
                complement: Some(complement),
                collocation,
                comparison: None,
                modifiers: Vec::new(),
                polarity: if negative {
                    Polarity::Negative
                } else {
                    Polarity::Affirmative
                },
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_negation(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        let aux = self.words.get(idx)?.text.clone();
        if !matches!(aux.as_str(), "do" | "does" | "did") || self.words.get(idx + 1)?.text != "not"
        {
            return None;
        }
        let (predicate, _, mut end) = self.verb(idx + 2)?;
        let mut patient = None;
        if let Some((p, next)) = self.entity(end) {
            patient = Some(p);
            end = next;
        }
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Negative,
                tense_aspect: if aux == "did" {
                    TenseAspect::Past
                } else {
                    TenseAspect::Simple
                },
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_passive(&self) -> Option<MeaningProgram> {
        let (patient, idx) = self.subject(0)?;
        if !matches!(self.words.get(idx)?.text.as_str(), "was" | "were") {
            return None;
        }
        let (predicate, _, mut end) = self.verb(idx + 1)?;
        let mut agent = None;
        if self.words.get(end).map(|w| w.text.as_str()) == Some("by") {
            let (by_agent, next) = self.subject(end + 1)?;
            agent = Some(by_agent);
            end = next;
        }
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent,
                patient: Some(patient),
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Passive,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_experiential(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        if !matches!(self.words.get(idx)?.text.as_str(), "have" | "has") {
            return None;
        }
        let form = self.words.get(idx + 1)?.text.as_str();
        const PARTICIPLES: &[&str] = &[
            "read", "seen", "eaten", "gone", "written", "done", "made", "taken", "given", "been",
            "spoken", "broken", "chosen", "known", "worked", "lived", "studied",
        ];
        if !PARTICIPLES.contains(&form) && !form.ends_with("ed") {
            return None;
        }
        let (predicate, _, mut end) = self.verb(idx + 1)?;
        let patient = if let Some((p, next)) = self.entity(end) {
            end = next;
            Some(p)
        } else {
            None
        };
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Experiential,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_can(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        if self.words.get(idx)?.text != "can" {
            return None;
        }
        let negative = self.words.get(idx + 1).map(|w| w.text.as_str()) == Some("not");
        let (predicate, _, mut end) = self.verb(if negative { idx + 2 } else { idx + 1 })?;
        let mut patient = None;
        if let Some((p, next)) = self.entity(end) {
            patient = Some(p);
            end = next;
        }
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: if negative {
                    Polarity::Negative
                } else {
                    Polarity::Affirmative
                },
                tense_aspect: TenseAspect::Modal("can".to_string()),
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_progressive(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        if !matches!(self.words.get(idx)?.text.as_str(), "is" | "are" | "am") {
            return None;
        }
        let negative = self.words.get(idx + 1).map(|w| w.text.as_str()) == Some("not");
        let (predicate, _, mut end) = self.verb(if negative { idx + 2 } else { idx + 1 })?;
        if !predicate.word.ends_with("ing") {
            return None;
        }
        let mut patient = None;
        if let Some((p, next)) = self.entity(end) {
            patient = Some(p);
            end = next;
        }
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: if negative {
                    Polarity::Negative
                } else {
                    Polarity::Affirmative
                },
                tense_aspect: TenseAspect::Progressive,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_afraid(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        if !matches!(self.words.get(idx)?.text.as_str(), "is" | "are" | "am")
            || self.words.get(idx + 1)?.text != "afraid"
            || self.words.get(idx + 2)?.text != "of"
        {
            return None;
        }
        let (patient, end) = self.entity(idx + 3)?;
        if end != self.words.len() {
            return None;
        }
        let predicate = Lexeme {
            word: "afraid".to_string(),
            lemma: "afraid".to_string(),
            zh: "害怕".to_string(),
            pos: "adjective".to_string(),
            span: self.span(idx + 1, idx + 2),
        };
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient: Some(patient),
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_comparison(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        if !matches!(self.words.get(idx)?.text.as_str(), "is" | "are" | "am") {
            return None;
        }
        let (predicate, end) = self.adjective(idx + 1)?;
        if self.words.get(end).map(|w| w.text.as_str()) != Some("than") {
            return None;
        }
        let (than, next) = self.entity(end + 1)?;
        if next != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient: None,
                complement: None,
                collocation: None,
                comparison: Some(than),
                modifiers: Vec::new(),
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_be_adjective(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        if !matches!(self.words.get(idx)?.text.as_str(), "is" | "are" | "am") {
            return None;
        }
        let negative = self.words.get(idx + 1).map(|w| w.text.as_str()) == Some("not");
        let (predicate, end) = self.adjective(if negative { idx + 2 } else { idx + 1 })?;
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient: None,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers: Vec::new(),
                polarity: if negative {
                    Polarity::Negative
                } else {
                    Polarity::Affirmative
                },
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_past(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        let (predicate, past, mut end) = self.verb(idx)?;
        if !past {
            return None;
        }
        let mut patient = None;
        let mut modifiers = Vec::new();
        if let Some((place, next)) = self.place(end) {
            modifiers.push(Modifier::Place(place));
            end = next;
        } else if let Some((p, next)) = self.entity(end) {
            patient = Some(p);
            end = next;
        }
        if let Some((time, next)) = self.time(end) {
            modifiers.push(Modifier::Time(time));
            end = next;
        }
        if end != self.words.len() {
            return None;
        }
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient,
                complement: None,
                collocation: None,
                comparison: None,
                modifiers,
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Past,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_simple(&self) -> Option<MeaningProgram> {
        let (agent, idx) = self.subject(0)?;
        let (predicate, past, mut end) = self.verb(idx)?;
        if past {
            return None;
        }
        let mut patient = None;
        let mut modifiers = Vec::new();
        if let Some((place, next)) = self.place(end) {
            modifiers.push(Modifier::Place(place));
            end = next;
        } else if let Some((manner, next)) = self.adverb(end) {
            modifiers.push(Modifier::Manner(manner));
            end = next;
        } else if let Some((degree, next)) = self.adjective(end) {
            if next == self.words.len() {
                modifiers.push(Modifier::Degree(degree.zh));
                end = next;
            }
        } else if let Some((p, next)) = self.entity_coordinated(end) {
            patient = Some(p);
            end = next;
        }
        if end != self.words.len() {
            return None;
        }
        let collocation = self.collocation(&predicate, patient.as_ref());
        Some(MeaningProgram {
            frames: vec![MeaningFrame {
                predicate,
                agent: Some(agent),
                patient,
                complement: None,
                collocation,
                comparison: None,
                modifiers,
                polarity: Polarity::Affirmative,
                tense_aspect: TenseAspect::Simple,
                question: QuestionType::None,
                source_spans: vec![self.span(0, self.words.len())],
            }],
            relation: ProgramRelation::Single,
        })
    }

    fn parse_bare_imperative(&self) -> Option<MeaningProgram> {
        self.parse_imperative(0, self.words.len())
    }
}

/// 把源文标准化为稳定 key（给测试/诊断查询语义规则使用）。
pub fn semantic_key(text: &str) -> String {
    normalize_key(text)
}
