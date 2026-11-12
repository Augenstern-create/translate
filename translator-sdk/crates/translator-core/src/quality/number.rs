//! 数字与数值语义检查。
//!
//! # 为什么"数字保护"和"数字检查"是两件事（§5）
//!
//! - **保护**（[`crate::protection`]）只处理"丢了就是错"的格式串：日期、版本号、
//!   百分比、编号。它们必须逐字节保真。
//! - **检查**（本模块）负责**语义**：`15%` 与 `百分之十五` 等价，
//!   `between 10 and 20` 与 `10 至 20 之间` 等价，但 `15%` 与 `50%` 不等价。
//!
//! 把两者混为一谈会得到两个坏结果：要么强制所有数字原样出现（把
//! `I have three books.` → `我有三本书。` 判成错误），要么完全不管（订单号被改也放行）。
//!
//! # 分级（§5.3）
//!
//! | 级别 | 例子 | 处理 |
//! |---|---|---|
//! | 确定性 | 订单号 `#10248`、版本号 `v2.1.3`、日期、百分比、区间上下限 | 不一致 → **致命** |
//! | 语义 | 金额币种、时间、数量词 | 能可靠判定才致命，否则只报告 |
//!
//! 刻意**不做**的事：猜"模型想表达哪个数字"。判定不了就报告，不阻断。

use super::report::{QualityIssue, QualityIssueKind, QualitySeverity};

/// 数字的语义类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NumberKind {
    /// `#10248`、`ORD-7788` 一类的编号。
    Identifier,
    /// `v2.1.3`、`2.4.1`（至少两段点号）。
    Version,
    /// `$39.99`、`€10`。
    Currency,
    /// `15%`。
    Percent,
    /// `2026-11-11`、`2026/11/11`。
    Date,
    /// `14:30`。
    Time,
    /// `three`、`3`、`1,250`。
    Quantity,
    /// `3rd`、`third`。
    Ordinal,
}

impl NumberKind {
    /// 是否属于"确定性完整性"类别（不一致即致命）。
    pub const fn is_strict(self) -> bool {
        matches!(
            self,
            Self::Identifier | Self::Version | Self::Date | Self::Percent
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identifier => "identifier",
            Self::Version => "version",
            Self::Currency => "currency",
            Self::Percent => "percent",
            Self::Date => "date",
            Self::Time => "time",
            Self::Quantity => "quantity",
            Self::Ordinal => "ordinal",
        }
    }
}

/// 一个数字 token。
#[derive(Clone, Debug, PartialEq)]
pub struct NumberToken {
    /// 原文写法。
    pub raw: String,
    pub kind: NumberKind,
    /// 归一化后的数值（无单位）。
    pub value: Option<f64>,
    /// 币种符号或代码（仅 `Currency`）。
    pub currency: Option<String>,
}

/// 一段区间的两端（`10–20`、`between 10 and 20`）。
#[derive(Clone, Debug, PartialEq)]
pub struct RangeToken {
    pub raw: String,
    pub low: f64,
    pub high: f64,
}

/// 检查结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NumberCheck {
    pub issues: Vec<QualityIssue>,
    /// 源文数字 token 数。
    pub in_source: usize,
    /// 在译文里找到的个数。
    pub preserved: usize,
    /// 源文严格标识符数量。
    pub strict_in_source: usize,
    /// 其中保留下来的数量。
    pub strict_preserved: usize,
}

impl NumberCheck {
    pub fn has_fatal(&self) -> bool {
        self.issues.iter().any(|i| {
            i.kind == QualityIssueKind::NumberRelationMismatch
                || (i.kind == QualityIssueKind::StrictIdentifierMismatch)
                || (i.kind == QualityIssueKind::NumberMissing
                    && i.severity == QualitySeverity::Critical)
        })
    }
}

/// 英文字母数字词。
const NUMBER_WORDS: [(&str, u64); 31] = [
    ("zero", 0),
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("thirteen", 13),
    ("fourteen", 14),
    ("fifteen", 15),
    ("sixteen", 16),
    ("seventeen", 17),
    ("eighteen", 18),
    ("nineteen", 19),
    ("twenty", 20),
    ("thirty", 30),
    ("forty", 40),
    ("fifty", 50),
    ("sixty", 60),
    ("seventy", 70),
    ("eighty", 80),
    ("ninety", 90),
    ("hundred", 100),
    ("thousand", 1_000),
    ("million", 1_000_000),
];

const ORDINAL_WORDS: [(&str, u64); 12] = [
    ("first", 1),
    ("second", 2),
    ("third", 3),
    ("fourth", 4),
    ("fifth", 5),
    ("sixth", 6),
    ("seventh", 7),
    ("eighth", 8),
    ("ninth", 9),
    ("tenth", 10),
    ("eleventh", 11),
    ("twelfth", 12),
];

