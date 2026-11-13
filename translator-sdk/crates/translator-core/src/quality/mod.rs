//! 译后质量检查：确定性完整性 + 数值语义 + 启发式诊断。
//!
//! # 为什么拆成四个文件（§5.4）
//!
//! 旧实现把所有检查塞在一个文件里，结果"确定性错误"和"启发式猜测"混在一起，
//! 无法回答"这条问题要不要拒绝返回"。现在职责是分层的：
//!
//! ```text
//! integrity.rs  空输出 / 标记残留 / 受保护内容丢失   → 一定致命
//! number.rs     编号、日期、百分比、金额、区间         → 分级：严格项致命，数量词只报告
//! heuristic.rs  否定、重复、语言、比例                → 只报告
//! report.rs     结果结构与汇总                        → 无判断逻辑
//! ```
//!
//! # 唯一的对外入口
//!
//! 宿主与协调器只调用 [`check`] / [`check_with_protection`]，不直接拼装问题列表，
//! 避免"某个调用点忘了跑完整性检查"。

pub mod heuristic;
pub mod integrity;
pub mod number;
pub mod report;

pub use number::{NumberCheck, NumberKind, NumberToken};
pub use report::{QualityIssue, QualityIssueKind, QualityReport, QualitySeverity};

use translator_types::{LanguageTag, TranslationMode};

use crate::protection::ProtectionPlan;

/// 对一段译文做译后检查。
///
/// `source` 是**送进模型前**的源文（已规范化、已保护前），`output` 是**已还原**
/// 的译文。不要在未还原的文本上调用：残留标记会被判成致命问题，这是刻意的。
pub fn check(
    source: &str,
    output: &str,
    target: &LanguageTag,
    mode: TranslationMode,
) -> QualityReport {
    let mut report = QualityReport {
        mode,
        ..QualityReport::default()
    };

    integrity::check(source, output, &mut report);
    if report.has_fatal() {
        // 空输出时后面的检查没有意义
        return report;
    }

    let number_check = number::check(source, output);
    report.numbers_in_source = number_check.in_source;
    report.numbers_preserved = number_check.preserved;
    report.strict_identifiers_in_source = number_check.strict_in_source;
    report.strict_identifiers_preserved = number_check.strict_preserved;
    report.issues.extend(number_check.issues);

    // Fast 模式不跑语义启发式：延迟与成本优先，完整性与数字仍然要查。
    if mode.is_precise() {
        heuristic::check(source, output, target, &mut report);
    }

    report
}

/// 带保护计划的检查：在 [`check`] 之外额外确认受保护内容都在。
pub fn check_with_protection(
    plan: &ProtectionPlan,
    restored: &str,
    target: &LanguageTag,
    mode: TranslationMode,
) -> QualityReport {
    let mut report = check(plan.source(), restored, target, mode);
    integrity::check_protected(plan, restored, &mut report);

    // 未翻译片段：只在这里做——需要**保护计划**（哪些英文本来就该原样保留）
    // 与**原文对应**，而不是靠英文词表猜。空输出时没有意义，跳过。
    if mode.is_precise() && !report.has_fatal() {
        let protected: Vec<String> = plan
            .spans()
            .iter()
            .map(|span| span.original.clone())
            .collect();
        for phrase in heuristic::untranslated_fragments(plan.source(), &protected, restored) {
            report.issues.push(QualityIssue::major(
                QualityIssueKind::Untranslated,
                format!("疑似未翻译的英文片段：{phrase}（原文中同样出现）"),
            ));
        }
    }
    report
}

/// 受控的译后修复：只处理**确定性的格式/语序**问题，不改写语义。
///
/// 1. `以下50%` / `以上20%` → `50%以下` / `20%以上`（数字与汉语范围词换位）；
/// 2. 去掉数字/字母后多余的空格标点，以及 `. .` / `。 。` 这类重复标点。
///
/// 这不是"翻译修正器"：它不会猜一个缺失的动词，也不会改术语。
pub fn repair_output(output: &str) -> String {
    let reordered = reorder_measure_phrases(output);
    collapse_spaced_punctuation(&reordered)
}

fn reorder_measure_phrases(input: &str) -> String {
    let mut text = input.to_string();
    for phrase in ["以下", "以上", "以内", "之外", "之下", "之上"] {
        let Some(pos) = text.find(phrase) else {
            continue;
        };
        let after = pos + phrase.len();
        let rest = &text[after..];
        let Some(len) = measure_number_len(rest) else {
            continue;
        };
        let number = rest[..len].to_string();
        let tail = &rest[len..];
        let unit_len = measure_unit_len(tail);
        let unit = &tail[..unit_len];
        let mut rebuilt = String::with_capacity(text.len());
        rebuilt.push_str(&text[..pos]);
        rebuilt.push_str(&number);
        rebuilt.push_str(unit);
        rebuilt.push_str(phrase);
        rebuilt.push_str(&tail[unit_len..]);
        text = rebuilt;
    }
    text
}

/// 量词/百分比数字长度（字节）：数字，可选小数部分，可选 `%`。
fn measure_number_len(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < bytes.len() && bytes[i] == b'%' {
        i += 1;
    }
    Some(i)
}

fn measure_unit_len(rest: &str) -> usize {
    for unit in [
        "公里",
        "公斤",
        "厘米",
        "毫米",
        "千米",
        "小时",
        "分钟",
        "毫秒",
        "摄氏度",
        "度",
        "米",
        "克",
        "元",
        "岁",
        "倍",
        "分",
        "秒",
        "天",
        "年",
        "个",
        "次",
        "人",
        "件",
        "台",
        "位",
        "升",
        "吨",
        "瓦",
        "伏",
        "安",
        "页",
        "行",
        "条",
        "张",
    ] {
        if rest.starts_with(unit) {
            return unit.len();
        }
    }
    0
}

