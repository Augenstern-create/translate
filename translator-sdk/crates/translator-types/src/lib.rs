//! 核心值类型：语言标签、请求、结果、错误码、能力与插件描述。
//!
//! 对应设计文档 `docs/01_翻译SDK完整开发设计.md` §4「稳定公共 API」。
//!
//! **本 crate 是整棵依赖树的最底层，必须保持零外部依赖。**
//! 任何插件（dictionary / nmt / llm / runtime）都只依赖这里的类型，
//! 而不应各自定义一套。

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ============================================================
// 语言标签
// ============================================================

/// BCP-47 风格的语言标签，例如 `en`、`zh-Hans`、`ja`。
///
/// P0 只做最基本的规范化与校验；完整的 BCP-47 校验留给后续阶段。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LanguageTag(String);

impl LanguageTag {
    /// 构造并规范化语言标签。
    ///
    /// 规范化规则（简化版）：
    /// - 第一段小写（语言）
    /// - 长度为 4 的段首字母大写（文字，如 `Hans`）
    /// - 长度为 2 或 3 的后续段大写（地区，如 `CN`）
    pub fn new(tag: impl AsRef<str>) -> Result<Self, TranslationError> {
        let raw = tag.as_ref().trim();
        if raw.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "语言标签不能为空",
            ));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(TranslationError::new(
                TranslationErrorCode::UnsupportedLanguage,
                format!("非法的语言标签：{raw}"),
            ));
        }

        let mut parts = Vec::new();
        for (idx, part) in raw.split(['-', '_']).filter(|s| !s.is_empty()).enumerate() {
            if idx == 0 {
                parts.push(part.to_ascii_lowercase());
            } else if part.len() == 4 {
                // 文字子标签：首字母大写
                let mut chars = part.chars();
                let head = chars
                    .next()
                    .map(|c| c.to_ascii_uppercase())
                    .unwrap_or_default();
                parts.push(format!("{head}{}", chars.as_str().to_ascii_lowercase()));
            } else {
                parts.push(part.to_ascii_uppercase());
            }
        }

        if parts.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!("无法解析的语言标签：{raw}"),
            ));
        }

        Ok(Self(parts.join("-")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LanguageTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for LanguageTag {
    type Err = TranslationError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

// ============================================================
// 能力枚举
// ============================================================

/// 引擎能力类别。字符串形式与设计文档 §2.1 的能力标识一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// `translation.dictionary`
    Dictionary,
    /// `translation.nmt`
    Nmt,
    /// `translation.remote`
    Remote,
}

impl Capability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Capability::Dictionary => "translation.dictionary",
            Capability::Nmt => "translation.nmt",
            Capability::Remote => "translation.remote",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ============================================================
// 请求与策略
// ============================================================
/// 协作式取消令牌：可在任意线程 `cancel()`，调用方在检查点观察。
///
/// 取消是**协作式**的：SDK 在阶段边界检查令牌；真正阻塞在 native 推理里的调用
/// 需要后端自己实现（Bergamot 后端的做法是终止独占 worker 进程，见 `docs/44`）。
/// 克隆共享同一个标志位。
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 请求取消。幂等。
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// 翻译模式（P11 / docs/57 §4）。
///
/// - [`TranslationMode::Precise`]：默认模式。允许更完整的语言分析、候选比较、
///   术语约束与译后检查，目标是准确、完整、自然。
/// - [`TranslationMode::Fast`]：为后续实时翻译预留。走更短的执行路径，但仍必须
///   保证占位符、数字和格式安全；**不会**因为“快”而放弃完整性检查。
///
/// 模式与具体 NMT Runtime 解耦：同一种模式可以跑在 Dictionary / ORT / Bergamot
/// / Remote 任意引擎上。模式必须进入缓存身份，Precise 与 Fast 的结果不得互相复用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TranslationMode {
    /// 精准模式（默认）。
    #[default]
    Precise,
    /// 快速模式（必须显式选择）。
    Fast,
}

impl TranslationMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Precise => "precise",
            Self::Fast => "fast",
        }
    }

    pub const fn is_precise(self) -> bool {
        matches!(self, Self::Precise)
    }
}

