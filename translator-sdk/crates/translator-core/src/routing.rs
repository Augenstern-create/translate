//! 兼容层：路由决策已统一到 [`crate::planner::TranslationPlanner`]（P17-A / A2）。
//!
//! 保留本模块是为了不破坏旧的 `translator_core::routing::*` 路径；新代码应使用
//! `planner::TranslationPlanner`。这里不包含第二套策略分支。

pub use crate::planner::{not_installed, RouteOutcome, TranslationPlanner};

use translator_types::{LanguageTag, TranslateRequest, TranslationError};

/// 兼容旧签名：用默认的“禁止自动远程回退”策略选择引擎。
///
/// 协调器已改用 [`TranslationPlanner`]；此函数只供旧测试/宿主过渡使用。
pub fn select(
    registry: &translator_plugin_api::PluginRegistry,
    req: &TranslateRequest,
    source: &LanguageTag,
    text: &str,
    prefer_dictionary: bool,
) -> Result<RouteOutcome, TranslationError> {
    TranslationPlanner::new(registry, translator_types::RemoteFallbackPolicy::Disabled).resolve(
        req,
        source,
        text,
        prefer_dictionary,
    )
}

/// 兼容旧签名：词典未命中后的本地 NMT 回退。
pub fn fallback_to_nmt(
    registry: &translator_plugin_api::PluginRegistry,
    source: &LanguageTag,
    target: &LanguageTag,
) -> Option<RouteOutcome> {
    TranslationPlanner::new(registry, translator_types::RemoteFallbackPolicy::Disabled)
        .fallback_after_dictionary_miss(source, target)
}
