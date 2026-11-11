//! 统一文本保护：**要么完整恢复，要么显式失败**。
//!
//! 这是重构中优先级最高的可靠性模块（§4）。旧实现有两条互不相干的保护路径
//! （`text::protect` 的宽松替换 + `protection::ProtectionPlan` 的分段策略），
//! 主链实际只用前者，而前者**不做**严格校验。现在只保留这一套：
//! [`ProtectionPlan`]。
//!
//! # 策略：默认整句标记，必要时分段
//!
//! | 策略 | 做法 | 优点 | 代价 |
//! |---|---|---|---|
//! | [`ProtectionStrategy::Marker`]（默认） | 受保护片段换成 `⟦P1⟧`，整句翻译，译后严格还原 | 上下文完整，语序自然 | 依赖模型不吞标记 |
//! | [`ProtectionStrategy::Segment`]（回退） | 受保护片段根本不进模型，只翻译普通片段再拼回 | 标记丢失在结构上不可能发生 | 句子被切开，连贯性下降 |
//!
//! 旧实测（`docs/18` §2.6）显示**没有任何一种标记形态是 100% 可靠的**，
//! 因此默认路径必须配"严格校验 + 一次受限重试 + 分段回退"，而不是"相信模型"：
//!
//! 1. 标记缺失、重复、未知、格式被破坏 → [`ProtectionError`]，转成
//!    `IntegrityFailure`，**不得**当作成功结果返回或写缓存；
//! 2. 协调器可以在同一请求内做一次受限重试（换分段策略或加纠正提示）；
//! 3. 仍失败就报错，不返回"看起来正常"的半成品（§4.3）。
//!
//! # 不变量
//!
//! 1. 受保护片段的字节**原样**出现（`{var}` 连花括号一起、`` `code` `` 连反引号一起）；
//! 2. 数量与身份一致：不丢失、不多出、不串换；
//! 3. 允许语序调整（标记可以换位置），不允许替换内容；
//! 4. 失败返回错误，不返回半成品；
//! 5. **输入里本来就含有标记形态时**必须换一种标记（或回退分段），
//!    否则用户文本会被误认为我们自己的标记（§4.4 最后一句）。

use std::fmt;

use translator_types::{TranslationError, TranslationErrorCode};

use crate::text::{protected_spans_with, ProtectedKind, SpanPolicy};

/// 保护模块的行为版本：进入缓存身份。改行为必须改这个串。
pub const PROTECTION_VERSION: &str = "protect-v2";

/// 实际采用的保护策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectionStrategy {
    /// 整句翻译 + 标记还原（默认）。
    Marker,
    /// 受保护片段不进模型，只翻译普通片段。
    Segment,
}

impl ProtectionStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Marker => "marker",
            Self::Segment => "segment",
        }
    }
}

/// 调用方对策略的**偏好**（`Auto` 才允许按碰撞情况回退）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrategyPreference {
    /// 先标记，标记与输入冲突时自动改分段。
    #[default]
    Auto,
    /// 强制标记；与输入冲突时报错（不静默改策略）。
    Marker,
    /// 强制分段。
    Segment,
}

/// 保护策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtectionPolicy {
    pub preference: StrategyPreference,
    pub spans: SpanPolicy,
}

impl Default for ProtectionPolicy {
    fn default() -> Self {
        Self {
            preference: StrategyPreference::Auto,
            spans: SpanPolicy::default(),
        }
    }
}

impl ProtectionPolicy {
    /// 文档/代码场景：额外保护路径与标识符。
    pub fn code_like() -> Self {
        Self {
            preference: StrategyPreference::Auto,
            spans: SpanPolicy::everything(),
        }
    }

    /// 强制分段（模型标记可靠性不足时的回退）。
    pub fn segment_only() -> Self {
        Self {
            preference: StrategyPreference::Segment,
            spans: SpanPolicy::default(),
        }
    }
}

/// 一条受保护片段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedSpan {
    /// 1 起的编号。
    pub id: u32,
    pub kind: ProtectedKind,
    /// 在**源文**（已规范化的文本）里的字节区间。
    pub start: usize,
    pub end: usize,
    /// 原文片段（恢复时逐字节贴回）。
    pub original: String,
}

