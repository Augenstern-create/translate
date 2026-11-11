//! 检查结果的数据结构。
//!
//! 只有"最终交给用户的字符串是否可接受"这类问题才进这里；
//! 具体检查逻辑在 [`super::integrity`] / [`super::number`] / [`super::heuristic`]。

use translator_types::TranslationMode;

/// 问题级别。顺序即严重程度（`Critical` 最高）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualitySeverity {
    /// 只提示，不影响交付。
    Minor,
    /// 很可能有问题，但不构成"确定性错误"。
    Major,
    /// 确定性问题：必须拒绝返回（或触发一次受限重试）。
    Critical,
}

impl QualitySeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Critical => "critical",
        }
    }
}

/// 问题类型。字符串形式进入报告与 JSONL，**必须稳定**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QualityIssueKind {
    // ---- 硬性（确定性）检查 ----
    /// 输出为空。
    EmptyOutput,
    /// 受保护标记残留在译文里。
    PlaceholderMarker,
    /// 受保护内容没有完整恢复（URL/邮箱/变量/代码被改或被吞）。
    ProtectedSpanMissing,
    /// 严格标识符（订单号/版本号）不一致。
    StrictIdentifierMismatch,
    /// 数值关系错误（如 `between 10 and 20` 译成 `20 至 30`）。
    NumberRelationMismatch,

    // ---- 数字检查 ----
    /// 源文里的数字在译文中找不到。
    NumberMissing,
    /// 译文出现了源文没有的数字。
    NumberAdded,

    // ---- 启发式（只报告） ----
    NegationDropped,
    DuplicateSegment,
    TargetLanguageMismatch,
    Untranslated,
    ConditionalDropped,
    PossibleOmission,
    PossibleHallucination,
}

impl QualityIssueKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmptyOutput => "empty_output",
            Self::PlaceholderMarker => "placeholder_marker",
            Self::ProtectedSpanMissing => "protected_span_missing",
            Self::StrictIdentifierMismatch => "strict_identifier_mismatch",
            Self::NumberRelationMismatch => "number_relation_mismatch",
            Self::NumberMissing => "number_missing",
            Self::NumberAdded => "number_added",
            Self::NegationDropped => "negation_dropped",
            Self::DuplicateSegment => "duplicate_segment",
            Self::TargetLanguageMismatch => "target_language_mismatch",
            Self::Untranslated => "untranslated",
            Self::ConditionalDropped => "conditional_dropped",
            Self::PossibleOmission => "possible_omission",
            Self::PossibleHallucination => "possible_hallucination",
        }
    }

    /// 是否属于"确定性完整性"类别：这类问题**一定**致命。
    pub const fn is_integrity(self) -> bool {
        matches!(
            self,
            Self::EmptyOutput
                | Self::PlaceholderMarker
                | Self::ProtectedSpanMissing
                | Self::StrictIdentifierMismatch
                | Self::NumberRelationMismatch
        )
    }
}

/// 单条问题。
///
/// `detail` **不得**包含整段原文，只允许放少量数字/标记/术语，
/// 避免把用户内容写进日志（设计文档 §4.1）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityIssue {
    pub kind: QualityIssueKind,
    pub severity: QualitySeverity,
    pub detail: String,
}

impl QualityIssue {
    pub fn new(
        kind: QualityIssueKind,
        severity: QualitySeverity,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            severity,
            detail: detail.into(),
        }
    }

    pub fn critical(kind: QualityIssueKind, detail: impl Into<String>) -> Self {
        Self::new(kind, QualitySeverity::Critical, detail)
    }

    pub fn major(kind: QualityIssueKind, detail: impl Into<String>) -> Self {
        Self::new(kind, QualitySeverity::Major, detail)
    }

    pub fn minor(kind: QualityIssueKind, detail: impl Into<String>) -> Self {
        Self::new(kind, QualitySeverity::Minor, detail)
    }
}

/// 一次译后检查的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityReport {
    pub mode: TranslationMode,
    pub issues: Vec<QualityIssue>,
    /// 源文里识别出的数字 token 数。
    pub numbers_in_source: usize,
    /// 其中在译文里找到的个数（按多重集计）。
    pub numbers_preserved: usize,
    /// 严格标识符数量与保留数量（订单号/版本号等）。
    pub strict_identifiers_in_source: usize,
    pub strict_identifiers_preserved: usize,
}

