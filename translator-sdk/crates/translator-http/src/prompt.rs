//! 提示词：受版本管理的外部资源 + 术语/占位符约束注入。
//!
//! 提示词是**产品资产**，不是散落在 Rust 里的字符串（§10）：
//!
//! ```text
//! prompts/en_zh/precise_v1.txt
//! prompts/en_zh/fast_v1.txt
//! ```
//!
//! 运行时可以从磁盘加载（便于对照评测），也可以直接用编译进二进制的内置副本
//! （`include_str!`，保证 SDK 单独分发时也能工作）。
//!
//! # 提示词里出现什么、不出现什么
//!
//! - 出现：目标语言、占位符 token 清单、命中的术语约束（§6.5：只发命中的）；
//! - 不出现：用户原文的任何额外副本、术语全表、内部策略名、模型自评指令。
//!
//! # 版本与缓存
//!
//! [`PromptTemplate::id`]（形如 `en_zh/precise_v1`）进入缓存身份。
//! 改提示词必须改版本号，否则会静默复用旧译文（§11.3）。

use std::path::Path;

use sha2::{Digest, Sha256};
use translator_types::{TermConstraint, TranslationMode};

use crate::config::BackendConfig;
use crate::errors::HttpError;
use crate::protocol::PromptMessage;

/// 内置提示词（与 `prompts/` 下的文件内容一致；由测试保证不漂移）。
pub const EMBEDDED_PRECISE_V1: &str = include_str!("../../../prompts/en_zh/precise_v1.txt");
/// 内置 Fast 提示词。
pub const EMBEDDED_FAST_V1: &str = include_str!("../../../prompts/en_zh/fast_v1.txt");

/// 一次提示词构造的输入。
pub struct PromptInput<'a> {
    /// 要翻译的文本（可能含保护标记）。
    pub text: &'a str,
    /// 本次源文**实际命中**的术语约束。
    pub glossary: &'a [TermConstraint],
    /// 文本中出现的保护 token（如 `⟦P1⟧`），逐字告知模型。
    pub placeholders: &'a [String],
    /// 上一次尝试失败时的纠正提示（例如"占位符丢失"）。
    pub retry_hint: Option<&'a str>,
}

/// 版本化的提示词模板。
#[derive(Clone, Debug)]
pub struct PromptTemplate {
    /// `en_zh/precise_v1`
    id: String,
    version: String,
    mode: TranslationMode,
    system: String,
    source_lang: String,
}

impl PromptTemplate {
    /// 从配置与磁盘构造。`prompt_dir` 为 `None` 时用内置副本。
    pub fn from_backend_config(cfg: &BackendConfig) -> Result<Self, HttpError> {
        let fast = false;
        let version = cfg.prompt_id(fast);
        let mode = TranslationMode::Precise;
        Self::resolve(
            cfg.prompt_dir.as_deref(),
            &cfg.prompt_lang_pair,
            &version,
            mode,
        )
    }

    /// 为指定模式构造提示词。
    pub fn for_mode(cfg: &BackendConfig, mode: TranslationMode) -> Result<Self, HttpError> {
        let version = cfg.prompt_id(!mode.is_precise());
        Self::resolve(
            cfg.prompt_dir.as_deref(),
            &cfg.prompt_lang_pair,
            &version,
            mode,
        )
    }

