//! 文本预处理：规范化、语言识别启发式、受保护跨度扫描。
//!
//! 本模块只做**扫描**（哪里必须原样保留）与**规范化**（折叠空白）。
//! "怎么保护、怎么严格还原"在 [`crate::protection`]；"数字语义对不对"在
//! `crate::quality::number`。三条职责不混在一起，避免再次出现两套保护路径
//! （§2.2）。
//!
//! # 扫描顺序（重要）
//!
//! 同一位置可能有多种解释（`https://x/` 里的 `/` 不是路径起点）。因此按
//! **更具体优先**的顺序尝试：代码 > URL > 邮箱 > 标签/模板标签 > 模板变量 >
//! 格式占位符 > 路径 > 数字/编号 > 标识符。
//!
//! 一旦某个匹配成功，扫描位置直接跳到它的结尾，内部不再重复保护
//! （`` `curl https://x/{id}` `` 整体是一个代码片段）。

use translator_types::LanguageTag;

/// 折叠多余空白并去除首尾空白。不做 NFKC——那会破坏代码与符号语义。
pub fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_space = false;

    for ch in input.trim().chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }

    out
}

/// 判断是否"像单个词条"。
///
/// 只用于**提示**（例如宿主想决定默认按钮文字），不用于自动切换 API：
/// `Run!` 很短但是祈使句，`light` 是单词但可能出现在句子里（§7.2）。
pub fn looks_like_single_word(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.chars().any(char::is_whitespace) {
        return false;
    }

    if trimmed
        .chars()
        .last()
        .map(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | ';' | '；'))
        .unwrap_or(false)
    {
        return false;
    }

    let cjk = trimmed.chars().any(|c| {
        ('\u{3040}'..='\u{30FF}').contains(&c)
            || ('\u{4E00}'..='\u{9FFF}').contains(&c)
            || ('\u{AC00}'..='\u{D7AF}').contains(&c)
    });

    let max_chars = if cjk { 4 } else { 32 };
    trimmed.chars().count() <= max_chars
}

/// 语言识别启发式（零模型）。
pub fn guess_language(text: &str) -> Option<LanguageTag> {
    crate::language::guess(text)
}

/// 受保护内容的种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProtectedKind {
    Url,
    Email,
    /// `{name}` / `{{ user.name }}` / `${var}` 形式的模板变量。
    TemplateVar,
    /// `%s` / `%d` / `%1$s` / `%.2f` 形式的格式化占位符。
    FormatSpec,
    /// 反引号代码片段（含反引号本身）。
    Code,
    /// 关键数字格式：百分比、十六进制、日期、版本号、`#10248` 编号。
    Number,
    /// 文件路径（可配置保护）。
    Path,
    /// HTML / 模板标签（`<b>`、`</p>`、`{% if %}`）。
    Tag,
    /// 技术标识符（`API_KEY`、`user_id`）；可配置保护。
    Identifier,
}

impl ProtectedKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Email => "email",
            Self::TemplateVar => "template_var",
            Self::FormatSpec => "format_spec",
            Self::Code => "code",
            Self::Number => "number",
            Self::Path => "path",
            Self::Tag => "tag",
            Self::Identifier => "identifier",
        }
    }
}

/// 原文里的一段受保护区间（字节偏移）。
///
/// `start` / `end` 的作用域是**传入 `protected_spans_with()` 的那份文本**。
/// 流水线里先 `normalize()` 再扫描，因此偏移基于规范化后的文本；
/// 不要拿它去切用户原始输入。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtectedSpan {
    pub start: usize,
    pub end: usize,
    pub kind: ProtectedKind,
}

/// 扫描策略：**确定性保护**默认全开，**可配置保护**默认关闭。
///
/// 路径与标识符默认关闭的原因：`/ usr` 这种排版、`foo_bar` 这种人写的词
/// 一旦被当成"必须原样保留"，译文会突然冒出一串英文，反而更差（§4.1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanPolicy {
    pub url: bool,
    pub email: bool,
    pub template_var: bool,
    pub format_spec: bool,
    pub code: bool,
    pub number: bool,
    pub path: bool,
    pub tag: bool,
    pub identifier: bool,
}

