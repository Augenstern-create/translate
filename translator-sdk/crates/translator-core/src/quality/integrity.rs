//! 确定性完整性检查：**失败就必须拒绝返回**。
//!
//! 这里只放"不需要理解语义就能判定"的规则：
//!
//! - 输出为空 / 只有空白；
//! - 受保护标记残留（`⟦P1⟧`、`#P1#`）；
//! - 受保护片段没有出现在输出里（被模型改写或吞掉）。
//!
//! 数值语义（百分比、金额、区间）在 [`super::number`]；它们里面也有一部分
//! 是确定性的（编号、日期、百分比），由 [`super::number::NumberCheck::has_fatal`]
//! 汇总。

use translator_types::LanguageTag;

use super::report::{QualityIssue, QualityIssueKind, QualityReport};
use crate::protection::ProtectionPlan;

/// 空输出与标记残留检查。
pub fn check(source: &str, output: &str, report: &mut QualityReport) {
    let _ = source;

    if output.trim().is_empty() {
        report.issues.push(QualityIssue::critical(
            QualityIssueKind::EmptyOutput,
            "引擎返回空译文",
        ));
        return;
    }

    if let Some(marker) = find_marker_residue(output) {
        report.issues.push(QualityIssue::critical(
            QualityIssueKind::PlaceholderMarker,
            format!("译文里残留受保护标记：{marker}"),
        ));
    }
}

/// 受保护片段是否全部原样出现在译文里。
///
/// 这是[`ProtectionPlan::verify`] 之外的**第二道**检查：即使调用方忘了调
/// `restore`，也不能让"丢了 URL"的译文通过。
pub fn check_protected(plan: &ProtectionPlan, restored: &str, report: &mut QualityReport) {
    for span in plan.spans() {
        if !restored.contains(&span.original) {
            report.issues.push(QualityIssue::critical(
                QualityIssueKind::ProtectedSpanMissing,
                format!(
                    "受保护内容（{}）没有出现在译文里：{}",
                    span.kind_str(),
                    excerpt(&span.original)
                ),
            ));
        }
    }
}

/// 目标语言是否是中文（用于决定"必须包含汉字"这类硬规则）。
pub fn target_is_chinese(target: &LanguageTag) -> bool {
    target.as_str().starts_with("zh")
}

/// 找残留标记。返回可读片段（不含整段原文）。
pub fn find_marker_residue(output: &str) -> Option<String> {
    // 只认我们自己的标记形态，避免把用户文本里的 `{P1}` 误判为残留
    for open in ["\u{27E6}P", "\u{3010}P", "\u{00AB}P"] {
        if let Some(idx) = output.find(open) {
            let excerpt: String = output[idx..].chars().take(10).collect();
            return Some(excerpt);
        }
    }
    if let Some(idx) = output.find("#P") {
        let rest = &output[idx + 2..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && rest[digits.len()..].starts_with('#') {
            let excerpt: String = output[idx..].chars().take(10).collect();
            return Some(excerpt);
        }
    }
    None
}

fn excerpt(text: &str) -> String {
    let mut out: String = text.chars().take(24).collect();
    if text.chars().count() > 24 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protection::{ProtectionPlan, ProtectionPolicy};

    fn plan(input: &str) -> ProtectionPlan {
        ProtectionPlan::prepare(input, &ProtectionPolicy::default()).unwrap()
    }

    #[test]
    fn 空输出致命() {
        let mut report = QualityReport::default();
        check("Hello", "   ", &mut report);
        assert!(report.has_fatal());
        assert_eq!(report.issues[0].kind, QualityIssueKind::EmptyOutput);
    }

    #[test]
    fn 残留标记致命() {
        for output in [
            "请访问 \u{27E6}P1\u{27E7} 并输入",
            "请访问 \u{3010}P2\u{3011}",
            "请访问 #P3# 并输入",
        ] {
            let mut report = QualityReport::default();
            check("x", output, &mut report);
            assert!(
                report
                    .issues
                    .iter()
                    .any(|i| i.kind == QualityIssueKind::PlaceholderMarker),
                "{output}"
            );
        }
    }

    #[test]
    fn 普通文本里的_p1_不算残留() {
        let mut report = QualityReport::default();
        check("x", "参数 {P1} 与 P1 都是普通文本", &mut report);
        assert!(report.issues.is_empty(), "{:?}", report.issues);
    }

    #[test]
    fn 受保护内容丢失致命() {
        let p = plan("Visit https://example.com and enter {code}.");
        let mut report = QualityReport::default();
        check_protected(&p, "请访问并输入。", &mut report);
        assert!(report.has_fatal());
        assert_eq!(
            report.issues.len(),
            2,
            "URL 与变量都应报出：{:?}",
            report.issues
        );
    }

    #[test]
    fn 受保护内容完整时不报() {
        let p = plan("Visit https://example.com and enter {code}.");
        let mut report = QualityReport::default();
        check_protected(
            &p,
            "请访问 https://example.com 并输入 {code}。",
            &mut report,
        );
        assert!(report.issues.is_empty(), "{:?}", report.issues);
    }
}