    fn resolve(
        dir: Option<&Path>,
        lang_pair: &str,
        version_id: &str,
        mode: TranslationMode,
    ) -> Result<Self, HttpError> {
        let (pair, version) = version_id
            .split_once('/')
            .ok_or_else(|| HttpError::config(format!("提示词标识非法：{version_id}")))?;
        if pair != lang_pair {
            return Err(HttpError::config(format!(
                "提示词语言对 {pair} 与配置 prompt_lang_pair={lang_pair} 不一致"
            )));
        }

        let embedded = match version {
            "precise_v1" => Some(EMBEDDED_PRECISE_V1),
            "fast_v1" => Some(EMBEDDED_FAST_V1),
            _ => None,
        };

        let system = match dir {
            Some(dir) => {
                let path = dir.join(pair).join(format!("{version}.txt"));
                if path.exists() {
                    std::fs::read_to_string(&path).map_err(|e| {
                        HttpError::config(format!("读取提示词 {} 失败：{e}", path.display()))
                    })?
                } else if let Some(embedded) = embedded {
                    // 目录存在但该版本文件缺失：用内置副本，但**不**静默改变
                    // 版本号（版本号仍来自配置，缓存身份一致）。
                    embedded.to_string()
                } else {
                    return Err(HttpError::config(format!(
                        "提示词文件不存在：{}，且该版本没有内置副本",
                        path.display()
                    )));
                }
            }
            None => embedded
                .ok_or_else(|| {
                    HttpError::config(format!(
                        "未知提示词版本 {version}：未配置 prompt_dir，且该版本没有内置副本"
                    ))
                })?
                .to_string(),
        };

        if system.trim().is_empty() {
            return Err(HttpError::config(format!("提示词 {version} 为空")));
        }

        Ok(Self {
            id: format!("{pair}/{version}"),
            version: version.to_string(),
            mode,
            system,
            source_lang: "en".to_string(),
        })
    }

    /// 内置 Precise 模板（测试与最小宿主使用）。
    pub fn embedded_precise() -> Self {
        Self {
            id: "en_zh/precise_v1".to_string(),
            version: "precise_v1".to_string(),
            mode: TranslationMode::Precise,
            system: EMBEDDED_PRECISE_V1.to_string(),
            source_lang: "en".to_string(),
        }
    }

