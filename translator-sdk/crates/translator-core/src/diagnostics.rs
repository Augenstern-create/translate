//! 结构化诊断。
//!
//! # 为什么不扩展 `TranslationResult`
//!
//! `TranslationResult` 是"最终结果"该有的东西。诊断信息有三个特点：
//!
//! - **体积大**：一次翻译可能有几十条问题与跨度；
//! - **会被日志/埋点带上**：其中含用户原文片段（URL、模板变量）；
//! - **并非所有人都要**：绝大多数宿主只读 `result.text`。
//!
//! 于是单独走 [`crate::coordinator::Coordinator::translate_with_diagnostics`]，
//! 主路径的返回类型保持不变。
//!
//! # 隐私提醒
//!
//! `protected_spans` 与术语违规里带的是**用户原文片段**。诊断对象只应回给
//! 发起请求的调用方，不要原样写进集中式日志（设计文档 §4.1）。

use translator_types::{
    TextKind, TokenUsage, TranslationMode, TranslationResult, TranslationRoute,
};

use crate::protection::ProtectedSpan;
use crate::quality::QualityReport;
use crate::terms::{TermPolicy, TermViolation};

/// 规则命中记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleTrace {
    pub id: String,
    pub version: u32,
    /// 实际生效的规则 id，按应用顺序。
    pub applied: Vec<String>,
}

/// 术语约束记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermTrace {
    pub id: String,
    pub version: u32,
    pub policy: TermPolicy,
    /// 源文命中的术语条数。
    pub hits: usize,
    /// 满足约束的条数。
    pub satisfied: usize,
    pub violations: Vec<TermViolation>,
}

/// 一次翻译的完整诊断。
#[derive(Clone, Debug)]
pub struct TranslationDiagnostics {
    pub engine_id: String,
    pub route: TranslationRoute,
    pub mode: TranslationMode,
    pub text_kind: TextKind,
    pub cache_hit: bool,
    /// 是否真的调用了模型（模板/缓存命中时为 `false`）。
    pub model_invoked: bool,
    /// 源语言是猜出来的（而非用户指定）。
    pub source_assumed: bool,
    pub rules_version: u32,
    pub resource_fingerprint: String,
    /// 人类可读的路由理由（不含原文）。
    pub route_reason: String,
    /// 实际使用的保护策略（`marker` / `segment`）。
    pub protection_strategy: String,
    /// 本次逻辑请求实际发出的物理 HTTP 调用数（含重试与分段）。
    pub http_calls: u32,
    /// 格式修复的处置结果（`unchanged` / `applied` / `rejected`）。
    ///
    /// `rejected` 表示"修复会破坏译文，已退回原候选"——排查质量问题时很有用。
    pub repair: String,
    /// 受保护跨度（含用户原文片段，注意隐私）。
    pub protected_spans: Vec<ProtectedSpan>,
    pub glossary: Option<TermTrace>,
    pub quality: Option<QualityReport>,
    /// 总尝试次数（1 = 一次成功，>1 表示发生过重试）。
    pub attempts: u32,
    pub elapsed_ms: u64,
    pub prompt_version: String,
    pub model_revision: Option<String>,
    pub usage: Option<TokenUsage>,
    pub rules: Option<RuleTrace>,
}

impl TranslationDiagnostics {
    /// 最小诊断（测试与词典路径用）。
    pub fn minimal(engine_id: impl Into<String>) -> Self {
        Self {
            engine_id: engine_id.into(),
            route: TranslationRoute::Remote,
            mode: TranslationMode::Precise,
            text_kind: TextKind::Auto,
            cache_hit: false,
            model_invoked: false,
            source_assumed: false,
            rules_version: 0,
            resource_fingerprint: String::new(),
            route_reason: String::new(),
            protection_strategy: "none".to_string(),
            http_calls: 0,
            repair: "unchanged".to_string(),
            protected_spans: Vec::new(),
            glossary: None,
            quality: None,
            attempts: 1,
            elapsed_ms: 0,
            prompt_version: String::new(),
            model_revision: None,
            usage: None,
            rules: None,
        }
    }

    /// 一行摘要（不含原文）。
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("引擎={}", self.engine_id),
            format!("路由={}", self.route),
            format!("模式={}", self.mode),
        ];
        if self.cache_hit {
            parts.push("命中缓存".to_string());
        }
        if !self.protected_spans.is_empty() {
            parts.push(format!("保护片段={}", self.protected_spans.len()));
        }
        if let Some(term) = &self.glossary {
            parts.push(format!("术语 {}/{}", term.satisfied, term.hits));
        }
        if let Some(quality) = &self.quality {
            parts.push(quality.summary());
        }
        if self.http_calls > 0 {
            parts.push(format!("HTTP调用={}", self.http_calls));
        }
        if self.attempts > 1 {
            parts.push(format!("尝试={}", self.attempts));
        }
        if self.repair == "rejected" {
            parts.push("格式修复已回退".to_string());
        }
        parts.join("；")
    }
}

/// 译文 + 诊断。
#[derive(Clone, Debug)]
pub struct TranslateOutput {
    pub result: TranslationResult,
    pub diagnostics: TranslationDiagnostics,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 最小诊断可以构造() {
        let d = TranslationDiagnostics::minimal("http.test");
        assert_eq!(d.engine_id, "http.test");
        assert!(!d.model_invoked);
        assert_eq!(d.attempts, 1);
        assert!(!d.summary().is_empty());
    }

    #[test]
    fn 摘要不含原文片段() {
        let mut d = TranslationDiagnostics::minimal("http.test");
        d.protected_spans.push(ProtectedSpan {
            id: 1,
            kind: crate::text::ProtectedKind::Url,
            start: 0,
            end: 5,
            original: "https://secret.example/x".to_string(),
        });
        let summary = d.summary();
        assert!(!summary.contains("secret"), "{summary}");
        assert!(summary.contains("保护片段=1"));
    }
}
