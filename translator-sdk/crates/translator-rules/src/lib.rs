//! 最小规则引擎：声明式 JSON → 构建期编译 → 只读执行。
//!
//! 对应设计文档 §7.2：
//! > 只做低成本、可测试的规则：NFC/NFKC 可配置规范化、大小写/标点、缩写、词形、
//! > 分句、术语表替换或约束、URL/邮箱/代码/变量保护、基于语言对的策略。
//! > 规则格式采用 YAML/JSON 开发，构建时编译为只读二进制，包含 `schema_version`、
//! > 语言对、优先级、匹配条件和测试样本。
//!
//! 已实现的动作只有两种，但覆盖了 §7.2 里最实用的两类：
//!
//! | 动作 | 对应 §7.2 条目 | 是否会改变语义 |
//! |---|---|---|
//! | `nfkc` | 可配置规范化 | 否（需显式开启，不做无条件 NFKC） |
//! | `replace` | 术语表替换、缩写展开 | 是，因此**必须**带金样本 |
//!
//! 未实现：分句、词形（需要 Lindera 与词典授权）、正则类匹配。
//! 正则未做是**有意的**——§7.3 明确要求防止灾难性回溯，而一个不带超时/长度限制的
//! 正则引擎恰好是这个风险的来源；宁可不做，也不做一个能拖死 UI 线程的版本。
//!
//! # 三层校验
//!
//! 1. **schema 层**：`schema_version` 必须已知，字段拼错直接报错（`deny_unknown_fields`）；
//! 2. **结构层**：id 唯一、语言对合法、`replace` 的 `from` 非空、每条规则必须带样本；
//! 3. **语义层（金样本）**：编译时把每条规则的 `samples` 按优先级跑一遍，
//!    实际输出 ≠ 期望输出就**拒绝加载**。规则改坏了不会被悄悄发布出去。
//!
//! # 二进制形态
//!
//! ```text
//! 偏移  长度  内容
//! 0     6     b"TRULE\0"        magic
//! 6     2     schema_version（u16 LE）
//! 8     4     rules version（u32 LE，冗余，便于快速判断缓存失效）
//! 12    4     payload 长度（u32 LE）
//! 16    N     payload（UTF-8 JSON，即开发格式原文）
//! 16+N  32    payload 的 sha256
//! ```
//!
//! 读取时先校验 magic/长度/哈希，再走一遍完整编译流程（包含金样本）。
//! 换句话说：**二进制形态只是“带校验的传输壳”，不是另一套解析器**，
//! 避免出现“JSON 与二进制行为不一致”这种经典 bug。

use std::fmt;
use std::path::Path;

use serde::Deserialize;
use translator_core::rules::{RuleOutcome, TextRules};
use translator_core::text;
use translator_types::LanguageTag;
use unicode_normalization::UnicodeNormalization;

/// 当前规则格式版本。格式不兼容变更时必须自增。
pub const SCHEMA_VERSION: u16 = 1;

const MAGIC: &[u8] = b"TRULE\0";
const HEADER_LEN: usize = 16;
const HASH_LEN: usize = 32;

// ============================================================
// 错误
// ============================================================