impl fmt::Display for TranslationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 文本结构类型（docs/62 §3）。
///
/// 四级策略只决定“如何组织翻译”，不改变实际引擎：同一个 `Translator` 仍然用
/// Dictionary / NMT / Remote 的既有 `TranslationEngine` 执行。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextKind {
    /// 由结构分析器判断（默认）。
    #[default]
    Auto,
    /// 单词/固定短语：优先词典，多义项，不强行选唯一译法。
    Word,
    /// 句子：NMT + 句级切分/保护/完整性检查。
    Sentence,
    /// 段落：整段 / 逐句 / 句群三种策略。
    Paragraph,
    /// 文章：Markdown/纯文本结构化分段，保留标题、列表、代码块。
    Document,
}

impl TextKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Word => "word",
            Self::Sentence => "sentence",
            Self::Paragraph => "paragraph",
            Self::Document => "document",
        }
    }

    /// 自动判断结果的稳定名字（不含 `Auto`）。
    pub const fn resolved_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Word => "word",
            Self::Sentence => "sentence",
            Self::Paragraph => "paragraph",
            Self::Document => "document",
        }
    }
}

impl fmt::Display for TextKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 实际执行路由（docs/67 §3.4）。
///
/// 与 `Capability` 不同：`Capability` 描述引擎“会什么”，`TranslationRoute`
/// 描述这次请求“实际走了哪条执行层”。`Mixed` 只用于段落/文章的聚合诊断。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TranslationRoute {
    /// 词典精确命中（Word/Phrase）。
    Dictionary,
    /// 规则/模板完整匹配（Sentence）。
    Rule,
    /// 本地 NMT。
    #[default]
    LocalNmt,
    /// 已获授权的远程 API。
    Remote,
    /// 段落/文章聚合后，多个路由混合。
    Mixed,
}

impl TranslationRoute {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dictionary => "dictionary",
            Self::Rule => "rule",
            Self::LocalNmt => "local_nmt",
            Self::Remote => "remote",
            Self::Mixed => "mixed",
        }
    }
}

impl fmt::Display for TranslationRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 模型兜底/规则未覆盖的原因（P17-A）。
///
/// 该分类只描述**这次请求为什么没有交给词典或规则直接完成**，不等于译文质量，
/// 也不自动证明词库缺失。学习链只对前五类做高价值候选挖掘；复杂篇章与用户
/// 显式选择模型不应被当成“缺词”。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FallbackReason {
    /// 完全缺少词条。
    DictionaryMiss,
    /// 缺少固定短语/搭配。
    PhraseMiss,
    /// 有多个义项但上下文不足以可靠消歧。
    SenseAmbiguous,
    /// 规则/模板未覆盖输入。
    RuleNotCovered,
    /// 规则匹配但未满足安全条件（当前版本尚未产生）。
    RuleRejected,
    /// 复杂语法需要模型。
    ComplexSyntax,
    /// 篇章上下文需要模型。
    LongContext,
    /// 用户显式选择模型。
    ExplicitModelRequest,
    /// 引擎故障或策略回退。
    EngineFallback,
}

impl FallbackReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DictionaryMiss => "dictionary_miss",
            Self::PhraseMiss => "phrase_miss",
            Self::SenseAmbiguous => "sense_ambiguous",
            Self::RuleNotCovered => "rule_not_covered",
            Self::RuleRejected => "rule_rejected",
            Self::ComplexSyntax => "complex_syntax",
            Self::LongContext => "long_context",
            Self::ExplicitModelRequest => "explicit_model_request",
            Self::EngineFallback => "engine_fallback",
        }
    }

    /// 是否值得进入词库/规则候选挖掘。
    pub const fn is_knowledge_gap(self) -> bool {
        matches!(
            self,
            Self::DictionaryMiss
                | Self::PhraseMiss
                | Self::SenseAmbiguous
                | Self::RuleNotCovered
                | Self::RuleRejected
        )
    }
}

