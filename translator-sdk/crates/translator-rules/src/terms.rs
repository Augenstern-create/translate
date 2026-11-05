//! 输出侧术语表（P2.4）。
//!
//! # 输入形态
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "id": "terms.zh-en.core",
//!   "version": 1,
//!   "terms": [
//!     { "target": "en", "preferred": "artificial intelligence",
//!       "variants": ["AI", "A.I."], "source_variants": ["人工智能"] },
//!     { "target": "*",  "preferred": "large language model", "variants": ["LLM"] }
//!   ]
//! }
//! ```
//!
//! `target` 为 `"*"` 表示对所有目标语言生效。
//!
//! # 源文触发条件（P2-S / S-03）
//!
//! `source_variants` 是**可选的源文触发条件**：只有源文里出现其中任意一个写法时，
//! 这条术语才会在译文里被检查/改写。
//!
//! 没有它的时候，`"planet earth" → "world"` 这种规则的含义是"**只要译文里出现
//! planet earth 就改**"——这在源文根本没提这件事时也可能触发，等于按字面改别人
//! 的句子。加上 `source_variants: ["世界"]` 之后，语义变成"源文提到世界、
//! 译文却写成 planet earth 时才纠正"，这才是术语约束该有的样子。
//!
//! 不带 `source_variants` 的条目仍然可用（等价于"永远触发"），但**默认策略是
//! `Warn`**（只报告不改），要改写必须显式 `Enforce`。同一个变体**不允许**在同一语言下
//! 出现两次（否则"该替换成哪个"就没有确定答案了），编译期直接报错。
//!
//! # 匹配规则（必须确定，不得依赖哈希顺序）
//!
//! 1. **大小写不敏感仅限 ASCII**：`"AI"` 能匹配 `"ai"`、`"Ai"`；
//!    非 ASCII 一律按码点精确比较（不做 locale 相关的 case folding——
//!    那需要完整 Unicode 大小写表，且会让"土耳其语 i"这类问题悄悄改变行为）。
//! 2. **整词匹配**：变体首尾都是 ASCII 字母数字时，要求前后不是"词字符"
//!    （字母/数字/下划线）。所以 `AI` 不会命中 `RAIL`、`AIs`、`AI_x`。
//!    CJK 变体没有词边界概念，按子串匹配。
//! 3. **最长优先**：同一位置多个变体都能匹配时取最长的那个。
//! 4. **单次扫描**：替换进来的 `preferred` 不会再被其它规则二次检查——
//!    否则两个互相包含的术语会来回替换成环。
//! 5. **受保护跨度不参与**：URL / 邮箱 / `{模板变量}` / 反引号代码里的内容
//!    用与规则引擎**同一个**扫描器（`translator_core::text::split_protected`）切出来，
//!    一律跳过。
//!
//! # 上限（为什么要有）
//!
//! 匹配是"每个变体扫一遍全文"，成本是 `O(文本长度 × 变体总长度)`。
//! 不给上限就等于给了一个可以拖慢每次翻译的入口。因此在编译期就卡死：
//! [`MAX_TERMS`] 条、[`MAX_TOTAL_VARIANT_BYTES`] 字节。真实术语表通常几十到几百条，
//! 这个上限只用来挡住异常输入。

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use translator_core::terms::{TargetTermGuard, TermContext, TermOutcome, TermViolation};
use translator_core::text;
use translator_types::LanguageTag;

/// 术语表 schema 版本。
pub const TERM_SCHEMA_VERSION: u16 = 1;
/// 最多术语条数。
pub const MAX_TERMS: usize = 4096;
/// 所有变体的总字节数上限。
pub const MAX_TOTAL_VARIANT_BYTES: usize = 256 * 1024;
/// 适配所有目标语言的键。
pub const ANY_TARGET: &str = "*";

#[derive(Debug)]
pub enum TermError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// 结构上不合法（缺字段、自相矛盾、超上限）。
    Invalid(String),
    /// schema 版本不支持。**明确拒收，不猜。**
    UnsupportedSchema(u16),
}

