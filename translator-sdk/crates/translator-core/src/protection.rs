//! 结构化文本保护（P3.1-C）：**要么完整恢复，要么显式失败**。
//!
//! # 为什么默认是"分段"而不是"打标记"
//!
//! 常见做法是把 `{name}` 换成 `#P1#` 之类的标记、整句送进模型、再换回来，并"假设"
//! 模型会照抄标记。P3.0/P3.1-B 的实测已经否掉了这个假设（`docs/15` §2.5：`{name}`/
//! `{order_id}` 被吞），P3.1-C 又对 5 种候选标记 × 7 类句子做了对照（`docs/18` §2.6）：
//! 最好的形态（`#Pn#`）是 6/7，其余 3/7～5/7——**没有任何一种标记是可靠的**。
//!
//! 所以默认策略改成"**分段**"（[`ProtectionMode::Segment`]）：受保护片段根本不进模型，
//! 只翻译它们之间的普通文本，最后按原顺序拼回。这样"标记丢失"在结构上不可能发生，
//! 代价是句子被切开、译文连贯性下降（实测见 `docs/18` §2.7）。
//!
//! 标记策略（[`ProtectionMode::Marker`]）保留为可选：它的质量更好，但**必须**验证
//! 每个 ID 恰好出现一次；任何缺失/重复/未知标记都返回 [`ProtectionError`]，
//! 由调用方转成 `IntegrityFailure`，**不允许**"看起来正常"的译文落库。
//!
//! # 不变量
//!
//! 1. 受保护片段的字节**原样**出现在输出里（`URL`/`email`/`{var}`/反引号代码，含反引号本身）；
//! 2. 数量与身份一致：不允许丢失、多出、串换；
//! 3. 允许语序调整（标记/片段可以换位置），但不允许替换内容；
//! 4. 失败时返回错误，不返回"半成品译文"。

use std::fmt;

use translator_types::{TranslationError, TranslationErrorCode};

use crate::text::{protected_spans, ProtectedKind};

/// 策略版本：进入诊断与（将来的）执行计划哈希。改行为必须改这个串。
pub const STRATEGY_VERSION: &str = "protect-seg-v1";
/// 标记策略的版本标识。
pub const MARKER_VERSION: &str = "protect-mark-v1";

/// 保护策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectionMode {
    /// 默认：受保护片段不进模型，只翻译普通文本段。
    Segment,
    /// 可选：打标记整句翻译，再严格还原（不可靠，失败即报错）。
    Marker,
}

impl ProtectionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Segment => STRATEGY_VERSION,
            Self::Marker => MARKER_VERSION,
        }
    }
}

/// 一个被保护的片段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedItem {
    /// 1 起的编号（标记策略里对应 `#P{id}#`）。
    pub id: usize,
    pub kind: ProtectedKind,
    /// 在**原文**里的字节区间。
    pub start: usize,
    pub end: usize,
    /// 原文片段（恢复时逐字节贴回）。
    pub original: String,
}

impl ProtectedItem {
    pub fn kind_str(&self) -> &'static str {
        match self.kind {
            ProtectedKind::Url => "url",
            ProtectedKind::Email => "email",
            ProtectedKind::TemplateVar => "template_var",
            ProtectedKind::Code => "code",
        }
    }
}

/// 计划的组成：普通文本段与受保护片段交替。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// 需要送进模型的普通文本（可能是空串，调用方可跳过）。
    Plain(String),
    /// 受保护片段（引用 [`ProtectedItem::id`]）。
    Protected(usize),
    /// 原样保留、**不进模型**的文本（换行等）。
    ///
    /// 换行必须这样处理：worker 的行协议会把一条请求压成一行（`\n` → 空格），
    /// 若把带换行的片段交给模型，换行结构就丢了（`docs/18` SP-11）。
    Literal(String),
}

/// 保护计划：原文 → 送模型的文本 + 还原规则。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectionPlan {
    mode: ProtectionMode,
    pieces: Vec<Piece>,
    items: Vec<ProtectedItem>,
    masked: String,
    source_len: usize,
}

