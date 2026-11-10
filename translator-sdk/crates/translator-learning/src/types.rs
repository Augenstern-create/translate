//! 学习链的稳定类型（P20-K）。
//!
//! 这些类型不进入翻译主链：主链只产出 `FallbackReason` 与诊断，学习链在
//! 用户授权后把它们变成候选知识。`Raw` 与 `Verified` 必须始终可区分。

use std::fmt;

use translator_types::{FallbackReason, TranslationRoute};

/// 学习数据隐私级别（docs/68 §11.3）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrivacyLevel {
    /// 默认：不持久化原文或译文，也不写学习记录。
    #[default]
    Off,
    /// 用户明确启用本地学习：保存必要候选与局部上下文。
    Local,
    /// 用户明确授权某些文本用于质量评审/知识库建设。
    AuthorizedCorpus,
    /// 独立授权远程传输（本 crate 不实现网络；仅作为授权边界标记）。
    RemoteTransfer,
}

impl PrivacyLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Local => "local",
            Self::AuthorizedCorpus => "authorized-corpus",
            Self::RemoteTransfer => "remote-transfer",
        }
    }

    pub const fn stores_source_text(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub const fn stores_translation(self) -> bool {
        matches!(self, Self::AuthorizedCorpus | Self::RemoteTransfer)
    }
}

impl fmt::Display for PrivacyLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 学习授权。**默认全关**。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LearningConsent {
    pub level: PrivacyLevel,
    /// 是否允许保存模型候选译文（仍然只是 Raw，不是正确标签）。
    pub store_model_output: bool,
    /// 数据保留天数；`None` 表示不自动清理。
    pub retention_days: Option<u32>,
    /// 是否允许把学习数据发送到远程服务。
    ///
    /// 本 crate 只记录这个授权位，不发起任何网络请求。翻译层也不会因为它
    /// 自动启用远程翻译：远程翻译仍由 `RemoteFallbackPolicy` 单独控制。
    pub allow_remote_upload: bool,
}

impl Default for LearningConsent {
    fn default() -> Self {
        Self {
            level: PrivacyLevel::Off,
            store_model_output: false,
            retention_days: None,
            allow_remote_upload: false,
        }
    }
}

impl LearningConsent {
    pub fn local() -> Self {
        Self {
            level: PrivacyLevel::Local,
            ..Self::default()
        }
    }

    pub fn authorized_corpus() -> Self {
        Self {
            level: PrivacyLevel::AuthorizedCorpus,
            store_model_output: true,
            ..Self::default()
        }
    }

    pub const fn enabled(self) -> bool {
        !matches!(self.level, PrivacyLevel::Off)
    }
}

/// 学习记录/候选的审核状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReviewStatus {
    /// 未验证的模型/运行数据。
    #[default]
    Raw,
    /// 多次出现并聚合，但仍未验证。
    Aggregated,
    /// 有其他独立证据支持，仍需发布门禁。
    Corroborated,
    /// 经过可信来源或人工审核与测试。
    Verified,
    /// 已进入发布的资源版本。
    Released,
    /// 被拒绝，不得发布。
    Rejected,
}

impl ReviewStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Aggregated => "aggregated",
            Self::Corroborated => "corroborated",
            Self::Verified => "verified",
            Self::Released => "released",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "raw" => Self::Raw,
            "aggregated" => Self::Aggregated,
            "corroborated" => Self::Corroborated,
            "verified" => Self::Verified,
            "released" => Self::Released,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }

    pub const fn can_release(self) -> bool {
        matches!(self, Self::Verified)
    }
}

impl fmt::Display for ReviewStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 候选知识类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CandidateKind {
    Word,
    Phrase,
    Pattern,
}

impl CandidateKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Phrase => "phrase",
            Self::Pattern => "pattern",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "word" => Self::Word,
            "phrase" => Self::Phrase,
            "pattern" => Self::Pattern,
            _ => return None,
        })
    }
}

impl fmt::Display for CandidateKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 写入学习记录时由调用方提供的最小事实。
///
/// 不直接复用 `TranslationDiagnostics`：学习 crate 不应依赖 core 的内部诊断，
/// 也避免把受保护片段等敏感细节带进来。
#[derive(Clone, Debug)]
pub struct LearningInput {
    pub source_language: String,
    pub target_language: String,
    pub source_text: String,
    /// 本次模型/规则的译文。仅作为候选，不是真值。
    pub translated_text: Option<String>,
    pub fallback_reason: FallbackReason,
    pub translation_route: TranslationRoute,
    pub model_identity: Option<String>,
    pub knowledge_version: String,
    /// 上下文身份（不是上下文正文）；用于避免把 bank 的不同义项错误合并。
    pub context_key: Option<String>,
}

/// 记录结果。隐私关闭时不会静默写入，而是明确返回 `Skipped`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    /// 新增或聚合了一条记录。
    Recorded { occurrence_count: u64 },
    /// 按隐私/敏感内容/原因策略跳过。
    Skipped(SkipReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    PrivacyOff,
    SensitiveContent,
    EmptySource,
    NotUsefulForLearning,
    StorageDisabled,
}

impl SkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrivacyOff => "privacy_off",
            Self::SensitiveContent => "sensitive_content",
            Self::EmptySource => "empty_source",
            Self::NotUsefulForLearning => "not_useful",
            Self::StorageDisabled => "storage_disabled",
        }
    }
}

/// 聚合后的候选知识。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeCandidate {
    pub id: i64,
    pub kind: CandidateKind,
    pub key: String,
    pub source_text: String,
    /// 候选译法；`None` 表示还没有可信/可展示的候选。
    pub proposed_target: Option<String>,
    pub fallback_reason: FallbackReason,
    pub occurrence_count: u64,
    pub distinct_contexts: u64,
    pub status: ReviewStatus,
    /// 来源说明（例如 `learning:model-fallback`）。
    pub provenance: String,
    pub review_note: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 学习链统计。与翻译 `RouteStats` 分开，不把“模型调用”与“候选条目”混为一谈。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LearningStats {
    pub records: u64,
    pub occurrences: u64,
    pub records_with_source_text: u64,
    pub records_with_translation: u64,
    pub candidates: u64,
    pub verified_candidates: u64,
    pub rejected_candidates: u64,
    pub releases: u64,
    pub by_reason: Vec<(FallbackReason, u64)>,
}

/// 发布版本信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: String,
    pub status: ReleaseStatus,
    pub created_at: i64,
    pub published_at: Option<i64>,
    pub entry_count: u64,
    pub manifest_sha256: String,
    pub directory: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseStatus {
    Staged,
    Published,
    Superseded,
}

impl ReleaseStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Published => "published",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "staged" => Self::Staged,
            "published" => Self::Published,
            "superseded" => Self::Superseded,
            _ => return None,
        })
    }
}