impl std::fmt::Display for TermError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "读取术语表失败：{e}"),
            Self::Json(e) => write!(f, "术语表 JSON 解析失败：{e}"),
            Self::Invalid(msg) => write!(f, "术语表不合法：{msg}"),
            Self::UnsupportedSchema(v) => write!(
                f,
                "不支持的术语表 schema_version={v}（本 SDK 支持 {TERM_SCHEMA_VERSION}）"
            ),
        }
    }
}

impl std::error::Error for TermError {}

impl From<std::io::Error> for TermError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for TermError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTable {
    schema_version: u16,
    id: String,
    version: u32,
    terms: Vec<RawTerm>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTerm {
    target: String,
    preferred: String,
    variants: Vec<String>,
    /// 源文触发条件。空表示"不限定源文"（总是检查）。
    #[serde(default)]
    source_variants: Vec<String>,
}

/// 编译后的一条替换规则。
#[derive(Clone, Debug)]
struct Rule {
    variant: String,
    preferred: String,
    /// 是否需要整词边界（变体首尾都是 ASCII 字母数字）。
    whole_word: bool,
    /// 源文触发条件。空表示"不限定源文"。
    ///
    /// 非空时：调用方必须提供源文，且源文包含其中之一才会生效；
    /// 拿不到源文（例如只做预览）时**跳过**该规则——无法确认触发条件就不动手。
    sources: Vec<String>,
}

/// 编译后的术语表。实现 [`TargetTermGuard`]，可直接注入 `Translator`。
#[derive(Clone, Debug)]
pub struct TargetTermTable {
    id: String,
    version: u32,
    /// 目标语言 → 规则（按变体长度降序，保证"最长优先"且顺序确定）。
    by_target: BTreeMap<String, Vec<Rule>>,
    total_variants: usize,
}

impl TargetTermTable {
    /// 从 JSON 文本编译。
    pub fn from_json(json: &str) -> Result<Self, TermError> {
        let raw: RawTable = serde_json::from_str(json)?;
        if raw.schema_version != TERM_SCHEMA_VERSION {
            return Err(TermError::UnsupportedSchema(raw.schema_version));
        }
        Self::compile(raw)
    }

    /// 从文件编译。
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, TermError> {
        let text = std::fs::read_to_string(path.as_ref())?;
        Self::from_json(&text)
    }

