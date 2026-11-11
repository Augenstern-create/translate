//! 术语表：**生成前约束 + 生成后校验**，而不是译后字符串替换。
//!
//! # 为什么改
//!
//! 旧实现是"输出侧守卫"：翻译完成后扫一遍译文，发现某个变体就（在 `Enforce`
//! 下）替换掉。这在下面三种情况下会直接把译文改坏（§6.1 / §6.3）：
//!
//! 1. 一个英文术语有多个中文对应（`model` → 模型/型号/模特）；
//! 2. 译文里的那个词并不对应源文里的术语（`planet earth` → `world`）；
//! 3. 替换会破坏受保护跨度（代码、URL、模板变量）。
//!
//! 新流程：
//!
//! ```text
//! 源文命中术语 → 只把这些约束送进提示词（§6.5 控制长度）
//!              → 模型在生成时就照着写
//!              → 译后再校验是否满足（Warn 只报告，Enforce 才拒绝）
//! ```
//!
//! `TermPolicy::Enforce` 的语义因此变为"**严格的术语必须满足，否则本次翻译失败**"，
//! 而不是"把译文里的词替换成标准词"。这样术语约束仍然可被审计，但不会静默改坏句子。
//!
//! # 冲突规则（§6.4）
//!
//! 1. 更长的源短语优先（`large language model` 优于 `language model`）；
//! 2. 领域规则优先于通用规则；
//! 3. 用户自定义优先于内置；
//! 4. 同级且目标词冲突 → **加载期报配置错误**，不随机选择。

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use translator_types::TermConstraint;

/// 术语匹配方式。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    /// 源文出现该短语即命中（默认）。
    #[default]
    Phrase,
    /// 必须作为完整词出现（前后不是字母数字）。
    Word,
}

/// 术语来源。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TermOrigin {
    /// 系统内置（产品/领域通用）。
    #[default]
    Builtin,
    /// 用户或项目自定义。
    User,
}

/// 一条术语。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TermEntry {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(default = "default_lang")]
    pub source_language: String,
    #[serde(default = "default_target_lang")]
    pub target_language: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub match_type: MatchType,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub priority: i32,
    /// `true` = 明确的业务术语约束，译后必须满足（`Enforce` 下不满足即失败）。
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub origin: TermOrigin,
}

fn default_lang() -> String {
    "en".to_string()
}
fn default_target_lang() -> String {
    "zh-CN".to_string()
}

/// 一处术语违规。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TermViolation {
    /// 术语 id。
    pub term_id: String,
    /// 期望的目标写法。
    pub preferred: String,
    /// 源文里的术语。
    pub found: String,
    /// 是否硬约束。
    pub strict: bool,
}

impl TermViolation {
    pub fn detail(&self) -> String {
        format!(
            "术语 {}：「{}」应译为「{}」",
            self.term_id, self.found, self.preferred
        )
    }
}

/// 一条术语的命中情况。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermHit {
    pub term_id: String,
    pub source: String,
    pub target: String,
    pub occurrences: usize,
    pub satisfied: bool,
    pub strict: bool,
}

/// 术语校验结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TermVerification {
    pub hits: Vec<TermHit>,
    pub violations: Vec<TermViolation>,
}

impl TermVerification {
    pub fn satisfied_count(&self) -> usize {
        self.hits.iter().filter(|h| h.satisfied).count()
    }

    /// 硬约束是否全部满足。
    pub fn strict_satisfied(&self) -> bool {
        !self.violations.iter().any(|v| v.strict)
    }

    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }
}

/// 术语表加载错误。
#[derive(Debug)]
pub enum GlossaryError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
    /// 两条同级术语对同一源术语给出不同目标写法。
    Conflict(String),
}

impl std::fmt::Display for GlossaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "读取术语表失败：{e}"),
            Self::Json(e) => write!(f, "术语表 JSON 非法：{e}"),
            Self::Invalid(msg) => write!(f, "术语表不合法：{msg}"),
            Self::Conflict(msg) => write!(f, "术语表冲突：{msg}"),
        }
    }
}

impl std::error::Error for GlossaryError {}
impl From<std::io::Error> for GlossaryError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for GlossaryError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    schema_version: u16,
    id: String,
    version: u32,
    #[serde(default)]
    domain: Option<String>,
    entries: Vec<TermEntry>,
}

