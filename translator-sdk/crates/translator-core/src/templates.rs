//! 固定 UI 文案的整句匹配。
//!
//! # 适用边界（§8.1）
//!
//! 只处理**确定性文本**：`Save`、`Cancel`、`File not found`、`Retry`。
//! 不得因为"某个模型把 `She went to school.` 翻得不好"就把它加进模板表——
//! 那正是旧架构反复堆规则、最后变成半成品规则引擎的起点。
//!
//! # 匹配方式（§8.2）
//!
//! | 方式 | 含义 | 何时用 |
//! |---|---|---|
//! | [`MatchMode::Exact`]（默认） | 去掉首尾空白后逐字比较 | 绝大多数 UI 文案 |
//! | [`MatchMode::Normalized`] | 折叠空白 + 英文小写 + 去句末标点 | 明确声明容忍大小写/标点的条目 |
//!
//! `Really?` 与 `Really!` 是不同语气，默认不能被合并成一条。
//!
//! # 作用域（§8.3）
//!
//! 同一个英文词在不同软件里可能对应不同中文（`Settings` → 设置/设定）。
//! 因此条目可以带 `app` / `page`，匹配时**更具体的作用域优先**。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use translator_types::LanguageTag;

/// 匹配方式。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// 逐字比较（去掉首尾空白）。
    #[default]
    Exact,
    /// 折叠空白 + 小写 + 去句末标点。仅对明确声明的条目生效。
    Normalized,
}

/// 模板作用域。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateScope {
    /// 应用标识（包名/可执行名）。`None` 表示不限定。
    pub app: Option<String>,
    /// 页面/界面标识。
    pub page: Option<String>,
}

impl TemplateScope {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn app(app: impl Into<String>) -> Self {
        Self {
            app: Some(app.into()),
            page: None,
        }
    }

    pub fn app_page(app: impl Into<String>, page: impl Into<String>) -> Self {
        Self {
            app: Some(app.into()),
            page: Some(page.into()),
        }
    }
}

/// 模板表契约。
pub trait SentenceTemplates: Send + Sync {
    fn id(&self) -> &str;
    fn version(&self) -> u32;
    /// 无作用域的整句匹配（等价于 [`TemplateScope::none`]）。
    fn match_exact(&self, text: &str, source: &LanguageTag, target: &LanguageTag)
        -> Option<String>;
    /// 带作用域的匹配。默认实现忽略作用域。
    fn match_scoped(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
        scope: &TemplateScope,
    ) -> Option<String> {
        let _ = scope;
        self.match_exact(text, source, target)
    }
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
    #[serde(default)]
    pairs: Vec<LegacyPair>,
    #[serde(default)]
    entries: Vec<EntrySpec>,
}

/// schema_version = 1 的旧条目（隐含 Normalized 语义）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPair {
    source: String,
    target: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntrySpec {
    source: String,
    target: String,
    #[serde(default)]
    r#match: MatchMode,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    page: Option<String>,
}

#[derive(Clone, Debug)]
struct CompiledEntry {
    source: String,
    target: String,
    mode: MatchMode,
    app: Option<String>,
    page: Option<String>,
    /// 声明顺序，用于同作用域下的确定性选择。
    order: usize,
}

/// 编译后的只读模板表。
#[derive(Clone, Debug)]
pub struct TemplateTable {
    id: String,
    version: u32,
    source: LanguageTag,
    target: LanguageTag,
    entries: Vec<CompiledEntry>,
    exact: HashMap<String, Vec<usize>>,
    normalized: HashMap<String, Vec<usize>>,
}

impl TemplateTable {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, TemplateError> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    pub fn from_json(json: &str) -> Result<Self, TemplateError> {
        let spec: Spec = serde_json::from_str(json)?;
        if !matches!(spec.schema_version, 1 | 2) {
            return Err(TemplateError::Invalid(format!(
                "不支持的 schema_version={}",
                spec.schema_version
            )));
        }
        if spec.id.trim().is_empty() {
            return Err(TemplateError::Invalid("id 不能为空".to_string()));
        }
        let source = LanguageTag::new(&spec.source)
            .map_err(|e| TemplateError::Invalid(format!("source 非法：{e}")))?;
        let target = LanguageTag::new(&spec.target)
            .map_err(|e| TemplateError::Invalid(format!("target 非法：{e}")))?;

