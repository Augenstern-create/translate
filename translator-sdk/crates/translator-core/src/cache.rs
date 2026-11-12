//! 查询缓存。
//!
//! # 缓存身份（§11.2）
//!
//! 一个译文能不能复用，取决于**所有影响输出的维度**。少写一项就会出现
//! "换了模型/提示词/术语表，用户仍看到旧译文"——这类 bug 极难被发现，
//! 因为结果看起来完全正常。
//!
//! ```text
//! source_text_hash       规范化 + 打标记后的文本
//! source_language        源语言
//! target_language        目标语言
//! backend_id             引擎 id（含后端名）
//! model_id               模型标识
//! model_revision         部署版本（远程服务可能别名不变换模型）
//! prompt_version         提示词版本
//! glossary_version       术语表 id@version
//! protection_version     保护模块版本
//! translation_mode       Precise / Fast
//! context_hash           上下文身份
//! privacy_scope          隐私隔离域
//! generation_config_hash 温度/上限/扩展字段
//! ```
//!
//! # 原则（§11.3）
//!
//! - 只缓存**通过完整性校验**的结果（由协调器保证：校验失败不写缓存）；
//! - 用户之间必须隔离（`privacy_scope`）；
//! - 术语表、提示词、模型身份变化必须让缓存失效（都在键里）；
//! - 不把译文/原文写进日志；
//! - 第一阶段只做进程内缓存，容量用 FIFO 淘汰。

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use translator_types::LanguageTag;
use translator_types::{TextKind, TranslationMode, TranslationResult, TranslationRoute};

use crate::diagnostics::TranslationDiagnostics;

/// 缓存条目：译文 + 当时算出来的诊断。
///
/// 诊断也要缓存，否则同一次请求"第一次报术语违规、第二次不报"，
/// 会让宿主以为问题自己好了。易变字段（耗时、`cache_hit`）在写入前清零。
#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub result: TranslationResult,
    pub diagnostics: TranslationDiagnostics,
}

/// 影响译文结果的全部维度。构造后不可变。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheIdentity {
    pub source: String,
    pub target: String,
    pub backend_id: String,
    pub model_id: Option<String>,
    pub model_revision: Option<String>,
    pub prompt_version: String,
    pub glossary_version: String,
    pub protection_version: String,
    pub mode: TranslationMode,
    pub text_kind: TextKind,
    pub strategy: String,
    pub route: TranslationRoute,
    pub context_fingerprint: String,
    pub privacy_scope: String,
    pub generation_config_hash: String,
}

impl CacheIdentity {
    /// 最小可用身份（测试与词典路径用）。
    pub fn new(backend_id: impl Into<String>, mode: TranslationMode) -> Self {
        Self {
            source: String::new(),
            target: String::new(),
            backend_id: backend_id.into(),
            model_id: None,
            model_revision: None,
            prompt_version: String::new(),
            glossary_version: String::new(),
            protection_version: String::new(),
            mode,
            text_kind: TextKind::Auto,
            strategy: "single".to_string(),
            route: TranslationRoute::Remote,
            context_fingerprint: String::new(),
            privacy_scope: "local".to_string(),
            generation_config_hash: String::new(),
        }
    }

    pub fn with_languages(mut self, source: impl Into<String>, target: impl Into<String>) -> Self {
        self.source = source.into();
        self.target = target.into();
        self
    }

    pub fn with_model(mut self, model_id: Option<String>, revision: Option<String>) -> Self {
        self.model_id = model_id;
        self.model_revision = revision;
        self
    }

    pub fn with_prompt(mut self, prompt_version: impl Into<String>) -> Self {
        self.prompt_version = prompt_version.into();
        self
    }

    pub fn with_glossary(mut self, glossary_version: impl Into<String>) -> Self {
        self.glossary_version = glossary_version.into();
        self
    }

    pub fn with_protection(mut self, protection_version: impl Into<String>) -> Self {
        self.protection_version = protection_version.into();
        self
    }

    pub fn with_kind(mut self, kind: TextKind) -> Self {
        self.text_kind = kind;
        self
    }

    pub fn with_strategy(mut self, strategy: impl Into<String>) -> Self {
        self.strategy = strategy.into();
        self
    }