#[derive(Debug)]
pub enum RuleError {
    Io(std::io::Error),
    /// JSON 语法或字段错误。
    Json(serde_json::Error),
    /// `schema_version` 未知：宁可不加载，也不猜字段含义。
    UnsupportedSchema(u16),
    /// 规则包 id 为空。
    EmptyId,
    /// 规则包没有任何规则。
    NoRules,
    /// 规则 id 重复，无法定位是哪一条坏了。
    DuplicateRuleId(String),
    /// 语言对写法非法。
    InvalidLanguagePair {
        rule: String,
        detail: String,
    },
    /// 规则本身不合法（例如 `replace.from` 为空）。
    InvalidRule {
        rule: String,
        detail: String,
    },
    /// 规则没有金样本。§7.2 要求规则必须带测试样本。
    MissingSamples(String),
    /// 金样本未通过。
    SampleFailed {
        rule: String,
        input: String,
        expect: String,
        actual: String,
    },
    /// 二进制头部或校验和不合法。
    Corrupt(String),
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "读取规则文件失败：{e}"),
            Self::Json(e) => write!(f, "规则 JSON 非法：{e}"),
            Self::UnsupportedSchema(v) => write!(
                f,
                "不支持的规则 schema_version={v}（本 SDK 支持 {SCHEMA_VERSION}）"
            ),
            Self::EmptyId => write!(f, "规则包 id 不能为空"),
            Self::NoRules => write!(f, "规则包至少要包含一条规则"),
            Self::DuplicateRuleId(id) => write!(f, "规则 id 重复：{id}"),
            Self::InvalidLanguagePair { rule, detail } => {
                write!(f, "规则 {rule} 的语言对非法：{detail}")
            }
            Self::InvalidRule { rule, detail } => write!(f, "规则 {rule} 不合法：{detail}"),
            Self::MissingSamples(id) => write!(
                f,
                "规则 {id} 缺少金样本（samples）：设计文档 §7.2 要求每条规则都必须带样本"
            ),
            Self::SampleFailed {
                rule,
                input,
                expect,
                actual,
            } => write!(
                f,
                "规则 {rule} 的金样本未通过：输入 {input:?} 期望 {expect:?}，实际 {actual:?}"
            ),
            Self::Corrupt(msg) => write!(f, "规则二进制不合法：{msg}"),
        }
    }
}

impl std::error::Error for RuleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RuleError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for RuleError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

