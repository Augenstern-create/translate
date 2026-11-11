//! 轻量后端路由。
//!
//! 旧 `planner.rs` 有 1400 行：它在同一条路径上同时做语言识别、结构分析、
//! 规则引擎选择、NMT/远程切换、预算判断与回退决策。重构后路由只回答一个
//! 问题：**这次请求该交给哪个引擎**（§3.3 / §9.3）。
//!
//! 决策优先级（从高到低）：
//!
//! 1. 显式指名引擎（`TranslationPolicy::ExplicitEngine`）；
//! 2. 显式策略（`DictionaryOnly` / `LocalOnly` / `RemoteOnly`）；
//! 3. 配置的默认后端（`RoutingConfig::preferred_backend`）；
//! 4. 本地/自托管 NMT 优先；
//! 5. 只有在 `RemoteFallbackPolicy` **显式授权**时才回退远程。
//!
//! 回退远程永远不是"默认行为"：把用户原文上传到第三方必须是被明确允许的。

use std::sync::Arc;

use translator_plugin_api::{PluginRegistry, TranslationEngine};
use translator_types::{
    Capability, LanguageTag, RemoteFallbackPolicy, RouteDecision, TranslateRequest,
    TranslationError, TranslationErrorCode, TranslationPolicy, TranslationRoute,
};

/// 路由配置。
#[derive(Clone, Debug, Default)]
pub struct RoutingConfig {
    /// 默认后端 id（配置里写 `http.remote` 这种完整 id）。
    pub preferred_backend: Option<String>,
    /// 远程回退授权。
    pub remote_fallback: RemoteFallbackPolicy,
}

/// 路由结果。
#[derive(Clone)]
pub struct RouteOutcome {
    pub engine: Arc<dyn TranslationEngine>,
    pub capability: Capability,
    pub route: TranslationRoute,
    /// 人类可读的理由（不含原文）。
    pub reason: String,
}

impl std::fmt::Debug for RouteOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteOutcome")
            .field("engine_id", &self.engine.descriptor().id)
            .field("capability", &self.capability)
            .field("route", &self.route)
            .field("reason", &self.reason)
            .finish()
    }
}

impl RouteOutcome {
    pub fn decision(&self) -> RouteDecision {
        RouteDecision {
            engine_id: self.engine.descriptor().id.clone(),
            capability: self.capability,
            reason: self.reason.clone(),
        }
    }

    pub fn engine_id(&self) -> String {
        self.engine.descriptor().id
    }
}

/// 路由器。
pub struct BackendRouter<'a> {
    registry: &'a PluginRegistry,
    config: &'a RoutingConfig,
}

impl<'a> BackendRouter<'a> {
    pub fn new(registry: &'a PluginRegistry, config: &'a RoutingConfig) -> Self {
        Self { registry, config }
    }