impl Default for QualityReport {
    fn default() -> Self {
        Self {
            mode: TranslationMode::Precise,
            issues: Vec::new(),
            numbers_in_source: 0,
            numbers_preserved: 0,
            strict_identifiers_in_source: 0,
            strict_identifiers_preserved: 0,
        }
    }
}

impl QualityReport {
    /// 是否包含会导致**拒绝返回**的问题。
    ///
    /// 只有确定性完整性问题和 Critical 级数字丢失会触发拒绝；
    /// 启发式问题（否定、比例、语言判断）永远不会单独判死一份译文。
    pub fn has_fatal(&self) -> bool {
        self.issues.iter().any(|issue| {
            issue.kind.is_integrity()
                || (issue.kind == QualityIssueKind::NumberMissing
                    && issue.severity == QualitySeverity::Critical)
        })
    }

    /// 是否存在 Critical 问题（含不拒绝返回但需要告警的）。
    pub fn has_critical(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == QualitySeverity::Critical)
    }

    /// 合并另一个片段/单元的检查结果（段落、文档聚合用）。
    pub fn merge(&mut self, other: QualityReport) {
        self.issues.extend(other.issues);
        self.numbers_in_source += other.numbers_in_source;
        self.numbers_preserved += other.numbers_preserved;
        self.strict_identifiers_in_source += other.strict_identifiers_in_source;
        self.strict_identifiers_preserved += other.strict_identifiers_preserved;
    }

    pub fn count(&self, severity: QualitySeverity) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == severity)
            .count()
    }

    /// 致命问题的一句话原因（用于错误消息）。
    pub fn fatal_summary(&self) -> String {
        let fatal: Vec<String> = self
            .issues
            .iter()
            .filter(|i| i.kind.is_integrity() || i.severity == QualitySeverity::Critical)
            .take(3)
            .map(|i| format!("{}：{}", i.kind.as_str(), i.detail))
            .collect();
        if fatal.is_empty() {
            "无致命问题".to_string()
        } else {
            fatal.join("；")
        }
    }

    /// 一行摘要（不含原文）。
    pub fn summary(&self) -> String {
        if self.issues.is_empty() {
            return format!(
                "模式={} 数字 {}/{} 保留；无完整性问题",
                self.mode, self.numbers_preserved, self.numbers_in_source
            );
        }
        let head: Vec<String> = self
            .issues
            .iter()
            .take(4)
            .map(|issue| {
                format!(
                    "{}[{}] {}",
                    issue.kind.as_str(),
                    issue.severity.as_str(),
                    issue.detail
                )
            })
            .collect();
        let more = self.issues.len().saturating_sub(head.len());
        format!(
            "模式={} 数字 {}/{} 保留；问题 {} 条：{}{}",
            self.mode,
            self.numbers_preserved,
            self.numbers_in_source,
            self.issues.len(),
            head.join("；"),
            if more > 0 {
                format!("；另有 {more} 条")
            } else {
                String::new()
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 完整性问题一定致命() {
        let mut report = QualityReport::default();
        report.issues.push(QualityIssue::major(
            QualityIssueKind::ProtectedSpanMissing,
            "x",
        ));
        assert!(report.has_fatal(), "完整性类别不看严重级别，必须致命");

        let mut heuristic = QualityReport::default();
        heuristic.issues.push(QualityIssue::critical(
            QualityIssueKind::TargetLanguageMismatch,
            "x",
        ));
        assert!(!heuristic.has_fatal(), "启发式问题不得单独拒绝返回");
        assert!(heuristic.has_critical());
    }

    #[test]
    fn 数字缺失只有_critical_才致命() {
        let mut report = QualityReport::default();
        report
            .issues
            .push(QualityIssue::major(QualityIssueKind::NumberMissing, "3"));
        assert!(!report.has_fatal());
        report.issues.push(QualityIssue::critical(
            QualityIssueKind::NumberMissing,
            "#10248",
        ));
        assert!(report.has_fatal());
    }

    #[test]
    fn 合并保留计数() {
        let mut a = QualityReport {
            numbers_in_source: 2,
            numbers_preserved: 1,
            strict_identifiers_in_source: 1,
            strict_identifiers_preserved: 1,
            ..QualityReport::default()
        };
        let b = QualityReport {
            numbers_in_source: 3,
            numbers_preserved: 3,
            ..QualityReport::default()
        };
        a.merge(b);
        assert_eq!(a.numbers_in_source, 5);
        assert_eq!(a.numbers_preserved, 4);
        assert_eq!(a.strict_identifiers_preserved, 1);
    }
}