// ============================================================
// 开发格式（JSON）
// ============================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleSpec {
    schema_version: u16,
    id: String,
    version: u32,
    /// 规则包自身的许可证（规则也是可分发资源，必须可追溯）。
    #[serde(default)]
    license: Option<String>,
    /// 金样本未显式写 `source`/`target` 时使用的语言对。
    #[serde(default)]
    sample_pair: Vec<String>,
    rules: Vec<RawRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    /// 同级按出现顺序执行；数值小的先执行。
    #[serde(default)]
    priority: i32,
    /// `[["zh-Hans","en"]]`；`"*"` 表示通配；为空表示所有语言对。
    #[serde(default)]
    language_pairs: Vec<Vec<String>>,
    #[serde(default)]
    when: Option<RawWhen>,
    action: RawAction,
    #[serde(default)]
    samples: Vec<RawSample>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWhen {
    /// 文本包含该子串时命中。
    #[serde(default)]
    contains: Option<String>,
    /// 文本呈单词形态时命中（复用 core 的判定，CJK 阈值更严）。
    #[serde(default)]
    is_single_word: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RawAction {
    /// 术语/缩写替换。`whole_word` 只对空格分隔语言有意义。
    Replace {
        from: String,
        to: String,
        #[serde(default)]
        whole_word: bool,
    },
    /// NFKC 规范化（全角→半角、兼容字符折叠）。
    Nfkc,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSample {
    input: String,
    expect: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

// ============================================================
// 编译产物
// ============================================================

#[derive(Debug, Clone)]
struct Pair {
    /// `None` 表示 `*` 通配。
    from: Option<LanguageTag>,
    to: Option<LanguageTag>,
}

impl Pair {
    fn matches(&self, source: &LanguageTag, target: &LanguageTag) -> bool {
        let from_ok = self.from.as_ref().map_or(true, |f| f == source);
        let to_ok = self.to.as_ref().map_or(true, |t| t == target);
        from_ok && to_ok
    }
}

#[derive(Debug)]
enum Action {
    Replace {
        from: String,
        to: String,
        whole_word: bool,
    },
    Nfkc,
}

#[derive(Debug)]
struct When {
    contains: Option<String>,
    is_single_word: bool,
}

impl When {
    fn matches(&self, text: &str) -> bool {
        if let Some(needle) = &self.contains {
            if !text.contains(needle.as_str()) {
                return false;
            }
        }
        if self.is_single_word && !text::looks_like_single_word(text) {
            return false;
        }
        true
    }
}

#[derive(Debug)]
struct Sample {
    input: String,
    expect: String,
    source: Option<LanguageTag>,
    target: Option<LanguageTag>,
}

#[derive(Debug)]
struct Rule {
    id: String,
    priority: i32,
    pairs: Vec<Pair>,
    when: Option<When>,
    action: Action,
    samples: Vec<Sample>,
}

impl Rule {
    fn matches(&self, text: &str, source: &LanguageTag, target: &LanguageTag) -> bool {
        // 空 language_pairs = 所有语言对
        if !self.pairs.is_empty() && !self.pairs.iter().any(|p| p.matches(source, target)) {
            return false;
        }
        match &self.when {
            Some(when) => when.matches(text),
            None => true,
        }
    }

    fn apply(&self, text: &str) -> String {
        match &self.action {
            Action::Nfkc => text.nfkc().collect(),
            Action::Replace {
                from,
                to,
                whole_word,
            } => replace_term(text, from, to, *whole_word),
        }
    }
}

// ============================================================
// 引擎
// ============================================================

/// 编译好的只读规则包。
#[derive(Debug)]
pub struct RuleEngine {
    id: String,
    version: u32,
    license: Option<String>,
    /// 已按优先级稳定排序。
    rules: Vec<Rule>,
    sample_pair: (LanguageTag, LanguageTag),
    /// 开发格式原文；`to_bytes` / 诊断需要。
    source_json: String,
}

impl RuleEngine {
    /// 从开发格式（JSON 文本）编译。
    pub fn from_json(json: &str) -> Result<Self, RuleError> {
        let spec: RuleSpec = serde_json::from_str(json)?;

        if spec.schema_version != SCHEMA_VERSION {
            return Err(RuleError::UnsupportedSchema(spec.schema_version));
        }
        if spec.id.trim().is_empty() {
            return Err(RuleError::EmptyId);
        }
        if spec.rules.is_empty() {
            return Err(RuleError::NoRules);
        }

        let sample_pair = resolve_sample_pair(&spec.sample_pair)?;

        // 规则 id 必须唯一：否则日志/样本失败时无法定位
        let mut seen = std::collections::HashSet::new();
        for raw in &spec.rules {
            if !seen.insert(raw.id.clone()) {
                return Err(RuleError::DuplicateRuleId(raw.id.clone()));
            }
        }

        let mut rules = Vec::with_capacity(spec.rules.len());
        for raw in &spec.rules {
            rules.push(compile_rule(raw)?);
        }
        // 稳定排序：priority 相同的保持文件内顺序
        rules.sort_by_key(|r| r.priority);

        let engine = Self {
            id: spec.id,
            version: spec.version,
            license: spec.license,
            rules,
            sample_pair,
            source_json: json.to_string(),
        };

        // 金样本：编译即验证，跑不过就不许加载
        engine.verify()?;
        Ok(engine)
    }

    /// 从二进制形态编译。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RuleError> {
        if bytes.len() < HEADER_LEN + HASH_LEN {
            return Err(RuleError::Corrupt(format!(
                "长度不足：{} 字节",
                bytes.len()
            )));
        }
        if &bytes[..MAGIC.len()] != MAGIC {
            return Err(RuleError::Corrupt("magic 不匹配".to_string()));
        }

        let schema = u16::from_le_bytes([bytes[6], bytes[7]]);
        if schema != SCHEMA_VERSION {
            return Err(RuleError::UnsupportedSchema(schema));
        }

        let header_version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let payload_len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;

        let payload_end = HEADER_LEN
            .checked_add(payload_len)
            .ok_or_else(|| RuleError::Corrupt("payload 长度溢出".to_string()))?;
        if bytes.len() != payload_end + HASH_LEN {
            return Err(RuleError::Corrupt(format!(
                "长度不一致：声明 {payload_len} 字节 payload，实际 {} 字节",
                bytes.len()
            )));
        }

        let payload = &bytes[HEADER_LEN..payload_end];
        let expected = &bytes[payload_end..];
        if sha256(payload).as_slice() != expected {
            return Err(RuleError::Corrupt("payload 校验和不匹配".to_string()));
        }

        let json = std::str::from_utf8(payload)
            .map_err(|e| RuleError::Corrupt(format!("payload 不是 UTF-8：{e}")))?;
        let engine = Self::from_json(json)?;

        // 头部版本与 payload 版本必须一致，防止拼装出错包
        if engine.version != header_version {
            return Err(RuleError::Corrupt(format!(
                "头部版本 {header_version} 与 payload 版本 {} 不一致",
                engine.version
            )));
        }
        Ok(engine)
    }

    /// 打包成二进制形态（带 magic 与 sha256）。
    pub fn to_bytes(&self) -> Vec<u8> {
        let payload = self.source_json.as_bytes();
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + HASH_LEN);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(&sha256(payload));
        out
    }

    /// 从文件加载，自动识别 JSON 与二进制形态。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RuleError> {
        let bytes = std::fs::read(path.as_ref())?;
        if bytes.starts_with(MAGIC) {
            Self::from_bytes(&bytes)
        } else {
            let json = std::str::from_utf8(&bytes)
                .map_err(|e| RuleError::Corrupt(format!("规则文件不是 UTF-8：{e}")))?;
            Self::from_json(json)
        }
    }

    /// 重新跑一遍全部金样本。
    ///
    /// 编辑工具在保存前应该调用它：规则改坏了要被拦住，而不是等线上输出变味。
    pub fn verify(&self) -> Result<(), RuleError> {
        for (idx, rule) in self.rules.iter().enumerate() {
            if rule.samples.is_empty() {
                return Err(RuleError::MissingSamples(rule.id.clone()));
            }
            for sample in &rule.samples {
                let (default_source, default_target) = self.sample_pair_for(rule);
                let source = sample.source.as_ref().unwrap_or(&default_source);
                let target = sample.target.as_ref().unwrap_or(&default_target);
                // 期望值 = 执行到「本规则为止」的结果（含本规则）
                let outcome = self.run(&sample.input, source, target, Some(idx));
                if outcome.text != sample.expect {
                    return Err(RuleError::SampleFailed {
                        rule: rule.id.clone(),
                        input: sample.input.clone(),
                        expect: sample.expect.clone(),
                        actual: outcome.text,
                    });
                }
            }
        }
        Ok(())
    }

    /// 规则包 id（`TextRules::id` 的固有方法版本）。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 规则包版本（`TextRules::version` 的固有方法版本）。
    pub fn version(&self) -> u32 {
        self.version
    }

    /// 规则数量（`TextRules::rule_count` 的固有方法版本）。
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// 规则包许可证（SPDX），未声明时为 `None`。
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// 开发格式原文（诊断/导出用）。
    pub fn source_json(&self) -> &str {
        &self.source_json
    }

    /// 规则包里的规则 id（按执行顺序）。
    pub fn rule_ids(&self) -> Vec<&str> {
        self.rules.iter().map(|r| r.id.as_str()).collect()
    }

    /// 应用全部规则（`TextRules::apply` 的固有方法版本，免去导入 trait）。
    pub fn apply(&self, text: &str, source: &LanguageTag, target: &LanguageTag) -> RuleOutcome {
        self.run(text, source, target, None)
    }

    /// 执行规则；`stop_after` 为包含式上限（金样本验证用）。
    fn run(
        &self,
        input: &str,
        source: &LanguageTag,
        target: &LanguageTag,
        stop_after: Option<usize>,
    ) -> RuleOutcome {
        let limit = match stop_after {
            Some(idx) => (idx + 1).min(self.rules.len()),
            None => self.rules.len(),
        };

        let mut current = input.to_string();
        let mut applied = Vec::new();
        for rule in self.rules.iter().take(limit) {
            if !rule.matches(&current, source, target) {
                continue;
            }
            let next = rule.apply(&current);
            if next != current {
                applied.push(rule.id.clone());
                current = next;
            }
        }

        RuleOutcome {
            text: current,
            applied,
            warnings: Vec::new(),
        }
    }

    fn sample_pair_for(&self, rule: &Rule) -> (LanguageTag, LanguageTag) {
        // 规则若声明了具体语言对，样本默认用它；否则用包级 sample_pair。
        for pair in &rule.pairs {
            if let (Some(from), Some(to)) = (&pair.from, &pair.to) {
                return (from.clone(), to.clone());
            }
        }
        self.sample_pair.clone()
    }
}