/// 把一段文本里的数字 token 提取出来（源文用）。
pub fn tokenize(text: &str) -> Vec<NumberToken> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];

        // 编号：`#10248` / `ORD-7788`
        if c == '#' && i + 1 < chars.len() && chars[i + 1].is_ascii_alphanumeric() {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '-') {
                j += 1;
            }
            let body: String = chars[start..j].iter().collect();
            if body.chars().any(|c| c.is_ascii_digit()) && body.len() >= 2 {
                out.push(NumberToken {
                    raw: format!("#{body}"),
                    kind: NumberKind::Identifier,
                    value: None,
                    currency: None,
                });
                i = j;
                continue;
            }
        }

        // 币种代码：`USD 39.99` / `39.99 USD`（数字前后都识别）
        if c.is_ascii_alphabetic() && is_word_start(&chars, i) {
            let start = i;
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let code: String = chars[start..j].iter().collect::<String>().to_uppercase();
            if let Some((canonical, symbol)) = currency_code(&code) {
                // 代码在数字**前面**：`USD 39.99`
                if let Some((value, end)) = number_after_code(&chars, j) {
                    out.push(NumberToken {
                        raw: format!("{code} {value}"),
                        kind: NumberKind::Currency,
                        value: Some(value),
                        currency: Some(format!("{canonical}|{symbol}")),
                    });
                    i = end;
                    continue;
                }
            }
            // 代码在数字**后面**的情况由下面的数字分支处理
        }

        // 币种符号：`$39.99` / `€10` / `₽500`
        if matches!(c, '$' | '€' | '£' | '¥' | '₹' | '₽' | '₩' | '₺') {
            let symbol = c;
            if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                let (value, end, raw_num) = parse_number(&chars, i + 1);
                if let Some(value) = value {
                    out.push(NumberToken {
                        raw: format!("{symbol}{raw_num}"),
                        kind: NumberKind::Currency,
                        value: Some(value),
                        currency: Some(symbol.to_string()),
                    });
                    i = end;
                    continue;
                }
            }
        }

        if c.is_ascii_digit() {
            // 日期与时间必须整段识别，否则 `2026-11-11` 会被拆成 `2026`，
            // 校验时只能看到年份，改月份也发现不了。
            if let Some((end, raw)) = date_prefix(&chars, i) {
                out.push(NumberToken {
                    raw,
                    kind: NumberKind::Date,
                    value: None,
                    currency: None,
                });
                i = end;
                continue;
            }
            if let Some((end, raw)) = time_prefix(&chars, i) {
                out.push(NumberToken {
                    raw,
                    kind: NumberKind::Time,
                    value: None,
                    currency: None,
                });
                i = end;
                continue;
            }
            let (value, end, raw) = parse_number(&chars, i);
            let Some(value) = value else {
                // 版本号 `2.1.3` 不是数值，但仍必须被识别（这是确定性检查对象）
                if raw.matches('.').count() >= 2 {
                    out.push(NumberToken {
                        raw,
                        kind: NumberKind::Version,
                        value: None,
                        currency: None,
                    });
                    i = end;
                    continue;
                }
                i += 1;
                continue;
            };
            let mut kind = classify_numeric(&chars, i, end, &raw);
            let mut currency = None;
            // `39.99 USD` / `1,250 EUR`：代码在数字后面
            if let Some((canonical, symbol)) = currency_code_after(&chars, end) {
                kind = NumberKind::Currency;
                currency = Some(format!("{canonical}|{symbol}"));
            }
            out.push(NumberToken {
                raw,
                kind,
                value: Some(value),
                currency,
            });
            i = end;
            continue;
        }

        // 英文数词
        if c.is_ascii_alphabetic() && is_word_start(&chars, i) {
            let start = i;
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let word: String = chars[start..j].iter().collect::<String>().to_lowercase();
            if let Some((_, v)) = ORDINAL_WORDS.iter().find(|(w, _)| *w == word) {
                // 只有确定是"序数词"时才当作数字：`first`/`second` 也常作副词
                // （"finishes first"）或时间单位（"every second"），
                // 误判会要求译文出现"第一/第二"，把正确译文判成漏数字。
                if ordinal_is_numeric(&chars, start, j) {
                    out.push(NumberToken {
                        raw: word,
                        kind: NumberKind::Ordinal,
                        value: Some(*v as f64),
                        currency: None,
                    });
                }
                i = j;
                continue;
            }
            if let Some((_, v)) = NUMBER_WORDS.iter().find(|(w, _)| *w == word) {
                // `one hundred` / `three thousand` 这类组合只取主词，不做完整英文数词解析：
                // 误判的代价（把正确译文判死）比收益大。
                out.push(NumberToken {
                    raw: word,
                    kind: NumberKind::Quantity,
                    value: Some(*v as f64),
                    currency: None,
                });
                i = j;
                continue;
            }
        }

        i += 1;
    }

    out
}

/// `2026-11-11` / `2026/11/11` 整段识别。
fn date_prefix(chars: &[char], i: usize) -> Option<(usize, String)> {
    let take_digits = |from: usize, min: usize, max: usize| -> Option<usize> {
        let mut j = from;
        while j < chars.len() && chars[j].is_ascii_digit() && j - from < max {
            j += 1;
        }
        (j - from >= min).then_some(j)
    };
    let year_end = take_digits(i, 4, 4)?;
    let sep1 = *chars.get(year_end)?;
    if !matches!(sep1, '-' | '/') {
        return None;
    }
    let month_end = take_digits(year_end + 1, 1, 2)?;
    let sep2 = *chars.get(month_end)?;
    if !matches!(sep2, '-' | '/') {
        return None;
    }
    let day_end = take_digits(month_end + 1, 1, 2)?;
    // 排除版本号 `2026.11.11`（点号）与时间 `2026-11-11T10` 之后的继续解析
    Some((day_end, chars[i..day_end].iter().collect()))
}

/// `14:30` / `9:05` 整段识别。
fn time_prefix(chars: &[char], i: usize) -> Option<(usize, String)> {
    let mut j = i;
    while j < chars.len() && chars[j].is_ascii_digit() && j - i < 2 {
        j += 1;
    }
    if j == i || chars.get(j) != Some(&':') {
        return None;
    }
    let colon = j;
    let mut k = colon + 1;
    while k < chars.len() && chars[k].is_ascii_digit() && k - colon - 1 < 2 {
        k += 1;
    }
    if k - colon - 1 != 2 {
        return None;
    }
    Some((k, chars[i..k].iter().collect()))
}

