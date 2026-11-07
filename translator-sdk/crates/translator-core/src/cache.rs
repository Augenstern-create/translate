//! 查询缓存。
//!
//! 设计文档 §6 要求缓存键必须包含：
//! 规范化文本哈希、源/目标语言、引擎与模型版本、术语表版本、规则版本、
//! 解码配置、隐私隔离域。
//!
//! **P0 简化说明**：模型版本在调用引擎前未知，因此键中先用 `engine_id` 占位，
//! 结果里带 `model_id` 供上层校验。这一简化已在 `CacheKey` 上注明，
//! P2 资源管理落地后应改为「先解析模型、再计算键」。

use std::collections::hash_map::{DefaultHasher, Entry};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use translator_types::{TranslateRequest, TranslationPolicy, TranslationResult};

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

/// 缓存键。所有影响译文结果的维度都必须进入键。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// 规范化 + 占位符保护后文本的哈希。
    pub text_hash: u64,
    pub source: String,
    pub target: String,
    pub engine_id: String,
    /// 后端提供的执行身份摘要（模型/词表/解码/保护策略）；`None` 表示后端不提供。
    pub engine_cache_identity: Option<String>,
    /// P0 恒为 `None`；模型版本确定后填入。
    pub model_id: Option<String>,
    pub glossary_id: Option<String>,
    pub rules_version: u32,
    pub decode_config: String,
    /// 隐私隔离域：不同隐私上下文不得共享缓存。
    pub privacy_domain: String,
    /// 已挂载资源的指纹（`id@version`）。
    ///
    /// 资源版本变化必须让旧缓存失效（ADR-011）；P2.4 会把它换成
    /// `ResolvedExecutionPlan` 的稳定指纹。
    pub resource_fingerprint: String,
}

impl CacheKey {
    pub fn new(
        normalized_text: &str,
        req: &TranslateRequest,
        source: &str,
        engine_id: &str,
        engine_cache_identity: Option<&str>,
        rules_version: u32,
        resource_fingerprint: &str,
    ) -> Self {
        let mut hasher = DefaultHasher::new();
        normalized_text.hash(&mut hasher);

        Self {
            text_hash: hasher.finish(),
            source: source.to_string(),
            target: req.target.as_str().to_string(),
            engine_id: engine_id.to_string(),
            engine_cache_identity: engine_cache_identity.map(str::to_string),
            model_id: None,
            glossary_id: req.glossary_id.clone(),
            rules_version,
            decode_config: "default".to_string(),
            // P8.3 / EP-04：远程策略与本地策略不得共享缓存。
            // 显式 engine:<id> 的隐私域由宿主负责；这里只隔离明确的 RemoteOnly。
            privacy_domain: if matches!(req.policy, TranslationPolicy::RemoteOnly) {
                "remote".to_string()
            } else {
                "local".to_string()
            },
            resource_fingerprint: resource_fingerprint.to_string(),
        }
    }
}

/// 带容量上限的查询缓存（简单 FIFO 淘汰）。
pub struct QueryCache {
    capacity: usize,
    map: HashMap<CacheKey, CacheEntry>,
    order: VecDeque<CacheKey>,
}

