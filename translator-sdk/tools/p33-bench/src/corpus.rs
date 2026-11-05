//! 固定语料：20 / 100 / 500 字的英文源句。
//!
//! 生成方式必须**可复现**：按词边界截断到不少于目标字符数，不随机、不依赖时间。

const BASE: &str = "The library opens at nine in the morning, and the reading room on the second \
floor stays quiet until noon. Visitors may borrow up to five books at a time, and the return desk \
closes thirty minutes before the building. A small cafe near the entrance serves tea and simple \
meals, and the staff will help you find a seat when the tables are full. On rainy days the queue \
outside is longer, so the librarian often opens the side door earlier than usual.";

/// 造一段至少 `chars` 个字符的文本（按词边界截断）。
pub fn text_of_chars(chars: usize) -> String {
    let mut out = String::new();
    while out.chars().count() < chars {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(BASE);
    }
    // 截到目标长度附近的词边界
    if out.chars().count() > chars {
        let mut cut = 0usize;
        for (idx, ch) in out.char_indices() {
            if idx >= chars {
                break;
            }
            if ch == ' ' {
                cut = idx;
            }
        }
        if cut > 0 {
            out.truncate(cut);
        }
    }
    out
}

/// 一句话里带 2 个受保护跨度（用于"分段模式 vs 纯文本"的对照）。
pub fn protected_sentence() -> String {
    "Hello {name}, please open https://example.com/docs before nine.".to_string()
}

/// 并发压测用的互不相同的句子（避免缓存命中掩盖真实吞吐）。
pub fn distinct(index: usize) -> String {
    format!("Request number {index}: the reading room opens at nine and closes at six.")
}