        let mut entries = Vec::new();
        let mut order = 0usize;
        for pair in spec.pairs {
            if pair.source.trim().is_empty() || pair.target.trim().is_empty() {
                return Err(TemplateError::Invalid("source/target 不能为空".to_string()));
            }
            entries.push(CompiledEntry {
                source: pair.source,
                target: pair.target,
                // 旧 schema 的条目是在"折叠空白 + 小写 + 去句末标点"的语义下写的，
                // 迁移期保留原行为，否则已有模板会突然全部失配。
                mode: MatchMode::Normalized,
                app: None,
                page: None,
                order,
            });
            order += 1;
        }
        for entry in spec.entries {
            if entry.source.trim().is_empty() || entry.target.trim().is_empty() {
                return Err(TemplateError::Invalid("source/target 不能为空".to_string()));
            }
            entries.push(CompiledEntry {
                source: entry.source,
                target: entry.target,
                mode: entry.r#match,
                app: entry.app,
                page: entry.page,
                order,
            });
            order += 1;
        }
        if entries.is_empty() {
            return Err(TemplateError::Invalid("模板不能为空".to_string()));
        }

        let mut exact: HashMap<String, Vec<usize>> = HashMap::new();
        let mut normalized: HashMap<String, Vec<usize>> = HashMap::new();
        for (idx, entry) in entries.iter().enumerate() {
            let bucket = match entry.mode {
                MatchMode::Exact => &mut exact,
                MatchMode::Normalized => &mut normalized,
            };
            let key = match entry.mode {
                MatchMode::Exact => entry.source.trim().to_string(),
                MatchMode::Normalized => normalize_key(&entry.source),
            };
            let slot = bucket.entry(key).or_default();
            // 相同作用域下的重复声明必须报错，不能"最后一条胜出"
            if slot
                .iter()
                .any(|other| entries[*other].app == entry.app && entries[*other].page == entry.page)
            {
                return Err(TemplateError::Invalid(format!(
                    "模板重复（同作用域）：{}",
                    entry.source
                )));
            }
            slot.push(idx);
        }

        Ok(Self {
            id: spec.id,
            version: spec.version,
            source,
            target,
            entries,
            exact,
            normalized,
        })
    }

    pub fn as_arc(self) -> Arc<dyn SentenceTemplates> {
        Arc::new(self)
    }

    /// 表的作用域是否覆盖给定作用域。
    fn covers(entry: &CompiledEntry, scope: &TemplateScope) -> bool {
        let app_ok = match (&entry.app, &scope.app) {
            (None, _) => true,
            (Some(want), Some(have)) => want == have,
            (Some(_), None) => false,
        };
        let page_ok = match (&entry.page, &scope.page) {
            (None, _) => true,
            (Some(want), Some(have)) => want == have,
            (Some(_), None) => false,
        };
        app_ok && page_ok
    }

    /// 从候选里挑"作用域最具体"的一条；同具体度时按声明顺序。
    fn pick(&self, candidates: &[usize], scope: &TemplateScope) -> Option<String> {
        let mut best: Option<(usize, &CompiledEntry)> = None;
        for idx in candidates {
            let entry = &self.entries[*idx];
            if !Self::covers(entry, scope) {
                continue;
            }
            let specificity = entry.app.is_some() as usize + entry.page.is_some() as usize;
            match best {
                Some((best_spec, _)) if best_spec > specificity => {}
                Some((best_spec, best_entry)) if best_spec == specificity => {
                    // 同具体度：先比声明顺序（先声明优先），保证确定性
                    if entry.order < best_entry.order {
                        best = Some((specificity, entry));
                    }
                }
                _ => best = Some((specificity, entry)),
            }
        }
        best.map(|(_, entry)| entry.target.clone())
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
        self.match_scoped(text, source, target, &TemplateScope::none())
    }

