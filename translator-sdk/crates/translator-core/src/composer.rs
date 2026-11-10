//! P19-G：日常英语组合式规则与上下文选义（第一版）。
//!
//! 这不是通用语法分析器，也不是第二套翻译器。它只处理**结构清楚、词义可由
//! 词典词性 + 规则词表确认、完整覆盖**的日常简单句；任何未知成分都返回
//! `None`，由现有 Planner 继续走 NMT。
//!
//! 设计边界：
//! - 句子规则数据化：词、量词、固定表达、短语动词都在 JSON；
//! - Rust 只保留通用 tokenization、词性/结构匹配和中文组合；
//! - 规则输出必须完整覆盖 token，且每个实词都要能在正式词典中找到对应词性；
//! - 不把模型译文写进规则数据。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use translator_analysis::{EnglishLemmatizer, MorphologyAnalyzer};
use translator_types::{DictionaryEntry, LanguageTag};

use crate::evidence::{LanguageEvidence, PatternKind, PatternMatch, TextSpan, UnitKind};
use crate::templates::normalize_key;

#[derive(Debug)]
pub enum CompositionError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
}

impl std::fmt::Display for CompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "读取组合规则失败：{e}"),
            Self::Json(e) => write!(f, "组合规则 JSON 非法：{e}"),
            Self::Invalid(msg) => write!(f, "组合规则不合法：{msg}"),
        }
    }
}
impl std::error::Error for CompositionError {}
impl From<std::io::Error> for CompositionError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for CompositionError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct WordSpec {
    pos: String,
    zh: String,
    #[serde(default)]
    classifier: Option<String>,
    #[serde(default)]
    alternates: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PhrasalSpec {
    verb: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SenseSpec {
    id: String,
    pos: String,
    zh: String,
    #[serde(default)]
    object: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    schema_version: u16,
    id: String,
    version: u32,
    source: String,
    target: String,
    words: BTreeMap<String, WordSpec>,
    #[serde(default)]
    collocations: BTreeMap<String, String>,
    #[serde(default)]
    senses: BTreeMap<String, Vec<SenseSpec>>,
    #[serde(default)]
    phrases: BTreeMap<String, String>,
    #[serde(default)]
    phrasal_verbs: BTreeMap<String, PhrasalSpec>,
    #[serde(default)]
    past_forms: Vec<String>,
}

/// 一次组合成功的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompositionOutcome {
    pub text: String,
    /// 规则 id（稳定，进入诊断）。
    pub rule_id: String,
    /// 输入被哪个句式使用（`simple-svo` / `please-imperative` …）。
    pub pattern: String,
    /// 共享语言证据（完整覆盖 + 规则 id）。
    pub evidence: LanguageEvidence,
}

/// 词元及原文位置。contraction 展开时多个 token 共享原始 span。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompositionToken {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompositionAnalysis {
    pub tokens: Vec<CompositionToken>,
    pub question: bool,
}

/// 第一版组合规则引擎。
pub struct CompositionEngine {
    id: String,
    version: u32,
    source: LanguageTag,
    target: LanguageTag,
    words: HashMap<String, WordSpec>,
    collocations: HashMap<String, String>,
    senses: HashMap<String, Vec<SenseSpec>>,
    phrases: HashMap<String, String>,
    phrasal_verbs: HashMap<String, PhrasalSpec>,
    past_forms: HashSet<String>,
    lemmatizer: EnglishLemmatizer,
}

impl std::fmt::Debug for CompositionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositionEngine")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("words", &self.words.len())
            .field("phrases", &self.phrases.len())
            .field("phrasal_verbs", &self.phrasal_verbs.len())
            .finish()
    }
}