impl TextRules for RuleEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> u32 {
        self.version
    }

    fn rule_count(&self) -> usize {
        self.rules.len()
    }

    fn apply(&self, text: &str, source: &LanguageTag, target: &LanguageTag) -> RuleOutcome {
        self.run(text, source, target, None)
    }
}

// ============================================================
// 辅助
// ============================================================

fn resolve_sample_pair(pair: &[String]) -> Result<(LanguageTag, LanguageTag), RuleError> {
    if pair.is_empty() {
        // 默认按项目首个语言对（中→英）验证样本
        let zh = LanguageTag::new("zh-Hans").map_err(|e| RuleError::InvalidLanguagePair {
            rule: "<sample_pair>".to_string(),
            detail: e.to_string(),
        })?;
        let en = LanguageTag::new("en").map_err(|e| RuleError::InvalidLanguagePair {
            rule: "<sample_pair>".to_string(),
            detail: e.to_string(),
        })?;
        return Ok((zh, en));
    }
    if pair.len() != 2 {
        return Err(RuleError::InvalidLanguagePair {
            rule: "<sample_pair>".to_string(),
            detail: format!("需要 2 个元素，实际 {}", pair.len()),
        });
    }
    Ok((parse_tag(&pair[0])?, parse_tag(&pair[1])?))
}