    /// 内置 Fast 模板。
    pub fn embedded_fast() -> Self {
        Self {
            id: "en_zh/fast_v1".to_string(),
            version: "fast_v1".to_string(),
            mode: TranslationMode::Fast,
            system: EMBEDDED_FAST_V1.to_string(),
            source_lang: "en".to_string(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn mode(&self) -> TranslationMode {
        self.mode
    }

    pub fn system_text(&self) -> &str {
        &self.system
    }

    /// 提示词内容指纹：进入缓存身份，防止"同名不同内容"。
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.id.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.system.as_bytes());
        let digest = hasher.finalize();
        digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
    }

    /// 构造发给模型的完整消息序列。
    pub fn build_messages(&self, input: &PromptInput<'_>, target_lang: &str) -> Vec<PromptMessage> {
        let mut system = String::with_capacity(self.system.len() + 256);
        system.push_str(self.system.trim_end());
        system.push_str("\n\n");
        system.push_str(&format!(
            "Source language: {}. Target language: {}.\n",
            self.source_lang, target_lang
        ));

        if !input.placeholders.is_empty() {
            system.push_str(
                "\nPlaceholder tokens in the input are opaque identifiers. \
                 Copy each token exactly as written (same brackets, same number), \
                 keep every token exactly once, and never translate or drop them:\n",
            );
            for token in input.placeholders {
                system.push_str("  ");
                system.push_str(token);
                system.push('\n');
            }
        }

        if !input.glossary.is_empty() {
            system.push_str(
                "\nTerminology constraints (when the English term appears below, \
                 render it exactly as the Chinese wording given):\n",
            );
            for term in input.glossary {
                let marker = if term.strict {
                    "[required]"
                } else {
                    "[preferred]"
                };
                system.push_str(&format!(
                    "  {} \"{}\" -> \"{}\"\n",
                    marker, term.source, term.target
                ));
            }
        }

        if let Some(hint) = input.retry_hint {
            system.push_str("\nCorrection for this attempt: ");
            system.push_str(hint);
            system.push('\n');
        }

        vec![
            PromptMessage::system(system),
            PromptMessage::user(input.text),
        ]
    }

    /// 目标语言展示名（进提示词）。
    pub fn target_display(&self, target: &str) -> String {
        if target.starts_with("zh") {
            "Simplified Chinese".to_string()
        } else if target.starts_with("en") {
            "English".to_string()
        } else {
            target.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(source: &str, target: &str, strict: bool) -> TermConstraint {
        TermConstraint {
            source: source.to_string(),
            target: target.to_string(),
            strict,
            matched: "phrase".to_string(),
        }
    }

    #[test]
    fn 内置提示词包含核心约束() {
        let prompt = PromptTemplate::embedded_precise();
        let text = prompt.system_text();
        assert!(text.contains("Simplified Chinese"));
        assert!(text.contains("Preserve"));
        // §10 的六条要求至少覆盖关键几条
        assert!(text.to_lowercase().contains("do not add or omit"));
        assert!(text.to_lowercase().contains("placeholder"));
        assert!(text.to_lowercase().contains("return only the translation"));
    }

    #[test]
    fn fast_提示词比_precise_短且仍要求只返回译文() {
        let precise = PromptTemplate::embedded_precise();
        let fast = PromptTemplate::embedded_fast();
        assert!(fast.system_text().len() < precise.system_text().len());
        assert!(fast
            .system_text()
            .to_lowercase()
            .contains("return only the translation"));
        assert_eq!(fast.mode(), TranslationMode::Fast);
    }

    #[test]
    fn 消息里逐字列出占位符与命中术语() {
        let prompt = PromptTemplate::embedded_precise();
        let glossary = vec![term("large language model", "大语言模型", true)];
        let placeholders = vec!["⟦P1⟧".to_string(), "⟦P2⟧".to_string()];
        let messages = prompt.build_messages(
            &PromptInput {
                text: "Please visit ⟦P1⟧ and enter ⟦P2⟧.",
                glossary: &glossary,
                placeholders: &placeholders,
                retry_hint: None,
            },
            "zh-CN",
        );

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("⟦P1⟧"));
        assert!(messages[0].content.contains("⟦P2⟧"));
        assert!(messages[0].content.contains("large language model"));
        assert!(messages[0].content.contains("大语言模型"));
        assert!(messages[0].content.contains("[required]"));
        // 原文只出现在 user 消息里，系统消息不得重复原文
        assert!(!messages[0].content.contains("Please visit"));
        assert_eq!(messages[1].content, "Please visit ⟦P1⟧ and enter ⟦P2⟧.");
    }

    #[test]
    fn 没有术语与占位符时不产生空小节() {
        let prompt = PromptTemplate::embedded_precise();
        let messages = prompt.build_messages(
            &PromptInput {
                text: "Hello.",
                glossary: &[],
                placeholders: &[],
                retry_hint: None,
            },
            "zh-CN",
        );
        assert!(!messages[0].content.contains("Placeholder tokens"));
        assert!(!messages[0].content.contains("Terminology constraints"));
    }

    #[test]
    fn 纠正提示进入系统消息而不是原文位置() {
        let prompt = PromptTemplate::embedded_fast();
        let messages = prompt.build_messages(
            &PromptInput {
                text: "Hello {name}.",
                glossary: &[],
                placeholders: &["⟦P1⟧".to_string()],
                retry_hint: Some("上次输出丢失了占位符 ⟦P1⟧，请原样保留"),
            },
            "zh-CN",
        );
        assert!(messages[0].content.contains("Correction for this attempt"));
        assert!(messages[0].content.contains("⟦P1⟧"));
        assert_eq!(messages[1].content, "Hello {name}.");
    }

    #[test]
    fn 指纹随内容与版本变化() {
        let a = PromptTemplate::embedded_precise();
        let b = PromptTemplate::embedded_fast();
        assert_ne!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.id(), "en_zh/precise_v1");
        assert_eq!(a.version(), "precise_v1");
    }

    #[test]
    fn 未知版本在没有内置副本时报配置错误() {
        let err =
            PromptTemplate::resolve(None, "en_zh", "en_zh/precise_v99", TranslationMode::Precise)
                .unwrap_err();
        assert_eq!(err.kind, crate::errors::HttpErrorKind::Config);
    }

    #[test]
    fn 语言对不一致时报配置错误() {
        let err =
            PromptTemplate::resolve(None, "ja_zh", "en_zh/precise_v1", TranslationMode::Precise)
                .unwrap_err();
        assert!(err.message.contains("不一致"), "{err}");
    }
}
