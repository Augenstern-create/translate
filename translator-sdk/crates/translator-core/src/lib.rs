//! 协调器、路由、缓存与文本预处理。
//!
//! 对应设计文档 `docs/01_翻译SDK完整开发设计.md` §6「翻译路由及处理流水线」。
//!
//! 流水线（P0 已实现的部分标 ✅）：
//!
//! ```text
//! Validate request            ✅
//!   → Normalize               ✅
//!   → Language ID (optional)  ✅ 启发式（真正的 LID 属 P1）
//!   → Apply rules             ✅ 声明式规则包（可选，见 `rules` 模块）
//!   → Protect placeholders    ✅ 基础版
//!   → Cache lookup            ✅
//!   → Select eligible engine  ✅ 按策略 + 能力 + 语言对
//!   → 执行翻译                 ✅
//!   → Restore placeholders    ✅ 含失败检测
//!   → Term constraint (output)✅ P2.4，不碰受保护跨度
//!   → 结果 + 来源 + 警告        ✅
//!   → 结构化诊断（可选入口）    ✅ P2.4 `translate_with_diagnostics`
//! ```

pub mod budget;
pub mod cache;
pub mod composer;
pub mod context_sense;
pub mod coordinator;
pub mod diagnostics;
pub mod evidence;
pub mod plan;
pub mod plan_cache;
pub mod planner;
pub mod protection;
pub mod quality;
pub mod routing;
pub mod rules;
pub mod semantic;
pub mod templates;
pub mod terms;
pub mod text;

pub use budget::{BudgetError, BudgetReservation, MemoryBudget, MemoryReport};
pub use cache::{CacheEntry, CacheKey, QueryCache};
pub use composer::{CompositionAnalysis, CompositionEngine, CompositionOutcome, CompositionToken};
pub use context_sense::{compose_senses, select_by_context};
pub use coordinator::{Coordinator, CoordinatorConfig};
pub use diagnostics::{
    RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics, UnitExecutionTrace,
};
pub use evidence::{
    ContextRef, LanguageEvidence, PatternKind, PatternMatch, SenseCandidate, TextSpan,
    TranslationConstraint, UnitKind,
};
pub use planner::{
    analyze, classify_fallback, plan as plan_text, plan_request, split_sentences, PlanShape,
    RequestPlan, TextAnalysis, TranslationPlan, TranslationPlanner, TranslationUnit,
};
pub use quality::{QualityIssue, QualityIssueKind, QualityReport, QualitySeverity};
pub use routing::RouteOutcome;
pub use rules::{NoRules, RuleOutcome, RulePreview, TextRules};
pub use semantic::{
    CandidateSelector, CandidateValidation, ChineseRealizer, Determiner, EntityRef, Lexeme,
    MeaningFrame, MeaningProgram, Modifier, Polarity, ProgramRelation, QuestionType,
    RealizationCandidate, Rejection, RejectionReason, SemanticAnalysis, SemanticCompositionEngine,
    SemanticOutcome, TenseAspect,
};
pub use templates::{SentenceTemplates, TemplateTable};
pub use terms::{
    NoTermGuard, TargetTermGuard, TermContext, TermOutcome, TermPolicy, TermViolation,
};
pub use translator_analysis::{LanguageIdentifier, MorphologyAnalyzer};
