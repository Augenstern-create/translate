//! 执行计划摘要作键的翻译结果缓存（`docs/17` §4.1.1 / ADR-035/038）。
//!
//! 为什么不用 `content_digest` 或 `id@version`：
//!
//! - `content_digest` 只覆盖权重，换 tokenizer / 换 beam / 换保护策略都不会变，
//!   用它作键会让"不同执行条件"命中同一条缓存（ADR-026）；
//! - `id@version` 连权重都不看（P2.5 缺口）。
//!
//! 这里用 [`crate::plan::ResolvedExecutionPlan::digest`]（`plan-v1`）作键的一部分，
//! 它把 engine / runtime / runtime build / 模型五文件摘要 / 语言对 / 解码参数 /
//! 保护策略 / 规则与术语 / 隐私域一起编码。**键里出现的每个字段都进摘要**，
//! 因此"模型内容变了"必然换成未命中，而不是悄悄返回旧译文。
//!
//! 三条设计取舍：
//!
//! 1. **FIFO 而不是 LRU**：不为了省一次哈希而在每条上维护双向链表；命中不改变淘汰顺序。
//!    代价是热键可能被后来的冷键挤掉，收益是代码可审计（`docs/19` D-32）。
//! 2. **只缓存成功结果**：失败（`IntegrityFailure`、超时、后端崩）绝不入缓存——
//!    "失败被缓存成成功"是最坏的一类 bug。
//! 3. **容量固定、无 TTL**：过期靠"换计划摘要"和显式 `clear()`，不靠时间。
//!    TTL 属于 P3.2 的引擎空闲回收（见 `translator-nmt-bergamot` 的 `evict_if_idle`），
//!    删缓存和卸模型是两件事。

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use translator_types::TranslationResult;

/// 缓存键的 schema 版本。改键的组成必须升这个数（旧条目自然失效）。
pub const PLAN_CACHE_SCHEMA: u32 = 1;

/// 缓存键：计划摘要 + 目标语言 + 原文摘要。
///
/// 原文用摘要而不是原文本身：键不该在内存里多存一份用户文本
/// （与"诊断不写受保护原文"同一条原则）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlanCacheKey {
    pub plan_digest: String,
    pub target: String,
    pub text_digest: String,
}

impl PlanCacheKey {
    pub fn new(plan_digest: impl Into<String>, target: impl Into<String>, text: &str) -> Self {
        Self {
            plan_digest: plan_digest.into(),
            target: target.into(),
            text_digest: digest_text(text),
        }
    }
}

/// 缓存统计。命中率是**观测值**，不是优化目标。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub evictions: u64,
}

impl PlanCacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// 有界缓存（FIFO 淘汰）。
pub struct PlanCache {
    capacity: usize,
    inner: Mutex<Inner>,
}