impl Default for SpanPolicy {
    fn default() -> Self {
        Self {
            url: true,
            email: true,
            template_var: true,
            format_spec: true,
            code: true,
            number: true,
            path: false,
            tag: true,
            identifier: false,
        }
    }
}

impl SpanPolicy {
    /// 全部打开（文档/代码场景）。
    pub fn everything() -> Self {
        Self {
            path: true,
            identifier: true,
            ..Self::default()
        }
    }

    /// 只保留最关键的四类（URL/邮箱/变量/代码）。
    pub fn minimal() -> Self {
        Self {
            url: true,
            email: true,
            template_var: true,
            format_spec: false,
            code: true,
            number: false,
            path: false,
            tag: false,
            identifier: false,
        }
    }
}

/// 用默认策略扫描受保护区间。
pub fn protected_spans(input: &str) -> Vec<ProtectedSpan> {
    protected_spans_with(input, &SpanPolicy::default())
}

/// 用指定策略扫描受保护区间。
pub fn protected_spans_with(input: &str, policy: &SpanPolicy) -> Vec<ProtectedSpan> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let byte_at = |idx: usize| chars.get(idx).map_or(input.len(), |(b, _)| *b);

    let mut out: Vec<ProtectedSpan> = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        let (byte_pos, c) = chars[i];

        let found = scan_at(&chars, i, policy, c);

        match found {
            Some((end_idx, kind)) => {
                if end_idx > i {
                    out.push(ProtectedSpan {
                        start: byte_pos,
                        end: byte_at(end_idx),
                        kind,
                    });
                    i = end_idx;
                    continue;
                }
                i += 1;
            }
            None => i += 1,
        }
    }

    out
}

/// 在位置 `i` 尝试所有匹配器。
///
/// **必须**是顺序 `if ... return`，不能写成 `else if` 链：邮箱匹配器对几乎
/// 每个字母/数字都会"试一次然后失败"，如果它失败就结束整条链，
/// 后面的模板变量、路径、标识符永远没有机会（这正是重构前扫描器的隐藏缺陷）。
fn scan_at(
    chars: &[(usize, char)],
    i: usize,
    policy: &SpanPolicy,
    c: char,
) -> Option<(usize, ProtectedKind)> {
    if policy.code && c == '`' {
        if let Some(end) = code_end(chars, i) {
            return Some((end, ProtectedKind::Code));
        }
    }
    if policy.url && starts_with_url(chars, i) {
        if let Some(end) = url_end(chars, i) {
            return Some((end, ProtectedKind::Url));
        }
    }
    if policy.email && is_email_char(c) && is_token_start(chars, i) {
        if let Some(end) = email_end(chars, i) {
            return Some((end, ProtectedKind::Email));
        }
    }
    if policy.tag && c == '<' {
        if let Some(end) = tag_end(chars, i) {
            return Some((end, ProtectedKind::Tag));
        }
    }
    if policy.tag && c == '{' && next_char(chars, i) == Some('{') {
        if let Some(end) = delimited_end(chars, i, "{{", "}}") {
            return Some((end, ProtectedKind::TemplateVar));
        }
    }
    if policy.tag && c == '{' && next_char(chars, i) == Some('%') {
        if let Some(end) = delimited_end(chars, i, "{%", "%}") {
            return Some((end, ProtectedKind::Tag));
        }
    }
    if policy.template_var && (c == '{' || (c == '$' && next_char(chars, i) == Some('{'))) {
        if let Some(end) = template_var_end(chars, i) {
            return Some((end, ProtectedKind::TemplateVar));
        }
    }
    if policy.format_spec && c == '%' {
        if let Some(end) = format_spec_end(chars, i) {
            return Some((end, ProtectedKind::FormatSpec));
        }
    }
    if policy.path && (c == '/' || c == '~' || is_windows_path_start(chars, i)) {
        if let Some(end) = path_end(chars, i) {
            return Some((end, ProtectedKind::Path));
        }
    }
    if policy.number && c == '#' {
        if let Some(end) = issue_number_end(chars, i) {
            return Some((end, ProtectedKind::Number));
        }
    }
    if policy.number && c.is_ascii_digit() {
        if let Some(end) = numeric_end(chars, i) {
            return Some((end, ProtectedKind::Number));
        }
    }
    if policy.identifier && is_identifier_start(chars, i) {
        if let Some(end) = identifier_end(chars, i) {
            return Some((end, ProtectedKind::Identifier));
        }
    }
    None
}