impl fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 本地模型失败/不可用时，是否允许自动改用远程模型。
///
/// **默认 [`RemoteFallbackPolicy::Disabled`]**：即使用户注册了远程插件，
/// `AutoLocalFirst` 也不会把原文发送出去。远程回退是独立的显式授权，
/// 与“启用了本地学习”没有关系。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RemoteFallbackPolicy {
    /// 永不自动回退远程（默认）。
    #[default]
    Disabled,
    /// 仅当本机没有可用模型时回退远程。
    OnLocalUnavailable,
    /// 本机没有模型，或本地模型执行失败时回退远程。
    OnLocalFailure,
}

impl RemoteFallbackPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::OnLocalUnavailable => "on-local-unavailable",
            Self::OnLocalFailure => "on-local-failure",
        }
    }

    pub const fn allows_unavailable(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub const fn allows_failure(self) -> bool {
        matches!(self, Self::OnLocalFailure)
    }
}

/// 路由与模型调用统计（P16-R / R5）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RouteStats {
    /// 执行单元数（段落/文章会按单元计数，不等于逻辑请求数）。
    pub executions: u64,
    pub dictionary_hits: u64,
    pub rule_matches: u64,
    /// 本地 NMT 真实推理次数（不含缓存命中）。
    pub nmt_calls: u64,
    /// 远程 API 真实调用次数（不含缓存命中）。
    pub remote_calls: u64,
    /// 词典未命中后回退到本地 NMT 的次数。
    pub nmt_fallbacks: u64,
    /// 本地模型不可用/失败后回退到已授权远程 API 的次数。
    pub remote_fallbacks: u64,
    pub cache_hits: u64,
    pub best_candidates: u64,
    pub failures: u64,
}

impl RouteStats {
    pub fn summary(&self) -> String {
        format!(
            "executions={} dictionary={} rule={} nmt_calls={} remote_calls={} nmt_fallbacks={} remote_fallbacks={} cache_hits={} best_candidates={} failures={}",
            self.executions,
            self.dictionary_hits,
            self.rule_matches,
            self.nmt_calls,
            self.remote_calls,
            self.nmt_fallbacks,
            self.remote_fallbacks,
            self.cache_hits,
            self.best_candidates,
            self.failures
        )
    }
}

/// 段落翻译策略（docs/62 §6.2）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ParagraphStrategy {
    /// 按长度/句数自动选择。
    #[default]
    Auto,
    /// A：整段送入模型（上下文完整，但有截断风险）。
    Whole,
    /// B：逐句翻译后按原顺序拼接（长度可控，上下文弱）。
    Sentence,
    /// C：句群翻译（每 2–3 句一组，兼顾上下文与长度）。
    Group,
    /// 同时跑整段与逐句，按完整性/质量选择（P13-Q3 候选比较）。
    Best,
    /// 前一句作为上下文提示，只输出当前句（P14-Q4 实验 B）。
    Context,
}

impl ParagraphStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Whole => "whole",
            Self::Sentence => "sentence",
            Self::Group => "group",
            Self::Best => "best",
            Self::Context => "context",
        }
    }
}

/// 单请求执行选项（向后兼容：`TranslateRequest` 不加字段）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TranslateOptions {
    pub mode: TranslationMode,
    pub text_kind: TextKind,
    pub paragraph_strategy: ParagraphStrategy,
    /// 分段/组织策略（进入缓存身份）。`None` 表示单路径 `single`。
    pub strategy: Option<String>,
    /// 上下文身份（可选）：同一句在不同上下文下需要不同缓存时由宿主提供。
    pub context_id: Option<String>,
    /// 上下文原文（可选）：P16-R 单词/短语消歧用；不用于模型输入。
    pub context_text: Option<String>,
}