    fn compile(raw: RawTable) -> Result<Self, TermError> {
        if raw.id.trim().is_empty() {
            return Err(TermError::Invalid("id 不能为空".to_string()));
        }
        if raw.terms.is_empty() {
            return Err(TermError::Invalid("terms 不能为空".to_string()));
        }
        if raw.terms.len() > MAX_TERMS {
            return Err(TermError::Invalid(format!(
                "术语条数 {} 超过上限 {MAX_TERMS}",
                raw.terms.len()
            )));
        }

        let mut by_target: BTreeMap<String, Vec<Rule>> = BTreeMap::new();
        // 用于检测"同一语言 + 同一变体出现两次"：按 (target, lowercase variant) 记录 preferred
        let mut seen: BTreeMap<(String, String), String> = BTreeMap::new();
        let mut total_variants = 0usize;
        let mut total_bytes = 0usize;

        for term in raw.terms {
            if term.target.trim().is_empty() {
                return Err(TermError::Invalid("target 不能为空".to_string()));
            }
            if term.preferred.trim().is_empty() {
                return Err(TermError::Invalid(format!(
                    "preferred 不能为空（target={}）",
                    term.target
                )));
            }
            if term.variants.is_empty() {
                return Err(TermError::Invalid(format!(
                    "variants 不能为空（preferred={}）",
                    term.preferred
                )));
            }
            for source in &term.source_variants {
                if source.trim().is_empty() {
                    return Err(TermError::Invalid(format!(
                        "source_variants 不能含空字符串（preferred={}）",
                        term.preferred
                    )));
                }
                total_bytes = total_bytes.saturating_add(source.len());
                if total_bytes > MAX_TOTAL_VARIANT_BYTES {
                    return Err(TermError::Invalid(format!(
                        "变体与触发条件总字节数超过上限 {MAX_TOTAL_VARIANT_BYTES}"
                    )));
                }
            }
            let sources: Vec<String> = term
                .source_variants
                .iter()
                .map(|s| s.trim().to_string())
                .collect();
            // 目标语言做一次规范化，避免 "EN" / "en" 变成两张表
            let target = if term.target == ANY_TARGET {
                ANY_TARGET.to_string()
            } else {
                LanguageTag::new(&term.target)
                    .map_err(|e| TermError::Invalid(format!("target 非法：{e}")))?
                    .as_str()
                    .to_string()
            };

            for variant in term.variants {
                let variant = variant.trim().to_string();
                if variant.is_empty() {
                    return Err(TermError::Invalid(format!(
                        "变体不能为空字符串（preferred={}）",
                        term.preferred
                    )));
                }
                if variant == term.preferred {
                    // 替换成自己 → 永远"有违规"，没有意义，属于配置错误
                    return Err(TermError::Invalid(format!(
                        "变体与 preferred 相同：{variant}"
                    )));
                }
                total_variants += 1;
                total_bytes = total_bytes.saturating_add(variant.len());
                if total_bytes > MAX_TOTAL_VARIANT_BYTES {
                    return Err(TermError::Invalid(format!(
                        "变体总字节数超过上限 {MAX_TOTAL_VARIANT_BYTES}"
                    )));
                }

                let key = (target.clone(), variant.to_lowercase());
                if let Some(existing) = seen.get(&key) {
                    if existing != &term.preferred {
                        return Err(TermError::Invalid(format!(
                            "target={target} 下变体「{variant}」同时映射到「{existing}」与「{}」，无法确定唯一结果",
                            term.preferred
                        )));
                    }
                    // 完全重复：忽略，不重复替换
                    continue;
                }
                seen.insert(key, term.preferred.clone());

                by_target.entry(target.clone()).or_default().push(Rule {
                    whole_word: needs_word_boundary(&variant),
                    variant,
                    preferred: term.preferred.clone(),
                    sources: sources.clone(),
                });
            }
        }

        for rules in by_target.values_mut() {
            // 最长优先；长度相同则按字节序，保证完全确定
            rules.sort_by(|a, b| {
                b.variant
                    .len()
                    .cmp(&a.variant.len())
                    .then_with(|| a.variant.cmp(&b.variant))
            });
        }

        Ok(Self {
            id: raw.id,
            version: raw.version,
            by_target,
            total_variants,
        })
    }

    /// 终端用户可以看的统计信息。
    pub fn summary(&self) -> String {
        let targets: Vec<String> = self
            .by_target
            .iter()
            .map(|(target, rules)| format!("{target}:{}", rules.len()))
            .collect();
        format!(
            "{}@{}（{} 个变体，其中 {} 条需源文触发；{}）",
            self.id,
            self.version,
            self.total_variants,
            self.source_gated_count(),
            targets.join(", ")
        )
    }

    /// 需要源文触发条件的规则条数（诊断用：这些规则在无源文的预览里不会生效）。
    pub fn source_gated_count(&self) -> usize {
        self.by_target
            .values()
            .flatten()
            .filter(|r| !r.sources.is_empty())
            .count()
    }

    /// 某目标语言实际生效的规则条数（便于诊断"为什么没生效"）。
    pub fn rule_count_for(&self, target: &str) -> usize {
        self.by_target.get(target).map(|r| r.len()).unwrap_or(0)
            + self.by_target.get(ANY_TARGET).map(|r| r.len()).unwrap_or(0)
    }

