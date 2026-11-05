//! 输出侧术语约束（P2.4）。
//!
//! # 为什么必须在**输出侧**再做一次
//!
//! 输入侧规则（`TextRules`）只能改"送进引擎的文本"。但译文里的术语是**引擎写出来的**：
//! 它可能把 `AI` 写成 `人工智能`、把 `large language model` 写成 `LLM`，
//! 也可能保留英文缩写而术语表要求中文全称。想约束"最终交给用户的字符串"，
//! 只能在引擎产出之后、返回之前做一次检查。
//!
//! 这与 ADR-003（词典 vs 句子分开建模）不冲突：这里约束的是**译文用词**，
//! 不是"这句话该不该由词典来译"。
//!
//! # 契约
//!
//! [`TargetTermGuard`] 的实现者必须做到三件事，缺一不可：
//!
//! 1. **不碰受保护跨度**：URL / 邮箱 / `{模板变量}` / 反引号代码里的内容一律不得改写。
//!    实现方自己调用 [`crate::text::split_protected`] 重新切分即可（还原后的文本
//!    仍然能被同一套扫描器认出来，因为还原就是把原字符串原样插回去）。
//! 2. **匹配必须确定**：同一输入在任何平台、任何次数下产出同样的结果，
//!    不允许依赖 `HashMap` 迭代顺序。
//! 3. **改写必须可解释**：每个被替换的地方都要作为 [`TermViolation`] 报出来，
//!    让宿主能说清"我改了哪里、为什么"。
//!
//! # 与 `TranslationWarning` 的关系
//!
//! **不新增枚举变体**。`TranslationWarning::TermMismatch` 在 P0 就存在，
//! 但它只能携带一个字符串，装不下"位置 + 期望值 + 实际值"。
//! 给公开枚举加变体会破坏宿主的穷尽匹配（ADR-008），因此
//! 结构化信息走 [`crate::diagnostics::TranslationDiagnostics`]，
//! 警告只保留"有术语不一致"这一个事实。

use translator_types::LanguageTag;

/// 一处术语违规。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TermViolation {
    /// 期望的术语（术语表里的 `preferred`）。
    pub preferred: String,
    /// 实际出现的非期望写法。
    pub found: String,
    /// 在**输出文本**里的字节偏移（UTF-8，`text[offset..]` 处开始）。
    pub offset: usize,
}

impl TermViolation {
    pub fn new(preferred: impl Into<String>, found: impl Into<String>, offset: usize) -> Self {
        Self {
            preferred: preferred.into(),
            found: found.into(),
            offset,
        }
    }
}

impl std::fmt::Display for TermViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "「{}」应为「{}」（偏移 {}）",
            self.found, self.preferred, self.offset
        )
    }
}

/// 术语检查结果。
#[derive(Clone, Debug, Default)]
pub struct TermOutcome {
    /// 处理后的文本。`Warn` 策略下调用方应忽略它，`Enforce` 下才采用。
    pub text: String,
    /// 发现的违规，按偏移升序（确定性要求）。
    pub violations: Vec<TermViolation>,
}

impl TermOutcome {
    /// 无违规、文本不变。
    pub fn unchanged(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            violations: Vec::new(),
        }
    }
}

/// 输出侧术语约束契约。
///
/// `Send + Sync`：协调器会被多线程共享（`Translator` 是 `Sync`）。
pub trait TargetTermGuard: Send + Sync {
    /// 术语表 id，进诊断与缓存键。
    fn id(&self) -> &str;
    /// 术语表版本，进诊断与缓存键。
    fn version(&self) -> u32;
    /// 检查并（可选地）改写译文。
    fn apply(&self, text: &str, source: &LanguageTag, target: &LanguageTag) -> TermOutcome;
}

/// 空实现：什么都不做。用于"没配术语表"的默认路径。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoTermGuard;

impl TargetTermGuard for NoTermGuard {
    fn id(&self) -> &str {
        "terms.none"
    }

    fn version(&self) -> u32 {
        0
    }

    fn apply(&self, text: &str, _source: &LanguageTag, _target: &LanguageTag) -> TermOutcome {
        TermOutcome::unchanged(text)
    }
}

/// 术语约束策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TermPolicy {
    /// 完全不检查（性能优先，或术语表只用于别处）。
    Off,
    /// 只报告违规、**不改文本**。审计与灰度期用这个。
    Warn,
    /// 按术语表改写文本（不碰受保护跨度），并报告每处替换。
    ///
    /// 这是默认值：宿主既然显式提供了术语表，通常就是要它生效。
    /// 想先看效果再决定，显式写 `.term_policy(TermPolicy::Warn)`。
    #[default]
    Enforce,
}

impl TermPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Warn => "warn",
            Self::Enforce => "enforce",
        }
    }

    /// 是否要把守卫改过的文本用起来。
    pub const fn applies_text(self) -> bool {
        matches!(self, Self::Enforce)
    }

    /// 是否要跑守卫（`Off` 时连扫描都省掉）。
    pub const fn runs(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(tag: &str) -> LanguageTag {
        LanguageTag::new(tag).unwrap()
    }

    #[test]
    fn 空守卫不改文本() {
        let outcome = NoTermGuard.apply("hello world", &lang("en"), &lang("zh-Hans"));
        assert_eq!(outcome.text, "hello world");
        assert!(outcome.violations.is_empty());
        assert_eq!(NoTermGuard.version(), 0);
    }

    #[test]
    fn 策略语义() {
        assert!(TermPolicy::Enforce.applies_text());
        assert!(!TermPolicy::Warn.applies_text());
        assert!(TermPolicy::Warn.runs());
        assert!(!TermPolicy::Off.runs());
        assert_eq!(TermPolicy::default(), TermPolicy::Enforce);
    }

    #[test]
    fn 违规展示包含期望与实际() {
        let v = TermViolation::new("large language model", "LLM", 7);
        let text = v.to_string();
        assert!(text.contains("LLM"));
        assert!(text.contains("large language model"));
        assert!(text.contains('7'));
    }
}