fn is_word_start(chars: &[char], i: usize) -> bool {
    match i.checked_sub(1).and_then(|p| chars.get(p)) {
        Some(c) => !c.is_ascii_alphanumeric() && *c != '_',
        None => true,
    }
}

/// 英文序数词是否真的在表达"第几个"。
///
/// 规则（保守）：
///
/// - 前面是 `every`/`each`/`per`/`a`/`an` → 时间单位用法（`every second`），不算序数；
/// - 前面必须是限定词（`the`/`this`/`my`/...，最多跨一个词），否则当作副词
///   （`finishes first`、`First, open ...`），不算序数。
fn ordinal_is_numeric(chars: &[char], start: usize, end: usize) -> bool {
    // 向前取最多两个词（只用于诊断；判定只用紧邻词）
    let mut words: Vec<String> = Vec::new();
    let mut i = start;
    while words.len() < 2 && i > 0 {
        // 跳过空白
        let mut k = i;
        while k > 0 && chars[k - 1].is_whitespace() {
            k -= 1;
        }
        if k == 0 {
            break;
        }
        let mut begin = k;
        while begin > 0 && chars[begin - 1].is_ascii_alphabetic() {
            begin -= 1;
        }
        if begin == k {
            break; // 不是词（标点/数字），停止
        }
        let word: String = chars[begin..k].iter().collect::<String>().to_lowercase();
        words.push(word);
        i = begin;
    }
    let _ = end;

    let Some(prev) = words.first() else {
        return false;
    };
    if matches!(prev.as_str(), "every" | "each" | "per" | "a" | "an") {
        return false;
    }
    // 只看**紧邻**的一个词：`the checklist first` 里的 `the` 属于 checklist，
    // 不能当作 first 的限定词（否则 "finishes the checklist first" 会误判）。
    const DETERMINERS: [&str; 11] = [
        "the", "this", "that", "these", "those", "my", "your", "his", "her", "its", "their",
    ];
    DETERMINERS.contains(&prev.as_str())
}

