//! 完整句子模板（P16-R / R2）。
//!
//! 只处理**完整输入精确匹配**的固定表达与 UI 文案；不做逐词拼接。
//! 模板必须带正例/反例，只有整句匹配才直接返回；否则交给 NMT。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use translator_types::{LanguageTag, TranslationError, TranslationErrorCode};

/// 模板表契约。
pub trait SentenceTemplates: Send + Sync {
    fn id(&self) -> &str;
    fn version(&self) -> u32;
    /// 完整输入精确匹配；`None` 表示不覆盖。
    fn match_exact(&self, text: &str, source: &LanguageTag, target: &LanguageTag)
        -> Option<String>;
    fn template_count(&self) -> usize;
}

#[derive(Debug)]
pub enum TemplateError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "读取模板失败：{e}"),
            Self::Json(e) => write!(f, "模板 JSON 非法：{e}"),
            Self::Invalid(msg) => write!(f, "模板不合法：{msg}"),
        }
    }
}
impl std::error::Error for TemplateError {}
impl From<std::io::Error> for TemplateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for TemplateError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    schema_version: u16,
    id: String,
    version: u32,
    source: String,
    target: String,
    pairs: Vec<Pair>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    source: String,
    target: String,
}

/// 编译后的只读模板表。
#[derive(Debug, Clone)]
pub struct TemplateTable {
    id: String,
    version: u32,
    source: LanguageTag,
    target: LanguageTag,
    entries: HashMap<String, String>,
}

impl TemplateTable {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, TemplateError> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    pub fn from_json(json: &str) -> Result<Self, TemplateError> {
        let spec: Spec = serde_json::from_str(json)?;
        if spec.schema_version != 1 {
            return Err(TemplateError::Invalid(format!(
                "不支持的 schema_version={}",
                spec.schema_version
            )));
        }
        if spec.id.trim().is_empty() {
            return Err(TemplateError::Invalid("id 不能为空".to_string()));
        }
        if spec.pairs.is_empty() {
            return Err(TemplateError::Invalid("模板不能为空".to_string()));
        }
        let source = LanguageTag::new(&spec.source)
            .map_err(|e| TemplateError::Invalid(format!("source 非法：{e}")))?;
        let target = LanguageTag::new(&spec.target)
            .map_err(|e| TemplateError::Invalid(format!("target 非法：{e}")))?;
        let mut entries = HashMap::new();
        for pair in spec.pairs {
            if pair.source.trim().is_empty() || pair.target.trim().is_empty() {
                return Err(TemplateError::Invalid("source/target 不能为空".to_string()));
            }
            let key = normalize_key(&pair.source);
            if entries.insert(key.clone(), pair.target.clone()).is_some() {
                return Err(TemplateError::Invalid(format!("模板重复：{key}")));
            }
        }
        Ok(Self {
            id: spec.id,
            version: spec.version,
            source,
            target,
            entries,
        })
    }

    pub fn as_arc(self) -> Arc<dyn SentenceTemplates> {
        Arc::new(self)
    }
}

impl SentenceTemplates for TemplateTable {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> u32 {
        self.version
    }

    fn match_exact(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> Option<String> {
        if source != &self.source || target != &self.target {
            return None;
        }
        self.entries.get(&normalize_key(text)).cloned()
    }

    fn template_count(&self) -> usize {
        self.entries.len()
    }
}

/// 英文大小写/空白归一，并去掉句末标点；模板与输入走同一函数。
pub fn normalize_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for c in text.trim().chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(c.to_ascii_lowercase());
            last_space = false;
        }
    }
    out.trim_end_matches(['.', '!', '?', '。', '！', '？'])
        .trim()
        .to_string()
}

/// 把模板加载错误转成 SDK 统一错误（构建期失败，不静默退化）。
pub fn template_error_to_translation(err: TemplateError) -> TranslationError {
    TranslationError::new(
        TranslationErrorCode::InvalidRequest,
        format!("模板加载失败：{err}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> TemplateTable {
        TemplateTable::from_json(
            r#"{
              "schema_version":1,"id":"templates.test","version":1,
              "source":"en","target":"zh-Hans",
              "pairs":[
                {"source":"Good morning.","target":"早上好。"},
                {"source":"Please restart the server.","target":"请重新启动服务器。"}
              ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn 完整匹配大小写和句末标点不敏感() {
        let t = table();
        let en = LanguageTag::new("en").unwrap();
        let zh = LanguageTag::new("zh-Hans").unwrap();
        assert_eq!(
            t.match_exact("good morning", &en, &zh).as_deref(),
            Some("早上好。")
        );
        assert_eq!(
            t.match_exact("  Please   restart the server! ", &en, &zh)
                .as_deref(),
            Some("请重新启动服务器。")
        );
    }

    #[test]
    fn 部分匹配不返回() {
        let t = table();
        let en = LanguageTag::new("en").unwrap();
        let zh = LanguageTag::new("zh-Hans").unwrap();
        assert_eq!(t.match_exact("Please restart", &en, &zh), None);
        assert_eq!(t.match_exact("Good morning my friend", &en, &zh), None);
    }

    #[test]
    fn 语言对不匹配不返回() {
        let t = table();
        let en = LanguageTag::new("en").unwrap();
        let ja = LanguageTag::new("ja").unwrap();
        assert_eq!(t.match_exact("Good morning.", &en, &ja), None);
    }
}