impl CompositionEngine {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, CompositionError> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    pub fn from_json(json: &str) -> Result<Self, CompositionError> {
        let spec: Spec = serde_json::from_str(json)?;
        if spec.schema_version != 1 {
            return Err(CompositionError::Invalid(format!(
                "不支持的 schema_version={}",
                spec.schema_version
            )));
        }
        if spec.id.trim().is_empty() {
            return Err(CompositionError::Invalid("id 不能为空".to_string()));
        }
        if spec.words.is_empty() {
            return Err(CompositionError::Invalid("words 不能为空".to_string()));
        }
        for (word, entry) in &spec.words {
            let function_word = matches!(
                entry.pos.as_str(),
                "auxiliary"
                    | "article"
                    | "preposition"
                    | "conjunction"
                    | "interjection"
                    | "determiner"
            );
            if word.trim().is_empty()
                || (entry.zh.trim().is_empty() && entry.alternates.is_empty() && !function_word)
            {
                return Err(CompositionError::Invalid(format!(
                    "词条 {word} 的 zh/alternates 不能为空"
                )));
            }
        }
        let source = LanguageTag::new(&spec.source)
            .map_err(|e| CompositionError::Invalid(format!("source 非法：{e}")))?;
        let target = LanguageTag::new(&spec.target)
            .map_err(|e| CompositionError::Invalid(format!("target 非法：{e}")))?;
        Ok(Self {
            id: spec.id,
            version: spec.version,
            source,
            target,
            words: spec.words.into_iter().collect(),
            collocations: spec
                .collocations
                .into_iter()
                .map(|(k, v)| (normalize_composition_key(&k), v))
                .collect(),
            senses: spec.senses.into_iter().collect(),
            phrases: spec
                .phrases
                .into_iter()
                .map(|(k, v)| (normalize_composition_key(&k), v))
                .collect(),
            phrasal_verbs: spec.phrasal_verbs.into_iter().collect(),
            past_forms: spec.past_forms.into_iter().collect(),
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
        self.phrases.len() + 12
    }

    pub fn source(&self) -> &LanguageTag {
        &self.source
    }

    pub fn target(&self) -> &LanguageTag {
        &self.target
    }

    /// 分析 token 与原文位置（G-01）。
    pub fn analyze(&self, text: &str) -> CompositionAnalysis {
        let tokenized = tokenize(text);
        CompositionAnalysis {
            tokens: tokenized.tokens,
            question: tokenized.question,
        }
    }

    /// 该输入是否值得尝试组合。用于避免对每个 NMT 句子都做多次词典查询。
    pub fn is_candidate(&self, text: &str) -> bool {
        let tokenized = tokenize(text);
        if tokenized.tokens.is_empty() || !tokenized.uncovered.is_empty() {
            return false;
        }
        if self.phrases.contains_key(&tokenized.key) {
            return true;
        }
        if tokenized.tokens.len() > 14 {
            return false;
        }
        let complex = [
            "that", "which", "who", "whom", "whose", "because", "although", "though", "while",
            "unless", "whereas", "however",
        ];
        if tokenized
            .tokens
            .iter()
            .any(|t| complex.contains(&t.text.as_str()))
        {
            return false;
        }
        matches!(
            tokenized.tokens[0].text.as_str(),
            "i" | "you"
                | "he"
                | "she"
                | "it"
                | "we"
                | "they"
                | "the"
                | "this"
                | "that"
                | "please"
                | "where"
                | "do"
                | "does"
                | "if"
        )
    }

    /// 需要预取词典的词形（原词 + lemma 候选）。调用方只读，不联网。
    pub fn lookup_candidates(&self, text: &str) -> Vec<String> {
        if !self.is_candidate(text) {
            return Vec::new();
        }
        let tokenized = tokenize(text);
        let mut out = Vec::new();
        for token in tokenized.tokens {
            push_unique(&mut out, token.text.clone());
            for lemma in self.lemmatizer.lemmas(&token.text).into_iter().take(4) {
                push_unique(&mut out, lemma);
            }
            if out.len() >= 24 {
                break;
            }
        }
        out
    }

    /// 组合翻译。`entries` 是 Coordinator 为 `lookup_candidates` 预取的正式词典条目。
    pub fn compose(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
        entries: &HashMap<String, Vec<DictionaryEntry>>,
    ) -> Option<CompositionOutcome> {
        if source != &self.source || target != &self.target {
            return None;
        }
        let tokenized = tokenize(text);
        if tokenized.tokens.is_empty() || !tokenized.uncovered.is_empty() {
            return None;
        }
        let dict = DictView { entries };

        if let Some(chinese) = self.phrases.get(&tokenized.key) {
            return Some(self.finish(
                text,
                tokenized.question,
                "phrase.exact",
                "fixed-expression",
                chinese.clone(),
            ));
        }

        if !self.is_candidate(text) {
            return None;
        }
        let words = &tokenized.tokens;
        let mut parser = Parser {
            engine: self,
            dict: &dict,
            words,
            question: tokenized.question,
            last: None,
        };

        if let Some((chinese, pattern)) = parser.parse() {
            return Some(self.finish(
                text,
                tokenized.question,
                &format!("rule.{pattern}"),
                pattern,
                chinese,
            ));
        }
        None
    }

    fn finish(
        &self,
        original: &str,
        question: bool,
        rule_id: &str,
        pattern: &str,
        chinese: String,
    ) -> CompositionOutcome {
        let mut chinese = chinese.trim().to_string();
        chinese = chinese
            .trim_end_matches(['.', '。', '!', '！', '?', '？'])
            .to_string();
        if chinese.is_empty() {
            chinese.push('。');
        } else if question {
            chinese.push('？');
        } else {
            chinese.push('。');
        }
        let mut evidence = LanguageEvidence::for_text(
            TextSpan::new(0, original.len()),
            UnitKind::Sentence,
            original,
        );
        evidence.add_pattern(PatternMatch {
            kind: PatternKind::Rule,
            id: rule_id.to_string(),
            span: TextSpan::new(0, original.len()),
            complete: true,
        });
        CompositionOutcome {
            text: chinese,
            rule_id: rule_id.to_string(),
            pattern: pattern.to_string(),
            evidence,
        }
    }

    pub(crate) fn lexicon_has_pos(&self, word: &str, pos: &str) -> bool {
        self.spec(word, pos).is_some()
    }

    pub(crate) fn phrasal_verb(&self, key: &str) -> Option<&str> {
        self.phrasal_verbs.get(key).map(|spec| spec.verb.as_str())
    }

    pub(crate) fn target_for_sense(&self, sense_id: &str) -> Option<&str> {
        self.senses
            .values()
            .flatten()
            .find(|sense| sense.id == sense_id)
            .map(|sense| sense.zh.as_str())
    }

    pub(crate) fn sense_targets(
        &self,
        lemma: &str,
        pos: &str,
        object: Option<&str>,
    ) -> Vec<(String, String)> {
        self.senses
            .get(lemma)
            .into_iter()
            .flatten()
            .filter(|sense| sense.pos == pos)
            .filter(|sense| {
                sense.object.is_none()
                    || object.is_some_and(|obj| sense.object.as_deref() == Some(obj))
            })
            .map(|sense| (sense.zh.clone(), sense.id.clone()))
            .collect()
    }

    pub(crate) fn collocation(&self, key: &str) -> Option<&str> {
        self.collocations
            .get(&normalize_composition_key(key))
            .map(String::as_str)
    }

    pub(crate) fn phrase(&self, key: &str) -> Option<&str> {
        self.phrases.get(key).map(String::as_str)
    }

    pub(crate) fn classifier(&self, word: &str) -> Option<&str> {
        self.words.get(word)?.classifier.as_deref()
    }

    fn spec(&self, word: &str, pos: &str) -> Option<&WordSpec> {
        let spec = self.words.get(word)?;
        if spec.pos == pos || spec.alternates.contains_key(pos) {
            Some(spec)
        } else {
            None
        }
    }

    pub(crate) fn zh(&self, word: &str, pos: &str) -> Option<&str> {
        let spec = self.words.get(word)?;
        if spec.pos == pos && !spec.zh.is_empty() {
            return Some(&spec.zh);
        }
        spec.alternates.get(pos).map(String::as_str)
    }

    pub(crate) fn noun_lemma(&self, word: &str) -> Option<String> {
        if self.spec(word, "noun").is_some() {
            return Some(word.to_string());
        }
        self.lemmatizer
            .lemmas(word)
            .into_iter()
            .find(|lemma| self.spec(lemma, "noun").is_some())
    }

    pub(crate) fn verb_lemma(&self, word: &str) -> Option<String> {
        if self.spec(word, "verb").is_some() {
            return Some(word.to_string());
        }
        self.lemmatizer
            .lemmas(word)
            .into_iter()
            .find(|lemma| self.spec(lemma, "verb").is_some())
    }

    pub(crate) fn adjective_lemma(&self, word: &str) -> Option<String> {
        if self.spec(word, "adjective").is_some() {
            return Some(word.to_string());
        }
        self.lemmatizer
            .lemmas(word)
            .into_iter()
            .find(|lemma| self.spec(lemma, "adjective").is_some())
    }

    pub(crate) fn is_past_form(&self, word: &str, lemma: &str) -> bool {
        word != lemma && (self.past_forms.contains(word) || word.ends_with("ed"))
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
                        && entry.parts_of_speech.iter().any(|p| pos_matches(p, pos))
                }) {
                    return true;
                }
            }
        }
        false
    }
}