impl TranslateOptions {
    pub fn new(mode: TranslationMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn with_text_kind(mut self, kind: TextKind) -> Self {
        self.text_kind = kind;
        self
    }

    pub fn with_paragraph_strategy(mut self, strategy: ParagraphStrategy) -> Self {
        self.paragraph_strategy = strategy;
        self
    }

    pub fn with_strategy(mut self, strategy: impl Into<String>) -> Self {
        self.strategy = Some(strategy.into());
        self
    }

    pub fn with_context_id(mut self, id: impl Into<String>) -> Self {
        self.context_id = Some(id.into());
        self
    }

    pub fn with_context_text(mut self, text: impl Into<String>) -> Self {
        self.context_text = Some(text.into());
        self
    }
}

/// 引擎选择策略。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum TranslationPolicy {
    /// 默认策略：能本地就本地，绝不自动回退到远程。
    #[default]
    AutoLocalFirst,
    /// 只允许词典（精确词条/短语）。
    DictionaryOnly,
    /// 只允许本地 NMT。
    LocalOnly,
    /// 只允许远程 LLM（需宿主显式授权，见设计文档 §11）。
    RemoteOnly,
    /// 指名使用某个引擎（调试与测试用）。
    ExplicitEngine(String),
}

impl TranslationPolicy {
    /// 仅看策略本身是否允许远程；`AutoLocalFirst` 仍可由
    /// [`RemoteFallbackPolicy`] 做**显式**受控回退，但默认禁止。
    pub fn is_remote_allowed(&self) -> bool {
        matches!(self, TranslationPolicy::RemoteOnly)
    }
}

/// 一次翻译请求。
#[derive(Clone, Debug)]
pub struct TranslateRequest {
    pub text: String,
    /// `None` 表示自动识别语言。
    pub source: Option<LanguageTag>,
    pub target: LanguageTag,
    pub policy: TranslationPolicy,
    pub glossary_id: Option<String>,
    pub timeout_ms: Option<u64>,
}

impl TranslateRequest {
    /// 便捷构造：默认 `AutoLocalFirst` 策略、自动识别源语言。
    pub fn new(text: impl Into<String>, target: LanguageTag) -> Self {
        Self {
            text: text.into(),
            source: None,
            target,
            policy: TranslationPolicy::default(),
            glossary_id: None,
            timeout_ms: None,
        }
    }

    pub fn with_source(mut self, source: LanguageTag) -> Self {
        self.source = Some(source);
        self
    }

    pub fn with_policy(mut self, policy: TranslationPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_glossary(mut self, glossary_id: impl Into<String>) -> Self {
        self.glossary_id = Some(glossary_id.into());
        self
    }

    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }
}

/// 一次翻译结果，带来源可追溯信息。
#[derive(Clone, Debug)]
pub struct TranslationResult {
    pub text: String,
    pub source: LanguageTag,
    pub target: LanguageTag,
    /// 实际产出结果的引擎 id。
    pub engine_id: String,
    /// 使用的模型 id（词典引擎一般为 `None`）。
    pub model_id: Option<String>,
    pub elapsed_ms: u64,
    pub warnings: Vec<TranslationWarning>,
}

/// 非致命警告。不得用警告替代错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranslationWarning {
    /// 占位符未能完整还原（设计文档 §6 要求必须暴露）。
    PlaceholderRestoreFailed(String),
    /// 源语言是猜测得到的，而非用户指定。
    LanguageAssumed(String),
    /// 命中缓存。
    CacheHit,
    /// 术语校验未通过。
    TermMismatch(String),
    /// 经英语中转（设计文档 §5.2，会降低质量并增加延迟）。
    PivotRoute(String),
    /// 段落/文章中某一段失败，其余段保留（T7.4 增量恢复）。
    SegmentFailed(String),
}

// ============================================================
// 查词
// ============================================================

#[derive(Clone, Debug)]
pub struct LookupRequest {
    pub term: String,
    pub language: Option<LanguageTag>,
    pub limit: usize,
}

impl LookupRequest {
    pub fn new(term: impl Into<String>) -> Self {
        Self {
            term: term.into(),
            language: None,
            limit: 10,
        }
    }

