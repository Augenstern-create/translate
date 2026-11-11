//! 规则引擎契约。
//!
//! `translator-core` 只定义**接口**，不认识 JSON、不认识文件格式，
//! 也不引入任何解析依赖（保持 `translator-types` / `translator-core` 的零重依赖）。
//! 具体实现（声明式 JSON + 构建期编译 + 金样本校验）在 `translator-rules`，
//! 由 `translator-api` 注入。
//!
//! 设计文档 §7.2 的边界：**只做低成本、可测试、不改变语义的规则**
//! （规范化、术语替换、缩写展开、保护类规则），完整句子一律交给 NMT。
//!
//! # 为什么规则在「源语言解析之后、占位符保护之前」执行
//!
//! 1. 规则按语言对生效，必须先知道源语言；
//! 2. 规则可能产生新的 URL/邮箱/代码形态，交给后面的保护环节统一处理，
//!    避免规则自己实现一套占位符逻辑；
//! 3. 规则改写后的文本参与路由（`looks_like_single_word`）与缓存键，
//!    因此同一句原文在不同规则包下不会串用缓存。

use std::fmt;

use translator_types::{LanguageTag, TranslationWarning};

/// 一次规则作用的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleOutcome {
    /// 规则处理后的文本。
    pub text: String,
    /// 实际**改变了文本**的规则 id（按执行顺序）。
    ///
    /// 命中条件但文本没变（例如已经规范化过）的规则不记入，
    /// 否则“生效规则”列表会被大量空操作淹没。
    pub applied: Vec<String>,
    /// 规则层产生的警告（例如术语约束未满足）。
    pub warnings: Vec<TranslationWarning>,
}

impl RuleOutcome {
    /// 无规则或规则未命中时的原样结果。
    pub fn unchanged(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            applied: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// 文本规则包。
///
/// 实现必须满足：
/// - **纯函数**：同样输入同样输出，不依赖时间/网络/随机数；
/// - `Send + Sync + Debug`：规则包随 `Translator` 跨线程共享；
/// - `apply` 不得 panic（输入是不可信数据，见设计文档 §7.3）。
pub trait TextRules: Send + Sync + fmt::Debug {
    /// 规则包 id，形如 `rules.zh-en.glossary`。
    fn id(&self) -> &str;

    /// 规则包版本。**修改任何规则都必须换版本**，否则旧缓存会被错误复用。
    fn version(&self) -> u32;

    /// 规则包内规则总数（诊断用）。
    fn rule_count(&self) -> usize;

    /// 应用规则。`source` / `target` 决定哪些规则生效。
    fn apply(&self, text: &str, source: &LanguageTag, target: &LanguageTag) -> RuleOutcome;
}

/// 空规则包：注册表未配置规则时的默认实现。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRules;

impl TextRules for NoRules {
    fn id(&self) -> &str {
        "rules.none"
    }

    fn version(&self) -> u32 {
        1
    }

    fn rule_count(&self) -> usize {
        0
    }

    fn apply(&self, text: &str, _source: &LanguageTag, _target: &LanguageTag) -> RuleOutcome {
        RuleOutcome::unchanged(text)
    }
}
