//! 翻译流水线的核心实现。
//!
//! # 九个模块，一条主链
//!
//! ```text
//! text       扫描受保护跨度（URL/邮箱/变量/代码/数字/路径/标签/标识符）
//! protection 统一保护计划（标记优先 + 分段回退 + 严格还原）
//! terms      术语表（生成前约束 + 生成后校验）
//! templates  固定 UI 文案整句匹配（作用域 + 精确/归一化）
//! cache      缓存身份（模型/提示词/术语表/保护版本全部进键）
//! routing    轻量后端路由（本地优先，远程需显式授权）
//! quality    译后质量门禁（完整性 + 数值语义 + 启发式诊断）
//! language   语言识别与形态分析的最小实现
//! rules      确定性文本变换接口（由 translator-rules 实现）
//! coordinator 按固定顺序调用以上模块，并守住完整性门禁
//! ```
//!
//! # 已经移除的东西
//!
//! `semantic` / `composer` / `pipeline` / `context_sense` / `planner` / `evidence`
//! / `plan` / `plan_cache` / `budget` 这些手写句法翻译与模型执行计划模块已归档到
//! `archive/pre-online-refactor-20261111` 分支。产品主线只有**一条**整句翻译路径：
//! 保护文本 → 调后端 → 还原 → 校验。

pub mod cache;
pub mod coordinator;
pub mod diagnostics;
pub mod language;
pub mod protection;
pub mod quality;
pub mod routing;
pub mod rules;
pub mod templates;
pub mod terms;
pub mod text;

pub use cache::{CacheEntry, CacheIdentity, CacheKey, CacheStats, QueryCache};
pub use coordinator::{Coordinator, CoordinatorConfig};
pub use diagnostics::{RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics};
pub use language::{guess, HeuristicIdentifier, LanguageIdentifier, MorphologyAnalyzer};
pub use protection::{
    Piece, ProtectedSpan, ProtectionError, ProtectionPlan, ProtectionPolicy, ProtectionStrategy,
    StrategyPreference, PROTECTION_VERSION,
};
pub use quality::{QualityIssue, QualityIssueKind, QualityReport, QualitySeverity};
pub use routing::{BackendRouter, RouteOutcome, RoutingConfig};
pub use rules::{NoRules, RuleOutcome, RulePreview, TextRules};
pub use templates::{MatchMode, SentenceTemplates, TemplateScope, TemplateTable};
pub use terms::{
    Glossary, GlossaryError, MatchType, NoTermGuard, TargetTermGuard, TermContext, TermEntry,
    TermHit, TermOrigin, TermOutcome, TermPolicy, TermVerification, TermViolation,
};
pub use text::{ProtectedKind, SpanPolicy};