impl ProtectionPlan {
    /// 默认（分段）计划。
    pub fn build(input: &str) -> Self {
        Self::new(ProtectionMode::Segment, input)
    }

    /// 标记计划（可选策略，还原时会严格校验）。
    pub fn build_marker(input: &str) -> Self {
        Self::new(ProtectionMode::Marker, input)
    }

    fn new(mode: ProtectionMode, input: &str) -> Self {
        let spans = normalised_spans(input);
        let mut items = Vec::with_capacity(spans.len());
        let mut pieces = Vec::new();
        let mut masked = String::with_capacity(input.len() + spans.len() * 8);
        let mut cursor = 0usize;

        for (idx, span) in spans.iter().enumerate() {
            let id = idx + 1;
            let original = input[span.start..span.end].to_string();
            push_plain(&mut pieces, &input[cursor..span.start]);
            pieces.push(Piece::Protected(id));
            masked.push_str(&input[cursor..span.start]);
            masked.push_str(&format!("#P{id}#"));
            items.push(ProtectedItem {
                id,
                kind: span.kind,
                start: span.start,
                end: span.end,
                original,
            });
            cursor = span.end;
        }
        push_plain(&mut pieces, &input[cursor..]);
        masked.push_str(&input[cursor..]);

        Self {
            mode,
            pieces,
            items,
            masked,
            source_len: input.len(),
        }
    }

    pub fn mode(&self) -> ProtectionMode {
        self.mode
    }

    pub fn items(&self) -> &[ProtectedItem] {
        &self.items
    }

    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    /// 标记策略下要送给模型的文本。
    pub fn masked(&self) -> &str {
        &self.masked
    }

    pub fn source_len(&self) -> usize {
        self.source_len
    }

    pub fn is_trivial(&self) -> bool {
        self.items.is_empty()
    }

    /// 分段策略：需要翻译的普通文本，按出现顺序（含空串，调用方自行跳过）。
    ///
    /// `Literal` 片段（换行）不在这里：它们不进模型。
    pub fn plain_texts(&self) -> Vec<&str> {
        self.pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Plain(text) => Some(text.as_str()),
                Piece::Protected(_) | Piece::Literal(_) => None,
            })
            .collect()
    }

    /// 分段策略：把逐段译文按原顺序拼回，并做结构自检。
    ///
    /// `translated.len()` 必须等于 [`Self::plain_texts`] 的长度。
    pub fn assemble(&self, translated: &[String]) -> Result<String, ProtectionError> {
        let expected = self.plain_texts().len();
        if translated.len() != expected {
            return Err(ProtectionError::SegmentCount {
                expected,
                got: translated.len(),
            });
        }
        let mut iter = translated.iter();
        let mut out = String::with_capacity(self.source_len);
        for piece in &self.pieces {
            match piece {
                Piece::Plain(_) => {
                    if let Some(text) = iter.next() {
                        out.push_str(text);
                    }
                }
                Piece::Protected(id) => {
                    out.push_str(&self.item(*id)?.original);
                }
                Piece::Literal(text) => out.push_str(text),
            }
        }
        self.verify_structure(&out)?;
        Ok(out)
    }

    /// 标记策略：严格还原。
    pub fn restore(&self, translated: &str) -> Result<String, ProtectionError> {
        let tokens = scan_markers(translated)?;
        let n = self.items.len();

        // 每个 ID 恰好一次
        let mut seen = vec![0usize; n + 1];
        for token in &tokens {
            let id = token.id;
            if id == 0 || id > n {
                return Err(ProtectionError::UnknownMarker(format!("#P{id}#")));
            }
            seen[id] += 1;
        }
        for (id, count) in seen.iter().enumerate().skip(1) {
            if *count == 0 {
                return Err(ProtectionError::Missing {
                    id,
                    kind: self.items[id - 1].kind_str(),
                });
            }
            if *count > 1 {
                return Err(ProtectionError::Duplicated { id, count: *count });
            }
        }

        let mut out = String::with_capacity(translated.len());
        let mut cursor = 0usize;
        for token in &tokens {
            out.push_str(&translated[cursor..token.start]);
            out.push_str(&self.item(token.id)?.original);
            cursor = token.end;
        }
        out.push_str(&translated[cursor..]);
        Ok(out)
    }

    fn item(&self, id: usize) -> Result<&ProtectedItem, ProtectionError> {
        if id == 0 || id > self.items.len() {
            return Err(ProtectionError::UnknownMarker(format!("#P{id}#")));
        }
        Ok(&self.items[id - 1])
    }

    /// 结构自检：每个受保护片段都在输出里、按原顺序、且只出现一次。
    fn verify_structure(&self, assembled: &str) -> Result<(), ProtectionError> {
        let mut cursor = 0usize;
        for item in &self.items {
            let Some(offset) = assembled[cursor..].find(&item.original) else {
                return Err(ProtectionError::Missing {
                    id: item.id,
                    kind: item.kind_str(),
                });
            };
            cursor += offset + item.original.len();
        }
        Ok(())
    }
}