/// 解析从 `start` 开始的数字，返回 (值, 结束位置, 原文)。
fn parse_number(chars: &[char], start: usize) -> (Option<f64>, usize, String) {
    let mut i = start;
    let mut raw = String::new();
    let mut normalized = String::new();
    let mut dots = 0usize;

    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            raw.push(c);
            normalized.push(c);
            i += 1;
        } else if c == ',' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            // 千分位：只在"后面恰好 3 位"时吸收，避免把 `1,2` 当数字
            let rest = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if rest == 3 {
                raw.push(',');
                i += 1;
            } else {
                break;
            }
        } else if c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() && dots < 3 {
            // 最多三段：`39.99`（小数）与 `v2.1.3`（版本号）都要能整段读出
            raw.push('.');
            normalized.push('.');
            dots += 1;
            i += 1;
        } else if c == ' ' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() && dots == 0 {
            // `1 250` 这类模型/排版造成的空格千分位
            let prev = normalized.chars().filter(|c| c.is_ascii_digit()).count();
            let next = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if (1..=3).contains(&prev) && next == 3 {
                raw.push(' ');
                i += 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    (normalized.parse::<f64>().ok(), i, raw)
}

fn classify_numeric(chars: &[char], start: usize, end: usize, raw: &str) -> NumberKind {
    // 日期 `2026-11-11` / `2026/11/11`
    if raw.chars().all(|c| c.is_ascii_digit()) && raw.len() == 4 {
        if let Some(next) = chars.get(end) {
            if matches!(*next, '-' | '/') && chars.get(end + 1).is_some_and(|c| c.is_ascii_digit())
            {
                return NumberKind::Date;
            }
        }
    }
    // 百分比（允许 `15 %`）
    let mut j = end;
    while j < chars.len() && chars[j] == ' ' {
        j += 1;
    }
    if chars.get(j) == Some(&'%') {
        return NumberKind::Percent;
    }
    // 时间 `14:30`
    if chars.get(end) == Some(&':') && chars.get(end + 1).is_some_and(|c| c.is_ascii_digit()) {
        return NumberKind::Time;
    }
    // 版本号：至少两段点号
    if raw.matches('.').count() >= 2 {
        return NumberKind::Version;
    }
    // 序数 `3rd`
    let suffix: String = chars[end..chars.len().min(end + 2)]
        .iter()
        .collect::<String>()
        .to_lowercase();
    if matches!(suffix.as_str(), "st" | "nd" | "rd" | "th") {
        return NumberKind::Ordinal;
    }
    let _ = start;
    NumberKind::Quantity
}

/// 找出区间表达（`10–20`、`10 至 20`、`between 10 and 20`）。
pub fn ranges(text: &str) -> Vec<RangeToken> {
    let tokens = tokenize(text);
    let mut out = Vec::new();

    for window in tokens.windows(2) {
        let (a, b) = (&window[0], &window[1]);
        let (Some(low), Some(high)) = (a.value, b.value) else {
            continue;
        };
        if !matches!(a.kind, NumberKind::Quantity) || !matches!(b.kind, NumberKind::Quantity) {
            continue;
        }
        // 两个 token 之间只允许出现分隔符/连接词
        let between = text
            .find(&a.raw)
            .zip(text.find(&b.raw))
            .map(|(x, y)| x < y)
            .unwrap_or(false);
        if !between {
            continue;
        }
        let a_end = text.find(&a.raw).map(|p| p + a.raw.len()).unwrap_or(0);
        let b_start = text.find(&b.raw).unwrap_or(0);
        if b_start < a_end {
            continue;
        }
        let filler = &text[a_end..b_start];
        let filler_clean: String = filler
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase();
        if matches!(
            filler_clean.as_str(),
            "-" | "–" | "—" | "~" | "至" | "到" | "and" | "to"
        ) {
            out.push(RangeToken {
                raw: format!("{}{}{}", a.raw, filler, b.raw),
                low,
                high,
            });
        }
    }
    out
}

/// 检查源文数字在译文里的完整性。
pub fn check(source: &str, output: &str) -> NumberCheck {
    let source_tokens = tokenize(source);
    let mut check = NumberCheck {
        in_source: source_tokens.len(),
        ..NumberCheck::default()
    };

    let output_values = output_values(output);
    let output_lower = output.to_lowercase();

    for token in &source_tokens {
        if token.kind.is_strict() {
            check.strict_in_source += 1;
        }

        let preserved = match token.kind {
            NumberKind::Identifier => {
                // 允许 `#10248` 或 `10248`（`#` 可能被本地化成"编号"）
                let digits: String = token.raw.chars().filter(|c| c.is_ascii_digit()).collect();
                let ok = output.contains(&token.raw) || output.contains(&digits);
                if !ok {
                    check.issues.push(QualityIssue::critical(
                        QualityIssueKind::StrictIdentifierMismatch,
                        format!("编号 {} 在译文中被改变或丢失", token.raw),
                    ));
                }
                ok
            }
            NumberKind::Version => {
                let core = token.raw.trim_start_matches(['v', 'V']);
                output.contains(&token.raw) || output.contains(core)
            }
            NumberKind::Currency => {
                let value_ok = token
                    .value
                    .is_some_and(|v| value_present(v, &output_values, output));
                let currency_ok = token
                    .currency
                    .as_deref()
                    .is_some_and(|sym| output.contains(sym) || currency_word_present(sym, output));
                if !value_ok {
                    check.issues.push(QualityIssue::major(
                        QualityIssueKind::NumberMissing,
                        format!("金额 {} 的数值未在译文中找到", token.raw),
                    ));
                } else if !currency_ok {
                    check.issues.push(QualityIssue::major(
                        QualityIssueKind::NumberMissing,
                        format!("金额 {} 的币种在译文中丢失", token.raw),
                    ));
                }
                value_ok
            }
            NumberKind::Percent => {
                let value_ok = token
                    .value
                    .is_some_and(|v| percent_present(v, &output_values, output));
                if !value_ok {
                    check.issues.push(QualityIssue::critical(
                        QualityIssueKind::StrictIdentifierMismatch,
                        format!("百分比 {} 未在译文里找到（数值被改变或丢失）", token.raw),
                    ));
                }
                value_ok
            }
            NumberKind::Date => {
                let ok = date_alternatives(&token.raw)
                    .iter()
                    .any(|form| output.contains(form.as_str()));
                if !ok {
                    check.issues.push(QualityIssue::critical(
                        QualityIssueKind::StrictIdentifierMismatch,
                        format!("日期 {} 未在译文里完整保留", token.raw),
                    ));
                }
                ok
            }
            NumberKind::Time => {
                let ok = time_alternatives(&token.raw)
                    .iter()
                    .any(|form| output.contains(form.as_str()));
                if !ok {
                    check.issues.push(QualityIssue::major(
                        QualityIssueKind::NumberMissing,
                        format!("时间 {} 未在译文里找到", token.raw),
                    ));
                }
                ok
            }
            NumberKind::Quantity | NumberKind::Ordinal => {
                let ok = token.value.is_some_and(|v| {
                    value_present(v, &output_values, output)
                        || chinese_forms(v)
                            .iter()
                            .any(|form| output.contains(form.as_str()))
                });
                if !ok {
                    check.issues.push(QualityIssue::major(
                        QualityIssueKind::NumberMissing,
                        format!("数量 {} 未在译文里找到", token.raw),
                    ));
                }
                ok
            }
        };

        if preserved {
            check.preserved += 1;
            if token.kind.is_strict() {
                check.strict_preserved += 1;
            }
        }
    }

    // 区间关系：上下限顺序与数值都必须保留
    for range in ranges(source) {
        let out_ranges = ranges(output);
        let matched = out_ranges
            .iter()
            .any(|r| approx_eq(r.low, range.low) && approx_eq(r.high, range.high));
        if !matched {
            // 只有译文里确实出现了区间时才判关系错误；完全缺失已由上面的数字检查覆盖
            if !out_ranges.is_empty() {
                check.issues.push(QualityIssue::critical(
                    QualityIssueKind::NumberRelationMismatch,
                    format!("区间 {} 的上下限在译文中被改变", range.raw),
                ));
            }
        }
    }

    let _ = output_lower;
    check
}

/// 译文里出现的数值（阿拉伯数字 + 中文数字）。
fn output_values(output: &str) -> Vec<f64> {
    let mut values: Vec<f64> = tokenize(output)
        .into_iter()
        .filter_map(|t| t.value)
        .collect();
    values.extend(chinese_numbers_in(output));
    values
}

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.005
}

fn value_present(value: f64, values: &[f64], output: &str) -> bool {
    if values.iter().any(|v| approx_eq(*v, value)) {
        return true;
    }
    // 处理 `1,250` 被写成 `1250`、`1 250` 的情况
    let trimmed = value.to_string();
    output.replace([',', ' '], "").contains(&trimmed)
}

fn percent_present(value: f64, values: &[f64], output: &str) -> bool {
    if output.contains('%') || output.contains('％') || output.contains("百分之") {
        if values.iter().any(|v| approx_eq(*v, value)) {
            return true;
        }
        if chinese_forms(value)
            .iter()
            .any(|form| output.contains(form.as_str()))
        {
            return true;
        }
    }
    false
}