    fn match_scoped(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
        scope: &TemplateScope,
    ) -> Option<String> {
        if source != &self.source || target != &self.target {
            return None;
        }
        // 先精确，再归一化：精确命中永远优先
        if let Some(candidates) = self.exact.get(text.trim()) {
            if let Some(hit) = self.pick(candidates, scope) {
                return Some(hit);
            }
        }
        let key = normalize_key(text);
        self.normalized
            .get(&key)
            .and_then(|candidates| self.pick(candidates, scope))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> LanguageTag {
        LanguageTag::new("en").unwrap()
    }
    fn zh() -> LanguageTag {
        LanguageTag::new("zh-Hans").unwrap()
    }

    fn v2_table() -> TemplateTable {
        TemplateTable::from_json(
            r#"{
              "schema_version": 2,
              "id": "templates.ui",
              "version": 2,
              "source": "en",
              "target": "zh-Hans",
              "entries": [
                {"source":"Save","target":"保存"},
                {"source":"File not found","target":"找不到文件"},
                {"source":"Settings","target":"设置"},
                {"source":"Settings","target":"偏好设置","app":"com.acme.editor"},
                {"source":"Settings","target":"选项","app":"com.acme.editor","page":"advanced"},
                {"source":"Really?","target":"真的吗？"},
                {"source":"Really!","target":"真的！"},
                {"source":"Cancel","target":"取消","match":"normalized"}
              ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn 精确匹配默认区分句末标点() {
        let t = v2_table();
        assert_eq!(t.match_exact("Save", &en(), &zh()).as_deref(), Some("保存"));
        assert_eq!(
            t.match_exact("Really?", &en(), &zh()).as_deref(),
            Some("真的吗？")
        );
        assert_eq!(
            t.match_exact("Really!", &en(), &zh()).as_deref(),
            Some("真的！")
        );
        // 精确模式下 `Save.` 不命中 `Save`
        assert_eq!(t.match_exact("Save.", &en(), &zh()), None);
    }

    #[test]
    fn 归一化匹配只在显式声明时生效() {
        let t = v2_table();
        // Cancel 声明了 normalized
        assert_eq!(
            t.match_exact("cancel!", &en(), &zh()).as_deref(),
            Some("取消")
        );
        assert_eq!(
            t.match_exact("  CANCEL  ", &en(), &zh()).as_deref(),
            Some("取消")
        );
        // Save 没声明，大小写不同就不命中
        assert_eq!(t.match_exact("save", &en(), &zh()), None);
    }

    #[test]
    fn 作用域更具体的优先() {
        let t = v2_table();
        let generic = t.match_scoped("Settings", &en(), &zh(), &TemplateScope::none());
        assert_eq!(generic.as_deref(), Some("设置"));

        let app = t.match_scoped(
            "Settings",
            &en(),
            &zh(),
            &TemplateScope::app("com.acme.editor"),
        );
        assert_eq!(app.as_deref(), Some("偏好设置"));

        let page = t.match_scoped(
            "Settings",
            &en(),
            &zh(),
            &TemplateScope::app_page("com.acme.editor", "advanced"),
        );
        assert_eq!(page.as_deref(), Some("选项"));

        // 作用域不匹配时回落通用条目
        let other_app = t.match_scoped("Settings", &en(), &zh(), &TemplateScope::app("other"));
        assert_eq!(other_app.as_deref(), Some("设置"));
    }

    #[test]
    fn 部分匹配不返回() {
        let t = v2_table();
        assert_eq!(t.match_exact("Please Save", &en(), &zh()), None);
        assert_eq!(t.match_exact("Sav", &en(), &zh()), None);
        assert_eq!(t.match_exact("Save my file", &en(), &zh()), None);
    }

    #[test]
    fn 语言对不匹配不返回() {
        let t = v2_table();
        let ja = LanguageTag::new("ja").unwrap();
        assert_eq!(t.match_exact("Save", &en(), &ja), None);
    }

    #[test]
    fn 旧格式仍然可用且保持归一化语义() {
        let t = TemplateTable::from_json(
            r#"{"schema_version":1,"id":"t","version":1,"source":"en","target":"zh-Hans",
                "pairs":[{"source":"Good morning.","target":"早上好。"}]}"#,
        )
        .unwrap();
        assert_eq!(
            t.match_exact("good morning", &en(), &zh()).as_deref(),
            Some("早上好。")
        );
    }

    #[test]
    fn 同作用域重复声明报错() {
        let err = TemplateTable::from_json(
            r#"{"schema_version":2,"id":"t","version":1,"source":"en","target":"zh-Hans",
                "entries":[{"source":"Save","target":"保存"},{"source":"Save","target":"存储"}]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, TemplateError::Invalid(_)), "{err}");
    }

    #[test]
    fn 不同作用域可以对应不同译文() {
        let t = v2_table();
        assert_eq!(t.template_count(), 8);
    }

    #[test]
    fn 空表报错() {
        let err = TemplateTable::from_json(
            r#"{"schema_version":2,"id":"t","version":1,"source":"en","target":"zh-Hans","entries":[]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, TemplateError::Invalid(_)));
    }

    #[test]
    fn 未知字段被拒绝() {
        let err = TemplateTable::from_json(
            r#"{"schema_version":2,"id":"t","version":1,"source":"en","target":"zh-Hans",
                "entries":[{"source":"Save","target":"保存","typo":1}]}"#,
        )
        .unwrap_err();
        assert!(matches!(err, TemplateError::Json(_)), "{err}");
    }
}