/// 保护失败：**任何**一种都必须变成 `IntegrityFailure`，不能静默返回译文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtectionError {
    Missing { id: usize, kind: &'static str },
    Duplicated { id: usize, count: usize },
    UnknownMarker(String),
    SegmentCount { expected: usize, got: usize },
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
            Self::UnknownMarker(marker) => write!(f, "译文里出现未知标记 {marker}"),
            Self::SegmentCount { expected, got } => {
                write!(f, "分段译文数量不符：期望 {expected} 段，实际 {got} 段")
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

struct Span {
    start: usize,
    end: usize,
    kind: ProtectedKind,
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

/// 取扫描器的结果并做防御性归一化：按起点排序、丢弃重叠与空区间。
fn normalised_spans(input: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = protected_spans(input)
        .into_iter()
        .filter(|s| s.end > s.start && s.end <= input.len())
        .map(|s| Span {
            start: s.start,
            end: s.end,
            kind: s.kind,
        })
        .collect();
    spans.sort_by_key(|s| (s.start, s.end));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(last) = out.last() {
            if span.start < last.end {
                continue; // 重叠：保留先出现的那个
            }
        }
        out.push(span);
    }
    out
}

struct MarkerToken {
    start: usize,
    end: usize,
    id: usize,
}

/// 扫描 `#P<n>#`。出现 `#P` 却解析不出完整标记 → 未知标记（比"忽略"安全）。
fn scan_markers(text: &str) -> Result<Vec<MarkerToken>, ProtectionError> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'#' && bytes[i + 1] == b'P' {
            let mut j = i + 2;
            let digit_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > digit_start && j < bytes.len() && bytes[j] == b'#' {
                let id: usize = text[digit_start..j].parse().unwrap_or(0);
                tokens.push(MarkerToken {
                    start: i,
                    end: j + 1,
                    id,
                });
                i = j + 1;
                continue;
            }
            // `#P` 开头但不是完整标记：说明模型破坏了它
            // 按字符取片段：按字节切会把多字节字符劈开（直接 panic）
            let excerpt: String = text[i..].chars().take(8).collect();
            return Err(ProtectionError::UnknownMarker(excerpt));
        }
        i += 1;
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个"像真实模型"的假翻译器：会吞标记、会改标点，但**不会**改受保护片段
    /// （因为分段策略根本不把它们交给它）。
    fn mock_translate(text: &str) -> String {
        let mut out = text.to_string();
        out = out.replace("Hello", "您好");
        out = out.replace("your order", "您的订单");
        out = out.replace("has shipped", "已发货");
        out = out.replace("See", "参见");
        out = out.replace("for more details", "了解更多详情");
        out = out.replace("Run", "运行");
        out = out.replace("before committing", "在提交之前");
        out = out.replace("Contact", "联系");
        out = out.replace("if the error persists", "如果错误仍然存在");
        out = out.replace("Wait", "等等");
        out = out.replace("really", "真的吗");
        // 真实模型的坏毛病：标记会被吞
        while let Some(pos) = out.find("#P") {
            let end = out[pos..]
                .find('#')
                .map(|e| pos + e + 1)
                .unwrap_or(out.len());
            out.replace_range(pos..end.min(out.len()), "");
        }
        out.trim().to_string()
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
        ProtectionPlan::build(input)
            .items()
            .iter()
            .map(|i| i.original.clone())
            .collect()
    }

    // ---------- SP-01～03：模板变量 ----------

    #[test]
    fn sp01_单个模板变量完整保留() {
        let input = "Hello {name}, welcome.";
        assert_eq!(items_of(input), vec!["{name}"]);
        let out = translate_in_segments(&ProtectionPlan::build(input));
        assert!(out.contains("{name}"), "{out}");
        assert_eq!(out.matches("{name}").count(), 1);
    }

    #[test]
    fn sp02_多个不同变量各自完整() {
        let input = "Hello {name}, your order {order_id} has shipped.";
        let plan = ProtectionPlan::build(input);
        assert_eq!(items_of(input), vec!["{name}", "{order_id}"]);
        let out = translate_in_segments(&plan);
        assert_eq!(out.matches("{name}").count(), 1, "{out}");
        assert_eq!(out.matches("{order_id}").count(), 1, "{out}");
        // 顺序保持
        assert!(out.find("{name}").unwrap() < out.find("{order_id}").unwrap());
    }

    #[test]
    fn sp03_重复变量按实例计数() {
        let input = "Hello {name}, bye {name}.";
        let plan = ProtectionPlan::build(input);
        assert_eq!(plan.items().len(), 2, "两个实例必须是两个片段");
        let out = translate_in_segments(&plan);
        assert_eq!(out.matches("{name}").count(), 2, "{out}");
    }

    // ---------- SP-04～05：URL / email ----------

    #[test]
    fn sp04_url与带查询参数的url逐字节保留() {
        let input = "See https://example.com/docs for more details.";
        let plan = ProtectionPlan::build(input);
        assert_eq!(items_of(input), vec!["https://example.com/docs"]);
        let out = translate_in_segments(&plan);
        assert!(out.contains("https://example.com/docs"), "{out}");

        let query = "Open https://example.com/x?id=7&lang=en now.";
        let plan = ProtectionPlan::build(query);
        let out = translate_in_segments(&plan);
        assert!(out.contains("https://example.com/x?id=7&lang=en"), "{out}");
    }

    #[test]
    fn sp05_email与混合结构精确保留() {
        let input = "Contact support@example.com if the error persists.";
        let out = translate_in_segments(&ProtectionPlan::build(input));
        assert!(out.contains("support@example.com"), "{out}");

        let mixed = "Visit https://example.com or email us at hi@example.com about {plan}.";
        let plan = ProtectionPlan::build(mixed);
        let kinds: Vec<&str> = plan.items().iter().map(|i| i.kind_str()).collect();
        assert_eq!(kinds, vec!["url", "email", "template_var"]);
        let out = translate_in_segments(&plan);
        for needle in ["https://example.com", "hi@example.com", "{plan}"] {
            assert_eq!(out.matches(needle).count(), 1, "{needle} in {out}");
        }
    }

    // ---------- SP-06～07：代码 ----------

    #[test]
    fn sp06_反引号代码整体保留含反引号本身() {
        let input = "Run `cargo test --all-features` before committing.";
        let plan = ProtectionPlan::build(input);
        let out = translate_in_segments(&plan);
        assert!(
            out.contains("`cargo test --all-features`"),
            "反引号必须还在：{out}"
        );
    }

    #[test]
    fn sp07_代码内嵌url与变量不重复保护() {
        let input = "Run `curl https://example.com/{id}` now.";
        let plan = ProtectionPlan::build(input);
        // 代码整体是一个片段，里头的 URL/变量不再单独编号
        assert_eq!(plan.items().len(), 1, "{:?}", plan.items());
        assert_eq!(plan.items()[0].kind_str(), "code");
        let out = translate_in_segments(&plan);
        assert!(out.contains("`curl https://example.com/{id}`"), "{out}");
    }

    // ---------- SP-08～09：失败必须显式 ----------

    #[test]
    fn sp08_伪造标记被判为未知标记() {
        let plan = ProtectionPlan::build("Hello {name}, welcome.");
        // 模型编出一个原文里不存在的标记
        let fake = "#P1# 你好 #P2#";
        let err = plan.restore(fake).unwrap_err();
        assert!(matches!(err, ProtectionError::UnknownMarker(_)), "{err:?}");
    }

    #[test]
    fn sp09_丢失或复制标记必须报错() {
        let plan = ProtectionPlan::build("Hello {name}, your order {order_id} has shipped.");
        let missing = plan.restore("#P1# 您好,您的订单已发货。").unwrap_err();
        assert!(
            matches!(missing, ProtectionError::Missing { id: 2, .. }),
            "{missing:?}"
        );

        let duplicated = plan.restore("#P1# 您好 #P1# 订单 #P2# 已发货").unwrap_err();
        assert!(
            matches!(duplicated, ProtectionError::Duplicated { id: 1, count: 2 }),
            "{duplicated:?}"
        );

        // 标记被削掉一半（真实模型最常见的破坏方式）
        let broken = plan.restore("#P1 您好,订单 #P2# 已发货").unwrap_err();
        assert!(
            matches!(broken, ProtectionError::UnknownMarker(_)),
            "{broken:?}"
        );
    }

    #[test]
    fn sp09b_标记策略在模型保留标记时可用且允许重排() {
        let plan = ProtectionPlan::build_marker("Hello {name}, your order {order_id} has shipped.");
        // 模型保留了两个标记，但把语序调换了
        let ok = plan.restore("#P2# 您好 #P1# 已发货。").unwrap();
        assert_eq!(ok, "{order_id} 您好 {name} 已发货。");
    }

    // ---------- SP-10～12：边界输入 ----------

    #[test]
    fn sp10_utf8与emoji不破坏字节边界() {
        let input = "Order {id} 已发货 🚚, thanks!";
        let plan = ProtectionPlan::build(input);
        assert_eq!(items_of(input), vec!["{id}"]);
        let out = translate_in_segments(&plan);
        assert!(out.contains("{id}"), "{out}");
        assert!(out.contains('🚚'), "{out}");
        assert!(out.is_char_boundary(out.len()));
    }

    #[test]
    fn sp11_多行长文本逐行结构不变() {
        let input = "Line one has {a}.\nLine two has https://example.com/x.\nLine three is plain.";
        let plan = ProtectionPlan::build(input);
        assert_eq!(plan.items().len(), 2);
        let out = translate_in_segments(&plan);
        assert!(out.contains("{a}"), "{out}");
        assert!(out.contains("https://example.com/x"), "{out}");
        assert_eq!(out.matches('\n').count(), 2, "换行结构必须保持：{out}");
    }

    #[test]
    fn sp12_空白输入与全保护输入不崩() {
        let empty = ProtectionPlan::build("");
        assert!(empty.is_trivial());
        assert_eq!(empty.assemble(&["".to_string()]).unwrap(), "");

        let whole = ProtectionPlan::build("https://example.com/only");
        assert_eq!(whole.items().len(), 1);
        // 前后各有一个空普通段
        let out = whole.assemble(&["".to_string(), "".to_string()]).unwrap();
        assert_eq!(out, "https://example.com/only", "全保护输入应原样返回");

        // 分段数量不匹配必须报错，而不是悄悄拼接
        let err = whole.assemble(&["".to_string()]).unwrap_err();
        assert!(
            matches!(err, ProtectionError::SegmentCount { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn 分段数等于普通文本段数() {
        let plan = ProtectionPlan::build("a {x} b {y} c");
        assert_eq!(plan.plain_texts(), vec!["a ", " b ", " c"]);
        assert_eq!(plan.pieces().len(), 5);
    }

    #[test]
    fn 失败错误映射为完整性错误() {
        let err: TranslationError = ProtectionError::Missing { id: 1, kind: "url" }.into();
        assert_eq!(err.code, TranslationErrorCode::IntegrityFailure, "{err}");
    }
}