/// 文本片段：规则引擎只能改写 `Plain`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    Plain(&'a str),
    Protected(&'a str),
}

/// 把输入切成「纯文本 / 受保护」片段（默认策略）。
pub fn split_protected(input: &str) -> Vec<Segment<'_>> {
    split_protected_with(input, &SpanPolicy::default())
}

/// 按策略切分。
pub fn split_protected_with<'a>(input: &'a str, policy: &SpanPolicy) -> Vec<Segment<'a>> {
    let spans = protected_spans_with(input, policy);
    let mut out = Vec::with_capacity(spans.len() * 2 + 1);
    let mut cursor = 0usize;

    for span in spans {
        if span.start > cursor {
            out.push(Segment::Plain(&input[cursor..span.start]));
        }
        out.push(Segment::Protected(&input[span.start..span.end]));
        cursor = span.end;
    }

    if cursor < input.len() {
        out.push(Segment::Plain(&input[cursor..]));
    }
    out
}

// ============================================================
// 各类型的匹配器
// ============================================================

fn next_char(chars: &[(usize, char)], i: usize) -> Option<char> {
    chars.get(i + 1).map(|(_, c)| *c)
}

fn char_slice(chars: &[(usize, char)], start: usize, end: usize) -> String {
    chars[start.min(chars.len())..end.min(chars.len())]
        .iter()
        .map(|(_, c)| *c)
        .collect()
}

/// 三反引号或单反引号代码片段。
fn code_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    if char_slice(chars, i, i + 3) == "```" {
        let mut j = i + 3;
        while j + 3 <= chars.len() {
            if char_slice(chars, j, j + 3) == "```" {
                return Some(j + 3);
            }
            j += 1;
        }
        return None;
    }
    chars[i + 1..]
        .iter()
        .position(|(_, c)| *c == '`')
        .map(|offset| i + 1 + offset + 1)
}

fn starts_with_url(chars: &[(usize, char)], i: usize) -> bool {
    let head = char_slice(chars, i, i + 9).to_ascii_lowercase();
    const SCHEMES: [&str; 6] = [
        "http://", "https://", "ftp://", "file://", "ws://", "wss://",
    ];
    SCHEMES.iter().any(|s| head.starts_with(s))
        || (head.starts_with("www.") && is_token_start(chars, i))
}

fn url_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;
    while j < chars.len() && !chars[j].1.is_whitespace() {
        if matches!(chars[j].1, '。' | '，' | '；' | '！' | '？' | '）' | '】') {
            break;
        }
        j += 1;
    }
    // 去掉 URL 末尾的英文标点（`see https://x.com.` 的点是句号）
    while j > i + 1
        && matches!(
            chars[j - 1].1,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | '"' | '\''
        )
    {
        j -= 1;
    }
    (j > i).then_some(j)
}

fn is_email_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-' | '@')
}

fn is_token_start(chars: &[(usize, char)], i: usize) -> bool {
    match i.checked_sub(1).and_then(|prev| chars.get(prev)) {
        Some((_, c)) => !c.is_ascii_alphanumeric() && !matches!(c, '_' | '.' | '%' | '+' | '-'),
        None => true,
    }
}

fn email_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;
    while j < chars.len() && is_email_char(chars[j].1) && chars[j].1 != '@' {
        j += 1;
    }
    if j >= chars.len() || chars[j].1 != '@' {
        return None;
    }
    let at = j;
    j += 1;

    let mut seen_dot = false;
    while j < chars.len() {
        let c = chars[j].1;
        if c == '.' {
            seen_dot = true;
            j += 1;
        } else if c.is_ascii_alphanumeric() || c == '-' {
            j += 1;
        } else {
            break;
        }
    }
    if !seen_dot || j == at + 1 {
        return None;
    }

    let tld_start = chars[..j]
        .iter()
        .rposition(|(_, c)| *c == '.')
        .map(|pos| pos + 1)?;
    let tld = char_slice(chars, tld_start, j);
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(j)
}

