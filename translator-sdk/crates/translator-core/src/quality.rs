//! 译后完整性检查（T2 / docs/57 §5）。
//!
//! 这里只做**确定性、可解释**的检查，不判断“译文好不好”：
//!
//! | 检查 | 何时执行 | 失败后果 |
//! |---|---|---|
//! | 输出为空 | Precise + Fast | `BackendFailed` |
//! | 受保护标记残留 | Precise + Fast | `IntegrityFailure` |
//! | 数字丢失/被改 | Precise + Fast | 关键数字 → `IntegrityFailure` |
//! | 否定丢失 | Precise | 诊断（Major），不改写译文 |
//! | 连续重复句段 | Precise | 诊断（Major） |
//! | 目标语言明显不对 | Precise | 诊断（Critical/Major） |
//! | 原文被原样返回 | Precise | 诊断（Major） |
//!
//! 设计约束：
//!
//! - **不猜测正确译文**。检查只报告，只有“关键数字丢失/占位符标记残留”会拒绝返回；
//!   否定、重复、语言判断都是启发式，误判时不能把一句正确的译文判死。
//! - **不把与参考译文不同当成错误**。这里根本没有参考译文。
//! - 检查成本是 O(文本长度)，Fast 模式只跑前两项，不跑语义启发式。

use translator_types::{LanguageTag, TranslationMode};

/// 问题级别。顺序即严重程度（`Critical` 最高）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualitySeverity {
    Minor,
    Major,
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

/// 问题类型。字符串形式进入报告与 JSONL，必须稳定。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QualityIssueKind {
    /// 源文里的数字在译文中找不到。
    NumberMissing,
    /// 译文出现了源文没有的数字（可能是格式/日期转换，也可能是不该有的编号）。
    NumberAdded,
    /// 受保护标记（`⟦P0⟧`）残留在译文里。
    PlaceholderMarker,
    /// 源文有否定词，译文里找不到任何否定表达。
    NegationDropped,
    /// 连续重复的句段。
    DuplicateSegment,
    /// 目标语言明显不符（中文目标却没有汉字等）。
    TargetLanguageMismatch,
    /// 译文与原文完全相同（多词输入）。
    Untranslated,
    /// 输出为空。
    EmptyOutput,
    /// 源文有条件关系，译文没有条件标记（启发式）。
    ConditionalDropped,
    /// 译文相对源文异常短，可能遗漏内容（启发式）。
    PossibleOmission,
    /// 译文相对源文异常长，可能追加了不存在的内容（启发式）。
    PossibleHallucination,
}

impl QualityIssueKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NumberMissing => "number_missing",
            Self::NumberAdded => "number_added",
            Self::PlaceholderMarker => "placeholder_marker",
            Self::NegationDropped => "negation_dropped",
            Self::DuplicateSegment => "duplicate_segment",
            Self::TargetLanguageMismatch => "target_language_mismatch",
            Self::Untranslated => "untranslated",
            Self::EmptyOutput => "empty_output",
            Self::ConditionalDropped => "conditional_dropped",
            Self::PossibleOmission => "possible_omission",
            Self::PossibleHallucination => "possible_hallucination",
        }
    }
}

/// 单条问题。`detail` 不得包含整段原文（只放少量数字/标记，避免把用户原文写进日志）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityIssue {
    pub kind: QualityIssueKind,
    pub severity: QualitySeverity,
    pub detail: String,
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
}

impl Default for QualityReport {
    fn default() -> Self {
        Self {
            mode: TranslationMode::Precise,
            issues: Vec::new(),
            numbers_in_source: 0,
            numbers_preserved: 0,
        }
    }
}

impl QualityReport {
    /// 是否包含会导致拒绝返回的问题。
    ///
    /// 目前只有两类：关键数字丢失、受保护标记残留。其它问题只进诊断。
    pub fn has_fatal(&self) -> bool {
        self.issues.iter().any(|issue| {
            matches!(
                issue.kind,
                QualityIssueKind::PlaceholderMarker | QualityIssueKind::EmptyOutput
            ) || (issue.kind == QualityIssueKind::NumberMissing
                && issue.severity == QualitySeverity::Critical)
        })
    }