/// 编译后的只读术语表。
#[derive(Clone, Debug)]
pub struct Glossary {
    id: String,
    version: u32,
    /// 默认领域（条目未显式声明时使用）。
    domain: Option<String>,
    /// 已按优先级排序的条目。
    entries: Vec<TermEntry>,
}

impl Glossary {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, GlossaryError> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    pub fn from_json(json: &str) -> Result<Self, GlossaryError> {
        let spec: Spec = serde_json::from_str(json)?;
        if spec.schema_version != 1 {
            return Err(GlossaryError::Invalid(format!(
                "不支持的 schema_version={}",
                spec.schema_version
            )));
        }
        if spec.id.trim().is_empty() {
            return Err(GlossaryError::Invalid("id 不能为空".to_string()));
        }
        Self::new(spec.id, spec.version, spec.domain, spec.entries)
    }

    pub fn new(
        id: impl Into<String>,
        version: u32,
        domain: Option<String>,
        mut entries: Vec<TermEntry>,
    ) -> Result<Self, GlossaryError> {
        for entry in &entries {
            if entry.id.trim().is_empty() {
                return Err(GlossaryError::Invalid("条目 id 不能为空".to_string()));
            }
            if entry.source.trim().is_empty() || entry.target.trim().is_empty() {
                return Err(GlossaryError::Invalid(format!(
                    "条目 {} 的 source/target 不能为空",
                    entry.id
                )));
            }
        }

        // §6.4 第 4 条：同级 + 同作用域 + 不同目标 → 配置错误
        let mut seen: BTreeMap<(String, Option<String>), (&str, &str)> = BTreeMap::new();
        for entry in &entries {
            let key = (
                normalize_term(&entry.source, entry.case_sensitive),
                entry.domain.clone(),
            );
            if let Some((other_id, other_target)) = seen.get(&key) {
                if *other_target != entry.target {
                    return Err(GlossaryError::Conflict(format!(
                        "「{}」同时被 {} 译为「{}」、被 {} 译为「{}」；请合并或改用不同 domain",
                        entry.source, other_id, other_target, entry.id, entry.target
                    )));
                }
            } else {
                seen.insert(key, (&entry.id, &entry.target));
            }
        }

        // 排序：长源短语优先 → 领域优先 → 用户优先 → priority 降序 → id 升序
        entries.sort_by(|a, b| {
            b.source
                .chars()
                .count()
                .cmp(&a.source.chars().count())
                .then_with(|| b.domain.is_some().cmp(&a.domain.is_some()))
                .then_with(|| {
                    let rank = |o: TermOrigin| if o == TermOrigin::User { 0 } else { 1 };
                    rank(a.origin).cmp(&rank(b.origin))
                })
                .then_with(|| b.priority.cmp(&a.priority))
                .then_with(|| a.id.cmp(&b.id))
        });

        Ok(Self {
            id: id.into(),
            version,
            domain,
            entries,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn entries(&self) -> &[TermEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 源文实际命中的条目（按优先级排序）。
    ///
    /// `domain` 为 `Some` 时，只返回该领域与通用条目。
    pub fn applicable(
        &self,
        source: &str,
        source_lang: &str,
        target_lang: &str,
        domain: Option<&str>,
    ) -> Vec<&TermEntry> {
        let domain = domain.or(self.domain.as_deref());
        self.entries
            .iter()
            .filter(|entry| {
                entry.source_language.eq_ignore_ascii_case(source_lang)
                    && entry.target_language.eq_ignore_ascii_case(target_lang)
            })
            .filter(|entry| match (&entry.domain, domain) {
                (None, _) => true,
                (Some(d), Some(wanted)) => d == wanted,
                (Some(_), None) => false,
            })
            .filter(|entry| count_matches(source, entry) > 0)
            .collect()
    }

    /// 生成本次翻译的术语约束（送进提示词）。
    pub fn constraints(
        &self,
        source: &str,
        source_lang: &str,
        target_lang: &str,
        domain: Option<&str>,
    ) -> Vec<TermConstraint> {
        self.applicable(source, source_lang, target_lang, domain)
            .into_iter()
            .map(|entry| TermConstraint {
                source: entry.source.clone(),
                target: entry.target.clone(),
                strict: entry.strict,
                matched: match entry.match_type {
                    MatchType::Phrase => "phrase".to_string(),
                    MatchType::Word => "word".to_string(),
                },
            })
            .collect()
    }

    /// 译后校验。
    pub fn verify(
        &self,
        source: &str,
        output: &str,
        source_lang: &str,
        target_lang: &str,
        domain: Option<&str>,
    ) -> TermVerification {
        let mut verification = TermVerification::default();
        for entry in self.applicable(source, source_lang, target_lang, domain) {
            let occurrences = count_matches(source, entry);
            let satisfied = output.contains(&entry.target);
            verification.hits.push(TermHit {
                term_id: entry.id.clone(),
                source: entry.source.clone(),
                target: entry.target.clone(),
                occurrences,
                satisfied,
                strict: entry.strict,
            });
            if !satisfied {
                verification.violations.push(TermViolation {
                    term_id: entry.id.clone(),
                    preferred: entry.target.clone(),
                    found: entry.source.clone(),
                    strict: entry.strict,
                });
            }
        }
        verification
    }
}

fn normalize_term(term: &str, case_sensitive: bool) -> String {
    let trimmed = term.trim();
    if case_sensitive {
        trimmed.to_string()
    } else {
        trimmed.to_lowercase()
    }
}

/// 统计源文里某个术语出现的次数。
fn count_matches(source: &str, entry: &TermEntry) -> usize {
    let (haystack, needle) = if entry.case_sensitive {
        (source.to_string(), entry.source.clone())
    } else {
        (source.to_lowercase(), entry.source.to_lowercase())
    };

    match entry.match_type {
        MatchType::Phrase => haystack.matches(&needle).count(),
        MatchType::Word => {
            let mut count = 0usize;
            let mut from = 0usize;
            while let Some(pos) = haystack[from..].find(&needle) {
                let at = from + pos;
                let end = at + needle.len();
                let left_ok = at == 0
                    || !haystack[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_');
                let right_ok = end >= haystack.len()
                    || !haystack[end..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_');
                if left_ok && right_ok {
                    count += 1;
                }
                from = end;
            }
            count
        }
    }
}

/// 术语约束策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TermPolicy {
    /// 完全不检查。
    Off,
    /// 只报告违规、**不改文本**（默认）。
    #[default]
    Warn,
    /// 硬约束（`strict = true`）不满足时，本次翻译**失败**。
    ///
    /// 注意：它不是"把译文里的词替换成标准词"——那会破坏语义（§6.3）。
    Enforce,
}

impl TermPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Warn => "warn",
            Self::Enforce => "enforce",
        }
    }

    /// 是否要跑术语校验。
    pub const fn runs(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// 是否把"硬约束未满足"当成失败。
    pub const fn rejects_unmet_strict(self) -> bool {
        matches!(self, Self::Enforce)
    }

    /// 兼容旧名字：旧语义是"用守卫改过的文本"，现在是 `false`。
    pub const fn applies_text(self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glossary(json: &str) -> Glossary {
        Glossary::from_json(json).unwrap()
    }

    const SAMPLE: &str = r#"{
      "schema_version": 1,
      "id": "glossary.software",
      "version": 3,
      "domain": "software",
      "entries": [
        {"id":"term_001","source":"large language model","target":"大语言模型",
         "case_sensitive":false,"match_type":"phrase","priority":100,"strict":true},
        {"id":"term_002","source":"language model","target":"语言模型","priority":10},
        {"id":"term_003","source":"model","target":"模型","priority":1},
        {"id":"term_004","source":"pull request","target":"拉取请求","strict":true,"origin":"user"}
      ]
    }"#;

    #[test]
    fn 加载并按长度优先排序() {
        let g = glossary(SAMPLE);
        assert_eq!(g.version(), 3);
        let sources: Vec<&str> = g.entries().iter().map(|e| e.source.as_str()).collect();
        assert_eq!(
            sources,
            vec![
                "large language model",
                "language model",
                "pull request",
                "model"
            ]
        );
    }

    #[test]
    fn 只发送源文实际命中的术语() {
        let g = glossary(SAMPLE);
        let constraints = g.constraints(
            "This large language model beats a small model.",
            "en",
            "zh-CN",
            None,
        );
        let sources: Vec<&str> = constraints.iter().map(|c| c.source.as_str()).collect();
        assert!(sources.contains(&"large language model"));
        assert!(sources.contains(&"model"));
        assert!(!sources.contains(&"pull request"), "未命中的术语不得发送");
    }

    #[test]
    fn 校验发现未满足的硬约束() {
        let g = glossary(SAMPLE);
        let v = g.verify(
            "Open a pull request with the new model.",
            "用新模型开一个 PR。",
            "en",
            "zh-CN",
            None,
        );
        assert!(!v.strict_satisfied());
        assert!(v
            .violations
            .iter()
            .any(|x| x.term_id == "term_004" && x.strict));
    }

    #[test]
    fn 校验通过时不报违规() {
        let g = glossary(SAMPLE);
        let v = g.verify(
            "Open a pull request.",
            "请提交拉取请求。",
            "en",
            "zh-CN",
            None,
        );
        assert!(v.strict_satisfied());
        assert!(v.violations.is_empty(), "{v:?}");
        assert_eq!(v.satisfied_count(), 1);
    }

    #[test]
    fn 领域限制生效() {
        let g = glossary(
            r#"{"schema_version":1,"id":"g","version":1,"entries":[
                {"id":"a","source":"bank","target":"银行","domain":"finance"},
                {"id":"b","source":"bank","target":"河岸","domain":"geography"}
            ]}"#,
        );
        let finance = g.applicable(
            "river bank and bank account",
            "en",
            "zh-CN",
            Some("finance"),
        );
        assert_eq!(finance.len(), 1);
        assert_eq!(finance[0].target, "银行");

        let geography = g.applicable("river bank", "en", "zh-CN", Some("geography"));
        assert_eq!(geography[0].target, "河岸");

        // 没指定领域时不返回领域条目（避免猜）
        assert!(g.applicable("river bank", "en", "zh-CN", None).is_empty());
    }

    #[test]
    fn 同级冲突在加载期报错() {
        let err = Glossary::from_json(
            r#"{"schema_version":1,"id":"g","version":1,"entries":[
                {"id":"a","source":"model","target":"模型"},
                {"id":"b","source":"model","target":"型号"}
            ]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, GlossaryError::Conflict(_)), "{err}");
    }

    #[test]
    fn 单词匹配不会命中词内片段() {
        let g = glossary(
            r#"{"schema_version":1,"id":"g","version":1,"entries":[
                {"id":"a","source":"cat","target":"猫","match_type":"word"}
            ]}"#,
        );
        assert!(g.applicable("the cat sat", "en", "zh-CN", None).len() == 1);
        assert!(g.applicable("concatenate", "en", "zh-CN", None).is_empty());
    }

    #[test]
    fn 大小写敏感可配置() {
        let insensitive = glossary(
            r#"{"schema_version":1,"id":"g","version":1,"entries":[
                {"id":"a","source":"API","target":"接口"}]}"#,
        );
        assert_eq!(
            insensitive.applicable("the api", "en", "zh-CN", None).len(),
            1
        );

        let sensitive = glossary(
            r#"{"schema_version":1,"id":"g","version":1,"entries":[
                {"id":"a","source":"API","target":"接口","case_sensitive":true}]}"#,
        );
        assert!(sensitive
            .applicable("the api", "en", "zh-CN", None)
            .is_empty());
        assert_eq!(
            sensitive.applicable("the API", "en", "zh-CN", None).len(),
            1
        );
    }

    #[test]
    fn 用户规则优先于内置() {
        let g = glossary(
            r#"{"schema_version":1,"id":"g","version":1,"entries":[
                {"id":"builtin","source":"llm","target":"大语言模型","domain":"a"},
                {"id":"user","source":"llm","target":"大型语言模型","domain":"b","origin":"user"}
            ]}"#,
        );
        // 两个条目属于不同 domain，因此不冲突；排序时用户规则在前
        assert_eq!(g.entries()[0].id, "user");
    }

    #[test]
    fn 默认策略只报告不改写() {
        assert_eq!(TermPolicy::default(), TermPolicy::Warn);
        assert!(!TermPolicy::default().applies_text());
        assert!(!TermPolicy::Warn.rejects_unmet_strict());
        assert!(TermPolicy::Enforce.rejects_unmet_strict());
    }

    #[test]
    fn 空术语表不产生约束() {
        let g = glossary(r#"{"schema_version":1,"id":"g","version":1,"entries":[]}"#);
        assert!(g.is_empty());
        assert!(g.constraints("anything", "en", "zh-CN", None).is_empty());
    }
}