/// HTML 标签 / 注释。
fn tag_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if j >= chars.len() {
        return None;
    }
    if char_slice(chars, i, i + 4) == "<!--" {
        let mut k = i + 4;
        while k + 3 <= chars.len() {
            if char_slice(chars, k, k + 3) == "-->" {
                return Some(k + 3);
            }
            k += 1;
        }
        return None;
    }
    if chars[j].1 == '/' {
        j += 1;
    }
    if j >= chars.len() || !chars[j].1.is_ascii_alphabetic() {
        return None;
    }
    while j < chars.len() && (chars[j].1.is_ascii_alphanumeric() || chars[j].1 == '-') {
        j += 1;
    }
    let mut k = j;
    while k < chars.len() && k - i < 200 {
        match chars[k].1 {
            '>' => return Some(k + 1),
            '\n' | '\r' | '<' => return None,
            _ => k += 1,
        }
    }
    None
}

/// `{{ ... }}` / `{% ... %}`。
fn delimited_end(chars: &[(usize, char)], i: usize, open: &str, close: &str) -> Option<usize> {
    let open_len = open.chars().count();
    if char_slice(chars, i, i + open_len) != open {
        return None;
    }
    let mut j = i + open_len;
    let close_len = close.chars().count();
    while j + close_len <= chars.len() {
        if char_slice(chars, j, j + close_len) == close {
            return Some(j + close_len);
        }
        if chars[j].1 == '\n' {
            return None;
        }
        j += 1;
    }
    None
}

/// `{name}` / `${name}`。
fn template_var_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let brace = if chars[i].1 == '$' { i + 1 } else { i };
    if chars.get(brace).map(|(_, c)| *c) != Some('{') {
        return None;
    }
    if brace + 1 >= chars.len() {
        return None;
    }
    let mut j = brace + 1;
    while j < chars.len() && j - brace <= 64 {
        match chars[j].1 {
            '}' => return Some(j + 1),
            '\n' | '{' => return None,
            _ => j += 1,
        }
    }
    None
}

/// `%s` / `%d` / `%1$s` / `%.2f` / `%%`。
fn format_spec_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if j >= chars.len() {
        return None;
    }
    if chars[j].1 == '%' {
        return Some(j + 1);
    }
    let mut digits = 0usize;
    while j < chars.len() && chars[j].1.is_ascii_digit() {
        j += 1;
        digits += 1;
    }
    if digits > 0 {
        if j < chars.len() && chars[j].1 == '$' {
            j += 1;
        } else {
            return None;
        }
    }
    while j < chars.len() && matches!(chars[j].1, '-' | '+' | ' ' | '#' | '0' | '.' | '\'') {
        j += 1;
    }
    while j < chars.len() && chars[j].1.is_ascii_digit() {
        j += 1;
    }
    if j < chars.len()
        && matches!(
            chars[j].1,
            's' | 'd' | 'i' | 'u' | 'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'x' | 'X' | 'o' | 'c' | 'p'
        )
    {
        Some(j + 1)
    } else {
        None
    }
}

fn is_windows_path_start(chars: &[(usize, char)], i: usize) -> bool {
    let is_drive = chars[i].1.is_ascii_alphabetic()
        && chars.get(i + 1).map(|(_, c)| *c) == Some(':')
        && matches!(chars.get(i + 2).map(|(_, c)| *c), Some('\\') | Some('/'));
    let is_unc = chars[i].1 == '\\'
        && chars.get(i + 1).map(|(_, c)| *c) == Some('\\')
        && chars
            .get(i + 2)
            .map(|(_, c)| *c)
            .is_some_and(|c| c.is_ascii_alphanumeric());
    is_drive || is_unc
}

/// 空格之后是否仍是路径片段（`Files\App`、`App\config.ini`、`config.ini`）。
fn next_segment_looks_like_path(chars: &[(usize, char)], start: usize) -> bool {
    let mut j = start;
    let mut seen_dot = false;
    while j < chars.len() && !chars[j].1.is_whitespace() {
        let c = chars[j].1;
        if c == '\\' || c == '/' {
            return true;
        }
        if c == '.' {
            seen_dot = true;
        }
        if matches!(c, ',' | ';' | '。' | ')' | '）' | '"' | '\'') {
            break;
        }
        j += 1;
    }
    seen_dot
}

