//! 协调器：串起校验 → 规范化 → 语言识别 → 占位符保护 → 路由 → 执行 → 还原 → 缓存。
//!
//! 对应设计文档 §6 的流水线。Coordinator 不认识任何具体引擎，
//! 只依赖 `translator-plugin-api` 的 trait，因此替换后端不需要改这里。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use translator_plugin_api::{BoxFuture, PluginRegistry};
use translator_types::{
    Capability, CapabilityInfo, DictionaryEntry, LookupRequest, RouteDecision, TranslateRequest,
    TranslationError, TranslationErrorCode, TranslationPolicy, TranslationResult,
    TranslationWarning,
};

use crate::cache::{CacheKey, QueryCache};
use crate::routing;
use crate::text;

/// 协调器配置。
#[derive(Clone, Debug)]
pub struct CoordinatorConfig {
    /// 查询缓存容量（0 表示禁用）。
    pub cache_capacity: usize,
    /// 规则包版本。规则变更必须让旧缓存失效，因此进入缓存键。
    pub rules_version: u32,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 256,
            rules_version: 1,
        }
    }
}

pub struct Coordinator {
    registry: Arc<PluginRegistry>,
    cache: Mutex<QueryCache>,
    config: CoordinatorConfig,
}

impl Coordinator {
    pub fn new(registry: Arc<PluginRegistry>, config: CoordinatorConfig) -> Self {
        let capacity = config.cache_capacity;
        Self {
            registry,
            cache: Mutex::new(QueryCache::new(capacity)),
            config,
        }
    }

    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    pub fn capabilities(&self) -> Vec<CapabilityInfo> {
        self.registry.capability_infos()
    }

    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// 只返回路由决策，不执行翻译，也不记录用户原文
    /// （设计文档 §4.1 `explain_route`）。
    pub fn explain_route(&self, req: &TranslateRequest) -> Result<RouteDecision, TranslationError> {
        let normalized = text::normalize(&req.text);
        if normalized.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "请求文本为空",
            ));
        }

        let source = self.resolve_source(&normalized, req)?;
        let outcome = routing::select(&self.registry, req, &source)?;
        Ok(outcome.decision)
    }

    pub fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move { self.run_translate(req).await })
    }

    pub fn lookup<'a>(
        &'a self,
        req: &'a LookupRequest,
    ) -> BoxFuture<'a, Result<Vec<DictionaryEntry>, TranslationError>> {
        Box::pin(async move {
            if req.term.trim().is_empty() {
                return Err(TranslationError::new(
                    TranslationErrorCode::InvalidRequest,
                    "查询词为空",
                ));
            }

            let mut all = Vec::new();
            for provider in self.registry.providers() {
                let mut entries = provider.lookup(req).await?;
                all.append(&mut entries);
                if all.len() >= req.limit {
                    break;
                }
            }

            all.truncate(req.limit);
            Ok(all)
        })
    }

    // ---------------- 内部实现 ----------------

    fn resolve_source(
        &self,
        normalized: &str,
        req: &TranslateRequest,
    ) -> Result<translator_types::LanguageTag, TranslationError> {
        if let Some(source) = &req.source {
            return Ok(source.clone());
        }

        text::guess_language(normalized).ok_or_else(|| {
            TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "无法自动识别源语言，请显式指定 source",
            )
        })
    }

    async fn run_translate(
        &self,
        req: &TranslateRequest,
    ) -> Result<TranslationResult, TranslationError> {
        // 1) 校验 + 规范化
        let normalized = text::normalize(&req.text);
        if normalized.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "请求文本为空",
            ));
        }

        // 2) 源语言解析
        let source = self.resolve_source(&normalized, req)?;
        if source == req.target {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!("源语言与目标语言相同：{source}"),
            ));
        }

        // 3) 占位符保护
        let protected = text::protect(&normalized);

        // 4) 路由
        let outcome = routing::select(&self.registry, req, &source)?;

        // 5) 交给引擎的请求：文本已保护、源语言已确定
        let resolved = TranslateRequest {
            text: protected.text.clone(),
            source: Some(source.clone()),
            ..req.clone()
        };

        // 6) 缓存查询
        let key = CacheKey::new(
            &protected.text,
            &resolved,
            source.as_str(),
            &outcome.decision.engine_id,
            self.config.rules_version,
        );

        if let Ok(cache) = self.cache.lock() {
            if let Some(hit) = cache.get(&key) {
                let mut hit = hit.clone();
                hit.warnings.push(TranslationWarning::CacheHit);
                hit.elapsed_ms = 0;
                return Ok(hit);
            }
        }

        // 7) 执行，词典未命中时在 AutoLocalFirst 下回退到 NMT
        let started = Instant::now();
        let mut result = match outcome.engine.translate(&resolved).await {
            Ok(result) => result,
            Err(err) if self.should_fallback(req, outcome.decision.capability, &err) => {
                match routing::fallback_to_nmt(&self.registry, &source, &req.target) {
                    Some(fallback) => fallback.engine.translate(&resolved).await?,
                    None => return Err(err),
                }
            }
            Err(err) => return Err(err),
        };

        // 8) 占位符还原；丢失必须告警，不得静默
        let (restored, missing) = text::restore(&protected, &result.text);
        if !missing.is_empty() {
            result
                .warnings
                .push(TranslationWarning::PlaceholderRestoreFailed(format!(
                    "{} 个占位符未被引擎保留：{}",
                    missing.len(),
                    missing.join(", ")
                )));
        }
        result.text = restored;

        // 9) 补全结果元信息
        result.source = source;
        result.target = req.target.clone();
        result.elapsed_ms = started.elapsed().as_millis() as u64;

        // 10) 写缓存（缓存副本不含 CacheHit 警告与耗时）
        if let Ok(mut cache) = self.cache.lock() {
            let mut cached = result.clone();
            cached
                .warnings
                .retain(|w| !matches!(w, TranslationWarning::CacheHit));
            cached.elapsed_ms = 0;
            cache.insert(key, cached);
        }

        Ok(result)
    }

    fn should_fallback(
        &self,
        req: &TranslateRequest,
        capability: Capability,
        err: &TranslationError,
    ) -> bool {
        // 只有 AutoLocalFirst + 词典能力 + "未命中/不可用" 才回退。
        // 其它策略下静默换引擎会掩盖问题（设计文档要求显式）。
        matches!(req.policy, TranslationPolicy::AutoLocalFirst)
            && capability == Capability::Dictionary
            && matches!(
                err.code,
                TranslationErrorCode::CapabilityUnavailable
                    | TranslationErrorCode::ResourceNotInstalled
            )
    }
}