    pub fn with_language(mut self, language: LanguageTag) -> Self {
        self.language = Some(language);
        self
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// 单条释义。每条释义都带 `source_id`，避免二次打包丢失许可来源
/// （设计文档 §7.1）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Sense {
    pub definition: String,
    pub source_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DictionaryEntry {
    pub headword: String,
    pub normalized: String,
    pub language: LanguageTag,
    pub readings: Vec<String>,
    pub parts_of_speech: Vec<String>,
    pub senses: Vec<Sense>,
    pub source_id: String,
    pub source_license: String,
    pub confidence: f32,
}

/// 语言识别候选。短文本或低置信度时必须返回多个候选，
/// 不要强行判定（设计文档 §7 与资源文档 §4）。
#[derive(Clone, Debug)]
pub struct LanguageGuess {
    pub language: LanguageTag,
    pub confidence: f32,
}

// ============================================================
// 模型与推理
// ============================================================

/// 模型清单。P0 只保留最小字段，完整 schema 见设计文档 §8.2。
#[derive(Clone, Debug)]
pub struct ModelManifest {
    pub id: String,
    pub architecture: String,
    pub source_language: LanguageTag,
    pub target_languages: Vec<LanguageTag>,
    pub estimated_peak_ram_bytes: Option<u64>,
}

/// 兼容性判定结果。**不允许通过字符串猜测兼容**（设计文档 §2.1）。
#[derive(Clone, Debug)]
pub struct CompatibilityReport {
    pub compatible: bool,
    pub reasons: Vec<String>,
}

impl CompatibilityReport {
    pub fn ok() -> Self {
        Self {
            compatible: true,
            reasons: Vec::new(),
        }
    }

    pub fn incompatible(reason: impl Into<String>) -> Self {
        Self {
            compatible: false,
            reasons: vec![reason.into()],
        }
    }
}

/// 资源预算约束（设计文档 §9.3）。
#[derive(Clone, Debug)]
pub struct ResourceLimits {
    /// 峰值内存上限（字节）。
    pub max_peak_ram_bytes: Option<u64>,
    /// 允许并发的推理数，移动端通常为 1。
    pub max_concurrent_inference: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_peak_ram_bytes: None,
            max_concurrent_inference: 1,
        }
    }
}

/// NMT 输入。
#[derive(Clone, Debug)]
pub struct NmtInput {
    pub text: String,
    pub source: LanguageTag,
    pub target: LanguageTag,
}

/// NMT 输出。
#[derive(Clone, Debug)]
pub struct NmtOutput {
    pub text: String,
}

// ============================================================
// 插件描述
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PluginApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl PluginApiVersion {
    pub const CURRENT: Self = Self { major: 0, minor: 1 };

    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// 主版本必须一致，次版本只要求“不低于”最低能力。
    pub fn accepts(&self, required: PluginApiVersion) -> bool {
        self.major == required.major && self.minor >= required.minor
    }
}

impl fmt::Display for PluginApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// 翻译引擎描述。
#[derive(Clone, Debug)]
pub struct EngineDescriptor {
    /// 全局唯一 id，形如 `org.example.nmt.mock`。
    pub id: &'static str,
    pub capabilities: &'static [Capability],
    /// 仅在**可兼容**插件之间用于排序，不能绕过质量/内存要求。
    pub priority: i32,
    pub plugin_api: PluginApiVersion,
    pub license: &'static str,
}

impl EngineDescriptor {
    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// 词典/语言学提供者描述。
#[derive(Clone, Debug)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub plugin_api: PluginApiVersion,
    pub license: &'static str,
}

/// 推理后端描述。
#[derive(Clone, Debug)]
pub struct BackendDescriptor {
    pub id: &'static str,
    pub plugin_api: PluginApiVersion,
    pub license: &'static str,
}

/// 对外暴露的能力清单条目。
#[derive(Clone, Debug)]
pub struct CapabilityInfo {
    pub engine_id: String,
    pub capabilities: Vec<Capability>,
    pub priority: i32,
    pub license: String,
}