fn pos_matches(actual: &str, wanted: &str) -> bool {
    let actual = actual.trim_end_matches('.').to_ascii_lowercase();
    match wanted {
        "verb" => matches!(actual.as_str(), "v" | "verb"),
        "noun" => matches!(actual.as_str(), "n" | "noun"),
        "adjective" => matches!(actual.as_str(), "adj" | "adjective"),
        "adverb" => matches!(actual.as_str(), "adv" | "adverb"),
        "pronoun" => matches!(actual.as_str(), "pron" | "pronoun"),
        _ => actual.eq_ignore_ascii_case(wanted),
    }
}

fn push_unique(items: &mut Vec<String>, value: String) {
    if !items.contains(&value) {
        items.push(value);
    }
}

pub(crate) struct Tokenized {
    pub(crate) tokens: Vec<CompositionToken>,
    /// 非字母、非可忽略标点的原文片段（数字、货币、百分号、Unicode 等）。
    /// 现阶段组合解析不处理这些片段，必须显式保留并拒绝完整覆盖声明。
    pub(crate) uncovered: Vec<TextSpan>,
    pub(crate) question: bool,
    pub(crate) key: String,
}

pub(crate) fn tokenize(text: &str) -> Tokenized {
    let question = text.trim_end().ends_with('?');
    let mut raw_tokens = Vec::new();
    let mut uncovered: Vec<TextSpan> = Vec::new();
    let mut i = 0usize;
    while i < text.len() {
        let ch = text[i..].chars().next().expect("valid char boundary");
        if ch.is_ascii_alphabetic() {
            let start = i;
            i += ch.len_utf8();
            while i < text.len() {
                let c = text[i..].chars().next().expect("valid char boundary");
                if c.is_ascii_alphabetic() || c == '\'' || c == '-' {
                    i += c.len_utf8();
                } else {
                    break;
                }
            }
            let part = &text[start..i];
            for expanded in expand_contraction(part) {
                raw_tokens.push(CompositionToken {
                    text: expanded,
                    start,
                    end: i,
                });
            }
            continue;
        }
        // 空白与句读可忽略；数字、货币、百分号、Unicode 字母等必须记录。
        if ch.is_whitespace()
            || matches!(
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
            )
        {
            i += ch.len_utf8();
            continue;
        }
        let start = i;
        i += ch.len_utf8();
        if let Some(last) = uncovered.last_mut() {
            if last.end == start {
                last.end = i;
                continue;
            }
        }
        uncovered.push(TextSpan::new(start, i));
    }
    let key = normalize_composition_key(
        &raw_tokens
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    Tokenized {
        tokens: raw_tokens,
        uncovered,
        question,
        key,
    }
}

fn expand_contraction(raw: &str) -> Vec<String> {
    let lower = raw.to_ascii_lowercase();
    let direct = match lower.as_str() {
        "i'm" => Some(vec!["i", "am"]),
        "i'll" => Some(vec!["i", "will"]),
        "i've" => Some(vec!["i", "have"]),
        "i'd" => Some(vec!["i", "would"]),
        "you're" => Some(vec!["you", "are"]),
        "we're" => Some(vec!["we", "are"]),
        "they're" => Some(vec!["they", "are"]),
        "he's" | "she's" | "it's" | "that's" => {
            let base = lower.trim_end_matches("'s");
            Some(vec![base, "is"])
        }
        "don't" => Some(vec!["do", "not"]),
        "doesn't" => Some(vec!["does", "not"]),
        "didn't" => Some(vec!["did", "not"]),
        "can't" => Some(vec!["can", "not"]),
        "won't" => Some(vec!["will", "not"]),
        "isn't" => Some(vec!["is", "not"]),
        "aren't" => Some(vec!["are", "not"]),
        "wasn't" => Some(vec!["was", "not"]),
        "weren't" => Some(vec!["were", "not"]),
        "haven't" => Some(vec!["have", "not"]),
        "hasn't" => Some(vec!["has", "not"]),
        "couldn't" => Some(vec!["could", "not"]),
        "shouldn't" => Some(vec!["should", "not"]),
        "wouldn't" => Some(vec!["would", "not"]),
        _ => None,
    };
    direct
        .unwrap_or_else(|| vec![lower.as_str()])
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn normalize_composition_key(text: &str) -> String {
    normalize_key(text)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || c.is_ascii_whitespace() || *c == '\'')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone)]