/// 文件路径（Windows 盘符/UNC、POSIX 绝对路径、`~/`）。
fn path_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    const TERMINATORS: [char; 13] = [
        '\n', '\r', ',', ';', '。', '，', '；', '、', ')', '）', '"', '\'', '<',
    ];

    if is_windows_path_start(chars, i) {
        let mut j = i + 2;
        while j < chars.len() && !TERMINATORS.contains(&chars[j].1) {
            // Windows 路径允许空格（`C:\Program Files\App`），但不能一路吃到句尾。
            // 只有"空格后紧跟的片段仍像路径片段"（含 \ / 或带扩展名）时才继续。
            if chars[j].1 == ' ' && !next_segment_looks_like_path(chars, j + 1) {
                break;
            }
            j += 1;
        }
        while j > i + 3 && chars[j - 1].1.is_whitespace() {
            j -= 1;
        }
        return (j > i + 3).then_some(j);
    }

    let is_posix = chars[i].1 == '/'
        && chars
            .get(i + 1)
            .map(|(_, c)| c.is_ascii_alphanumeric() || matches!(*c, '_' | '.' | '-'))
            .unwrap_or(false)
        && is_token_start(chars, i);
    let is_home = chars[i].1 == '~' && chars.get(i + 1).map(|(_, c)| *c) == Some('/');
    if !is_posix && !is_home {
        return None;
    }

    let mut j = i;
    let mut slashes = 0usize;
    while j < chars.len() {
        let c = chars[j].1;
        if c == '/' {
            slashes += 1;
            j += 1;
        } else if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '~' | '+' | '%') {
            j += 1;
        } else {
            break;
        }
    }
    (slashes >= 2 && j - i >= 4).then_some(j)
}

/// `#10248` 形式的编号。
fn issue_number_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i + 1;
    let start = j;
    while j < chars.len() && (chars[j].1.is_ascii_alphanumeric() || chars[j].1 == '-') {
        j += 1;
    }
    if j - start < 2 {
        return None;
    }
    let token = char_slice(chars, start, j);
    token.chars().any(|c| c.is_ascii_digit()).then_some(j)
}

/// 识别必须原样保留的数字格式（百分比/十六进制/日期/版本号）。
fn numeric_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;

    if chars[i].1 == '0' && i + 1 < chars.len() && matches!(chars[i + 1].1, 'x' | 'X') {
        j = i + 2;
        let start = j;
        while j < chars.len() && chars[j].1.is_ascii_hexdigit() {
            j += 1;
        }
        return (j > start).then_some(j);
    }

    let mut saw_dash = false;
    let mut saw_slash = false;
    let mut dots = 0usize;
    while j < chars.len() {
        let c = chars[j].1;
        let next_is_digit = j + 1 < chars.len() && chars[j + 1].1.is_ascii_digit();
        if c.is_ascii_digit() {
            j += 1;
        } else if c == ':' && next_is_digit {
            // 时间 `14:30`：只吃掉分隔符，语义值由数字检查负责
            j += 1;
        } else if c == '-' && next_is_digit {
            saw_dash = true;
            j += 1;
        } else if c == '/' && j + 1 < chars.len() && chars[j + 1].1.is_ascii_digit() {
            saw_slash = true;
            j += 1;
        } else if c == '.' && j + 1 < chars.len() && chars[j + 1].1.is_ascii_digit() {
            dots += 1;
            j += 1;
        } else {
            break;
        }
    }

    if j < chars.len() && chars[j].1 == '%' {
        return Some(j + 1);
    }
    if saw_dash || saw_slash || dots >= 2 {
        return Some(j);
    }
    None
}

/// 只对"含下划线"的标识符感兴趣（`API_KEY`、`user_id`），
/// 避免把普通英文单词整片保护起来。
fn is_identifier_start(chars: &[(usize, char)], i: usize) -> bool {
    chars[i].1.is_ascii_alphabetic()
        && is_token_start(chars, i)
        && chars[i..].iter().take(48).any(|(_, c)| *c == '_')
}