/// 一个日期可能被写成的等价格式（只列"数值相同"的写法）。
fn date_alternatives(raw: &str) -> Vec<String> {
    let parts: Vec<&str> = raw.split(['-', '/']).collect();
    if parts.len() != 3 {
        return vec![raw.to_string()];
    }
    let (y, m, d) = (parts[0], parts[1], parts[2]);
    let m_num: u32 = m.parse().unwrap_or(0);
    let d_num: u32 = d.parse().unwrap_or(0);
    let mut out = vec![
        raw.to_string(),
        format!("{y}年{m_num}月{d_num}日"),
        format!("{y} 年 {m_num} 月 {d_num} 日"),
        format!("{y}/{m}"),
    ];
    // `2026/11/11` 与 `2026-11-11` 只差分隔符
    out.push(format!("{y}/{m}/{d}"));
    out.push(format!("{y}.{m}.{d}"));
    out
}

/// 一个时间可能被写成的等价格式（含 12 小时制的下午写法）。
fn time_alternatives(raw: &str) -> Vec<String> {
    let Some((h, m)) = raw.split_once(':') else {
        return vec![raw.to_string()];
    };
    let (Ok(hour), Ok(minute)) = (h.parse::<u32>(), m.parse::<u32>()) else {
        return vec![raw.to_string()];
    };
    let mut out = vec![
        raw.to_string(),
        format!("{hour}时{minute:02}分"),
        format!("{hour}时{minute}分"),
        format!("{hour}点{minute:02}分"),
        format!("{hour}点{minute}分"),
        format!("{hour}:{minute:02}"),
    ];
    // 中文常用口语：`两点半`、`九点一刻`、`差一刻十点`
    let hour_cn: Vec<String> = chinese_forms(hour as f64);
    for form in &hour_cn {
        out.push(format!("{form}点{minute:02}分"));
        out.push(format!("{form}点{minute}分"));
    }
    if minute == 30 {
        out.push(format!("{hour}点半"));
        for form in &hour_cn {
            out.push(format!("{form}点半"));
            out.push(format!("{form}时30分"));
        }
    }
    if minute == 15 {
        out.push(format!("{hour}点一刻"));
        for form in &hour_cn {
            out.push(format!("{form}点一刻"));
        }
    }
    if minute == 45 {
        let next = if hour == 23 { 24 } else { hour + 1 };
        out.push(format!("差一刻{next}点"));
    }
    if hour < 12 {
        // 上午/凌晨：12 小时制下小时数不变，`2:00 AM` → `凌晨2点`
        for prefix in ["上午", "凌晨", "早上"] {
            out.push(format!("{prefix}{hour}点{minute:02}分"));
            out.push(format!("{prefix} {hour}点{minute:02}分"));
            out.push(format!("{prefix}{hour}点"));
            out.push(format!("{prefix}{hour}点半"));
        }
    }
    if hour >= 12 {
        let h12 = if hour == 12 { 12 } else { hour - 12 };
        for prefix in ["下午", "傍晚", "晚上"] {
            out.push(format!("{prefix}{h12}:{minute:02}"));
            out.push(format!("{prefix} {h12}:{minute:02}"));
            out.push(format!("{prefix}{h12}点{minute:02}分"));
            out.push(format!("{prefix} {h12}点{minute:02}分"));
            out.push(format!("{prefix}{h12}点半"));
        }
    }
    out
}

/// 币种代码表：代码 → (规范名, 符号)。
fn currency_code(code: &str) -> Option<(&'static str, &'static str)> {
    match code {
        "USD" => Some(("USD", "$")),
        "EUR" => Some(("EUR", "€")),
        "GBP" => Some(("GBP", "£")),
        "CNY" | "RMB" => Some(("CNY", "¥")),
        "JPY" => Some(("JPY", "¥")),
        "INR" => Some(("INR", "₹")),
        "RUB" => Some(("RUB", "₽")),
        "KRW" => Some(("KRW", "₩")),
        "TRY" => Some(("TRY", "₺")),
        "BTC" => Some(("BTC", "₿")),
        "ETH" => Some(("ETH", "Ξ")),
        _ => None,
    }
}

/// `USD 39.99`：代码后面跟数字。
fn number_after_code(chars: &[char], from: usize) -> Option<(f64, usize)> {
    let mut j = from;
    while j < chars.len() && chars[j] == ' ' {
        j += 1;
    }
    if j >= chars.len() || !chars[j].is_ascii_digit() {
        return None;
    }
    let (value, end, _) = parse_number(chars, j);
    value.map(|value| (value, end))
}

/// 数字**后面**是否紧跟币种代码（`39.99 USD`）。
fn currency_code_after(chars: &[char], from: usize) -> Option<(&'static str, &'static str)> {
    let mut j = from;
    while j < chars.len() && chars[j] == ' ' {
        j += 1;
    }
    if j >= chars.len() || !chars[j].is_ascii_alphabetic() {
        return None;
    }
    let start = j;
    while j < chars.len() && chars[j].is_ascii_alphabetic() {
        j += 1;
    }
    let code: String = chars[start..j].iter().collect::<String>().to_uppercase();
    currency_code(&code)
}

/// 译文里是否出现了该币种（符号或中文/代码写法）。
fn currency_word_present(symbol: &str, output: &str) -> bool {
    // 复合写法：`USD|$`（来自代码分支）
    if let Some((code, sym)) = symbol.split_once('|') {
        return currency_word_present(sym, output) || output.contains(code);
    }
    match symbol {
        "$" => ["美元", "美金", "USD", "刀", "元", "块"]
            .iter()
            .any(|w| output.contains(w)),
        "€" => ["欧元", "EUR"].iter().any(|w| output.contains(w)),
        "£" => ["英镑", "GBP"].iter().any(|w| output.contains(w)),
        "¥" => ["元", "人民币", "日元", "CNY", "JPY", "RMB"]
            .iter()
            .any(|w| output.contains(w)),
        "₹" => ["卢比", "INR"].iter().any(|w| output.contains(w)),
        "₽" => ["卢布", "RUB"].iter().any(|w| output.contains(w)),
        "₩" => ["韩元", "KRW"].iter().any(|w| output.contains(w)),
        "₺" => ["里拉", "TRY"].iter().any(|w| output.contains(w)),
        "₿" => ["比特币", "BTC"].iter().any(|w| output.contains(w)),
        "Ξ" => ["以太", "以太坊", "ETH"].iter().any(|w| output.contains(w)),
        _ => false,
    }
}