/// 路由决策说明。`explain_route()` 返回此结构，
/// **不得把用户原文写进日志**（设计文档 §4.1）。
#[derive(Clone, Debug)]
pub struct RouteDecision {
    pub engine_id: String,
    pub capability: Capability,
    /// 人类可读的选择理由，仅描述规则命中情况，不含原文。
    pub reason: String,
}

// ============================================================
// 错误
// ============================================================

/// 错误码。由设计文档 §4 固定，不得随意增删。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TranslationErrorCode {
    InvalidRequest,
    UnsupportedLanguage,
    ResourceNotInstalled,
    CapabilityUnavailable,
    RuntimeInitFailed,
    OutOfBudget,
    Cancelled,
    Timeout,
    BackendFailed,
    NetworkNotAuthorized,
    ProviderRejected,
    IntegrityFailure,
}

impl TranslationErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "InvalidRequest",
            Self::UnsupportedLanguage => "UnsupportedLanguage",
            Self::ResourceNotInstalled => "ResourceNotInstalled",
            Self::CapabilityUnavailable => "CapabilityUnavailable",
            Self::RuntimeInitFailed => "RuntimeInitFailed",
            Self::OutOfBudget => "OutOfBudget",
            Self::Cancelled => "Cancelled",
            Self::Timeout => "Timeout",
            Self::BackendFailed => "BackendFailed",
            Self::NetworkNotAuthorized => "NetworkNotAuthorized",
            Self::ProviderRejected => "ProviderRejected",
            Self::IntegrityFailure => "IntegrityFailure",
        }
    }
}

impl fmt::Display for TranslationErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 统一错误类型。附带可安装资源建议，便于 UI 直接引导用户。
#[derive(Clone, Debug)]
pub struct TranslationError {
    pub code: TranslationErrorCode,
    pub message: String,
    pub suggested_packages: Vec<String>,
}

impl TranslationError {
    /// 取消错误（码固定为 `Cancelled`，不新增错误码）。
    pub fn cancelled() -> Self {
        Self::new(TranslationErrorCode::Cancelled, "请求已取消")
    }

    pub fn new(code: TranslationErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            suggested_packages: Vec::new(),
        }
    }

    pub fn with_suggestions<S: Into<String>, I: IntoIterator<Item = S>>(
        mut self,
        items: I,
    ) -> Self {
        self.suggested_packages = items.into_iter().map(Into::into).collect();
        self
    }
}

impl fmt::Display for TranslationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)?;
        if !self.suggested_packages.is_empty() {
            write!(f, "（建议安装：{}）", self.suggested_packages.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for TranslationError {}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 语言标签应被规范化() {
        assert_eq!(LanguageTag::new("ZH_hans").unwrap().as_str(), "zh-Hans");
        assert_eq!(LanguageTag::new("EN").unwrap().as_str(), "en");
        assert_eq!(LanguageTag::new("zh-CN").unwrap().as_str(), "zh-CN");
        assert_eq!(LanguageTag::new("  ja  ").unwrap().as_str(), "ja");
    }

    #[test]
    fn 空标签应被拒绝() {
        assert_eq!(
            LanguageTag::new("   ").unwrap_err().code,
            TranslationErrorCode::InvalidRequest
        );
    }

    #[test]
    fn 非法字符应被拒绝() {
        assert_eq!(
            LanguageTag::new("en us!").unwrap_err().code,
            TranslationErrorCode::UnsupportedLanguage
        );
    }

    #[test]
    fn 插件版本需主版本一致() {
        let v = PluginApiVersion::new(0, 2);
        assert!(v.accepts(PluginApiVersion::new(0, 1)));
        assert!(!v.accepts(PluginApiVersion::new(0, 3)));
        assert!(!v.accepts(PluginApiVersion::new(1, 0)));
    }

    #[test]
    fn 默认策略不允许远程() {
        assert!(!TranslationPolicy::LocalOnly.is_remote_allowed());
        assert!(TranslationPolicy::RemoteOnly.is_remote_allowed());
    }
}
