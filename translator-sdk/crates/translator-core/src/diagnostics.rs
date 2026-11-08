//! 结构化诊断（P2.4）。
//!
//! # 为什么不扩展 `TranslationResult`
//!
//! `TranslationResult` 是 P0 就固定下来的结构，字段全是"最终结果"该有的东西。
//! 诊断信息（用了哪条规则、保护了哪些跨度、术语改了哪里）有三个特点：
//!
//! - **体积大**：一次翻译可能有几十条违规与跨度；
//! - **会被日志/埋点带上**：其中含用户原文片段（URL、模板变量），
//!   塞进主结果会让人在不知情的情况下把原文写进日志；
//! - **并非所有人都要**：绝大多数宿主只读 `result.text`。
//!
//! 于是把它做成**单独入口** [`crate::coordinator::Coordinator::translate_with_diagnostics`]，
//! 主路径 `translate()` 的返回类型一动不动（ADR-008 的落法）。
//!
//! # 隐私提醒
//!
//! `protected_spans` 与 `restore_missing` 里带的是**用户原文片段**
//! （URL、邮箱、`{变量}`）。诊断对象只应回给发起请求的调用方，
//! 不要原样写进集中式日志——设计文档 §4.1 明确禁止把用户原文写进日志。

use translator_types::{Capability, TranslationMode};

use crate::quality::QualityReport;
use crate::terms::{TermPolicy, TermViolation};
use crate::text::ProtectedSpan;

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
    /// 实际替换处数（`Warn` 策略下恒为 0）。
    pub replaced: usize,
    pub violations: Vec<TermViolation>,
}

/// 一次翻译的完整诊断。
#[derive(Clone, Debug)]
pub struct TranslationDiagnostics {
    /// 实际执行翻译的引擎。
    pub engine_id: String,
    pub capability: Capability,
    /// 路由理由（不含原文）。
    pub route_reason: String,
    pub cache_hit: bool,
    /// 源语言是猜出来的而不是调用方指定的。
    pub source_assumed: bool,
    pub rules_version: u32,
    pub resource_fingerprint: String,
    /// 实际使用的翻译模式（Precise / Fast）。
    pub mode: TranslationMode,
    /// 译后完整性检查结果；每次翻译都会跑（Fast 只跑数字/占位符）。
    pub quality: Option<QualityReport>,
    /// 规则包存在时才有。
    pub rules: Option<RuleTrace>,
    /// 进入引擎前被保护的跨度（偏移基于**发送给引擎的文本**）。
    pub protected_spans: Vec<ProtectedSpan>,
    /// 引擎没能保留的受保护片段原文。非空即意味着还原不完整。
    pub restore_missing: Vec<String>,
    /// 术语表存在且策略不是 `Off` 时才有。
    pub term: Option<TermTrace>,
}

impl TranslationDiagnostics {
    /// 人类可读摘要，一行。
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("引擎={} 能力={}", self.engine_id, self.capability.as_str()),
            format!("模式={}", self.mode),
        ];
        if self.cache_hit {
            parts.push("缓存命中".to_string());
        }
        if self.source_assumed {
            parts.push("源语言为猜测".to_string());
        }
        if let Some(rules) = &self.rules {
            if rules.applied.is_empty() {
                parts.push(format!("规则={}@{}（未命中）", rules.id, rules.version));
            } else {
                parts.push(format!(
                    "规则={}@{} 命中 {} 条",
                    rules.id,
                    rules.version,
                    rules.applied.len()
                ));
            }
        }
        if !self.protected_spans.is_empty() {
            parts.push(format!("受保护片段 {} 个", self.protected_spans.len()));
        }
        if !self.restore_missing.is_empty() {
            parts.push(format!("还原失败 {} 处", self.restore_missing.len()));
        }
        if let Some(quality) = &self.quality {
            if !quality.issues.is_empty() {
                parts.push(format!(
                    "译后检查={} 条（critical {}）",
                    quality.issues.len(),
                    quality.count(crate::quality::QualitySeverity::Critical)
                ));
            }
        }
        if let Some(term) = &self.term {
            if term.violations.is_empty() {
                parts.push(format!("术语={}@{}（无违规）", term.id, term.version));
            } else {
                parts.push(format!(
                    "术语={}@{} 违规 {} 处，替换 {} 处",
                    term.id,
                    term.version,
                    term.violations.len(),
                    term.replaced
                ));
            }
        }
        parts.join("；")
    }
}

/// 带诊断的翻译结果。
///
/// 字段公开、可解构：`let TranslateOutput { result, diagnostics } = ...`。
#[derive(Clone, Debug)]
pub struct TranslateOutput {
    pub result: translator_types::TranslationResult,
    pub diagnostics: TranslationDiagnostics,
}

/// 空诊断（缓存与测试用）。
impl TranslationDiagnostics {
    /// 用于缓存写回时替换掉易变字段。
    pub(crate) fn mark_cached(&mut self) {
        self.cache_hit = true;
    }
}