struct NounPhrase {
    zh: String,
    end: usize,
}

struct Verb {
    zh: String,
    lemma: String,
    past: bool,
    ing: bool,
    end: usize,
}

struct Parser<'a> {
    engine: &'a CompositionEngine,
    dict: &'a DictView<'a>,
    words: &'a [CompositionToken],
    question: bool,
    last: Option<String>,
}

impl Parser<'_> {
    fn parse(&mut self) -> Option<(String, &'static str)> {
        if self.try_if().is_some() {
            return self.last_result("if-condition");
        }
        if self.try_please().is_some() {
            return self.last_result("please-imperative");
        }
        if self.try_where_is().is_some() {
            return self.last_result("where-is");
        }
        if self.try_do_question().is_some() {
            return self.last_result("do-question");
        }
        if self.try_negation().is_some() {
            return self.last_result("negation");
        }
        if self.try_want_to().is_some() {
            return self.last_result("want-to");
        }
        if self.try_can().is_some() {
            return self.last_result("can-modal");
        }
        if self.try_progressive().is_some() {
            return self.last_result("progressive");
        }
        if self.try_afraid_of().is_some() {
            return self.last_result("afraid-of");
        }
        if self.try_be_adjective().is_some() {
            return self.last_result("be-adjective");
        }
        if self.try_past_svo().is_some() {
            return self.last_result("past-svo");
        }
        if self.try_simple_svo().is_some() {
            return self.last_result("simple-svo");
        }
        None
    }

    // 小技巧：各 try_* 只在成功时把候选写入 `last`；这里统一取。
    fn last_result(&mut self, pattern: &'static str) -> Option<(String, &'static str)> {
        self.last.take().map(|text| (text, pattern))
    }

    fn subject(&mut self, i: usize) -> Option<(String, usize)> {
        let token = self.words.get(i)?;
        if self.engine.words.get(&token.text).map(|s| s.pos.as_str()) == Some("pronoun") {
            return Some((self.engine.zh(&token.text, "pronoun")?.to_string(), i + 1));
        }
        let np = self.noun_phrase(i)?;
        Some((np.zh, np.end))
    }

    fn noun_phrase(&mut self, i: usize) -> Option<NounPhrase> {
        let mut idx = i;
        let mut determiner: Option<&str> = None;
        if let Some(token) = self.words.get(idx) {
            if matches!(
                token.text.as_str(),
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
                    | "some"
            ) {
                determiner = Some(&token.text);
                idx += 1;
            }
        }

        let mut adjectives: Vec<String> = Vec::new();
        while adjectives.len() < 2 {
            let Some(token) = self.words.get(idx) else {
                break;
            };
            let Some(lemma) = self.engine.adjective_lemma(&token.text) else {
                break;
            };
            // 只有后面确实还有名词时才把歧义词当形容词（a light bag）。
            let has_noun_after = self
                .words
                .get(idx + 1)
                .and_then(|next| self.engine.noun_lemma(&next.text))
                .is_some();
            if !has_noun_after {
                break;
            }
            if !self.dict.has_pos(&token.text, &lemma, "adjective") {
                break;
            }
            adjectives.push(
                self.engine
                    .zh(&lemma, "adjective")
                    .or_else(|| self.engine.zh(&token.text, "adjective"))?
                    .to_string(),
            );
            idx += 1;
        }

        let token = self.words.get(idx)?;
        let lemma = self.engine.noun_lemma(&token.text)?;
        if !self.dict.has_pos(&token.text, &lemma, "noun") {
            return None;
        }
        let spec = self.engine.words.get(&lemma)?;
        let noun_zh = spec
            .alternates
            .get("noun")
            .or_else(|| (spec.pos == "noun").then_some(&spec.zh))?;
        let mut zh = noun_zh.clone();
        if !adjectives.is_empty() {
            let mut prefix = adjectives.join("的");
            prefix.push('的');
            zh = format!("{prefix}{zh}");
        }
        if let Some(det) = determiner {
            match det {
                "a" | "an" => {
                    let classifier = spec.classifier.as_deref()?;
                    zh = format!("一{classifier}{zh}");
                }
                "the" => {}
                "some" => zh = format!("一些{zh}"),
                "this" => zh = format!("这个{zh}"),
                "that" => zh = format!("那个{zh}"),
                "my" => zh = format!("我的{zh}"),
                "your" => zh = format!("你的{zh}"),
                "his" => zh = format!("他的{zh}"),
                "her" => zh = format!("她的{zh}"),
                "our" => zh = format!("我们的{zh}"),
                "their" => zh = format!("他们的{zh}"),
                _ => return None,
            }
        }
        Some(NounPhrase { zh, end: idx + 1 })
    }

    fn verb(&mut self, i: usize) -> Option<Verb> {
        let token = self.words.get(i)?;
        if let Some(next) = self.words.get(i + 1) {
            let phrase = format!("{} {}", token.text, next.text);
            if let Some(spec) = self.engine.phrasal_verbs.get(&phrase) {
                return Some(Verb {
                    zh: spec.verb.clone(),
                    lemma: phrase,
                    past: false,
                    ing: false,
                    end: i + 2,
                });
            }
        }
        let lemma = self.engine.verb_lemma(&token.text)?;
        if !self.dict.has_pos(&token.text, &lemma, "verb") {
            return None;
        }
        let spec = self.engine.words.get(&lemma)?;
        let zh = spec
            .alternates
            .get("verb")
            .or_else(|| (spec.pos == "verb").then_some(&spec.zh))?
            .clone();
        Some(Verb {
            zh,
            lemma: lemma.clone(),
            past: self.engine.is_past_form(&token.text, &lemma),
            ing: token.text.ends_with("ing") && token.text != lemma,
            end: i + 1,
        })
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

    fn try_please(&mut self) -> Option<()> {
        if self.words.first()?.text != "please" {
            return None;
        }
        let verb = self.verb(1)?;
        let mut out = format!("请{}", verb.zh);
        let mut idx = verb.end;
        if let Some(np) = self.noun_phrase(idx) {
            out.push_str(&np.zh);
            idx = np.end;
        }
        if idx != self.words.len() {
            return None;
        }
        self.last = Some(out);
        Some(())
    }

    fn try_where_is(&mut self) -> Option<()> {
        if self.words.len() < 3 || self.words[0].text != "where" {
            return None;
        }
        if !matches!(self.words[1].text.as_str(), "is" | "are") {
            return None;
        }
        let np = self.noun_phrase(2)?;
        if np.end != self.words.len() {
            return None;
        }
        self.last = Some(format!("{}在哪里", np.zh));
        Some(())
    }

    fn try_do_question(&mut self) -> Option<()> {
        if !self.question || self.words.len() < 3 {
            return None;
        }
        if !matches!(self.words[0].text.as_str(), "do" | "does") {
            return None;
        }
        let (subject, idx) = self.subject(1)?;
        let verb = self.verb(idx)?;
        let mut out = format!("{subject}{}", verb.zh);
        let mut idx = verb.end;
        if let Some(np) = self.noun_phrase(idx) {
            out.push_str(&np.zh);
            idx = np.end;
        }
        if idx != self.words.len() {
            return None;
        }
        self.last = Some(format!("{out}吗"));
        Some(())
    }

    fn try_negation(&mut self) -> Option<()> {
        if self.question || self.words.len() < 4 {
            return None;
        }
        let (subject, idx) = self.subject(0)?;
        if self.words.get(idx)?.text == "be" {
            return None;
        }
        let aux = self.words.get(idx)?.text.clone();
        if !matches!(aux.as_str(), "do" | "does" | "did" | "is" | "are" | "am") {
            return None;
        }
        if self.words.get(idx + 1)?.text != "not" {
            return None;
        }
        if matches!(aux.as_str(), "is" | "are" | "am") {
            let token = self.words.get(idx + 2)?;
            let lemma = self.engine.adjective_lemma(&token.text)?;
            if !self.dict.has_pos(&token.text, &lemma, "adjective") || idx + 3 != self.words.len() {
                return None;
            }
            let adj = self.engine.zh(&lemma, "adjective")?.to_string();
            self.last = Some(format!("{subject}不{adj}"));
            return Some(());
        }
        let verb = self.verb(idx + 2)?;
        let mut out = subject.to_string();
        if aux == "did" {
            out.push_str("没有");
        } else {
            out.push('不');
        }
        out.push_str(&verb.zh);
        let mut end = verb.end;
        if let Some(np) = self.noun_phrase(end) {
            out.push_str(&np.zh);
            end = np.end;
        }
        if end != self.words.len() {
            return None;
        }
        self.last = Some(out);
        Some(())
    }

    fn try_want_to(&mut self) -> Option<()> {
        let (subject, idx) = self.subject(0)?;
        if self.words.get(idx)?.text != "want" && self.words.get(idx)?.text != "wants" {
            return None;
        }
        if self.words.get(idx + 1)?.text != "to" {
            return None;
        }
        let verb = self.verb(idx + 2)?;
        let mut out = format!("{subject}想{}", verb.zh);
        let mut end = verb.end;
        if let Some(np) = self.noun_phrase(end) {
            out.push_str(&np.zh);
            end = np.end;
        }
        if end != self.words.len() {
            return None;
        }
        self.last = Some(out);
        Some(())
    }

    fn try_can(&mut self) -> Option<()> {
        let (subject, idx) = self.subject(0)?;
        if self.words.get(idx)?.text != "can" {
            return None;
        }
        let verb = self.verb(idx + 1)?;
        let mut out = format!("{subject}会{}", verb.zh);
        let mut end = verb.end;
        if let Some(np) = self.noun_phrase(end) {
            out.push_str(&np.zh);
            end = np.end;
        }
        if end != self.words.len() {
            return None;
        }
        self.last = Some(out);
        Some(())
    }

    fn try_progressive(&mut self) -> Option<()> {
        let (subject, idx) = self.subject(0)?;
        let aux = self.words.get(idx)?.text.clone();
        if !matches!(aux.as_str(), "is" | "are" | "am") {
            return None;
        }
        let verb = self.verb(idx + 1)?;
        if !verb.ing {
            return None;
        }
        let mut out = format!("{subject}正在{}", verb.zh);
        let mut end = verb.end;
        if let Some(np) = self.noun_phrase(end) {
            out.push_str(&np.zh);
            end = np.end;
        }
        if end != self.words.len() {
            return None;
        }
        self.last = Some(out);
        Some(())
    }

    fn try_afraid_of(&mut self) -> Option<()> {
        let (subject, idx) = self.subject(0)?;
        let aux = self.words.get(idx)?.text.clone();
        if !matches!(aux.as_str(), "is" | "are" | "am") {
            return None;
        }
        if self.words.get(idx + 1)?.text != "afraid" {
            return None;
        }
        if self.words.get(idx + 2)?.text != "of" {
            return None;
        }
        let np = self.noun_phrase(idx + 3)?;
        if np.end != self.words.len() {
            return None;
        }
        if !self.dict.has_pos("afraid", "afraid", "adjective") {
            return None;
        }
        self.last = Some(format!("{subject}害怕{}", np.zh));
        Some(())
    }

    fn try_be_adjective(&mut self) -> Option<()> {
        let (subject, idx) = self.subject(0)?;
        let aux = self.words.get(idx)?.text.clone();
        if !matches!(aux.as_str(), "is" | "are" | "am") {
            return None;
        }
        let token = self.words.get(idx + 1)?;
        let lemma = self.engine.adjective_lemma(&token.text)?;
        if !self.dict.has_pos(&token.text, &lemma, "adjective") || idx + 2 != self.words.len() {
            return None;
        }
        let adj = self.engine.zh(&lemma, "adjective")?.to_string();
        self.last = Some(format!("{subject}很{adj}"));
        Some(())
    }

    fn try_past_svo(&mut self) -> Option<()> {
        if self.question {
            return None;
        }
        let (subject, idx) = self.subject(0)?;
        let verb = self.verb(idx)?;
        if !verb.past {
            return None;
        }
        let mut out = subject.to_string();
        let mut end = verb.end;
        if let Some((place, next)) = self.place(end) {
            if matches!(verb.lemma.as_str(), "go" | "come") && place == "家" {
                out.push_str("回家了");
            } else {
                out.push_str(&verb.zh);
                out.push_str(&place);
                out.push('了');
            }
            end = next;
        } else if let Some(np) = self.noun_phrase(end) {
            out.push_str(&verb.zh);
            out.push('了');
            out.push_str(&np.zh);
            end = np.end;
        } else {
            out.push_str(&verb.zh);
            out.push('了');
        }
        if end != self.words.len() {
            return None;
        }
        self.last = Some(out);
        Some(())
    }

    fn try_simple_svo(&mut self) -> Option<()> {
        if self.question {
            return None;
        }
        let (subject, idx) = self.subject(0)?;
        let verb = self.verb(idx)?;
        if verb.past {
            return None;
        }
        let mut out = subject.to_string();
        let mut end = verb.end;
        if let Some((place, next)) = self.place(end) {
            if (verb.lemma == "go" || verb.lemma == "come") && place == "家" {
                out.push_str("回家");
            } else if verb.lemma == "stay" && place == "家" {
                out.push_str("待在家里");
            } else {
                out.push_str(&verb.zh);
                out.push_str(&place);
            }
            end = next;
        } else if let Some(np) = self.noun_phrase(end) {
            out.push_str(&verb.zh);
            out.push_str(&np.zh);
            end = np.end;
        } else {
            out.push_str(&verb.zh);
        }
        if end != self.words.len() {
            return None;
        }
        self.last = Some(out);
        Some(())
    }

    fn try_if(&mut self) -> Option<()> {
        if self.words.first()?.text != "if" || self.words.len() < 6 {
            return None;
        }
        let (first_subject, idx) = self.subject(1)?;
        let first_verb = self.verb(idx)?;
        if first_verb.end >= self.words.len() {
            return None;
        }
        // `will` must appear after the first clause; comma is dropped by tokenization.
        let will_pos = self.words[first_verb.end..]
            .iter()
            .position(|t| t.text == "will")
            .map(|p| first_verb.end + p)?;
        if will_pos == first_verb.end {
            return None;
        }
        // 去掉逗号后，第二分句主语位于第一动词与 will 之间。
        let (second_subject, idx2) = self.subject(first_verb.end)?;
        if idx2 != will_pos {
            return None;
        }
        let second_verb = self.verb(will_pos + 1)?;
        let mut main = format!("{second_subject}就{}", second_verb.zh);
        let mut end = second_verb.end;
        if let Some((place, next)) = self.place(end) {
            if second_verb.lemma == "stay" && place == "家" {
                main.push_str("在家里");
            } else {
                main.push_str(&place);
            }
            end = next;
        } else if let Some(np) = self.noun_phrase(end) {
            main.push_str(&np.zh);
            end = np.end;
        }
        if end != self.words.len() {
            return None;
        }
        let condition = if first_subject == "它" && first_verb.lemma == "rain" {
            "如果下雨".to_string()
        } else {
            format!("如果{first_subject}{}", first_verb.zh)
        };
        self.last = Some(format!("{condition}，{main}"));
        Some(())
    }
}