    /// 为一次翻译选择引擎。
    pub fn route(
        &self,
        req: &TranslateRequest,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> Result<RouteOutcome, TranslationError> {
        match &req.policy {
            TranslationPolicy::DictionaryOnly => Err(TranslationError::new(
                TranslationErrorCode::CapabilityUnavailable,
                "句子翻译不使用词典引擎；单词/短语查询请调用 lookup()",
            )),
            TranslationPolicy::ExplicitEngine(id) => {
                let engine = self.registry.find_engine(id).ok_or_else(|| {
                    TranslationError::new(
                        TranslationErrorCode::CapabilityUnavailable,
                        format!("请求指定的引擎不存在：{id}"),
                    )
                })?;
                if !engine.supports(source, target) {
                    return Err(unsupported_pair(source, target, id));
                }
                let capability = primary_capability(&engine);
                Ok(RouteOutcome {
                    route: route_for(&engine, capability),
                    capability,
                    engine,
                    reason: format!("显式指定引擎 {id}"),
                })
            }
            TranslationPolicy::RemoteOnly => {
                let engine = self
                    .find_target(Capability::Remote, source, target)?
                    .ok_or_else(|| {
                        TranslationError::new(
                            TranslationErrorCode::CapabilityUnavailable,
                            "策略要求远程引擎，但没有注册可用的远程引擎",
                        )
                    })?;
                Ok(self.outcome(engine, Capability::Remote, "策略 RemoteOnly"))
            }
            TranslationPolicy::LocalOnly => {
                let engine = self.find_local(source, target)?.ok_or_else(|| {
                    TranslationError::new(
                        TranslationErrorCode::CapabilityUnavailable,
                        "策略要求本地引擎，但没有注册可用的本地引擎",
                    )
                })?;
                Ok(self.outcome(engine, Capability::Nmt, "策略 LocalOnly"))
            }
            TranslationPolicy::AutoLocalFirst => {
                // 1) 配置的默认后端（仍必须支持该语言对）
                if let Some(preferred) = &self.config.preferred_backend {
                    if let Some(engine) = self.registry.find_engine(preferred) {
                        if engine.supports(source, target) {
                            let capability = primary_capability(&engine);
                            return Ok(RouteOutcome {
                                route: route_for(&engine, capability),
                                capability,
                                engine,
                                reason: format!("配置的默认后端 {preferred}"),
                            });
                        }
                    }
                }
                // 2) 本地优先
                if let Some(engine) = self.find_local(source, target)? {
                    return Ok(self.outcome(engine, Capability::Nmt, "本地 NMT 优先"));
                }
                // 3) 显式授权后才回退远程
                if self.config.remote_fallback.allows_unavailable() {
                    if let Some(engine) = self.find_target(Capability::Remote, source, target)? {
                        return Ok(RouteOutcome {
                            route: TranslationRoute::Remote,
                            capability: Capability::Remote,
                            engine,
                            reason: "本地不可用且已授权远程回退".to_string(),
                        });
                    }
                }
                Err(TranslationError::new(
                    TranslationErrorCode::CapabilityUnavailable,
                    format!(
                        "没有可用的翻译后端支持 {}→{}；请注册本地引擎，或显式启用远程回退",
                        source.as_str(),
                        target.as_str()
                    ),
                ))
            }
        }
    }

    fn outcome(
        &self,
        engine: Arc<dyn TranslationEngine>,
        capability: Capability,
        reason: &str,
    ) -> RouteOutcome {
        RouteOutcome {
            route: route_for(&engine, capability),
            capability,
            engine,
            reason: reason.to_string(),
        }
    }

    /// 找支持该语言对的本地（非远程）NMT 引擎。
    fn find_local(
        &self,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> Result<Option<Arc<dyn TranslationEngine>>, TranslationError> {
        let candidates = self.registry.engines_with(Capability::Nmt);
        Ok(candidates.into_iter().find(|engine| {
            engine.supports(source, target) && !engine.descriptor().supports(Capability::Remote)
        }))
    }

    fn find_target(
        &self,
        capability: Capability,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> Result<Option<Arc<dyn TranslationEngine>>, TranslationError> {
        let candidates = self.registry.engines_with(capability);
        Ok(candidates
            .into_iter()
            .find(|engine| engine.supports(source, target)))
    }
}

fn primary_capability(engine: &Arc<dyn TranslationEngine>) -> Capability {
    let descriptor = engine.descriptor();
    if descriptor.supports(Capability::Remote) {
        Capability::Remote
    } else if descriptor.supports(Capability::Nmt) {
        Capability::Nmt
    } else {
        Capability::Dictionary
    }
}

fn route_for(engine: &Arc<dyn TranslationEngine>, capability: Capability) -> TranslationRoute {
    let _ = engine;
    match capability {
        Capability::Remote => TranslationRoute::Remote,
        Capability::Nmt => TranslationRoute::LocalNmt,
        Capability::Dictionary => TranslationRoute::Dictionary,
    }
}

fn unsupported_pair(
    source: &LanguageTag,
    target: &LanguageTag,
    engine_id: &str,
) -> TranslationError {
    TranslationError::new(
        TranslationErrorCode::UnsupportedLanguage,
        format!(
            "引擎 {engine_id} 不支持 {}→{}",
            source.as_str(),
            target.as_str()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_plugin_api::mocks::{MockDictionaryEngine, MockNmtEngine, MockRemoteEngine};

    fn en() -> LanguageTag {
        LanguageTag::new("en").unwrap()
    }
    fn zh() -> LanguageTag {
        LanguageTag::new("zh-CN").unwrap()
    }

    fn registry_with(engines: &[&str]) -> PluginRegistry {
        let mut registry = PluginRegistry::new();
        for id in engines {
            match *id {
                "dictionary" => registry
                    .register_engine(MockDictionaryEngine::new())
                    .unwrap(),
                "nmt" => registry
                    .register_engine(MockNmtEngine::with_priority(10))
                    .unwrap(),
                "remote" => registry
                    .register_engine(MockRemoteEngine::new(true))
                    .unwrap(),
                other => panic!("未知 mock：{other}"),
            }
        }
        registry
    }

    fn req(policy: TranslationPolicy) -> TranslateRequest {
        TranslateRequest::new("Hello", zh()).with_policy(policy)
    }

    #[test]
    fn 默认策略优先本地() {
        let registry = registry_with(&["nmt", "remote"]);
        let config = RoutingConfig::default();
        let router = BackendRouter::new(&registry, &config);
        let outcome = router
            .route(&req(TranslationPolicy::AutoLocalFirst), &en(), &zh())
            .unwrap();
        assert_eq!(outcome.engine_id(), "mock.nmt");
        assert_eq!(outcome.route, TranslationRoute::LocalNmt);
    }

    #[test]
    fn 默认策略不会静默上传到远程() {
        let registry = registry_with(&["remote"]);
        let config = RoutingConfig::default();
        let router = BackendRouter::new(&registry, &config);
        let err = router
            .route(&req(TranslationPolicy::AutoLocalFirst), &en(), &zh())
            .unwrap_err();
        assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
    }

    #[test]
    fn 显式授权后可以回退远程() {
        let registry = registry_with(&["remote"]);
        let config = RoutingConfig {
            preferred_backend: None,
            remote_fallback: RemoteFallbackPolicy::OnLocalUnavailable,
        };
        let router = BackendRouter::new(&registry, &config);
        let outcome = router
            .route(&req(TranslationPolicy::AutoLocalFirst), &en(), &zh())
            .unwrap();
        assert_eq!(outcome.engine_id(), "mock.remote");
        assert_eq!(outcome.route, TranslationRoute::Remote);
    }

    #[test]
    fn 配置的默认后端优先() {
        let registry = registry_with(&["nmt", "remote"]);
        let config = RoutingConfig {
            preferred_backend: Some("mock.remote".to_string()),
            remote_fallback: RemoteFallbackPolicy::Disabled,
        };
        let router = BackendRouter::new(&registry, &config);
        let outcome = router
            .route(&req(TranslationPolicy::AutoLocalFirst), &en(), &zh())
            .unwrap();
        assert_eq!(outcome.engine_id(), "mock.remote");
    }

    #[test]
    fn 指名引擎不存在时报错() {
        let registry = registry_with(&["nmt"]);
        let config = RoutingConfig::default();
        let router = BackendRouter::new(&registry, &config);
        let err = router
            .route(
                &req(TranslationPolicy::ExplicitEngine("nope".into())),
                &en(),
                &zh(),
            )
            .unwrap_err();
        assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
    }

    #[test]
    fn 句子翻译不使用词典引擎() {
        let registry = registry_with(&["dictionary"]);
        let config = RoutingConfig::default();
        let router = BackendRouter::new(&registry, &config);
        let err = router
            .route(&req(TranslationPolicy::DictionaryOnly), &en(), &zh())
            .unwrap_err();
        assert!(err.message.contains("lookup()"), "{}", err.message);
    }

    #[test]
    fn 远程专用策略() {
        let registry = registry_with(&["nmt", "remote"]);
        let config = RoutingConfig::default();
        let router = BackendRouter::new(&registry, &config);
        let outcome = router
            .route(&req(TranslationPolicy::RemoteOnly), &en(), &zh())
            .unwrap();
        assert_eq!(outcome.engine_id(), "mock.remote");
    }

    #[test]
    fn 语言对不支持时报错() {
        let registry = registry_with(&["nmt"]);
        let config = RoutingConfig::default();
        let router = BackendRouter::new(&registry, &config);
        let ja = LanguageTag::new("ja").unwrap();
        // MockNmtEngine supports 任意语言对，因此这里构造"没有任何引擎"的场景
        let empty = PluginRegistry::new();
        let empty_config = RoutingConfig::default();
        let router2 = BackendRouter::new(&empty, &empty_config);
        let err = router2
            .route(&req(TranslationPolicy::AutoLocalFirst), &ja, &zh())
            .unwrap_err();
        assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
        // 而支持的组合可以路由
        assert!(router
            .route(&req(TranslationPolicy::AutoLocalFirst), &ja, &zh())
            .is_ok());
    }
}