// ============================================================
// 中文数字
// ============================================================

const CN_DIGITS: [char; 10] = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
const CN_UNITS: [char; 4] = [' ', '十', '百', '千'];
const CN_BIG: [char; 3] = [' ', '万', '亿'];

fn cn_digit(c: char) -> Option<u64> {
    match c {
        '零' | '〇' => Some(0),
        '一' => Some(1),
        '二' | '两' | '兩' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    }
}

fn is_cn_number_char(c: char) -> bool {
    cn_digit(c).is_some() || matches!(c, '十' | '百' | '千' | '万' | '亿')
}

/// 一个整数转成中文数字（0 < n < 1e8）。
fn int_to_chinese(n: u64) -> String {
    if n == 0 {
        return "零".to_string();
    }
    let groups: Vec<u64> = {
        let mut n = n;
        let mut out = Vec::new();
        while n > 0 {
            out.push(n % 10_000);
            n /= 10_000;
        }
        out
    };

    let mut result = String::new();
    for (idx, group) in groups.iter().enumerate().rev() {
        if *group == 0 {
            continue;
        }
        let big = CN_BIG.get(idx).copied().unwrap_or(' ');
        let section = four_digits_to_chinese(*group);
        // 高组之后若低组不足千位，需要补"零"
        if !result.is_empty() && *group < 1000 {
            result.push('零');
        }
        result.push_str(&section);
        if big != ' ' {
            result.push(big);
        }
    }
    result
}

fn four_digits_to_chinese(n: u64) -> String {
    let mut out = String::new();
    let mut pending_zero = false;
    let mut started = false;
    for (pos, unit) in CN_UNITS.iter().enumerate().rev() {
        let d = (n / 10u64.pow(pos as u32)) % 10;
        if d == 0 {
            if started {
                pending_zero = true;
            }
            continue;
        }
        if pending_zero {
            out.push('零');
            pending_zero = false;
        }
        // 十位上的一省略（十五，而不是一十五）
        if !(pos == 1 && d == 1 && !started) {
            out.push(CN_DIGITS[d as usize]);
        }
        if *unit != ' ' {
            out.push(*unit);
        }
        started = true;
    }
    if out.is_empty() {
        "零".to_string()
    } else {
        out
    }
}

/// 生成一个数值可能被写成的中文形式（用于"等价"判定，不是唯一正确答案）。
pub fn chinese_forms(value: f64) -> Vec<String> {
    let mut out = Vec::new();
    if value.fract().abs() < f64::EPSILON && (0.0..100_000_000.0).contains(&value) {
        let n = value as u64;
        out.push(int_to_chinese(n));
        // `两` 的变体（两小时、两千）
        if n == 2 {
            out.push("两".to_string());
        }
        if (2..10).contains(&n) {
            out.push(format!("两{}", int_to_chinese(n)));
        }
    } else if (value - 0.5).abs() < f64::EPSILON {
        // `半` 是最常见的小数写法（`一半`/`两个半`）
        out.push("半".to_string());
        out.push("零点五".to_string());
    } else if (value.fract() - 0.5).abs() < f64::EPSILON && value >= 1.0 {
        let whole = (value - 0.5) as u64;
        out.push(format!("{}个半", int_to_chinese(whole)));
        out.push(format!("{}半", int_to_chinese(whole)));
        out.push(format!("{}点五", int_to_chinese(whole)));
    } else if value >= 0.0 {
        // 小数：整数部分 + 点 + 逐位小数
        let text = format!("{value}");
        if let Some((int_part, frac_part)) = text.split_once('.') {
            if let Ok(int_value) = int_part.parse::<u64>() {
                let mut s = if int_value == 0 {
                    String::new()
                } else {
                    int_to_chinese(int_value)
                };
                s.push('点');
                for c in frac_part.chars() {
                    if let Some(d) = c.to_digit(10) {
                        s.push(CN_DIGITS[d as usize]);
                    }
                }
                out.push(s);
            }
        }
    }
    out
}

/// 解析中文数字（`十五`、`二十`、`一百零五`、`三千五百`）。
pub fn parse_chinese_number(text: &str) -> Option<f64> {
    let mut total = 0u64;
    let mut section = 0u64;
    let mut current: Option<u64> = None;
    let mut saw_any = false;
    let mut used_unit = false;
    // `半` 是后缀：`一半` / `两个半` / `半`。先把它摘掉，
    // 剩下的部分按普通中文数字解析（量词如 `个` 会自然中断解析）。
    let half = text.contains('半');
    let body: String = text.chars().filter(|c| *c != '半').collect();

    for c in body.chars() {
        if let Some(d) = cn_digit(c) {
            current = Some(current.unwrap_or(0) * 10 + d);
            saw_any = true;
            continue;
        }
        used_unit = true;
        if let Some(mult) = match c {
            '十' => Some(10u64),
            '百' => Some(100),
            '千' => Some(1_000),
            _ => None,
        } {
            section += current.unwrap_or(1) * mult;
            current = None;
            saw_any = true;
            continue;
        }
        if c == '万' {
            section += current.unwrap_or(0);
            total += section.max(1) * 10_000;
            section = 0;
            current = None;
            saw_any = true;
            continue;
        }
        if c == '亿' {
            section += current.unwrap_or(0);
            total = (total + section.max(1)) * 100_000_000;
            section = 0;
            current = None;
            saw_any = true;
            continue;
        }
        if saw_any {
            break;
        }
    }

    if !saw_any && !half {
        return None;
    }
    let mut base = (total + section + current.unwrap_or(0)) as f64;
    if half {
        // `一半` 是 0.5（"一个半" 才是 1.5）；`两个半` 是 2.5。
        // 判断依据：只有"一"且没有量级单位时，按"半个"理解。
        let only_one = base == 1.0 && !used_unit;
        if only_one {
            base = 0.0;
        }
        base += 0.5;
    }
    Some(base)
}