    pub fn with_route(mut self, route: TranslationRoute) -> Self {
        self.route = route;
        self
    }

    pub fn with_context(mut self, context_fingerprint: impl Into<String>) -> Self {
        self.context_fingerprint = context_fingerprint.into();
        self
    }

    pub fn with_privacy_scope(mut self, scope: impl Into<String>) -> Self {
        self.privacy_scope = scope.into();
        self
    }

    pub fn with_generation_config(mut self, hash: impl Into<String>) -> Self {
        self.generation_config_hash = hash.into();
        self
    }
}

/// 缓存键。所有字段参与哈希；不再单独暴露"部分匹配"。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    text_hash: u64,
    identity: CacheIdentity,
}

impl CacheKey {
    pub fn new(normalized_text: &str, identity: CacheIdentity) -> Self {
        let mut hasher = DefaultHasher::new();
        normalized_text.hash(&mut hasher);
        Self {
            text_hash: hasher.finish(),
            identity,
        }
    }

    pub fn from_parts(text_hash: u64, identity: CacheIdentity) -> Self {
        Self {
            text_hash,
            identity,
        }
    }

    pub fn text_hash(&self) -> u64 {
        self.text_hash
    }

    pub fn identity(&self) -> &CacheIdentity {
        &self.identity
    }