fn parse_tag(raw: &str) -> Result<LanguageTag, RuleError> {
    LanguageTag::new(raw).map_err(|e| RuleError::InvalidLanguagePair {
        rule: "<sample_pair>".to_string(),
        detail: e.to_string(),
    })
}

fn compile_rule(raw: &RawRule) -> Result<Rule, RuleError> {
    let mut pairs = Vec::with_capacity(raw.language_pairs.len());
    for pair in &raw.language_pairs {
        if pair.len() != 2 {
            return Err(RuleError::InvalidLanguagePair {
                rule: raw.id.clone(),
                detail: format!("需要 [source, target]，实际 {} 个元素", pair.len()),
            });
        }
        let parse = |tag: &str| -> Result<Option<LanguageTag>, RuleError> {
            if tag == "*" {
                return Ok(None);
            }
            LanguageTag::new(tag)
                .map(Some)
                .map_err(|e| RuleError::InvalidLanguagePair {
                    rule: raw.id.clone(),
                    detail: e.to_string(),
                })
        };
        pairs.push(Pair {
            from: parse(&pair[0])?,
            to: parse(&pair[1])?,
        });
    }

    let action = match &raw.action {
        RawAction::Nfkc => Action::Nfkc,
        RawAction::Replace {
            from,
            to,
            whole_word,
        } => {
            if from.is_empty() {
                return Err(RuleError::InvalidRule {
                    rule: raw.id.clone(),
                    detail: "replace.from 不能为空".to_string(),
                });
            }
            if from == to {
                // 空操作规则通常是写错了，直接拒绝，避免"看起来生效了"
                return Err(RuleError::InvalidRule {
                    rule: raw.id.clone(),
                    detail: "replace.from 与 replace.to 相同（空操作）".to_string(),
                });
            }
            Action::Replace {
                from: from.clone(),
                to: to.clone(),
                whole_word: *whole_word,
            }
        }
    };

    let samples = raw
        .samples
        .iter()
        .map(|s| {
            Ok(Sample {
                input: s.input.clone(),
                expect: s.expect.clone(),
                source: match &s.source {
                    Some(raw) => Some(parse_tag(raw)?),
                    None => None,
                },
                target: match &s.target {
                    Some(raw) => Some(parse_tag(raw)?),
                    None => None,
                },
            })
        })
        .collect::<Result<Vec<_>, RuleError>>()?;

    Ok(Rule {
        id: raw.id.clone(),
        priority: raw.priority,
        pairs,
        when: raw.when.as_ref().map(|w| When {
            contains: w.contains.clone(),
            is_single_word: w.is_single_word,
        }),
        action,
        samples,
    })
}