/// 提取文本里所有中文数字的数值。
pub fn chinese_numbers_in(text: &str) -> Vec<f64> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if !is_cn_number_char(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_cn_number_char(chars[i]) {
            i += 1;
        }
        let slice: String = chars[start..i].iter().collect();
        if let Some(v) = parse_chinese_number(&slice) {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, NumberKind)> {
        tokenize(text)
            .into_iter()
            .map(|t| (t.raw, t.kind))
            .collect()
    }

    #[test]
    fn 分类覆盖关键类型() {
        let tokens = kinds("Order #10248 on 2026-11-11 at 14:30 costs $39.99, 15% off, v2.1.3.");
        let find = |kind: NumberKind| {
            tokens
                .iter()
                .filter(|(_, k)| *k == kind)
                .map(|(r, _)| r.clone())
                .collect::<Vec<_>>()
        };
        assert!(find(NumberKind::Identifier).contains(&"#10248".to_string()));
        assert!(find(NumberKind::Date).contains(&"2026-11-11".to_string()));
        assert!(find(NumberKind::Time).contains(&"14:30".to_string()));
        assert!(find(NumberKind::Currency).contains(&"$39.99".to_string()));
        assert!(find(NumberKind::Percent).contains(&"15".to_string()));
        assert!(find(NumberKind::Version).contains(&"2.1.3".to_string()));
    }

    #[test]
    fn 编号严格保留_短写也接受() {
        let ok = check("Order #10248 shipped.", "订单 #10248 已发货。");
        assert!(ok.issues.is_empty(), "{:?}", ok.issues);
        assert_eq!(ok.strict_preserved, 1);

        let digits_only = check("Order #10248 shipped.", "订单 10248 已发货。");
        assert!(digits_only.issues.is_empty(), "{:?}", digits_only.issues);
    }

    #[test]
    fn 编号被改必须致命() {
        let check = check("Order #10248 shipped.", "订单 #10249 已发货。");
        assert!(check.has_fatal(), "{:?}", check.issues);
    }

    #[test]
    fn 百分比支持中文写法且能识别被改() {
        let chinese = check("Save 15% today.", "今天省百分之十五。");
        assert!(!chinese.has_fatal(), "{:?}", chinese.issues);

        let arabic = check("Save 15% today.", "今天省 15%。");
        assert!(!arabic.has_fatal(), "{:?}", arabic.issues);

        let wrong = check("Save 15% today.", "今天省 50%。");
        assert!(wrong.has_fatal(), "{:?}", wrong.issues);
    }

    #[test]
    fn 数量允许中英文数字互转() {
        let chinese = check("I have three books.", "我有三本书。");
        assert!(!chinese.has_fatal(), "{:?}", chinese.issues);
        assert!(chinese
            .issues
            .iter()
            .all(|i| i.severity != QualitySeverity::Critical));

        let arabic = check("I have three books.", "我有 3 本书。");
        assert!(!arabic.has_fatal(), "{:?}", arabic.issues);

        let missing = check("I have three books.", "我有书。");
        assert!(missing
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NumberMissing));
        assert!(!missing.has_fatal(), "普通数量丢失不该致命");
    }

    #[test]
    fn 金额校验数值与币种() {
        let ok = check("It costs $39.99.", "价格是 39.99 美元。");
        assert!(!ok.has_fatal(), "{:?}", ok.issues);
        assert!(ok.issues.is_empty(), "{:?}", ok.issues);

        let lost_currency = check("It costs $39.99.", "价格是 39.99。");
        assert!(lost_currency
            .issues
            .iter()
            .any(|i| i.detail.contains("币种")));

        let wrong_value = check("It costs $39.99.", "价格是 93.99 美元。");
        assert!(wrong_value
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NumberMissing));
    }

    #[test]
    fn 日期允许等价格式() {
        let iso = check("Due on 2026-11-11.", "截止 2026-11-11。");
        assert!(iso.issues.is_empty(), "{:?}", iso.issues);
        let chinese = check("Due on 2026-11-11.", "截止 2026年11月11日。");
        assert!(chinese.issues.is_empty(), "{:?}", chinese.issues);
        let changed = check("Due on 2026-11-11.", "截止 2026年12月11日。");
        assert!(changed.has_fatal(), "{:?}", changed.issues);
    }

    #[test]
    fn 时间支持上下午换算() {
        let direct = check("Starts at 14:30.", "14:30 开始。");
        assert!(direct.issues.is_empty(), "{:?}", direct.issues);
        let converted = check("Starts at 14:30.", "下午 2:30 开始。");
        assert!(converted.issues.is_empty(), "{:?}", converted.issues);
    }

    #[test]
    fn 上午与凌晨写法也等价() {
        let morning = check("The backup finished at 2:00 AM.", "备份在凌晨2点完成。");
        assert!(morning.issues.is_empty(), "{:?}", morning.issues);
        let nine = check("The meeting starts at 9:00.", "会议上午9点开始。");
        assert!(nine.issues.is_empty(), "{:?}", nine.issues);
    }

    #[test]
    fn 区间上下限关系必须保持() {
        let ok = check("Takes between 10 and 20 minutes.", "需要 10 至 20 分钟。");
        assert!(!ok.has_fatal(), "{:?}", ok.issues);

        let swapped = check("Takes between 10 and 20 minutes.", "需要 20 至 30 分钟。");
        assert!(swapped.has_fatal(), "{:?}", swapped.issues);
    }

    #[test]
    fn 中文数字解析() {
        assert_eq!(parse_chinese_number("十五"), Some(15.0));
        assert_eq!(parse_chinese_number("二十"), Some(20.0));
        assert_eq!(parse_chinese_number("一百零五"), Some(105.0));
        assert_eq!(parse_chinese_number("三千五百"), Some(3500.0));
        assert_eq!(parse_chinese_number("两"), Some(2.0));
        assert_eq!(parse_chinese_number("一万二千"), Some(12000.0));
        assert_eq!(parse_chinese_number("零"), Some(0.0));
        assert_eq!(parse_chinese_number("你好"), None);
    }

    #[test]
    fn 中文数字生成() {
        assert_eq!(int_to_chinese(0), "零");
        assert_eq!(int_to_chinese(10), "十");
        assert_eq!(int_to_chinese(15), "十五");
        assert_eq!(int_to_chinese(20), "二十");
        assert_eq!(int_to_chinese(105), "一百零五");
        assert_eq!(int_to_chinese(1250), "一千二百五十");
        assert_eq!(int_to_chinese(12000), "一万二千");
        assert!(chinese_forms(2.0).contains(&"两".to_string()));
        assert!(chinese_forms(39.99).iter().any(|s| s.contains('点')));
    }

    #[test]
    fn 序数用语义值比较() {
        let ok = check("This is the third attempt.", "这是第三次尝试。");
        assert!(!ok.has_fatal(), "{:?}", ok.issues);
        let wrong = check("This is the third attempt.", "这是第五次尝试。");
        assert!(wrong
            .issues
            .iter()
            .any(|i| i.kind == QualityIssueKind::NumberMissing));
    }

    #[test]
    fn 千分位与空格千分位等价() {
        let check = check("Total 1,250 USD.", "总计 1250 美元。");
        assert!(!check.has_fatal(), "{:?}", check.issues);
    }

    #[test]
    fn 序数词只在真正表序数时才校验() {
        // `every second` 是时间单位：不该要求译文出现"第二"
        let unit = check(
            "It handles thousands of tasks every second.",
            "每秒处理数千个任务。",
        );
        assert!(unit.issues.is_empty(), "{:?}", unit.issues);

        // `finishes first` 是副词
        let adverb = check("Whoever finishes first should help.", "谁先完成谁帮忙。");
        assert!(adverb.issues.is_empty(), "{:?}", adverb.issues);

        // `First, ...` 是连接词
        let conjunction = check("First, open the panel.", "首先，打开面板。");
        assert!(conjunction.issues.is_empty(), "{:?}", conjunction.issues);

        // 真正的序数仍然校验
        let ordinal = check("This is the second attempt.", "这是第二次尝试。");
        assert!(ordinal.issues.is_empty(), "{:?}", ordinal.issues);

        let wrong = check("This is the second attempt.", "这是第三次尝试。");
        assert!(
            wrong.issues.iter().any(|i| i.detail.contains("second")),
            "{:?}",
            wrong.issues
        );
    }

    #[test]
    fn 半与口语时间写法可识别() {
        assert_eq!(parse_chinese_number("半"), Some(0.5));
        assert_eq!(parse_chinese_number("一半"), Some(0.5));
        assert_eq!(parse_chinese_number("两个半"), Some(2.5));
        assert!(chinese_forms(0.5).contains(&"半".to_string()));

        let half_hour = check("The meeting starts at 2:30.", "会议两点半开始。");
        assert!(half_hour.issues.is_empty(), "{:?}", half_hour.issues);

        let quarter = check("The meeting starts at 9:15.", "会议九点一刻开始。");
        assert!(quarter.issues.is_empty(), "{:?}", quarter.issues);

        let wrong = check("The meeting starts at 2:30.", "会议三点半开始。");
        assert!(!wrong.issues.is_empty(), "时间被改必须报告");
    }

    #[test]
    fn 币种代码与加密货币可识别() {
        let code_prefix = check("It costs USD 39.99.", "价格 39.99 美元。");
        assert!(!code_prefix.has_fatal(), "{:?}", code_prefix.issues);
        assert!(code_prefix.issues.is_empty(), "{:?}", code_prefix.issues);

        let code_suffix = check("It costs 39.99 EUR.", "价格 39.99 欧元。");
        assert!(code_suffix.issues.is_empty(), "{:?}", code_suffix.issues);

        let crypto = check("Paid 0.5 BTC today.", "今天支付 0.5 比特币。");
        assert!(crypto.issues.is_empty(), "{:?}", crypto.issues);

        let rupee = check("Paid ₹500 now.", "现在支付 500 卢比。");
        assert!(rupee.issues.is_empty(), "{:?}", rupee.issues);

        // 币种丢失仍要报告
        let lost = check("It costs USD 39.99.", "价格 39.99。");
        assert!(lost.issues.iter().any(|i| i.detail.contains("币种")));
    }

    #[test]
    fn 没有数字时检查是空的() {
        let check = check("Nothing numeric here.", "这里没有数字。");
        assert_eq!(check.in_source, 0);
        assert!(check.issues.is_empty());
    }
}