struct Inner {
    map: HashMap<PlanCacheKey, TranslationResult>,
    order: VecDeque<PlanCacheKey>,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl PlanCache {
    /// 容量为 0 时**不缓存**（`get` 永远未命中、`put` 是空操作），便于宿主显式关掉。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                order: VecDeque::new(),
                hits: 0,
                misses: 0,
                evictions: 0,
            }),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn get(&self, key: &PlanCacheKey) -> Option<TranslationResult> {
        // 容量 0 = 关闭：既不命中，也不记"未命中"——关掉的缓存不该出现在命中率里
        if self.capacity == 0 {
            return None;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // 先把结果 clone 出来再计数：`map.get` 的不可变借用与 `hits += 1` 冲突
        let hit = inner.map.get(key).cloned();
        match hit {
            Some(result) => {
                inner.hits += 1;
                Some(result)
            }
            None => {
                inner.misses += 1;
                None
            }
        }
    }

    /// 只放成功结果。容量 0 直接忽略。
    pub fn put(&self, key: PlanCacheKey, result: TranslationResult) {
        if self.capacity == 0 {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let std::collections::hash_map::Entry::Occupied(mut slot) = inner.map.entry(key.clone())
        {
            // 同一个键重复写入：值更新，淘汰顺序不变（FIFO 不因重写而"变新"）
            slot.insert(result);
            return;
        }
        while inner.order.len() >= self.capacity {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            if inner.map.remove(&oldest).is_some() {
                inner.evictions += 1;
            }
        }
        inner.order.push_back(key.clone());
        inner.map.insert(key, result);
    }

    pub fn stats(&self) -> PlanCacheStats {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        PlanCacheStats {
            hits: inner.hits,
            misses: inner.misses,
            entries: inner.map.len(),
            evictions: inner.evictions,
        }
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.map.clear();
        inner.order.clear();
    }
}

/// 原文摘要：sha256 十六进制（不用 `DefaultHasher`——它不保证跨版本稳定，
/// 而缓存键要能被"同一次构建内的不同进程"复现）。
pub fn digest_text(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 给日志/诊断用的短标识（前 16 位）。
pub fn short(text: &str) -> String {
    digest_text(text).chars().take(16).collect()
}

/// 只在测试里用：把 `PlanCacheKey` 打散成 `u64`，验证"不同计划 → 不同键"。
#[doc(hidden)]
pub fn hash_key(key: &PlanCacheKey) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_types::{LanguageTag, TranslationResult};

    fn result(text: &str) -> TranslationResult {
        TranslationResult {
            text: text.to_string(),
            source: LanguageTag::new("en").unwrap(),
            target: LanguageTag::new("zh-Hans").unwrap(),
            engine_id: "test".into(),
            model_id: Some("m@1".into()),
            elapsed_ms: 1,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn 同一计划同一文本命中() {
        let cache = PlanCache::new(4);
        let key = PlanCacheKey::new("digest-a", "zh-Hans", "hello");
        cache.put(key.clone(), result("你好"));
        assert_eq!(cache.get(&key).unwrap().text, "你好");
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.entries), (1, 0, 1));
    }

    #[test]
    fn 计划摘要不同必然未命中() {
        // LC-09 的最小内核：内容指纹变了 = 计划摘要变了 = 不能用旧缓存
        let cache = PlanCache::new(4);
        let a = PlanCacheKey::new("plan-a", "zh-Hans", "hello");
        let b = PlanCacheKey::new("plan-b", "zh-Hans", "hello");
        cache.put(a.clone(), result("旧模型的译文"));
        assert!(cache.get(&b).is_none(), "换了计划摘要必须未命中");
        assert!(cache.get(&a).is_some());
    }

    #[test]
    fn 目标语言与原文都进键() {
        let a = PlanCacheKey::new("p", "zh-Hans", "hello");
        let b = PlanCacheKey::new("p", "zh-Hant", "hello");
        let c = PlanCacheKey::new("p", "zh-Hans", "hello!");
        assert_ne!(hash_key(&a), hash_key(&b));
        assert_ne!(hash_key(&a), hash_key(&c));
        assert_ne!(a.text_digest, c.text_digest);
    }

    #[test]
    fn 容量满时按先进先出淘汰且计数() {
        let cache = PlanCache::new(2);
        cache.put(PlanCacheKey::new("p", "t", "one"), result("1"));
        cache.put(PlanCacheKey::new("p", "t", "two"), result("2"));
        cache.put(PlanCacheKey::new("p", "t", "three"), result("3"));
        let stats = cache.stats();
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.evictions, 1);
        assert!(cache.get(&PlanCacheKey::new("p", "t", "one")).is_none());
        assert!(cache.get(&PlanCacheKey::new("p", "t", "three")).is_some());
    }

    #[test]
    fn 容量为零等于关掉缓存() {
        let cache = PlanCache::new(0);
        let key = PlanCacheKey::new("p", "t", "x");
        cache.put(key.clone(), result("y"));
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn 键里不存原文() {
        let key = PlanCacheKey::new("p", "t", "my secret text");
        assert!(!format!("{key:?}").contains("my secret text"));
    }
}
