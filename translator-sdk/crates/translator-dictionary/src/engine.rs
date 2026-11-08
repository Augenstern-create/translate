//! 把 [`DictionaryPackage`] 适配成插件契约的两套接口：
//! 一个 [`TranslationEngine`]（单词直译）和一个 [`DictionaryProvider`]（查词）。

use std::sync::Arc;
use std::time::Instant;

use translator_plugin_api::{BoxFuture, DictionaryProvider, TranslationEngine};
use translator_types::{
    Capability, DictionaryEntry, EngineDescriptor, LanguageTag, LookupRequest, PluginApiVersion,
    ProviderDescriptor, TranslateRequest, TranslationError, TranslationErrorCode,
    TranslationResult,
};

use crate::{DictionaryError, DictionaryPackage};

/// 基于词典包的翻译引擎：只做**精确词条**直译。
///
/// 未命中时返回 `CapabilityUnavailable`，由 Coordinator 在 `AutoLocalFirst`
/// 下自动回退到本地 NMT（设计文档 §6）。
pub struct DictionaryEngine {
    package: Arc<DictionaryPackage>,
    priority: i32,
    /// 包级唯一 id：多词典包共存时不能都叫 `dictionary.package`。
    id: &'static str,
}

fn engine_id_of(package: &DictionaryPackage) -> &'static str {
    // 包 id 来自 manifest；这里只需在进程内唯一。字符串很小，按包泄漏一次，
    // 换取 `EngineDescriptor.id: &'static str` 的公共契约不变。
    Box::leak(format!("dictionary.package:{}", package.manifest().id).into_boxed_str())
}

impl DictionaryEngine {
    pub fn new(package: Arc<DictionaryPackage>) -> Self {
        let id = engine_id_of(&package);
        Self {
            package,
            priority: 100,
            id,
        }
    }

    pub fn with_priority(package: Arc<DictionaryPackage>, priority: i32) -> Self {
        let id = engine_id_of(&package);
        Self {
            package,
            priority,
            id,
        }
    }

    pub fn package(&self) -> &DictionaryPackage {
        &self.package
    }
}

impl TranslationEngine for DictionaryEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: self.id,
            capabilities: &[Capability::Dictionary],
            priority: self.priority,
            plugin_api: PluginApiVersion::CURRENT,
            // 引擎本身的许可是 SDK 的；数据许可在 manifest 里单独声明
            license: "Apache-2.0 OR MIT",
        }
    }

    fn supports(&self, source: &LanguageTag, target: &LanguageTag) -> bool {
        source.as_str() == self.package.language()
            && self
                .package
                .manifest()
                .target_languages
                .iter()
                .any(|t| t == target.as_str())
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            let started = Instant::now();

            let source = req.source.clone().ok_or_else(|| {
                TranslationError::new(
                    TranslationErrorCode::InvalidRequest,
                    "词典引擎需要显式源语言",
                )
            })?;

            let term = req.text.trim();
            let mut entry = self
                .package
                .lookup_exact(term)
                .map_err(TranslationError::from)?;
            if entry.is_none() {
                // 英语查询大小写不敏感：包里的 headword 可能是小写，用户输入
                // 句首大写（"Hello"）不应因此绕过词典直译。
                let lower = term.to_lowercase();
                if lower != term {
                    entry = self
                        .package
                        .lookup_exact(&lower)
                        .map_err(TranslationError::from)?;
                }
            }
            let entry = entry.ok_or_else(|| {
                TranslationError::new(
                    TranslationErrorCode::CapabilityUnavailable,
                    "词典未命中：无精确词条",
                )
            })?;

            // T1.4：不能机械取第一义。单词直译返回**前几个不同义项**，
            // 让宿主/用户看到多义性；整句翻译仍由 NMT 承担，不做词典拼接。
            let text = compose_senses(&entry.senses, 3);

            if text.is_empty() {
                return Err(TranslationError::new(
                    TranslationErrorCode::BackendFailed,
                    "词条没有释义",
                ));
            }

            Ok(TranslationResult {
                text,
                source,
                target: req.target.clone(),
                engine_id: self.id.to_string(),
                model_id: Some(self.package.manifest().id.clone()),
                elapsed_ms: started.elapsed().as_millis() as u64,
                warnings: Vec::new(),
            })
        })
    }
}

/// 把多个义项拼成一段可读的直译文本。
///
/// - 去重时忽略领域前缀（`[计] 操作系统` 与 `操作系统` 视为同一条）；
/// - 最多 `limit` 条，用中文分号连接；
/// - 总长度按字符截断，避免一个长释义把结果撑爆。
fn compose_senses(senses: &[translator_types::Sense], limit: usize) -> String {
    const MAX_CHARS: usize = 160;
    let mut picked: Vec<&str> = Vec::new();
    let mut normalized: Vec<String> = Vec::new();
    for sense in senses {
        let text = sense.definition.trim();
        if text.is_empty() {
            continue;
        }
        let key = strip_domain_prefix(text);
        if normalized.iter().any(|existing| existing == &key) {
            continue;
        }
        normalized.push(key);
        picked.push(text);
        if picked.len() >= limit {
            break;
        }
    }

    let mut out = String::new();
    for part in picked {
        let part = part.trim_end_matches(['。', '.', ' ']);
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('；');
        }
        out.push_str(part);
        if out.chars().count() >= MAX_CHARS {
            break;
        }
    }
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS).collect();
        out.push('…');
    }
    out
}

fn strip_domain_prefix(text: &str) -> String {
    if let Some(rest) = text.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return rest[end + 1..].trim().to_string();
        }
    }
    text.to_string()
}

/// 基于词典包的查词提供者。
pub struct DictionaryProviderImpl {
    package: Arc<DictionaryPackage>,
    id: &'static str,
}

impl DictionaryProviderImpl {
    pub fn new(package: Arc<DictionaryPackage>) -> Self {
        let id = Box::leak(
            format!("dictionary.package.provider:{}", package.manifest().id).into_boxed_str(),
        );
        Self { package, id }
    }

    pub fn package(&self) -> &DictionaryPackage {
        &self.package
    }
}

impl DictionaryProvider for DictionaryProviderImpl {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id,
            plugin_api: PluginApiVersion::CURRENT,
            license: "Apache-2.0 OR MIT",
        }
    }

    fn lookup<'a>(
        &'a self,
        req: &'a LookupRequest,
    ) -> BoxFuture<'a, Result<Vec<DictionaryEntry>, TranslationError>> {
        Box::pin(async move {
            // 语言不匹配时返回空列表，而不是报错：
            // 多词典共存时由 Coordinator 汇总，单个包不认识该语言属正常情况。
            if let Some(lang) = &req.language {
                if lang.as_str() != self.package.language() {
                    return Ok(Vec::new());
                }
            }

            self.package
                .lookup(req.term.trim(), req.limit)
                .map_err(|e: DictionaryError| TranslationError::from(e))
        })
    }
}