impl ProtectedSpan {
    pub fn kind_str(&self) -> &'static str {
        self.kind.as_str()
    }
}

/// 分段策略的组成：普通文本段与受保护片段交替。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// 需要送进模型的普通文本（可能是空串，调用方可跳过）。
    Plain(String),
    /// 受保护片段（引用 [`ProtectedSpan::id`]）。
    Protected(u32),
    /// 原样保留、**不进模型**的文本（换行等）。
    Literal(String),
}

/// 一种标记形态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MarkerFormat {
    open: &'static str,
    close: &'static str,
}

impl MarkerFormat {
    fn token(self, id: u32) -> String {
        format!("{}{}{}", self.open, id, self.close)
    }
}

/// 候选标记形态，按"模型最容易原样保留"排序。
///
/// 选数学括号 `⟦⟧` 而不是 `{}`/`[]`：后者会与模板变量、Markdown 链接混淆，
/// 模型也更倾向于"顺手规范化"它们。
const MARKERS: [MarkerFormat; 4] = [
    MarkerFormat {
        open: "\u{27E6}P",
        close: "\u{27E7}",
    },
    MarkerFormat {
        open: "\u{3010}P",
        close: "\u{3011}",
    },
    MarkerFormat {
        open: "\u{00AB}P",
        close: "\u{00BB}",
    },
    MarkerFormat {
        open: "#P",
        close: "#",
    },
];

/// 保护计划：源文 → 送模型的文本 + 还原规则。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectionPlan {
    source: String,
    strategy: ProtectionStrategy,
    spans: Vec<ProtectedSpan>,
    pieces: Vec<Piece>,
    masked: String,
    marker: MarkerFormat,
}

impl ProtectionPlan {
    /// 按策略构造计划。
    pub fn prepare(source: &str, policy: &ProtectionPolicy) -> Result<Self, ProtectionError> {
        let found = protected_spans_with(source, &policy.spans);
        let spans: Vec<ProtectedSpan> = found
            .into_iter()
            .enumerate()
            .map(|(idx, span)| ProtectedSpan {
                id: (idx + 1) as u32,
                kind: span.kind,
                start: span.start,
                end: span.end,
                original: source[span.start..span.end].to_string(),
            })
            .collect();

        let strategy = match policy.preference {
            StrategyPreference::Segment => ProtectionStrategy::Segment,
            StrategyPreference::Marker => ProtectionStrategy::Marker,
            StrategyPreference::Auto => {
                if spans.is_empty() {
                    // 没有受保护片段时标记/分段等价，统一记 Marker 以免缓存身份抖动
                    ProtectionStrategy::Marker
                } else if MARKERS.iter().any(|m| !source.contains(m.open)) {
                    ProtectionStrategy::Marker
                } else {
                    // 输入本身含所有候选标记形态：只能分段，否则用户文本会被当成标记
                    ProtectionStrategy::Segment
                }
            }
        };

        let marker = if strategy == ProtectionStrategy::Marker {
            *MARKERS
                .iter()
                .find(|m| !source.contains(m.open))
                .ok_or_else(|| {
                    ProtectionError::MarkerCollision("源文包含全部候选标记形态".to_string())
                })?
        } else {
            MARKERS[0]
        };

        let mut pieces = Vec::new();
        let mut masked = String::with_capacity(source.len() + spans.len() * 8);
        let mut cursor = 0usize;

        for span in &spans {
            push_plain(&mut pieces, &source[cursor..span.start]);
            pieces.push(Piece::Protected(span.id));
            masked.push_str(&source[cursor..span.start]);
            masked.push_str(&marker.token(span.id));
            cursor = span.end;
        }
        push_plain(&mut pieces, &source[cursor..]);
        masked.push_str(&source[cursor..]);

        Ok(Self {
            source: source.to_string(),
            strategy,
            spans,
            pieces,
            masked,
            marker,
        })
    }

    /// 便捷构造：默认策略。
    pub fn build(input: &str) -> Self {
        Self::prepare(input, &ProtectionPolicy::default())
            .expect("默认策略不会因标记碰撞失败（Auto 会回退分段）")
    }