impl QueryCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
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

    pub fn get(&self, key: &CacheKey) -> Option<&CacheEntry> {
        self.map.get(key)
    }

    pub fn insert(&mut self, key: CacheKey, value: CacheEntry) {
        if self.capacity == 0 {
            return;
        }

        // 已存在：就地更新，不改变淘汰顺序
        match self.map.entry(key.clone()) {
            Entry::Occupied(mut entry) => {
                entry.insert(value);
                return;
            }
            Entry::Vacant(_) => {}
        }

        while self.map.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            } else {
                break;
            }
        }

        self.order.push_back(key.clone());
        self.map.insert(key, value);
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_types::{LanguageTag, TranslateRequest};

    fn sample_key(text: &str) -> CacheKey {
        let req = TranslateRequest::new(text, LanguageTag::new("zh-Hans").unwrap());
        CacheKey::new(text, &req, "en", "mock.nmt", None, 1, "")
    }

    #[test]
    fn 资源指纹不同不得共享缓存() {
        let target = LanguageTag::new("zh-Hans").unwrap();
        let req = TranslateRequest::new("hi", target);
        let a = CacheKey::new(
            "hi",
            &req,
            "en",
            "dictionary.package",
            None,
            1,
            "dict.zh-en@0.1.0",
        );
        let b = CacheKey::new(
            "hi",
            &req,
            "en",
            "dictionary.package",
            None,
            1,
            "dict.zh-en@0.2.0",
        );
        assert_ne!(a, b, "换资源版本后必须换缓存键（ADR-011）");
    }

    #[test]
    fn 远程与本地策略不共享缓存键() {
        let target = LanguageTag::new("zh-Hans").unwrap();
        let local = TranslateRequest::new("hi", target.clone());
        let remote = TranslateRequest::new("hi", target).with_policy(TranslationPolicy::RemoteOnly);
        let a = CacheKey::new("hi", &local, "en", "mock.nmt", None, 1, "");
        let b = CacheKey::new("hi", &remote, "en", "mock.nmt", None, 1, "");
        assert_ne!(a, b, "隐私域不同的请求不得共享缓存（EP-04）");
        assert_eq!(a.privacy_domain, "local");
        assert_eq!(b.privacy_domain, "remote");
    }

    #[test]
    fn 容量为0时不缓存() {
        let mut cache = QueryCache::new(0);
        cache.insert(sample_key("a"), dummy_entry());
        assert!(cache.is_empty());
    }

    #[test]
    fn 超出容量应淘汰最旧的() {
        let mut cache = QueryCache::new(2);
        let k1 = sample_key("a");
        let k2 = sample_key("b");
        let k3 = sample_key("c");

        cache.insert(k1.clone(), dummy_entry());
        cache.insert(k2.clone(), dummy_entry());
        cache.insert(k3.clone(), dummy_entry());

        assert_eq!(cache.len(), 2);
        assert!(cache.get(&k1).is_none(), "最旧的应被淘汰");
        assert!(cache.get(&k3).is_some());
    }

    #[test]
    fn 缓存条目带诊断可按文本取回() {
        let mut cache = QueryCache::new(2);
        let key = sample_key("hi");
        cache.insert(key.clone(), dummy_entry());
        assert_eq!(cache.get(&key).unwrap().diagnostics.engine_id, "mock.nmt");
    }

    #[test]
    fn 不同引擎不得共享缓存() {
        let req = TranslateRequest::new("hi", LanguageTag::new("zh-Hans").unwrap());
        let a = CacheKey::new("hi", &req, "en", "mock.nmt", None, 1, "");
        let b = CacheKey::new("hi", &req, "en", "mock.dictionary", None, 1, "");
        assert_ne!(a, b);
    }

    #[test]
    fn 不同术语表不得共享缓存() {
        let base = TranslateRequest::new("hi", LanguageTag::new("zh-Hans").unwrap());
        let with_glossary = base.clone().with_glossary("g1");
        let a = CacheKey::new("hi", &base, "en", "mock.nmt", None, 1, "");
        let b = CacheKey::new("hi", &with_glossary, "en", "mock.nmt", None, 1, "");
        assert_ne!(a, b);
    }

    fn dummy_result() -> TranslationResult {
        TranslationResult {
            text: "译文".to_string(),
            source: LanguageTag::new("en").unwrap(),
            target: LanguageTag::new("zh-Hans").unwrap(),
            engine_id: "mock.nmt".to_string(),
            model_id: None,
            elapsed_ms: 0,
            warnings: Vec::new(),
        }
    }

    fn dummy_entry() -> CacheEntry {
        CacheEntry {
            result: dummy_result(),
            diagnostics: TranslationDiagnostics {
                engine_id: "mock.nmt".to_string(),
                capability: translator_types::Capability::Nmt,
                route_reason: "测试".to_string(),
                cache_hit: false,
                source_assumed: false,
                rules_version: 1,
                resource_fingerprint: String::new(),
                rules: None,
                protected_spans: Vec::new(),
                restore_missing: Vec::new(),
                term: None,
            },
        }
    }
}