/// `API_KEY` / `user_id` / `retry_count_v2`。
fn identifier_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;
    let mut has_underscore = false;
    while j < chars.len() {
        let c = chars[j].1;
        if c.is_ascii_alphanumeric() {
            j += 1;
        } else if c == '_' {
            has_underscore = true;
            j += 1;
        } else {
            break;
        }
    }
    if !has_underscore || j - i < 3 {
        return None;
    }
    if chars.get(j).map(|(_, c)| *c) == Some('(') {
        return None;
    }
    Some(j)
}

// ============================================================
// 旧式"整段替换成 ⟦Pn⟧"API（保留给不需要严格校验的内部调用）
// ============================================================

/// 保护后的文本。
#[derive(Clone, Debug)]
pub struct ProtectedText {
    pub text: String,
    pub segments: Vec<String>,
}

impl ProtectedText {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    fn marker(index: usize) -> String {
        format!("\u{27E6}P{index}\u{27E7}")
    }
}

/// 保护 URL、邮箱、`{占位符}`、格式化占位符与反引号代码。
///
/// ⚠️ 这个 API **不做**严格还原校验（缺标记只会进 `missing` 列表）。
/// 真正的翻译主链必须走 [`crate::protection::ProtectionPlan`]。
pub fn protect(input: &str) -> ProtectedText {
    let mut segments: Vec<String> = Vec::new();
    let mut out = String::with_capacity(input.len());

    for segment in split_protected(input) {
        match segment {
            Segment::Plain(text) => out.push_str(text),
            Segment::Protected(text) => {
                out.push_str(&ProtectedText::marker(segments.len()));
                segments.push(text.to_string());
            }
        }
    }

    ProtectedText {
        text: out,
        segments,
    }
}