    /// 强制标记策略（与输入冲突时报错）。
    pub fn build_marker(input: &str) -> Result<Self, ProtectionError> {
        Self::prepare(
            input,
            &ProtectionPolicy {
                preference: StrategyPreference::Marker,
                spans: SpanPolicy::default(),
            },
        )
    }

    /// 强制分段策略。
    pub fn build_segment(input: &str) -> Self {
        Self::prepare(input, &ProtectionPolicy::segment_only()).expect("分段策略不依赖标记")
    }

    pub fn strategy(&self) -> ProtectionStrategy {
        self.strategy
    }

    pub fn spans(&self) -> &[ProtectedSpan] {
        &self.spans
    }

    /// 兼容旧名字（`items`）。
    pub fn items(&self) -> &[ProtectedSpan] {
        &self.spans
    }

    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    /// 送给模型的文本（标记策略下是打了标记的整句）。
    pub fn masked(&self) -> &str {
        &self.masked
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn is_trivial(&self) -> bool {
        self.spans.is_empty()
    }

    /// 文本中出现的标记 token（供提示词逐字告知模型）。
    pub fn marker_tokens(&self) -> Vec<String> {
        if self.strategy != ProtectionStrategy::Marker {
            return Vec::new();
        }
        self.spans
            .iter()
            .map(|span| self.marker.token(span.id))
            .collect()
    }

    /// 分段策略：需要翻译的普通文本，按出现顺序（含空串，调用方自行跳过）。
    pub fn plain_texts(&self) -> Vec<&str> {
        self.pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Plain(text) => Some(text.as_str()),
                Piece::Protected(_) | Piece::Literal(_) => None,
            })
            .collect()
    }

    /// 标记策略：严格还原。
    ///
    /// 校验内容（缺一不可）：
    ///
    /// - 每个 ID 恰好出现一次；
    /// - 不允许出现未知 ID；
    /// - 不允许出现"半截标记"（`⟦P1`、`P1⟧`）；
    /// - 不允许出现其它候选标记形态（说明模型改写了自己的输出）；
    /// - 还原后不允许残留标记起始符。
    pub fn restore(&self, output: &str) -> Result<String, ProtectionError> {
        if self.strategy != ProtectionStrategy::Marker {
            return Err(ProtectionError::WrongStrategy {
                expected: "marker",
                actual: self.strategy.as_str(),
            });
        }
        self.verify(output)?;

        let tokens = scan_markers(output, self.marker)?;
        let mut out = String::with_capacity(output.len());
        let mut cursor = 0usize;
        for token in &tokens {
            out.push_str(&output[cursor..token.start]);
            out.push_str(&self.span(token.id)?.original);
            cursor = token.end;
        }
        out.push_str(&output[cursor..]);

        // 还原后不得再出现标记起始符（否则说明有未被识别的标记残留）
        if out.contains(self.marker.open) {
            let excerpt: String = out
                .match_indices(self.marker.open)
                .next()
                .map(|(idx, _)| out[idx..].chars().take(8).collect())
                .unwrap_or_default();
            return Err(ProtectionError::UnknownMarker(excerpt));
        }

        // 标记策略允许语序调整，因此这里只校验"出现次数"，不校验顺序。
        self.verify_presence(&out)?;
        Ok(out)
    }

    /// 只校验、不还原：模型输出是否满足"每个标记恰好一次"。
    pub fn verify(&self, output: &str) -> Result<(), ProtectionError> {
        if self.strategy != ProtectionStrategy::Marker {
            return Ok(());
        }

        // 其它候选标记形态出现 → 模型改写了标记。
        // 例外：源文本身就含有那种形态（用户贴了 `⟦P1⟧`），那不是模型改写。
        for other in MARKERS
            .iter()
            .filter(|m| **m != self.marker && !self.source.contains(m.open))
        {
            if output.contains(other.open) {
                let excerpt: String = output
                    .match_indices(other.open)
                    .next()
                    .map(|(idx, _)| output[idx..].chars().take(8).collect())
                    .unwrap_or_default();
                return Err(ProtectionError::UnknownMarker(excerpt));
            }
        }

        let tokens = scan_markers(output, self.marker)?;
        let mut seen = vec![0usize; self.spans.len() + 1];
        for token in &tokens {
            let id = token.id;
            if id == 0 || id as usize > self.spans.len() {
                return Err(ProtectionError::UnknownMarker(self.marker.token(id)));
            }
            seen[id as usize] += 1;
        }
        for (id, count) in seen.iter().enumerate().skip(1) {
            if *count == 0 {
                return Err(ProtectionError::Missing {
                    id: id as u32,
                    kind: self.spans[id - 1].kind_str(),
                });
            }
            if *count > 1 {
                return Err(ProtectionError::Duplicated {
                    id: id as u32,
                    count: *count,
                });
            }
        }
        Ok(())
    }

    /// 分段策略：把逐段译文按原顺序拼回，并做结构自检。
    pub fn assemble(&self, translated: &[String]) -> Result<String, ProtectionError> {
        if self.strategy != ProtectionStrategy::Segment {
            return Err(ProtectionError::WrongStrategy {
                expected: "segment",
                actual: self.strategy.as_str(),
            });
        }
        let expected = self.plain_texts().len();
        if translated.len() != expected {
            return Err(ProtectionError::SegmentCount {
                expected,
                got: translated.len(),
            });
        }

        let mut iter = translated.iter();
        let mut out = String::with_capacity(self.source.len());
        for piece in &self.pieces {
            match piece {
                Piece::Plain(_) => {
                    if let Some(text) = iter.next() {
                        push_piece(&mut out, text);
                    }
                }
                Piece::Protected(id) => {
                    push_piece(&mut out, &self.span(*id)?.original);
                }
                Piece::Literal(text) => out.push_str(text),
            }
        }
        self.verify_content(&out)?;
        Ok(out)
    }

    /// 出现次数自检（允许语序调整）。
    fn verify_presence(&self, text: &str) -> Result<(), ProtectionError> {
        for span in &self.spans {
            let expected = self.source.matches(&span.original).count().max(1);
            let got = text.matches(&span.original).count();
            if got < expected {
                return Err(ProtectionError::Missing {
                    id: span.id,
                    kind: span.kind_str(),
                });
            }
        }
        Ok(())
    }

    /// 结构自检：每个受保护片段都在输出里、按原顺序。
    fn verify_content(&self, text: &str) -> Result<(), ProtectionError> {
        let mut cursor = 0usize;
        for span in &self.spans {
            let Some(offset) = text[cursor..].find(&span.original) else {
                return Err(ProtectionError::Missing {
                    id: span.id,
                    kind: span.kind_str(),
                });
            };
            cursor += offset + span.original.len();
        }
        Ok(())
    }

    fn span(&self, id: u32) -> Result<&ProtectedSpan, ProtectionError> {
        if id == 0 || id as usize > self.spans.len() {
            return Err(ProtectionError::UnknownMarker(self.marker.token(id)));
        }
        Ok(&self.spans[id as usize - 1])
    }
}