/// 术语替换。`whole_word` 时要求两侧不是单词字符（只对空格分隔语言有意义）。
fn replace_term(text: &str, from: &str, to: &str, whole_word: bool) -> String {
    if !whole_word {
        return text.replace(from, to);
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(pos) = text[cursor..].find(from) {
        let start = cursor + pos;
        let end = start + from.len();
        let before_ok = text[..start]
            .chars()
            .next_back()
            .map_or(true, |c| !is_word_char(c));
        let after_ok = text[end..]
            .chars()
            .next()
            .map_or(true, |c| !is_word_char(c));

        out.push_str(&text[cursor..start]);
        if before_ok && after_ok {
            out.push_str(to);
        } else {
            out.push_str(from);
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn sha256(bytes: &[u8]) -> [u8; HASH_LEN] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; HASH_LEN];
    out.copy_from_slice(&digest);
    out
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn zh() -> LanguageTag {
        LanguageTag::new("zh-Hans").unwrap()
    }

    fn en() -> LanguageTag {
        LanguageTag::new("en").unwrap()
    }

    const BASE: &str = r#"{
        "schema_version": 1,
        "id": "rules.test.minimal",
        "version": 7,
        "sample_pair": ["zh-Hans", "en"],
        "rules": [
            {
                "id": "norm.nfkc",
                "priority": 10,
                "action": { "kind": "nfkc" },
                "samples": [{ "input": "ＡＢＣ", "expect": "ABC" }]
            },
            {
                "id": "term.robot",
                "priority": 50,
                "language_pairs": [["zh-Hans", "en"]],
                "action": { "kind": "replace", "from": "机器人", "to": "robot" },
                "samples": [{ "input": "机器人", "expect": "robot" }]
            },
            {
                "id": "term.llm",
                "priority": 60,
                "language_pairs": [["en", "zh-Hans"]],
                "action": {
                    "kind": "replace",
                    "from": "LLM",
                    "to": "large language model",
                    "whole_word": true
                },
                "samples": [
                    {
                        "input": "the LLM works",
                        "expect": "the large language model works",
                        "source": "en",
                        "target": "zh-Hans"
                    }
                ]
            }
        ]
    }"#;

    #[test]
    fn 编译成功并暴露元信息() {
        let engine = RuleEngine::from_json(BASE).expect("应能编译");
        assert_eq!(engine.id(), "rules.test.minimal");
        assert_eq!(engine.version(), 7);
        assert_eq!(engine.rule_count(), 3);
        // 已按 priority 升序稳定排序
        assert_eq!(
            engine.rule_ids(),
            ["norm.nfkc", "term.robot", "term.llm"],
            "规则应按优先级执行"
        );
    }

    #[test]
    fn nfkc把全角转半角() {
        let engine = RuleEngine::from_json(BASE).unwrap();
        let outcome = engine.apply("ｃａｔ ＡＢＣ", &en(), &zh());
        assert_eq!(outcome.text, "cat ABC");
        assert_eq!(outcome.applied, ["norm.nfkc"]);
    }

    #[test]
    fn 语言对不匹配的规则不生效() {
        let engine = RuleEngine::from_json(BASE).unwrap();
        // term.robot 只在 zh-Hans→en 生效
        let outcome = engine.apply("机器人", &en(), &zh());
        assert_eq!(outcome.text, "机器人");
        assert!(outcome.applied.is_empty());
    }

    #[test]
    fn whole_word只匹配整词() {
        let engine = RuleEngine::from_json(BASE).unwrap();
        let hit = engine.apply("the LLM works", &en(), &zh());
        assert_eq!(hit.text, "the large language model works");

        // "LLMs" 里的 LLM 不该被替换
        let miss = engine.apply("LLMs are many", &en(), &zh());
        assert_eq!(miss.text, "LLMs are many");
        assert!(miss.applied.is_empty());
    }

    #[test]
    fn 优先级决定执行顺序() {
        // NFKC(10) 先把全角转成 ABC，后面的 replace(50) 才能命中
        let json = r#"{
            "schema_version": 1,
            "id": "rules.test.order",
            "version": 1,
            "rules": [
                { "id": "norm.nfkc", "priority": 10, "action": { "kind": "nfkc" },
                  "samples": [{ "input": "ＡＢＣ", "expect": "ABC" }] },
                { "id": "term.abc", "priority": 50,
                  "action": { "kind": "replace", "from": "ABC", "to": "xyz" },
                  "samples": [{ "input": "ＡＢＣ", "expect": "xyz" }] }
            ]
        }"#;
        let engine = RuleEngine::from_json(json).expect("顺序正确时应编译通过");
        let outcome = engine.apply("ＡＢＣ", &zh(), &en());
        assert_eq!(outcome.text, "xyz");
        assert_eq!(outcome.applied, ["norm.nfkc", "term.abc"]);
    }

    #[test]
    fn 缺样本被拒绝() {
        let json = r#"{
            "schema_version": 1,
            "id": "rules.test.nosample",
            "version": 1,
            "rules": [
                { "id": "term.x", "action": { "kind": "replace", "from": "a", "to": "b" } }
            ]
        }"#;
        let err = RuleEngine::from_json(json).unwrap_err();
        assert!(
            matches!(err, RuleError::MissingSamples(ref id) if id == "term.x"),
            "实际：{err}"
        );
    }

    #[test]
    fn 样本不通过被拒绝() {
        let json = r#"{
            "schema_version": 1,
            "id": "rules.test.badsample",
            "version": 1,
            "rules": [
                { "id": "term.x",
                  "action": { "kind": "replace", "from": "a", "to": "b" },
                  "samples": [{ "input": "a", "expect": "c" }] }
            ]
        }"#;
        let err = RuleEngine::from_json(json).unwrap_err();
        match err {
            RuleError::SampleFailed {
                ref rule,
                ref actual,
                ..
            } => {
                assert_eq!(rule, "term.x");
                assert_eq!(actual, "b", "错误信息里要给出实际输出");
            }
            other => panic!("应是 SampleFailed，实际：{other}"),
        }
    }

    #[test]
    fn 未知schema被拒绝() {
        let json = r#"{ "schema_version": 99, "id": "x", "version": 1, "rules": [] }"#;
        assert!(matches!(
            RuleEngine::from_json(json),
            Err(RuleError::UnsupportedSchema(99))
        ));
    }

    #[test]
    fn 重复规则id被拒绝() {
        let json = r#"{
            "schema_version": 1,
            "id": "rules.test.dup",
            "version": 1,
            "rules": [
                { "id": "same", "action": { "kind": "nfkc" },
                  "samples": [{ "input": "Ａ", "expect": "A" }] },
                { "id": "same", "action": { "kind": "nfkc" },
                  "samples": [{ "input": "Ａ", "expect": "A" }] }
            ]
        }"#;
        assert!(matches!(
            RuleEngine::from_json(json),
            Err(RuleError::DuplicateRuleId(ref id)) if id == "same"
        ));
    }

    #[test]
    fn 空操作规则被拒绝() {
        let json = r#"{
            "schema_version": 1,
            "id": "rules.test.noop",
            "version": 1,
            "rules": [
                { "id": "noop", "action": { "kind": "replace", "from": "a", "to": "a" },
                  "samples": [{ "input": "a", "expect": "a" }] }
            ]
        }"#;
        assert!(matches!(
            RuleEngine::from_json(json),
            Err(RuleError::InvalidRule { .. })
        ));
    }

    #[test]
    fn 二进制往返保持行为一致() {
        let engine = RuleEngine::from_json(BASE).unwrap();
        let bytes = engine.to_bytes();
        assert!(bytes.starts_with(b"TRULE\0"), "二进制应带 magic");

        let restored = RuleEngine::from_bytes(&bytes).expect("应能读回");
        assert_eq!(restored.id(), engine.id());
        assert_eq!(restored.version(), engine.version());
        assert_eq!(restored.rule_ids(), engine.rule_ids());

        let a = engine.apply("机器人", &zh(), &en());
        let b = restored.apply("机器人", &zh(), &en());
        assert_eq!(a, b, "两种形态的行为必须一致");
    }

    #[test]
    fn 二进制被篡改会被检出() {
        let mut bytes = RuleEngine::from_json(BASE).unwrap().to_bytes();
        // 改 payload 中间一个字节
        let mid = HEADER_LEN + 10;
        bytes[mid] ^= 0x01;
        assert!(matches!(
            RuleEngine::from_bytes(&bytes),
            Err(RuleError::Corrupt(_))
        ));
    }

    #[test]
    fn 从文件加载自动识别两种格式() {
        let dir = tempfile::tempdir().unwrap();
        let engine = RuleEngine::from_json(BASE).unwrap();

        let json_path = dir.path().join("rules.json");
        std::fs::write(&json_path, BASE).unwrap();
        let from_json = RuleEngine::load(&json_path).expect("JSON 文件应可加载");

        let bin_path = dir.path().join("rules.bin");
        std::fs::write(&bin_path, engine.to_bytes()).unwrap();
        let from_bin = RuleEngine::load(&bin_path).expect("二进制文件应可加载");

        assert_eq!(from_json.rule_ids(), from_bin.rule_ids());
        assert_eq!(from_json.version(), 7);
    }

    #[test]
    fn 条件命中才修改文本() {
        let json = r#"{
            "schema_version": 1,
            "id": "rules.test.when",
            "version": 1,
            "rules": [
                { "id": "term.single",
                  "when": { "is_single_word": true },
                  "action": { "kind": "replace", "from": "cat", "to": "feline" },
                  "samples": [{ "input": "cat", "expect": "feline" }] },
                { "id": "term.ctx",
                  "when": { "contains": "urgent" },
                  "action": { "kind": "replace", "from": "asap", "to": "as soon as possible" },
                  "samples": [{ "input": "urgent asap", "expect": "urgent as soon as possible" }] }
            ]
        }"#;
        let engine = RuleEngine::from_json(json).expect("条件规则应编译通过");

        // is_single_word：整句里不命中
        let sentence = engine.apply("the cat sat on the mat", &en(), &zh());
        assert_eq!(sentence.text, "the cat sat on the mat");

        // contains：命中
        let hit = engine.apply("urgent asap", &en(), &zh());
        assert_eq!(hit.text, "urgent as soon as possible");
    }

    #[test]
    fn 语言对非法被拒绝() {
        let json = r#"{
            "schema_version": 1,
            "id": "rules.test.badpair",
            "version": 1,
            "rules": [
                { "id": "term.x", "language_pairs": [["zh-Hans"]],
                  "action": { "kind": "nfkc" },
                  "samples": [{ "input": "Ａ", "expect": "A" }] }
            ]
        }"#;
        assert!(matches!(
            RuleEngine::from_json(json),
            Err(RuleError::InvalidLanguagePair { .. })
        ));
    }

    #[test]
    fn 通配语言对在所有方向上生效() {
        let json = r#"{
            "schema_version": 1,
            "id": "rules.test.wildcard",
            "version": 1,
            "rules": [
                { "id": "norm.nfkc", "language_pairs": [["*", "*"]],
                  "action": { "kind": "nfkc" },
                  "samples": [{ "input": "Ａ", "expect": "A" }] }
            ]
        }"#;
        let engine = RuleEngine::from_json(json).unwrap();
        assert_eq!(engine.apply("Ａ", &en(), &zh()).text, "A");
        assert_eq!(engine.apply("Ａ", &zh(), &en()).text, "A");
    }

    #[test]
    fn 不得panic在畸形输入上() {
        // 规则作用在任意不可信文本上：空串、纯空白、超长重复串都不许崩
        let engine = RuleEngine::from_json(BASE).unwrap();
        for input in ["", " ", "\u{0}\u{1}", &"机器人".repeat(1000)] {
            let _ = engine.apply(input, &zh(), &en());
        }
    }
}