/// 还原占位符。返回 `(还原后的文本, 未能还原的占位符列表)`。
pub fn restore(protected: &ProtectedText, translated: &str) -> (String, Vec<String>) {
    let mut out = translated.to_string();
    let mut missing = Vec::new();

    for (idx, seg) in protected.segments.iter().enumerate() {
        let marker = ProtectedText::marker(idx);
        if out.contains(&marker) {
            out = out.replace(&marker, seg);
        } else {
            missing.push(seg.clone());
        }
    }

    (out, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(input: &str) -> Vec<(&str, ProtectedKind)> {
        protected_spans(input)
            .into_iter()
            .map(|s| (&input[s.start..s.end], s.kind))
            .collect()
    }

    #[test]
    fn 规范化折叠空白但不改全角() {
        assert_eq!(normalize("  a   b \n c  "), "a b c");
        assert_eq!(normalize("Ａ　Ｂ"), "Ａ Ｂ");
    }

    #[test]
    fn url_包含查询参数与端口() {
        let spans = kinds("See https://example.com:8443/a/b?id=3&x=%20 now.");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, "https://example.com:8443/a/b?id=3&x=%20");
        assert_eq!(spans[0].1, ProtectedKind::Url);
    }

    #[test]
    fn url_末尾的句号不属于链接() {
        let spans = kinds("Open https://example.com/a.");
        assert_eq!(spans[0].0, "https://example.com/a");
    }

    #[test]
    fn 邮箱与普通英文混合() {
        let spans = kinds("Contact test@example.com for help.");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, "test@example.com");
        assert_eq!(spans[0].1, ProtectedKind::Email);
    }

    #[test]
    fn www_开头的地址也算_url() {
        let spans = kinds("Visit www.example.com today.");
        assert_eq!(spans[0].0, "www.example.com");
    }

    #[test]
    fn 模板变量与双花括号() {
        let spans = kinds("Hello {name}, order {{ user.order_id }} and ${total}.");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].0, "{name}");
        assert_eq!(spans[1].0, "{{ user.order_id }}");
        assert_eq!(spans[2].0, "${total}");
    }

    #[test]
    fn 格式化占位符而不是百分号数字() {
        let spans = kinds("Value %s at 15% and %1$s plus %.2f and {}.");
        let texts: Vec<&str> = spans.iter().map(|(t, _)| *t).collect();
        assert!(texts.contains(&"%s"));
        assert!(texts.contains(&"%1$s"));
        assert!(texts.contains(&"%.2f"));
        assert!(texts.contains(&"15%"));
        assert!(texts.contains(&"{}"));
        assert!(!texts.contains(&"%"), "{texts:?}");
    }

    #[test]
    fn 代码片段包含反引号且内部不再细分() {
        let spans = kinds("Run `curl https://example.com/{id}` now.");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, "`curl https://example.com/{id}`");
        assert_eq!(spans[0].1, ProtectedKind::Code);
    }

    #[test]
    fn 三反引号代码块整体保护() {
        let text = "before\n```rust\nlet x = 1;\n```\nafter";
        let spans = kinds(text);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].0.starts_with("```rust"));
        assert!(spans[0].0.ends_with("```"));
    }

    #[test]
    fn html_标签与模板标签() {
        let spans = kinds("Use <b>bold</b> in {% if user %}view{% endif %}.");
        let texts: Vec<&str> = spans.iter().map(|(t, _)| *t).collect();
        assert!(texts.contains(&"<b>"), "{texts:?}");
        assert!(texts.contains(&"</b>"));
        assert!(texts.contains(&"{% if user %}"));
        assert!(texts.contains(&"{% endif %}"));
    }

    #[test]
    fn 编号与关键数字格式() {
        let spans = kinds("Order #10248 shipped on 2026-11-11 with v2.1.3 at 0x1F.");
        let texts: Vec<&str> = spans.iter().map(|(t, _)| *t).collect();
        assert!(texts.contains(&"#10248"), "{texts:?}");
        assert!(texts.contains(&"2026-11-11"));
        assert!(texts.contains(&"2.1.3"), "{texts:?}");
        assert!(texts.contains(&"0x1F"));
    }

    #[test]
    fn 普通数量不保护() {
        assert!(kinds("I have three books and 3 apples.").is_empty());
    }

    #[test]
    fn 路径默认不保护可显式打开() {
        let text = "Open C:\\Program Files\\App\\config.ini or /var/log/app.log now.";
        assert!(kinds(text).is_empty());
        let spans = protected_spans_with(text, &SpanPolicy::everything());
        let texts: Vec<&str> = spans.iter().map(|s| &text[s.start..s.end]).collect();
        assert!(
            texts.contains(&"C:\\Program Files\\App\\config.ini"),
            "{texts:?}"
        );
        assert!(texts.contains(&"/var/log/app.log"), "{texts:?}");
    }

    #[test]
    fn 单斜杠不算路径() {
        let spans = protected_spans_with("this and/or that", &SpanPolicy::everything());
        assert!(spans.is_empty(), "{spans:?}");
    }

    #[test]
    fn 标识符默认可配置() {
        let text = "Set API_KEY and user_id before starting.";
        assert!(kinds(text).is_empty());
        let spans = protected_spans_with(text, &SpanPolicy::everything());
        let texts: Vec<&str> = spans.iter().map(|s| &text[s.start..s.end]).collect();
        assert!(texts.contains(&"API_KEY"), "{texts:?}");
        assert!(texts.contains(&"user_id"), "{texts:?}");
    }

    #[test]
    fn 函数调用不被当成标识符() {
        let spans = protected_spans_with("Call get_user( now", &SpanPolicy::everything());
        assert!(spans.is_empty(), "{spans:?}");
    }

    #[test]
    fn 中文与_unicode_混合不劈开字节() {
        let text = "订单 {id} 已发货 🚚，链接 https://example.com/x。";
        let spans = kinds(text);
        let texts: Vec<&str> = spans.iter().map(|(t, _)| *t).collect();
        assert!(texts.contains(&"{id}"));
        assert!(texts.contains(&"https://example.com/x"), "{texts:?}");
    }

    #[test]
    fn 切分片段覆盖全文且不重叠() {
        let text = "Run `a` with {b} at https://x.example/y and #12.";
        let segments = split_protected(text);
        let rebuilt: String = segments
            .iter()
            .map(|s| match s {
                Segment::Plain(t) | Segment::Protected(t) => *t,
            })
            .collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn 单词判定() {
        assert!(looks_like_single_word("hello"));
        assert!(looks_like_single_word("猫"));
        assert!(!looks_like_single_word("hello world"));
        assert!(!looks_like_single_word("hello."));
        assert!(!looks_like_single_word(""));
        // 中文整句不能被当成单词
        assert!(!looks_like_single_word("我喜欢计算机"));
    }
}