/// 保护失败：**任何**一种都必须变成 `IntegrityFailure`，不能静默返回译文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtectionError {
    Missing {
        id: u32,
        kind: &'static str,
    },
    Duplicated {
        id: u32,
        count: usize,
    },
    /// 未知标记 / 半截标记 / 模型改写的标记。
    UnknownMarker(String),
    SegmentCount {
        expected: usize,
        got: usize,
    },
    /// 源文本身就含有候选标记形态，且调用方强制要求标记策略。
    MarkerCollision(String),
    /// 用错策略调用（例如对分段计划调用 restore）。
    WrongStrategy {
        expected: &'static str,
        actual: &'static str,
    },
}

impl fmt::Display for ProtectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { id, kind } => {
                write!(f, "受保护片段 #{id}（{kind}）在译文里丢失")
            }
            Self::Duplicated { id, count } => {
                write!(f, "受保护片段 #{id} 在译文里出现了 {count} 次")
            }
            Self::UnknownMarker(marker) => {
                write!(f, "译文里出现未知/被破坏的标记：{marker}")
            }
            Self::SegmentCount { expected, got } => {
                write!(f, "分段译文数量不符：期望 {expected} 段，实际 {got} 段")
            }
            Self::MarkerCollision(msg) => write!(f, "保护标记与源文冲突：{msg}"),
            Self::WrongStrategy { expected, actual } => {
                write!(f, "保护策略不匹配：需要 {expected}，实际 {actual}")
            }
        }
    }
}