    fn rules_for<'a>(&'a self, target: &str) -> impl Iterator<Item = &'a Rule> {
        self.by_target
            .get(target)
            .into_iter()
            .flatten()
            .chain(self.by_target.get(ANY_TARGET).into_iter().flatten())
    }
}

impl TargetTermGuard for TargetTermTable {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> u32 {
        self.version
    }

    /// 只凭译文检查：需要源文触发条件的规则一律跳过。
    ///
    /// 这是刻意的保守行为——拿不到源文就无法确认"源文真的提到了这个术语"，
    /// 凭译文猜等于按字面改别人的句子（S-03）。
    fn apply(&self, input: &str, _source: &LanguageTag, target: &LanguageTag) -> TermOutcome {
        self.run(input, None, target)
    }

    fn apply_with_context(&self, ctx: &TermContext<'_>) -> TermOutcome {
        self.run(ctx.target_text, Some(ctx.source_text), ctx.target)
    }
}

impl TargetTermTable {
    /// 统一实现：`source_text` 为 `None` 表示"没有源文"。
    fn run(&self, input: &str, source_text: Option<&str>, target: &LanguageTag) -> TermOutcome {
        let rules: Vec<&Rule> = self
            .rules_for(target.as_str())
            .filter(|rule| match (source_text, rule.sources.is_empty()) {
                // 没有触发条件：总是适用
                (_, true) => true,
                // 有触发条件但拿不到源文：跳过
                (None, false) => false,
                // 有触发条件：源文必须命中其中之一
                (Some(source), false) => rule
                    .sources
                    .iter()
                    .any(|s| !find_all(source, s, needs_word_boundary(s)).is_empty()),
            })
            .collect();
        if rules.is_empty() || input.is_empty() {
            return TermOutcome::unchanged(input);
        }

        // 受保护跨度：与规则引擎共用的同一个扫描器（偏移为 UTF-8 字节）
        let protected: Vec<(usize, usize)> = text::protected_spans(input)
            .into_iter()
            .map(|span| (span.start, span.end))
            .collect();

        // 收集所有候选命中
        let mut candidates: Vec<Candidate> = Vec::new();
        for (idx, rule) in rules.iter().enumerate() {
            for start in find_all(input, &rule.variant, rule.whole_word) {
                let end = start + rule.variant.len();
                if protected
                    .iter()
                    .any(|(p_start, p_end)| start < *p_end && end > *p_start)
                {
                    continue; // 落在受保护跨度里，跳过
                }
                candidates.push(Candidate {
                    start,
                    end,
                    rule: idx,
                });
            }
        }
        if candidates.is_empty() {
            return TermOutcome::unchanged(input);
        }

        // 最长优先 + 左到右贪心，保证不重叠且完全确定
        candidates.sort_by(|a, b| {
            a.start
                .cmp(&b.start)
                .then_with(|| b.end.cmp(&a.end))
                .then_with(|| a.rule.cmp(&b.rule))
        });

        let mut out = String::with_capacity(input.len());
        let mut violations = Vec::new();
        let mut cursor = 0usize;
        for candidate in candidates {
            if candidate.start < cursor {
                continue; // 与已接受的重叠
            }
            let rule = rules[candidate.rule];
            out.push_str(&input[cursor..candidate.start]);
            let offset = out.len();
            out.push_str(&rule.preferred);
            violations.push(TermViolation::new(
                rule.preferred.clone(),
                rule.variant.clone(),
                offset,
            ));
            cursor = candidate.end;
        }
        out.push_str(&input[cursor..]);

        TermOutcome {
            text: out,
            violations,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    start: usize,
    end: usize,
    rule: usize,
}

/// 变体是否需要整词边界：首尾都是 ASCII 字母数字时才要求。
fn needs_word_boundary(variant: &str) -> bool {
    let first = variant.chars().next();
    let last = variant.chars().last();
    matches!(first, Some(c) if c.is_ascii_alphanumeric())
        && matches!(last, Some(c) if c.is_ascii_alphanumeric())
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 找出所有匹配的起始字节偏移。
fn find_all(haystack: &str, needle: &str, whole_word: bool) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut prev_char: Option<char> = None;
    for (pos, ch) in haystack.char_indices() {
        let _ = ch;
        if matches_at(haystack, pos, needle) {
            let end = pos + needle.len();
            let ok_before = !whole_word || prev_char.map(|c| !is_word_char(c)).unwrap_or(true);
            let ok_after = if whole_word {
                haystack[end..]
                    .chars()
                    .next()
                    .map(|c| !is_word_char(c))
                    .unwrap_or(true)
            } else {
                true
            };
            if ok_before && ok_after {
                out.push(pos);
            }
        }
        prev_char = Some(ch);
    }
    out
}

/// 从 `pos` 起是否匹配 `needle`（ASCII 大小写不敏感，其余精确）。
fn matches_at(haystack: &str, pos: usize, needle: &str) -> bool {
    let mut rest = &haystack[pos..];
    for nc in needle.chars() {
        let Some(hc) = rest.chars().next() else {
            return false;
        };
        let same = if nc.is_ascii() && hc.is_ascii() {
            nc.eq_ignore_ascii_case(&hc)
        } else {
            nc == hc
        };
        if !same {
            return false;
        }
        rest = &rest[hc.len_utf8()..];
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = r#"{
        "schema_version": 1,
        "id": "terms.test",
        "version": 3,
        "terms": [
            { "target": "en", "preferred": "artificial intelligence", "variants": ["AI", "A.I."] },
            { "target": "en", "preferred": "large language model", "variants": ["LLM"] },
            { "target": "*",  "preferred": "computer", "variants": ["計算機"] }
        ]
    }"#;

    fn table() -> TargetTermTable {
        TargetTermTable::from_json(TABLE).expect("术语表应能编译")
    }

    fn en() -> LanguageTag {
        LanguageTag::new("en").unwrap()
    }

    fn zh() -> LanguageTag {
        LanguageTag::new("zh-Hans").unwrap()
    }

    #[test]
    fn 编译后可报出统计() {
        let t = table();
        assert_eq!(t.id(), "terms.test");
        assert_eq!(t.version(), 3);
        assert_eq!(t.total_variants, 4);
        // en 有 3 条，加 "*" 的 1 条
        assert_eq!(t.rule_count_for("en"), 4);
        assert_eq!(t.rule_count_for("zh-Hans"), 1);
        assert!(t.summary().contains("terms.test@3"));
    }

    #[test]
    fn 未知schema版本被拒收() {
        let json = TABLE.replace("\"schema_version\": 1", "\"schema_version\": 2");
        assert!(matches!(
            TargetTermTable::from_json(&json),
            Err(TermError::UnsupportedSchema(2))
        ));
    }

    #[test]
    fn 空变体被拒收() {
        let json = TABLE.replace("[\"AI\", \"A.I.\"]", "[\"\"]");
        assert!(matches!(
            TargetTermTable::from_json(&json),
            Err(TermError::Invalid(_))
        ));
    }

    #[test]
    fn 变体与规范术语相同时被拒收() {
        let json = TABLE.replace(
            "\"variants\": [\"AI\", \"A.I.\"]",
            "\"variants\": [\"artificial intelligence\"]",
        );
        assert!(matches!(
            TargetTermTable::from_json(&json),
            Err(TermError::Invalid(_))
        ));
    }

    #[test]
    fn 同一变体映射到两个不同规范术语时被拒收() {
        let json = r#"{
            "schema_version": 1, "id": "t", "version": 1,
            "terms": [
                { "target": "en", "preferred": "aaa", "variants": ["X"] },
                { "target": "en", "preferred": "bbb", "variants": ["x"] }
            ]
        }"#;
        assert!(matches!(
            TargetTermTable::from_json(json),
            Err(TermError::Invalid(_))
        ));
    }

    #[test]
    fn 重复的同义配置被折叠() {
        let json = r#"{
            "schema_version": 1, "id": "t", "version": 1,
            "terms": [
                { "target": "en", "preferred": "aaa", "variants": ["X", "x"] }
            ]
        }"#;
        let t = TargetTermTable::from_json(json).unwrap();
        assert_eq!(t.rule_count_for("en"), 1, "大小写不同但同义的变体只算一次");
    }

