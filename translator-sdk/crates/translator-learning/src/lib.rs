//! 持续学习链（P17-A / P20-K 基础）。
//!
//! 这不是“自动训练”或“自动改词库”。它只做四件事：
//!
//! 1. 在用户明确授权后，记录词典/规则无法覆盖的事实与原因；
//! 2. 去重、聚合，并提取单词、短语和保守的句式候选；
//! 3. 用显式审核状态区分 `Raw` / `Verified`，模型输出不得直接发布；
//! 4. 把已验证候选写成可追踪、可回滚的 staging 版本，供 ResourceStore 发布。
//!
//! **默认 `PrivacyLevel::Off`：不写任何学习记录。**

pub mod store;
pub mod types;

pub use store::{LearningError, LearningStore};
pub use types::{
    CandidateKind, KnowledgeCandidate, LearningConsent, LearningInput, LearningStats, PrivacyLevel,
    RecordOutcome, ReleaseInfo, ReleaseStatus, ReviewStatus, SkipReason,
};

/// 数据库 schema 版本；不兼容变更必须提升它。
pub const SCHEMA_VERSION: i64 = 1;