impl std::error::Error for ProtectionError {}

impl From<ProtectionError> for TranslationError {
    fn from(err: ProtectionError) -> Self {
        TranslationError::new(TranslationErrorCode::IntegrityFailure, err.to_string())
    }
}

/// 把一段普通文本拆成"可翻译片段 + 换行字面量"。
fn push_plain(pieces: &mut Vec<Piece>, text: &str) {
    if text.is_empty() {
        pieces.push(Piece::Plain(String::new()));
        return;
    }
    let mut start = 0usize;
    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            pieces.push(Piece::Plain(text[start..i].to_string()));
            pieces.push(Piece::Literal("\n".to_string()));
            start = i + 1;
        }
    }
    pieces.push(Piece::Plain(text[start..].to_string()));
}

/// 拼接片段时防止两个数字粘连（`10:30` + `2026` → `10:30 2026`）。
fn push_piece(out: &mut String, text: &str) {
    let last_digit = out
        .chars()
        .next_back()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false);
    let first_digit = text
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false);
    if last_digit && first_digit {
        out.push(' ');
    }
    out.push_str(text);
}

struct MarkerToken {
    start: usize,
    end: usize,
    id: u32,
}

/// 扫描标记。出现 `open` 却解析不出完整 token → 未知标记（比"忽略"安全）。
fn scan_markers(text: &str, marker: MarkerFormat) -> Result<Vec<MarkerToken>, ProtectionError> {
    let mut tokens = Vec::new();
    let mut search_from = 0usize;

    while let Some(rel) = text[search_from..].find(marker.open) {
        let start = search_from + rel;
        let after_open = start + marker.open.len();
        let digits_start = after_open;
        let mut cursor = after_open;

        while cursor < text.len() && text.as_bytes()[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor > digits_start && text[cursor..].starts_with(marker.close) {
            let id: u32 = text[digits_start..cursor].parse().unwrap_or(0);
            let end = cursor + marker.close.len();
            tokens.push(MarkerToken { start, end, id });
            search_from = end;
            continue;
        }

        let excerpt: String = text[start..].chars().take(10).collect();
        return Err(ProtectionError::UnknownMarker(excerpt));
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个"像真实模型"的假翻译器：会改词、会改标点，**不会**动标记
    /// （真实模型可能吞标记，那正是严格校验要抓的）。
    fn mock_translate(text: &str) -> String {
        let mut out = text.to_string();
        for (from, to) in [
            ("Hello", "您好"),
            ("your order", "您的订单"),
            ("has shipped", "已发货"),
            ("See", "参见"),
            ("for more details", "了解更多详情"),
            ("Run", "运行"),
            ("before committing", "在提交之前"),
            ("Contact", "联系"),
            ("if the error persists", "如果错误仍然存在"),
            ("Please visit", "请访问"),
            ("and enter", "并输入"),
            ("The file at", "文件"),
            ("was not found", "未找到"),
            ("Order", "订单"),
            ("was paid on", "付款日期为"),
        ] {
            out = out.replace(from, to);
        }
        out.trim().to_string()
    }

    fn marker_plan(input: &str) -> ProtectionPlan {
        ProtectionPlan::prepare(input, &ProtectionPolicy::default()).unwrap()
    }

    fn translate_with_markers(plan: &ProtectionPlan) -> String {
        plan.restore(&mock_translate(plan.masked()))
            .expect("应能还原")
    }

    fn translate_in_segments(plan: &ProtectionPlan) -> String {
        let parts: Vec<String> = plan
            .plain_texts()
            .into_iter()
            .map(|text| {
                if text.trim().is_empty() {
                    text.to_string()
                } else {
                    mock_translate(text)
                }
            })
            .collect();
        plan.assemble(&parts).expect("分段还原不该失败")
    }

    fn items_of(input: &str) -> Vec<String> {
        marker_plan(input)
            .items()
            .iter()
            .map(|i| i.original.clone())
            .collect()
    }

    // ---------- 基础：一个/多个/重复占位符 ----------

    #[test]
    fn sp01_单个模板变量完整保留() {
        let input = "Hello {name}, welcome.";
        assert_eq!(items_of(input), vec!["{name}"]);
        let out = translate_with_markers(&marker_plan(input));
        assert!(out.contains("{name}"), "{out}");
        assert_eq!(out.matches("{name}").count(), 1);
    }

    #[test]
    fn sp02_多个不同变量各自完整() {
        let input = "Hello {name}, your order {order_id} has shipped.";
        assert_eq!(items_of(input), vec!["{name}", "{order_id}"]);
        let out = translate_with_markers(&marker_plan(input));
        assert_eq!(out.matches("{name}").count(), 1, "{out}");
        assert_eq!(out.matches("{order_id}").count(), 1, "{out}");
    }

    #[test]
    fn sp03_重复变量按实例计数() {
        let input = "Hello {name}, bye {name}.";
        let plan = marker_plan(input);
        assert_eq!(plan.items().len(), 2, "两个实例必须是两个片段");
        let out = translate_with_markers(&plan);
        assert_eq!(out.matches("{name}").count(), 2, "{out}");
    }

    #[test]
    fn sp03b_相邻占位符不粘连() {
        let input = "Send {a}{b} now.";
        let plan = marker_plan(input);
        let out = translate_with_markers(&plan);
        assert!(out.contains("{a}{b}"), "{out}");
    }

    // ---------- URL / 邮箱 ----------

    #[test]
    fn sp04_url与带查询参数的url逐字节保留() {
        let input = "See https://example.com/docs for more details.";
        assert_eq!(items_of(input), vec!["https://example.com/docs"]);
        let out = translate_with_markers(&marker_plan(input));
        assert!(out.contains("https://example.com/docs"), "{out}");

        let query = "Open https://example.com/x?id=7&lang=en now.";
        let plan = marker_plan(query);
        let out = translate_with_markers(&plan);
        assert!(out.contains("https://example.com/x?id=7&lang=en"), "{out}");
    }

    #[test]
    fn sp04b_url里的数字不会被单独当成数字片段() {
        let input = "Open https://example.com/v2/10248 now.";
        let plan = marker_plan(input);
        assert_eq!(plan.items().len(), 1, "{:?}", plan.items());
        assert_eq!(plan.items()[0].original, "https://example.com/v2/10248");
    }

    #[test]
    fn sp05_email与混合结构精确保留() {
        let input = "Contact support@example.com if the error persists.";
        let out = translate_with_markers(&marker_plan(input));
        assert!(out.contains("support@example.com"), "{out}");

        let mixed = "Visit https://example.com or email hi@example.com about {plan}.";
        let plan = marker_plan(mixed);
        let kinds: Vec<&str> = plan.items().iter().map(|i| i.kind_str()).collect();
        assert_eq!(kinds, vec!["url", "email", "template_var"]);
        let out = translate_with_markers(&plan);
        for needle in ["https://example.com", "hi@example.com", "{plan}"] {
            assert_eq!(out.matches(needle).count(), 1, "{needle} in {out}");
        }
    }

    // ---------- 代码 ----------

    #[test]
    fn sp06_反引号代码整体保留含反引号本身() {
        let input = "Run `cargo test --all-features` before committing.";
        let out = translate_with_markers(&marker_plan(input));
        assert!(
            out.contains("`cargo test --all-features`"),
            "反引号必须还在：{out}"
        );
    }

    #[test]
    fn sp07_代码内嵌url与变量不重复保护() {
        let input = "Run `curl https://example.com/{id}` now.";
        let plan = marker_plan(input);
        assert_eq!(plan.items().len(), 1, "{:?}", plan.items());
        assert_eq!(plan.items()[0].kind_str(), "code");
        let out = translate_with_markers(&plan);
        assert!(out.contains("`curl https://example.com/{id}`"), "{out}");
    }

    // ---------- 失败必须显式 ----------

    #[test]
    fn sp08_伪造标记被判为未知标记() {
        let plan = marker_plan("Hello {name}, welcome.");
        let fake = "\u{27E6}P1\u{27E7} 你好 \u{27E6}P2\u{27E7}";
        let err = plan.restore(fake).unwrap_err();
        assert!(matches!(err, ProtectionError::UnknownMarker(_)), "{err:?}");
    }

    #[test]
    fn sp09_丢失或复制标记必须报错() {
        let plan = marker_plan("Hello {name}, your order {order_id} has shipped.");

        let missing = plan
            .restore("\u{27E6}P1\u{27E7} 您好,您的订单已发货。")
            .unwrap_err();
        assert!(
            matches!(missing, ProtectionError::Missing { id: 2, .. }),
            "{missing:?}"
        );

        let duplicated = plan
            .restore("\u{27E6}P1\u{27E7} 您好 \u{27E6}P1\u{27E7} 订单 \u{27E6}P2\u{27E7} 已发货")
            .unwrap_err();
        assert!(
            matches!(duplicated, ProtectionError::Duplicated { id: 1, count: 2 }),
            "{duplicated:?}"
        );
    }

    #[test]
    fn sp09b_标记被削掉一半必须报错() {
        let plan = marker_plan("Hello {name}, welcome.");
        let broken = plan.restore("\u{27E6}P1 你好").unwrap_err();
        assert!(
            matches!(broken, ProtectionError::UnknownMarker(_)),
            "{broken:?}"
        );
        let half = plan.restore("P1\u{27E7} 你好").unwrap_err();
        assert!(matches!(half, ProtectionError::Missing { .. }), "{half:?}");
    }

    #[test]
    fn sp09c_模型改写标记形态也算失败() {
        let plan = marker_plan("Hello {name}.");
        let rewritten = plan.restore("#P1# 你好。").unwrap_err();
        assert!(
            matches!(rewritten, ProtectionError::UnknownMarker(_)),
            "{rewritten:?}"
        );
    }

    #[test]
    fn sp09d_允许语序调整但不允许换内容() {
        let plan = marker_plan("Hello {name}, your order {order_id} has shipped.");
        let reordered = plan
            .restore("\u{27E6}P2\u{27E7} 您好 \u{27E6}P1\u{27E7} 已发货。")
            .unwrap();
        assert_eq!(reordered, "{order_id} 您好 {name} 已发货。");
    }

    // ---------- 边界输入 ----------

    #[test]
    fn sp10_utf8与emoji不破坏字节边界() {
        let input = "Order {id} 已发货 🚚, thanks!";
        assert_eq!(items_of(input), vec!["{id}"]);
        let out = translate_with_markers(&marker_plan(input));
        assert!(out.contains("{id}"), "{out}");
        assert!(out.contains('🚚'), "{out}");
        assert!(out.is_char_boundary(out.len()));
    }

    #[test]
    fn sp11_多行长文本逐行结构不变() {
        let input = "Line one has {a}.\nLine two has https://example.com/x.\nLine three is plain.";
        let plan = marker_plan(input);
        assert_eq!(plan.items().len(), 2);
        let out = translate_with_markers(&plan);
        assert!(out.contains("{a}"), "{out}");
        assert!(out.contains("https://example.com/x"), "{out}");
        assert_eq!(out.matches('\n').count(), 2, "换行结构必须保持：{out}");
    }

    #[test]
    fn sp11b_分段策略同样保持换行() {
        let input = "Line one has {a}.\nLine two has https://example.com/x.";
        let plan = ProtectionPlan::build_segment(input);
        let out = translate_in_segments(&plan);
        assert!(out.contains("{a}"), "{out}");
        assert!(out.contains("https://example.com/x"), "{out}");
        assert_eq!(out.matches('\n').count(), 1, "{out}");
    }

    #[test]
    fn sp12_空白输入与全保护输入不崩() {
        let empty = marker_plan("");
        assert!(empty.is_trivial());
        assert_eq!(empty.restore("").unwrap(), "");

        let whole = ProtectionPlan::build_segment("https://example.com/only");
        assert_eq!(whole.items().len(), 1);
        let out = whole.assemble(&["".to_string(), "".to_string()]).unwrap();
        assert_eq!(out, "https://example.com/only");

        let err = whole.assemble(&["".to_string()]).unwrap_err();
        assert!(
            matches!(err, ProtectionError::SegmentCount { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn sp13_分段数等于普通文本段数() {
        let plan = ProtectionPlan::build_segment("a {x} b {y} c");
        assert_eq!(plan.plain_texts(), vec!["a ", " b ", " c"]);
        assert_eq!(plan.pieces().len(), 5);
    }

    // ---------- 标记碰撞与策略 ----------

    #[test]
    fn sp14_源文含默认标记时自动换标记() {
        let input = "The log shows \u{27E6}P1\u{27E7} as a literal token, and {var} too.";
        let plan = ProtectionPlan::prepare(input, &ProtectionPolicy::default()).unwrap();
        assert_eq!(plan.strategy(), ProtectionStrategy::Marker);
        let tokens = plan.marker_tokens();
        assert_eq!(tokens.len(), 1);
        assert!(
            !tokens[0].starts_with('\u{27E6}'),
            "应换用其它标记：{tokens:?}"
        );
        // 原样送、原样还原：用户文本里的 `⟦P1⟧` 必须逐字保留，不能被当成我们的标记
        let out = plan.restore(plan.masked()).unwrap();
        assert_eq!(out, input);
        assert!(out.contains("\u{27E6}P1\u{27E7}"), "{out}");
        assert!(out.contains("{var}"), "{out}");
    }

    #[test]
    fn sp15_源文含全部候选标记时回退分段() {
        let input = "\u{27E6}P1\u{27E7} \u{3010}P2\u{3011} \u{00AB}P3\u{00BB} #P4# {x}";
        let plan = ProtectionPlan::prepare(input, &ProtectionPolicy::default()).unwrap();
        assert_eq!(plan.strategy(), ProtectionStrategy::Segment);
        assert!(plan.marker_tokens().is_empty());
    }

    #[test]
    fn sp16_强制标记遇到碰撞报错而不是静默改策略() {
        let input = "\u{27E6}P1\u{27E7} \u{3010}P2\u{3011} \u{00AB}P3\u{00BB} #P4#";
        let err = ProtectionPlan::prepare(
            input,
            &ProtectionPolicy {
                preference: StrategyPreference::Marker,
                spans: SpanPolicy::default(),
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ProtectionError::MarkerCollision(_)),
            "{err:?}"
        );
    }

    #[test]
    fn sp17_策略用错时报错而不是给半成品() {
        let plan = ProtectionPlan::build_segment("a {x} b");
        assert!(matches!(
            plan.restore("任意译文").unwrap_err(),
            ProtectionError::WrongStrategy { .. }
        ));
        let marker = marker_plan("a {x} b");
        assert!(matches!(
            marker.assemble(&["a".into(), "b".into()]).unwrap_err(),
            ProtectionError::WrongStrategy { .. }
        ));
    }

    #[test]
    fn sp18_纯文本没有标记时还原是恒等() {
        let plan = marker_plan("Nothing to protect here.");
        assert!(plan.is_trivial());
        assert_eq!(plan.masked(), "Nothing to protect here.");
        assert_eq!(
            plan.restore("这里没有什么要保护的。").unwrap(),
            "这里没有什么要保护的。"
        );
    }

    #[test]
    fn sp19_格式化占位符与百分比共存() {
        let input = "Value %s equals 15% of {total}.";
        let plan = marker_plan(input);
        let originals: Vec<&str> = plan.items().iter().map(|i| i.original.as_str()).collect();
        assert!(originals.contains(&"%s"), "{originals:?}");
        assert!(originals.contains(&"15%"));
        assert!(originals.contains(&"{total}"));
        let out = translate_with_markers(&plan);
        for needle in ["%s", "15%", "{total}"] {
            assert!(out.contains(needle), "{needle} in {out}");
        }
    }

    #[test]
    fn sp20_编号与日期整体保留() {
        let input = "Order #10248 was paid on 2026-11-11.";
        let plan = marker_plan(input);
        let out = translate_with_markers(&plan);
        assert!(out.contains("#10248"), "{out}");
        assert!(out.contains("2026-11-11"), "{out}");
    }

    #[test]
    fn 计划是确定性的() {
        let input = "Hello {name}, see https://example.com/x now.";
        let a = marker_plan(input);
        let b = marker_plan(input);
        assert_eq!(a.masked(), b.masked());
        assert_eq!(a.items(), b.items());
    }

    #[test]
    fn 失败错误映射为完整性错误() {
        let err: TranslationError = ProtectionError::Missing { id: 1, kind: "url" }.into();
        assert_eq!(err.code, TranslationErrorCode::IntegrityFailure);
    }
}