    #[test]
    fn 整词匹配不会命中更长单词的一部分() {
        let t = table();
        let outcome = t.apply("RAIL and AIs and AI", &zh(), &en());
        assert_eq!(outcome.text, "RAIL and AIs and artificial intelligence");
        assert_eq!(outcome.violations.len(), 1);
        assert_eq!(outcome.violations[0].found, "AI");
    }

    #[test]
    fn ascii大小写不敏感() {
        let t = table();
        let outcome = t.apply("an ai model", &zh(), &en());
        assert_eq!(outcome.text, "an artificial intelligence model");
        assert_eq!(outcome.violations[0].found, "AI", "报出术语表里的写法");
    }

    #[test]
    fn 受保护跨度里的内容不被改写() {
        let t = table();
        let input = "see https://example.invalid/AI and `let AI = 1` and {AI} but AI too";
        let outcome = t.apply(input, &zh(), &en());
        assert!(
            outcome.text.contains("https://example.invalid/AI "),
            "URL 不得被改写：{}",
            outcome.text
        );
        assert!(outcome.text.contains("`let AI = 1`"), "{}", outcome.text);
        assert!(outcome.text.contains("{AI}"), "{}", outcome.text);
        assert!(
            outcome.text.ends_with("but artificial intelligence too"),
            "{}",
            outcome.text
        );
        assert_eq!(outcome.violations.len(), 1, "只改裸文本那一处");
    }