fn collapse_spaced_punctuation(input: &str) -> String {
    const PUNCT: &[char] = &[
        '.', ',', '!', '?', ';', ':', '。', '，', '！', '？', '；', '：',
    ];
    let mut out = String::with_capacity(input.len());
    let mut space_since_last = false;
    for c in input.chars() {
        if c.is_whitespace() {
            out.push(c);
            space_since_last = true;
            continue;
        }
        if PUNCT.contains(&c) {
            let last = out.chars().rev().find(|x| !x.is_whitespace());
            if space_since_last && last == Some(c) {
                while out.ends_with(' ') {
                    out.pop();
                }
                space_since_last = false;
                continue;
            }
            while out.ends_with(' ') {
                out.pop();
            }
        }
        out.push(c);
        space_since_last = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zh() -> LanguageTag {
        LanguageTag::new("zh-Hans").unwrap()
    }

    fn precise(source: &str, output: &str) -> QualityReport {
        check(source, output, &zh(), TranslationMode::Precise)
    }

    #[test]
    fn 正常译文没有致命问题() {
        let r = precise(
            "The server has 3 nodes and 1,024 GB RAM.",
            "服务器有 3 个节点和 1024 GB 内存。",
        );
        assert_eq!(r.numbers_in_source, 2);
        assert_eq!(r.numbers_preserved, 2);
        assert!(!r.has_fatal(), "{r:?}");
    }

    #[test]
    fn 关键编号丢失致命() {
        let r = precise("Order #10248 shipped.", "订单已发货。");
        assert!(r.has_fatal());
        assert_eq!(r.strict_identifiers_in_source, 1);
        assert_eq!(r.strict_identifiers_preserved, 0);
    }

    #[test]
    fn 空输出直接致命且不继续跑其它检查() {
        let r = precise("Hello world.", "");
        assert!(r.has_fatal());
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].kind, QualityIssueKind::EmptyOutput);
    }

    #[test]
    fn fast_模式只查完整性与数字() {
        let r = check(
            "The service is not available for 3 users.",
            "服务可用。",
            &zh(),
            TranslationMode::Fast,
        );
        assert!(!r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NegationDropped));
        // 但数字仍然要查
        assert!(r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NumberMissing));
    }

    #[test]
    fn 带保护的检查能发现丢失的_url() {
        let plan = crate::protection::ProtectionPlan::build(
            "See https://example.com/x for more details now.",
        );
        let r = check_with_protection(&plan, "更多详情请参见。", &zh(), TranslationMode::Precise);
        assert!(r.has_fatal());
        assert!(r
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::ProtectedSpanMissing));
    }

    #[test]
    fn 修复只做确定性换位() {
        assert_eq!(repair_output("温度在以上20度。"), "温度在20度以上。");
        assert_eq!(repair_output("折扣以下50% 。"), "折扣50%以下。");
        // 不改变语义：不动正常句子
        assert_eq!(repair_output("这是一个正常句子。"), "这是一个正常句子。");
    }

    #[test]
    fn 修复不猜测缺失内容() {
        let input = "他去了。";
        assert_eq!(repair_output(input), input);
    }

    #[test]
    fn 汇总不包含原文() {
        let r = precise(
            "Order #10248 shipped to secret@example.com.",
            "订单已发货。",
        );
        let summary = r.summary();
        assert!(!summary.contains("secret@example.com"), "{summary}");
    }
}

#[cfg(test)]
mod untranslated_plan_tests {
    use super::*;
    use crate::protection::{ProtectionPlan, ProtectionPolicy};
    use translator_types::LanguageTag;

    fn plan(source: &str) -> ProtectionPlan {
        ProtectionPlan::prepare(source, &ProtectionPolicy::default()).expect("保护计划")
    }

    #[test]
    fn 终检会报告残留英文片段() {
        let source =
            "Everything is geared to being a weapon, displayed in accompanying media with gratuitous effect.";
        let plan = plan(source);
        let target = LanguageTag::new("zh-Hans").unwrap();
        // 真实的坏译文：两个片段没翻译
        let bad = "一切都 geared to being a weapon，在 accompanying media 中以无谓的效果加以展示。";
        let report = check_with_protection(&plan, bad, &target, TranslationMode::Precise);
        let hits: Vec<&str> = report
            .issues
            .iter()
            .filter(|i| i.kind == QualityIssueKind::Untranslated)
            .map(|i| i.detail.as_str())
            .collect();
        assert!(
            hits.iter().any(|d| d.contains("accompanying media")),
            "应报告 2 词残留：{hits:?}"
        );
    }

    #[test]
    fn 正常译文不会被误报() {
        let source =
            "Everything is geared to being a weapon, displayed in accompanying media with gratuitous effect.";
        let plan = plan(source);
        let target = LanguageTag::new("zh-Hans").unwrap();
        let good = "一切都朝着“武器”的方向设计，并在配套的媒体中被过度夸张地展示出来。";
        let report = check_with_protection(&plan, good, &target, TranslationMode::Precise);
        assert!(
            !report
                .issues
                .iter()
                .any(|i| i.kind == QualityIssueKind::Untranslated),
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn url_与代码保留不算未翻译() {
        let source = "Open https://example.com/docs and run `cargo build` before the meeting.";
        let plan = plan(source);
        let target = LanguageTag::new("zh-Hans").unwrap();
        let output = "开会前请打开 https://example.com/docs 并运行 `cargo build`。";
        let report = check_with_protection(&plan, output, &target, TranslationMode::Precise);
        assert!(
            !report
                .issues
                .iter()
                .any(|i| i.kind == QualityIssueKind::Untranslated),
            "{:?}",
            report.issues
        );
    }
}