    /// 跨进程稳定的键标识（十六进制）。只用于磁盘缓存的索引，不参与内存哈希。
    pub fn stable_id(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

/// 磁盘缓存条目的身份（与内存键一一对应）。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedEntry {
    text_hash: u64,
    source: String,
    target: String,
    backend_id: String,
    model_id: Option<String>,
    model_revision: Option<String>,
    prompt_version: String,
    glossary_version: String,
    protection_version: String,
    mode: String,
    text_kind: String,
    strategy: String,
    route: String,
    context_fingerprint: String,
    privacy_scope: String,
    generation_config_hash: String,
    /// 译文（**会落盘**，只有显式配置 `cache_path` 时才写）。
    text: String,
    engine_id: String,
    result_model_id: Option<String>,
    /// 条目自校验和：文件被截断/手改/跨版本污染时，条目直接失效。
    checksum: String,
}

/// 条目校验和：把"键 + 译文 + 引擎"一起哈希。
///
/// 只用于发现文件损坏/被修改，不用于安全认证（本地文件没有攻击者模型）。
fn entry_checksum(
    text_hash: u64,
    source: &str,
    target: &str,
    engine_id: &str,
    text: &str,
) -> String {
    let mut hasher = DefaultHasher::new();
    text_hash.hash(&mut hasher);
    source.hash(&mut hasher);
    target.hash(&mut hasher);
    engine_id.hash(&mut hasher);
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 磁盘缓存文件格式。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedCache {
    schema_version: u32,
    entries: Vec<PersistedEntry>,
}

const CACHE_SCHEMA_VERSION: u32 = 1;
/// 磁盘缓存最多保存的条目数（防止文件无限增长）。
const PERSIST_MAX_ENTRIES: usize = 512;

fn mode_str(mode: TranslationMode) -> String {
    mode.as_str().to_string()
}
fn mode_parse(raw: &str) -> TranslationMode {
    if raw == "fast" {
        TranslationMode::Fast
    } else {
        TranslationMode::Precise
    }
}
fn kind_parse(raw: &str) -> TextKind {
    match raw {
        "word" | "phrase" => TextKind::Word,
        "sentence" => TextKind::Sentence,
        "paragraph" => TextKind::Paragraph,
        "document" => TextKind::Document,
        _ => TextKind::Auto,
    }
}
fn route_parse(raw: &str) -> TranslationRoute {
    match raw {
        "dictionary" => TranslationRoute::Dictionary,
        "rule" => TranslationRoute::Rule,
        "local_nmt" => TranslationRoute::LocalNmt,
        "mixed" => TranslationRoute::Mixed,
        _ => TranslationRoute::Remote,
    }
}

pub struct QueryCache {
    capacity: usize,
    map: HashMap<CacheKey, CacheEntry>,
    order: VecDeque<CacheKey>,
    hits: u64,
    inserts: u64,
    evictions: u64,
    invalidated: u64,
}

impl QueryCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
            hits: 0,
            inserts: 0,
            evictions: 0,
            invalidated: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn enabled(&self) -> bool {
        self.capacity > 0
    }

    pub fn get(&mut self, key: &CacheKey) -> Option<&CacheEntry> {
        if let Some(entry) = self.map.get(key) {
            self.hits += 1;
            return Some(entry);
        }
        None
    }

    pub fn contains(&self, key: &CacheKey) -> bool {
        self.map.contains_key(key)
    }

    pub fn insert(&mut self, key: CacheKey, value: CacheEntry) {
        if self.capacity == 0 {
            return;
        }
        self.inserts += 1;
        if self.map.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                if self.map.remove(&oldest).is_some() {
                    self.evictions += 1;
                }
            } else {
                break;
            }
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// 把缓存写入文件（原子替换）。
    ///
    /// 只在显式配置了 `cache_path` 时调用。写入的是**译文**，
    /// 因此这一步必须是用户显式选择（隐私），见 docs/known-issues.md K-12。
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut entries: Vec<PersistedEntry> = self
            .map
            .iter()
            .take(PERSIST_MAX_ENTRIES)
            .map(|(key, entry)| {
                let id = &key.identity;
                PersistedEntry {
                    text_hash: key.text_hash,
                    source: id.source.clone(),
                    target: id.target.clone(),
                    backend_id: id.backend_id.clone(),
                    model_id: id.model_id.clone(),
                    model_revision: id.model_revision.clone(),
                    prompt_version: id.prompt_version.clone(),
                    glossary_version: id.glossary_version.clone(),
                    protection_version: id.protection_version.clone(),
                    mode: mode_str(id.mode),
                    text_kind: id.text_kind.as_str().to_string(),
                    strategy: id.strategy.clone(),
                    route: id.route.as_str().to_string(),
                    context_fingerprint: id.context_fingerprint.clone(),
                    privacy_scope: id.privacy_scope.clone(),
                    generation_config_hash: id.generation_config_hash.clone(),
                    checksum: entry_checksum(
                        key.text_hash,
                        &id.source,
                        &id.target,
                        &entry.result.engine_id,
                        &entry.result.text,
                    ),
                    text: entry.result.text.clone(),
                    engine_id: entry.result.engine_id.clone(),
                    result_model_id: entry.result.model_id.clone(),
                }
            })
            .collect();
        entries.sort_by(|a, b| {
            (a.text_hash, &a.backend_id, &a.prompt_version).cmp(&(
                b.text_hash,
                &b.backend_id,
                &b.prompt_version,
            ))
        });

        let payload = PersistedCache {
            schema_version: CACHE_SCHEMA_VERSION,
            entries,
        };
        let json = serde_json::to_string(&payload)
            .map_err(|e| std::io::Error::other(format!("缓存序列化失败：{e}")))?;

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        // 先写临时文件再替换：进程中断也不会留下半个文件。
        //
        // Windows 上目标文件可能被杀毒/索引服务短暂占用，`rename` 会失败；
        // 这时先备份旧文件再替换，任何一步失败都恢复旧文件——
        // **绝不允许**出现"缓存写坏且旧数据也丢了"的情况。
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &json)?;
        let first_error = match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };

        // 目标不是普通文件（目录/设备等）时不要"挪走再写"，直接失败
        if path.exists() && !path.is_file() {
            let _ = std::fs::remove_file(&tmp);
            return Err(first_error);
        }

        let backup = path.with_extension("bak");
        let had_old = path.is_file();
        if had_old {
            std::fs::rename(path, &backup)?;
        }
        match std::fs::rename(&tmp, path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&backup);
                Ok(())
            }
            Err(err) => {
                if had_old {
                    // 尽力恢复旧缓存；恢复失败也要把错误报出去
                    let _ = std::fs::rename(&backup, path);
                }
                let _ = std::fs::remove_file(&tmp);
                Err(err)
            }
        }
    }

    /// 从文件加载。文件缺失/损坏/版本不符时返回空缓存（不 panic、不阻塞启动）。
    pub fn load_from_file(path: &std::path::Path, capacity: usize) -> (Self, usize) {
        let mut cache = Self::new(capacity);
        let Ok(text) = std::fs::read_to_string(path) else {
            return (cache, 0);
        };
        let Ok(payload) = serde_json::from_str::<PersistedCache>(&text) else {
            return (cache, 0);
        };
        if payload.schema_version != CACHE_SCHEMA_VERSION {
            return (cache, 0);
        }

        let mut loaded = 0usize;
        for entry in payload.entries.into_iter().take(capacity) {
            // 1) 自校验：文件被截断/手改/旧格式污染 → 跳过该条目
            let expected = entry_checksum(
                entry.text_hash,
                &entry.source,
                &entry.target,
                &entry.engine_id,
                &entry.text,
            );
            if expected != entry.checksum {
                continue;
            }
            // 2) 语言标签必须真实可解析；解析失败说明条目不可信 → 跳过
            let Ok(result_source) = LanguageTag::new(entry.source.as_str()) else {
                continue;
            };
            let Ok(result_target) = LanguageTag::new(entry.target.as_str()) else {
                continue;
            };

            let identity = CacheIdentity::new(entry.backend_id, mode_parse(&entry.mode))
                .with_languages(entry.source, entry.target)
                .with_model(entry.model_id, entry.model_revision)
                .with_prompt(entry.prompt_version)
                .with_glossary(entry.glossary_version)
                .with_protection(entry.protection_version)
                .with_kind(kind_parse(&entry.text_kind))
                .with_strategy(entry.strategy)
                .with_route(route_parse(&entry.route))
                .with_context(entry.context_fingerprint)
                .with_privacy_scope(entry.privacy_scope)
                .with_generation_config(entry.generation_config_hash);

            let key = CacheKey {
                text_hash: entry.text_hash,
                identity,
            };
            // 语言标签用磁盘记录里的真实值（上面已校验可解析）
            let result = TranslationResult {
                text: entry.text,
                source: result_source,
                target: result_target,
                engine_id: entry.engine_id.clone(),
                model_id: entry.result_model_id,
                elapsed_ms: 0,
                warnings: Vec::new(),
            };
            cache.insert(
                key,
                CacheEntry {
                    result,
                    diagnostics: TranslationDiagnostics::minimal(entry.engine_id),
                },
            );
            loaded += 1;
        }
        (cache, loaded)
    }

    /// 删除一个条目（命中的条目未通过校验时使用）。
    pub fn remove(&mut self, key: &CacheKey) -> bool {
        if self.map.remove(key).is_some() {
            self.order.retain(|existing| existing != key);
            self.invalidated += 1;
            return true;
        }
        false
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            capacity: self.capacity,
            len: self.map.len(),
            hits: self.hits,
            inserts: self.inserts,
            evictions: self.evictions,
            invalidated: self.invalidated,
        }
    }
}

