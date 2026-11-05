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

use translator_types::{TranslateRequest, TranslationResult};

/// 缓存键。所有影响译文结果的维度都必须进入键。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// 规范化 + 占位符保护后文本的哈希。
    pub text_hash: u64,
    pub source: String,
    pub target: String,
    pub engine_id: String,
    /// P0 恒为 `None`；模型版本确定后填入。
    pub model_id: Option<String>,
    pub glossary_id: Option<String>,
    pub rules_version: u32,
    pub decode_config: String,
    /// 隐私隔离域：不同隐私上下文不得共享缓存。
    pub privacy_domain: String,
}

impl CacheKey {
    pub fn new(
        normalized_text: &str,
        req: &TranslateRequest,
        source: &str,
        engine_id: &str,
        rules_version: u32,
    ) -> Self {
        let mut hasher = DefaultHasher::new();
        normalized_text.hash(&mut hasher);

        Self {
            text_hash: hasher.finish(),
            source: source.to_string(),
            target: req.target.as_str().to_string(),
            engine_id: engine_id.to_string(),
            model_id: None,
            glossary_id: req.glossary_id.clone(),
            rules_version,
            decode_config: "default".to_string(),
            privacy_domain: "default".to_string(),
        }
    }
}

/// 带容量上限的查询缓存（简单 FIFO 淘汰）。
pub struct QueryCache {
    capacity: usize,
    map: HashMap<CacheKey, TranslationResult>,
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

    pub fn get(&self, key: &CacheKey) -> Option<&TranslationResult> {
        self.map.get(key)
    }

    pub fn insert(&mut self, key: CacheKey, value: TranslationResult) {
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
        CacheKey::new(text, &req, "en", "mock.nmt", 1)
    }

    #[test]
    fn 容量为0时不缓存() {
        let mut cache = QueryCache::new(0);
        cache.insert(sample_key("a"), dummy_result());
        assert!(cache.is_empty());
    }

    #[test]
    fn 超出容量应淘汰最旧的() {
        let mut cache = QueryCache::new(2);
        let k1 = sample_key("a");
        let k2 = sample_key("b");
        let k3 = sample_key("c");

        cache.insert(k1.clone(), dummy_result());
        cache.insert(k2.clone(), dummy_result());
        cache.insert(k3.clone(), dummy_result());

        assert_eq!(cache.len(), 2);
        assert!(cache.get(&k1).is_none(), "最旧的应被淘汰");
        assert!(cache.get(&k3).is_some());
    }

    #[test]
    fn 不同引擎不得共享缓存() {
        let req = TranslateRequest::new("hi", LanguageTag::new("zh-Hans").unwrap());
        let a = CacheKey::new("hi", &req, "en", "mock.nmt", 1);
        let b = CacheKey::new("hi", &req, "en", "mock.dictionary", 1);
        assert_ne!(a, b);
    }

    #[test]
    fn 不同术语表不得共享缓存() {
        let base = TranslateRequest::new("hi", LanguageTag::new("zh-Hans").unwrap());
        let with_glossary = base.clone().with_glossary("g1");
        let a = CacheKey::new("hi", &base, "en", "mock.nmt", 1);
        let b = CacheKey::new("hi", &with_glossary, "en", "mock.nmt", 1);
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
}