    /// 是否存在 Critical 问题（包含不会拒绝返回的语言判断）。
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
    }

    pub fn count(&self, severity: QualitySeverity) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == severity)
            .count()
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

/// 对一段译文做译后检查。
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

    if output.trim().is_empty() {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::EmptyOutput,
            severity: QualitySeverity::Critical,
            detail: "引擎返回空译文".to_string(),
        });
        return report;
    }

    if output.contains('\u{27E6}') && output.contains("P") && output.contains('\u{27E7}') {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::PlaceholderMarker,
            severity: QualitySeverity::Critical,
            detail: "译文里残留受保护标记".to_string(),
        });
    }

    // 数字检查：两种模式都做（Fast 也不允许破坏关键数字）。
    let (source_numbers, output_numbers) = (numeric_tokens(source), numeric_tokens(output));
    report.numbers_in_source = source_numbers.len();
    let mut remaining = output_numbers.clone();
    for token in &source_numbers {
        if let Some(pos) = remaining
            .iter()
            .position(|item| item.canonical == token.canonical)
        {
            remaining.remove(pos);
            report.numbers_preserved += 1;
        } else if chinese_forms(&token.canonical)
            .iter()
            .any(|form| output.contains(form))
        {
            // 译文用了规范中文数字（两小时三十分钟、百分之十五）：
            // 数值语义保留，不能因为不是阿拉伯数字就判丢失。
            report.numbers_preserved += 1;
        } else {
            report.issues.push(QualityIssue {
                kind: QualityIssueKind::NumberMissing,
                severity: if token.critical {
                    QualitySeverity::Critical
                } else {
                    QualitySeverity::Major
                },
                detail: format!("源文数字 {} 未在译文中找到", token.raw),
            });
        }
    }
    for token in &remaining {
        // 译文多出的数字只报告，不拒绝：日期/编号格式化可能合法。
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::NumberAdded,
            severity: QualitySeverity::Minor,
            detail: format!("译文多出数字 {}", token.raw),
        });
    }

    // Fast 到此为止：语义启发式不跑。
    if !mode.is_precise() {
        return report;
    }

    let lower_source = source.to_lowercase();
    let negation_cues = count_negation_cues(&lower_source);
    if negation_cues > 0 && !has_target_negation(output) {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::NegationDropped,
            severity: QualitySeverity::Major,
            detail: format!("源文有 {negation_cues} 个否定线索，译文没有否定表达"),
        });
    }

    let source_words = source.split_whitespace().count();
    let ratio = output.chars().count() as f32 / source.chars().count().max(1) as f32;
    if source_words >= 10 && ratio < 0.18 {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::PossibleOmission,
            severity: QualitySeverity::Major,
            detail: format!("译文/源文字符比仅 {ratio:.2}，可能遗漏内容"),
        });
    }
    if source_words >= 5 && ratio > 2.5 {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::PossibleHallucination,
            severity: QualitySeverity::Major,
            detail: format!("译文/源文字符比 {ratio:.2}，可能追加了内容"),
        });
    }
    if has_conditional_cue(&lower_source) && !has_target_conditional(output) {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::ConditionalDropped,
            severity: QualitySeverity::Major,
            detail: "源文有条件关系，译文没有条件标记".to_string(),
        });
    }

    if let Some(segment) = duplicated_segment(output) {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::DuplicateSegment,
            severity: QualitySeverity::Major,
            detail: format!("连续重复句段：{segment}"),
        });
    }

    if target.as_str().starts_with("zh") && !target_looks_chinese(output) {
        let severity = if output.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 8 {
            QualitySeverity::Critical
        } else {
            QualitySeverity::Major
        };
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::TargetLanguageMismatch,
            severity,
            detail: "目标语言为中文，但译文没有足够的汉字".to_string(),
        });
    } else if target.as_str().starts_with("en") && !output.chars().any(|c| c.is_ascii_alphabetic())
    {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::TargetLanguageMismatch,
            severity: QualitySeverity::Major,
            detail: "目标语言为英文，但译文没有拉丁字母".to_string(),
        });
    }

    if source.trim().eq_ignore_ascii_case(output.trim()) && source.split_whitespace().count() >= 2 {
        report.issues.push(QualityIssue {
            kind: QualityIssueKind::Untranslated,
            severity: QualitySeverity::Major,
            detail: "译文与原文完全相同".to_string(),
        });
    }

    report
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NumberToken {
    raw: String,
    canonical: String,
    critical: bool,
}