/// 缓存统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub capacity: usize,
    pub len: usize,
    pub hits: u64,
    pub inserts: u64,
    pub evictions: u64,
    /// 命中但因完整性/校验失败被丢弃的条目数。
    pub invalidated: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_types::{LanguageTag, TranslationWarning};

    fn entry(text: &str) -> CacheEntry {
        let lang_en = LanguageTag::new("en").unwrap();
        let lang_zh = LanguageTag::new("zh-CN").unwrap();
        CacheEntry {
            result: TranslationResult {
                text: text.to_string(),
                source: lang_en,
                target: lang_zh,
                engine_id: "http.test".to_string(),
                model_id: Some("m".to_string()),
                elapsed_ms: 0,
                warnings: Vec::new(),
            },
            diagnostics: TranslationDiagnostics::minimal("http.test"),
        }
    }

    fn identity() -> CacheIdentity {
        CacheIdentity::new("http.test", TranslationMode::Precise)
            .with_languages("en", "zh-CN")
            .with_model(Some("m".into()), Some("2026-11-01".into()))
            .with_prompt("en_zh/precise_v1")
            .with_protection(crate::protection::PROTECTION_VERSION)
            .with_route(TranslationRoute::Remote)
    }

    #[test]
    fn 相同输入命中() {
        let mut cache = QueryCache::new(4);
        let key = CacheKey::new("hello", identity());
        cache.insert(key.clone(), entry("你好"));
        assert_eq!(cache.get(&key).unwrap().result.text, "你好");
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn 任一维度变化都不得命中() {
        let base = identity();
        let key = CacheKey::new("hello", base.clone());
        let mut cache = QueryCache::new(16);
        cache.insert(key.clone(), entry("你好"));

        let variants = [
            base.clone()
                .with_model(Some("m".into()), Some("2026-12-01".into())),
            base.clone().with_prompt("en_zh/precise_v2"),
            base.clone().with_glossary("glossary.software@3"),
            base.clone().with_protection("protect-v3"),
            base.clone().with_languages("en", "zh-Hans"),
            base.clone().with_context("doc-42"),
            base.clone().with_generation_config("temp=0.7"),
            base.clone().with_privacy_scope("user-7"),
            base.clone().with_route(TranslationRoute::Rule),
            base.clone().with_kind(TextKind::Paragraph),
            base.clone().with_strategy("paragraph-group"),
        ];
        for variant in variants {
            let other = CacheKey::new("hello", variant.clone());
            assert!(!cache.contains(&other), "缓存身份变化仍然命中：{variant:?}");
        }
    }

    #[test]
    fn 模式不同不共享缓存() {
        let mut cache = QueryCache::new(4);
        let precise = CacheKey::new(
            "hello",
            identity()
                .with_generation_config("")
                .with_prompt("en_zh/precise_v1"),
        );
        cache.insert(precise, entry("你好"));

        let mut fast_identity = identity();
        fast_identity.mode = TranslationMode::Fast;
        fast_identity.prompt_version = "en_zh/fast_v1".to_string();
        let fast = CacheKey::new("hello", fast_identity);
        assert!(!cache.contains(&fast));
    }

    #[test]
    fn 文本不同不命中() {
        let mut cache = QueryCache::new(4);
        cache.insert(CacheKey::new("hello", identity()), entry("你好"));
        assert!(!cache.contains(&CacheKey::new("hello!", identity())));
    }

    #[test]
    fn 容量为_0_表示禁用() {
        let mut cache = QueryCache::new(0);
        let key = CacheKey::new("hello", identity());
        cache.insert(key.clone(), entry("你好"));
        assert!(!cache.enabled());
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn fifo_淘汰最老的() {
        let mut cache = QueryCache::new(2);
        let k1 = CacheKey::new("a", identity());
        let k2 = CacheKey::new("b", identity());
        let k3 = CacheKey::new("c", identity());
        cache.insert(k1.clone(), entry("甲"));
        cache.insert(k2.clone(), entry("乙"));
        cache.insert(k3.clone(), entry("丙"));
        assert!(!cache.contains(&k1));
        assert!(cache.contains(&k2));
        assert!(cache.contains(&k3));
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn 重复插入同一键不重复排队() {
        let mut cache = QueryCache::new(2);
        let key = CacheKey::new("a", identity());
        cache.insert(key.clone(), entry("甲"));
        cache.insert(key.clone(), entry("甲2"));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&key).unwrap().result.text, "甲2");
    }

    #[test]
    fn 清理后为空() {
        let mut cache = QueryCache::new(4);
        cache.insert(CacheKey::new("a", identity()), entry("甲"));
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn 磁盘缓存可以往返加载() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");

        let mut cache = QueryCache::new(8);
        let key = CacheKey::new("hello", identity());
        cache.insert(key.clone(), entry("你好"));
        cache.save_to_file(&path).unwrap();

        let (loaded, count) = QueryCache::load_from_file(&path, 8);
        assert_eq!(count, 1);
        let hit = loaded.map.get(&key).expect("同一身份+文本应命中磁盘缓存");
        assert_eq!(hit.result.text, "你好");
    }

    #[test]
    fn 磁盘缓存恢复真实语言标签() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");

        let mut cache = QueryCache::new(8);
        let ja = LanguageTag::new("ja").unwrap();
        let ko = LanguageTag::new("ko").unwrap();
        let id = identity().with_languages("ja".to_string(), "ko".to_string());
        let key = CacheKey::new("こんにちは", id);
        cache.insert(
            key.clone(),
            CacheEntry {
                result: TranslationResult {
                    text: "안녕하세요".to_string(),
                    source: ja.clone(),
                    target: ko.clone(),
                    engine_id: "mock".to_string(),
                    model_id: None,
                    elapsed_ms: 0,
                    warnings: Vec::new(),
                },
                diagnostics: TranslationDiagnostics::minimal("mock".to_string()),
            },
        );
        cache.save_to_file(&path).unwrap();

        let (loaded, count) = QueryCache::load_from_file(&path, 8);
        assert_eq!(count, 1);
        let hit = loaded.map.get(&key).expect("应命中");
        assert_eq!(hit.result.source.as_str(), "ja", "源语言不得被写死");
        assert_eq!(hit.result.target.as_str(), "ko", "目标语言不得被写死");
    }

    #[test]
    fn 被篡改的缓存条目会被跳过() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");

        let mut cache = QueryCache::new(8);
        cache.insert(CacheKey::new("hello", identity()), entry("你好"));
        cache.save_to_file(&path).unwrap();

        // 手改译文但不更新校验和
        let raw = std::fs::read_to_string(&path).unwrap();
        let tampered = raw.replace("你好", "被篡改的译文");
        assert_ne!(raw, tampered, "文件应包含原文");
        std::fs::write(&path, tampered).unwrap();

        let (loaded, count) = QueryCache::load_from_file(&path, 8);
        assert_eq!(count, 0, "校验和不匹配的条目必须被丢弃");
        assert!(loaded.is_empty());
    }

    #[test]
    fn 语言标签非法的条目被跳过() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");

        let mut cache = QueryCache::new(8);
        cache.insert(CacheKey::new("hello", identity()), entry("你好"));
        cache.save_to_file(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let broken = raw.replace("\"target\":\"zh-CN\"", "\"target\":\"not a tag!\"");
        assert_ne!(raw, broken);
        std::fs::write(&path, broken).unwrap();

        let (loaded, count) = QueryCache::load_from_file(&path, 8);
        assert_eq!(count, 0, "语言标签非法的条目必须被丢弃");
        assert!(loaded.is_empty());
    }

    #[test]
    fn 反复写入缓存不会破坏旧文件() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");

        for round in 0..3 {
            let mut cache = QueryCache::new(8);
            let key = CacheKey::new("hello", identity());
            cache.insert(key.clone(), entry(&format!("译文{round}")));
            cache.save_to_file(&path).unwrap();

            let (loaded, count) = QueryCache::load_from_file(&path, 8);
            assert_eq!(count, 1, "第 {round} 轮应能读回一条");
            assert_eq!(
                loaded.map.get(&key).map(|e| e.result.text.as_str()),
                Some(format!("译文{round}").as_str())
            );
            // 临时文件不得残留
            assert!(!path.with_extension("tmp").exists(), "临时文件应被清理");
        }
    }

    #[test]
    fn 写入失败时保留旧缓存() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let mut cache = QueryCache::new(8);
        cache.insert(CacheKey::new("hello", identity()), entry("旧译文"));
        cache.save_to_file(&path).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        // 把目标变成一个目录：rename 一定失败
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        let mut cache2 = QueryCache::new(8);
        cache2.insert(CacheKey::new("hello", identity()), entry("新译文"));
        assert!(cache2.save_to_file(&blocked).is_err(), "写入目录应失败");

        // 旧缓存不受影响
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn 损坏的缓存文件被忽略而不是崩溃() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        std::fs::write(&path, "{ not json").unwrap();

        let (cache, count) = QueryCache::load_from_file(&path, 8);
        assert_eq!(count, 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn 版本不符的缓存文件被忽略() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        std::fs::write(&path, r#"{"schema_version":99,"entries":[]}"#).unwrap();
        let (_, count) = QueryCache::load_from_file(&path, 8);
        assert_eq!(count, 0);
    }

    #[test]
    fn 缓存不改变警告内容() {
        let mut cache = QueryCache::new(4);
        let key = CacheKey::new("a", identity());
        let mut e = entry("甲");
        e.result
            .warnings
            .push(TranslationWarning::LanguageAssumed("en".into()));
        cache.insert(key.clone(), e);
        let hit = cache.get(&key).unwrap();
        assert_eq!(hit.result.warnings.len(), 1);
    }
}