    #[test]
    fn 最长优先() {
        let json = r#"{
            "schema_version": 1, "id": "t", "version": 1,
            "terms": [
                { "target": "en", "preferred": "SHORT", "variants": ["AI"] },
                { "target": "en", "preferred": "LONG", "variants": ["AI system"] }
            ]
        }"#;
        let t = TargetTermTable::from_json(json).unwrap();
        let outcome = t.apply("an AI system", &zh(), &en());
        assert_eq!(outcome.text, "an LONG");
        assert_eq!(outcome.violations.len(), 1);
    }

    #[test]
    fn 违规偏移指向输出文本() {
        let t = table();
        let outcome = t.apply("X AI Y LLM", &zh(), &en());
        for violation in &outcome.violations {
            assert!(
                outcome.text[violation.offset..].starts_with(&violation.preferred),
                "偏移 {} 处应是「{}」，实际「{}」",
                violation.offset,
                violation.preferred,
                &outcome.text[violation.offset..]
            );
        }
        // 偏移升序
        let offsets: Vec<usize> = outcome.violations.iter().map(|v| v.offset).collect();
        let mut sorted = offsets.clone();
        sorted.sort_unstable();
        assert_eq!(offsets, sorted);
    }

    #[test]
    fn 通配目标语言对所有语言生效() {
        let t = table();
        let outcome = t.apply("計算機", &en(), &zh());
        assert_eq!(outcome.text, "computer");
        let outcome_en = t.apply("計算機", &zh(), &en());
        assert_eq!(outcome_en.text, "computer", "\"*\" 规则对 en 也生效");
    }

    #[test]
    fn 无违规时原样返回() {
        let t = table();
        let input = "nothing to fix here";
        let outcome = t.apply(input, &zh(), &en());
        assert_eq!(outcome.text, input);
        assert!(outcome.violations.is_empty());
    }

    #[test]
    fn 上限会挡住异常输入() {
        let mut terms = Vec::new();
        for i in 0..(MAX_TERMS + 1) {
            terms.push(format!(
                r#"{{"target":"en","preferred":"p{i}","variants":["v{i}"]}}"#
            ));
        }
        let json = format!(
            r#"{{"schema_version":1,"id":"t","version":1,"terms":[{}]}}"#,
            terms.join(",")
        );
        assert!(matches!(
            TargetTermTable::from_json(&json),
            Err(TermError::Invalid(_))
        ));
    }

    #[test]
    fn 空表被拒收() {
        let json = r#"{"schema_version":1,"id":"t","version":1,"terms":[]}"#;
        assert!(matches!(
            TargetTermTable::from_json(json),
            Err(TermError::Invalid(_))
        ));
    }

    #[test]
    fn 未知字段不会被静默忽略() {
        let json = TABLE.replace("\"version\": 3", "\"version\": 3, \"typo_field\": 1");
        assert!(matches!(
            TargetTermTable::from_json(&json),
            Err(TermError::Json(_))
        ));
    }

    // ---------- 源文触发条件（P2-S / S-03） ----------

    const GATED: &str = r#"{
        "schema_version": 1,
        "id": "terms.gated",
        "version": 1,
        "terms": [
            { "target": "en", "preferred": "world", "variants": ["planet earth"],
              "source_variants": ["世界"] },
            { "target": "en", "preferred": "computer", "variants": ["calc"] }
        ]
    }"#;

    fn gated() -> TargetTermTable {
        TargetTermTable::from_json(GATED).unwrap()
    }

    fn ctx<'a>(source: &'a str, target_text: &'a str, tgt: &'a LanguageTag) -> TermContext<'a> {
        TermContext {
            source_text: source,
            target_text,
            source: &EN,
            target: tgt,
        }
    }

    static EN: std::sync::LazyLock<LanguageTag> =
        std::sync::LazyLock::new(|| LanguageTag::new("en").unwrap());

    #[test]
    fn 源文命中触发条件时才改写() {
        let t = gated();
        let target = en();

        // 源文提到"世界" → 触发
        let hit = t.apply_with_context(&ctx("世界很大", "planet earth is big", &target));
        assert_eq!(hit.text, "world is big");
        assert_eq!(hit.violations.len(), 1);

        // 源文没提 → 不改（这正是 P2 之前缺的那道闸）
        let miss = t.apply_with_context(&ctx("火星很小", "planet earth is big", &target));
        assert_eq!(miss.text, "planet earth is big", "源文没触发就不许改");
        assert!(miss.violations.is_empty());
    }

    #[test]
    fn 没有源文时跳过需要触发的规则() {
        let t = gated();
        let target = en();
        // apply() 没有源文：gated 规则必须跳过，无触发条件的规则照常
        let outcome = t.apply(
            "planet earth and a calc",
            &LanguageTag::new("zh-Hans").unwrap(),
            &target,
        );
        assert_eq!(outcome.text, "planet earth and a computer");
        assert_eq!(outcome.violations.len(), 1, "只有无触发条件的那条生效");
    }

    #[test]
    fn 触发条件的统计可见() {
        let t = gated();
        assert_eq!(t.source_gated_count(), 1);
        assert!(t.summary().contains("需源文触发"));
    }

    #[test]
    fn 空触发条件被拒收() {
        let json = GATED.replace(
            "\"source_variants\": [\"世界\"]",
            "\"source_variants\": [\"\"]",
        );
        assert!(matches!(
            TargetTermTable::from_json(&json),
            Err(TermError::Invalid(_))
        ));
    }

    #[test]
    fn 中文变体按子串匹配() {
        let json = r#"{
            "schema_version": 1, "id": "t", "version": 1,
            "terms": [ { "target": "zh-Hans", "preferred": "人工智能", "variants": ["AI技术", "AI 技术"] } ]
        }"#;
        let t = TargetTermTable::from_json(json).unwrap();
        let outcome = t.apply("这是AI技术报告", &en(), &zh());
        assert_eq!(outcome.text, "这是人工智能报告");
    }
}