/// 受控的译后修复：只处理**确定性的格式/语序**问题，不改写语义。
///
/// 1. `以下50%` / `以上20%` → `50%以下` / `20%以上`（数字与汉语范围词换位）；
/// 2. 去掉数字/字母后多余的空格标点，以及 `. .` / `。 。` 这类重复标点。
///
/// 这不是“翻译修正器”：它不会猜一个缺失的动词，也不会改术语。
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
        // 单位跟在数字后：`20度` / `5公里` / `3倍`。单位应留在数字旁边，
        // 范围词放到单位之后（20度以上），而不是插在数字和单位之间。
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

/// 提取数字 token。
///
/// 规则：
/// - 全角数字先归一为半角；
/// - 连续数字，可含一个千分位逗号或小数点；
/// - 可带 `%`；
/// - 序号（`1st` / `2nd` / `3rd` / `4th`）不参与"关键数字"判定：
///   中文写成“第一”是正确译法，不能因为找不到 `1` 就判错。
fn numeric_tokens(text: &str) -> Vec<NumberToken> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if !is_digit(c) {
            i += 1;
            continue;
        }

        let start = i;
        let mut raw = String::new();
        let mut canonical = String::new();
        let mut dots = 0usize;
        while i < chars.len() {
            let ch = chars[i];
            if is_digit(ch) {
                let d = to_ascii_digit(ch);
                raw.push(ch);
                canonical.push(d);
                i += 1;
            } else if ch == ',' && i + 1 < chars.len() && is_digit(chars[i + 1]) {
                // 千分位：只在“逗号后还有 3 位数字”时吸收，避免把 1,2 当数字。
                let rest = chars[i + 1..].iter().take_while(|c| is_digit(**c)).count();
                if rest == 3 && !canonical.is_empty() {
                    raw.push(ch);
                    i += 1;
                } else {
                    break;
                }
            } else if ch == '.' && i + 1 < chars.len() && is_digit(chars[i + 1]) && dots == 0 {
                raw.push(ch);
                canonical.push('.');
                dots += 1;
                i += 1;
            } else if ch == ' ' && i + 1 < chars.len() && is_digit(chars[i + 1]) && dots == 0 {
                // 模型常把千分位逗号写成空格：`1,250` → `1 250`、`10,000` → `10 000`。
                // 只有“前一组 1–3 位、后一组恰好 3 位”才合并，避免把两个独立数字
                // （例如 “3 200”）强行拼成一个。
                let prev_digits = canonical.chars().filter(|c| c.is_ascii_digit()).count();
                let next_digits = chars[i + 1..].iter().take_while(|c| is_digit(**c)).count();
                if (1..=3).contains(&prev_digits) && next_digits == 3 {
                    raw.push(' ');
                    i += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // 模型有时写成 `15 %`（数字与百分号之间有空格），语义不变。
        let mut percent_at = i;
        while percent_at < chars.len() && chars[percent_at] == ' ' {
            percent_at += 1;
        }
        if percent_at < chars.len() && chars[percent_at] == '%' {
            raw.push('%');
            canonical.push('%');
            i = percent_at + 1;
        }

        let ordinal = matches!(
            chars[i..]
                .iter()
                .take(2)
                .collect::<String>()
                .to_ascii_lowercase()
                .as_str(),
            "st" | "nd" | "rd" | "th"
        );
        let digits = canonical.chars().filter(|c| c.is_ascii_digit()).count();
        let critical =
            !ordinal && (digits >= 2 || canonical.contains('.') || canonical.contains('%'));
        // `07:15` 与 `7: 15` 是同一个时间；前导零不改变数值。
        let canonical = normalize_number(&canonical);
        out.push(NumberToken {
            raw: chars[start..i].iter().collect(),
            canonical,
            critical,
        });
    }
    out
}

/// 去掉整数部分的前导零，保留小数/百分号。
///
/// `07` → `7`、`001` → `1`、`0` → `0`、`0.05` → `0.05`、`15%` → `15%`。
fn normalize_number(raw: &str) -> String {
    let (body, suffix) = if let Some(stripped) = raw.strip_suffix('%') {
        (stripped, "%")
    } else {
        (raw, "")
    };
    let (int_part, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let trimmed = int_part.trim_start_matches('0');
    let int_part = if trimmed.is_empty() { "0" } else { trimmed };
    match frac {
        Some(f) => format!("{int_part}.{f}{suffix}"),
        None => format!("{int_part}{suffix}"),
    }
}

/// 一个数字的常见中文写法（用于数字保真的等价判断）。
fn chinese_forms(canonical: &str) -> Vec<String> {
    let (body, percent) = if let Some(stripped) = canonical.strip_suffix('%') {
        (stripped, true)
    } else {
        (canonical, false)
    };
    let Ok(value) = body.parse::<u64>() else {
        return Vec::new();
    };
    let mut forms = Vec::new();
    let standard = chinese_integer(value);
    if !standard.is_empty() {
        forms.push(if percent {
            format!("百分之{standard}")
        } else {
            standard
        });
    }
    if value == 2 {
        forms.push(if percent {
            "百分之两".to_string()
        } else {
            "两".to_string()
        });
    }
    // 年份/编号的逐位读法（二〇二六）
    if value >= 1000 {
        let digits: String = value
            .to_string()
            .chars()
            .map(|c| match c {
                '0' => '零',
                '1' => '一',
                '2' => '二',
                '3' => '三',
                '4' => '四',
                '5' => '五',
                '6' => '六',
                '7' => '七',
                '8' => '八',
                '9' => '九',
                _ => c,
            })
            .collect();
        forms.push(if percent {
            format!("百分之{digits}")
        } else {
            digits
        });
    }
    forms
}

fn chinese_integer(mut n: u64) -> String {
    if n == 0 {
        return "零".to_string();
    }
    if n > 9999 {
        return String::new();
    }
    const DIGITS: [&str; 10] = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    let units = ["", "十", "百", "千"];
    let mut parts: Vec<String> = Vec::new();
    let mut unit = 0usize;
    let mut zero_pending = false;
    while n > 0 {
        let d = (n % 10) as usize;
        if d == 0 {
            if !parts.is_empty() {
                zero_pending = true;
            }
        } else {
            let mut piece = String::new();
            if zero_pending {
                piece.push('零');
                zero_pending = false;
            }
            piece.push_str(DIGITS[d]);
            piece.push_str(units[unit]);
            parts.push(piece);
        }
        n /= 10;
        unit += 1;
    }
    parts.reverse();
    let mut out = parts.concat();
    if out.starts_with("一十") {
        out = out.replacen("一十", "十", 1);
    }
    out
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit() || ('\u{FF10}'..='\u{FF19}').contains(&c)
}

fn to_ascii_digit(c: char) -> char {
    if c.is_ascii_digit() {
        c
    } else {
        char::from_u32(c as u32 - 0xFF10 + '0' as u32).unwrap_or('0')
    }
}

fn has_conditional_cue(lower_source: &str) -> bool {
    let padded = format!(" {lower_source} ");
    [
        " if ",
        " unless ",
        " when ",
        " once ",
        " provided ",
        " as long as ",
    ]
    .iter()
    .any(|cue| padded.contains(cue))
}

fn has_target_conditional(output: &str) -> bool {
    [
        "如果", "若", "假如", "要是", "除非", "当", "一旦", "只要", "只有", "否则", "即使", "时",
        "后", "一经", "完成", "的话", "则", "便", "就",
    ]
    .iter()
    .any(|cue| output.contains(cue))
}

fn count_negation_cues(lower_source: &str) -> usize {
    const CUES: &[&str] = &[
        "not",
        "n't",
        "never",
        "none",
        "nobody",
        "nothing",
        "neither",
        "nor",
        "without",
        "cannot",
        "can't",
        "don't",
        "doesn't",
        "didn't",
        "isn't",
        "aren't",
        "wasn't",
        "weren't",
        "won't",
        "wouldn't",
        "shouldn't",
        "couldn't",
        "haven't",
        "hasn't",
        "hadn't",
        "no",
        "hardly",
        "barely",
        "rarely",
        "seldom",
    ];
    let mut count = 0usize;
    let bytes = lower_source.as_bytes();
    for cue in CUES {
        let mut from = 0usize;
        while let Some(pos) = lower_source[from..].find(cue) {
            let at = from + pos;
            let end = at + cue.len();
            let left_ok =
                at == 0 || !bytes[at - 1].is_ascii_alphanumeric() || cue.starts_with("n't");
            let right_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if left_ok && right_ok {
                count += 1;
                // "not only" 不算否定线索：中文“不仅”仍含“不”，会自然通过，
                // 这里少计一次可以降低误报。
                if *cue == "not" && lower_source[end..].trim_start().starts_with("only") {
                    count = count.saturating_sub(1);
                }
            }
            from = end;
        }
    }
    count
}

fn has_target_negation(output: &str) -> bool {
    const NEGATORS: &[&str] = &[
        "不", "没", "無", "无", "未", "别", "別", "非", "否", "勿", "莫", "甭", "休", "毋",
    ];
    output
        .chars()
        .any(|c| NEGATORS.contains(&c.to_string().as_str()))
}

fn target_looks_chinese(output: &str) -> bool {
    let total = output.chars().filter(|c| !c.is_whitespace()).count();
    let han = output
        .chars()
        .filter(|c| ('\u{4E00}'..='\u{9FFF}').contains(c))
        .count();
    // 很短的输出（例如 "OK"）允许没有汉字；超过 8 个非空白字符仍无汉字才算错。
    han > 0 || total < 8
}

fn duplicated_segment(output: &str) -> Option<String> {
    let segments: Vec<&str> = output
        .split(['。', '！', '？', '!', '?', ';', '；', ',', '，', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 2 {
        // 没有标点时，检查整句是否由同一短语反复组成（如“你好你好你好”）。
        let chars: Vec<char> = output.chars().filter(|c| !c.is_whitespace()).collect();
        for period in 1..=chars.len() / 3 {
            let block: String = chars[..period].iter().collect();
            let repeats = chars
                .chunks(period)
                .filter(|c| c.iter().collect::<String>() == block)
                .count();
            if repeats >= 3 && period >= 2 {
                return Some(block);
            }
        }
        return None;
    }
    for idx in 1..segments.len() {
        if segments[idx] == segments[idx - 1] && segments[idx].chars().count() >= 4 {
            return Some(segments[idx].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zh() -> LanguageTag {
        LanguageTag::new("zh-Hans").unwrap()
    }

    #[test]
    fn 数字保留时无问题() {
        let report = check(
            "The server has 3 nodes and 1,024 GB RAM.",
            "服务器有 3 个节点和 1024 GB 内存。",
            &zh(),
            TranslationMode::Precise,
        );
        assert_eq!(report.numbers_in_source, 2);
        assert_eq!(report.numbers_preserved, 2);
        assert!(!report.has_fatal(), "{report:?}");
    }

    #[test]
    fn 关键数字丢失是致命问题() {
        let report = check(
            "Version 2.10 is ready.",
            "版本已就绪。",
            &zh(),
            TranslationMode::Precise,
        );
        assert!(report.has_fatal());
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NumberMissing
                && i.severity == QualitySeverity::Critical));
    }

    #[test]
    fn 个位数丢失不拒绝返回() {
        let report = check(
            "I have 5 apples.",
            "我有苹果。",
            &zh(),
            TranslationMode::Fast,
        );
        assert!(!report.has_fatal(), "{report:?}");
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NumberMissing));
    }

    #[test]
    fn 千分位空格与前导零不算数字丢失() {
        let report = check(
            "The report has 1,250 pages at 07:15.",
            "报告有1 250页，时间是7: 15。",
            &zh(),
            TranslationMode::Precise,
        );
        assert_eq!(report.numbers_in_source, 3);
        assert_eq!(report.numbers_preserved, 3, "{report:?}");
        assert!(!report.has_fatal(), "{report:?}");
    }

    #[test]
    fn 修复百分比与范围词顺序() {
        assert_eq!(
            repair_output("CPU使用率维持在以下50%. ."),
            "CPU使用率维持在50%以下."
        );
        assert_eq!(repair_output("温度在以上20度。 。"), "温度在20度以上。");
        assert_eq!(repair_output("版本是2.4.1..."), "版本是2.4.1...");
    }

    #[test]
    fn 条件关系丢失被报告() {
        let report = check(
            "If the token expires, the client must log in again.",
            "令牌过期，客户端必须重新登录。",
            &zh(),
            TranslationMode::Precise,
        );
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::ConditionalDropped));
    }

    #[test]
    fn 中文数字不判为丢失() {
        assert_eq!(chinese_integer(30), "三十");
        assert_eq!(chinese_integer(2), "二");
        assert!(
            chinese_forms("30").iter().any(|f| f == "三十"),
            "{:?}",
            chinese_forms("30")
        );
        let report = check(
            "The job finished in 2 hours and 30 minutes.",
            "工作在两个小时三十分钟内完成。",
            &zh(),
            TranslationMode::Precise,
        );
        assert_eq!(report.numbers_in_source, 2);
        assert_eq!(report.numbers_preserved, 2, "{report:?}");
        assert!(!report.has_fatal(), "{report:?}");
    }

    #[test]
    fn 序数词不作为数字错误() {
        let report = check(
            "It is the 3rd attempt.",
            "这是第三次尝试。",
            &zh(),
            TranslationMode::Precise,
        );
        assert_eq!(report.numbers_in_source, 1);
        assert!(!report.has_fatal(), "{report:?}");
    }

    #[test]
    fn 否定丢失在精准模式报告() {
        let report = check(
            "I do not agree.",
            "我同意。",
            &zh(),
            TranslationMode::Precise,
        );
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NegationDropped));
        assert!(!report.has_fatal());
    }

    #[test]
    fn 快速模式不跑否定启发式() {
        let report = check("I do not agree.", "我同意。", &zh(), TranslationMode::Fast);
        assert!(!report
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NegationDropped));
    }

    #[test]
    fn 残留保护标记是致命问题() {
        let report = check("Hi {name}", "你好 ⟦P0⟧", &zh(), TranslationMode::Fast);
        assert!(report.has_fatal());
    }

    #[test]
    fn 连续重复句段被报告() {
        let report = check(
            "The library opens at nine.",
            "图书馆九点开放。图书馆九点开放。",
            &zh(),
            TranslationMode::Precise,
        );
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::DuplicateSegment));
    }

    #[test]
    fn 中文目标却输出英文被报告() {
        let report = check(
            "This is a complete sentence.",
            "This is a complete sentence.",
            &zh(),
            TranslationMode::Precise,
        );
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::TargetLanguageMismatch));
    }
}
