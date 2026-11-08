//! 引擎选择（路由）。
//!
//! 对应设计文档 §6「AutoLocalFirst 的推荐决策」：
//!
//! 1. 长度、格式、语言检测不确定时，不要用查词替代整句翻译；
//! 2. 单词请求默认走词典；
//! 3. 完整句子优先本地 NMT；
//! 4. 未安装则返回 `ResourceNotInstalled` 并附安装建议；
//! 5. **仅当策略明确允许且宿主授权时**才选择远程引擎。

use std::sync::Arc;

use translator_plugin_api::{PluginRegistry, TranslationEngine};
use translator_types::{
    Capability, LanguageTag, RouteDecision, TranslateRequest, TranslationError,
    TranslationErrorCode,
};

use crate::text;

/// 路由结果：选中的引擎 + 可解释的决策记录。
pub struct RouteOutcome {
    pub engine: Arc<dyn TranslationEngine>,
    pub decision: RouteDecision,
}

fn pick(
    registry: &PluginRegistry,
    capability: Capability,
    source: &LanguageTag,
    target: &LanguageTag,
) -> Option<Arc<dyn TranslationEngine>> {
    registry
        .engines_with(capability)
        .into_iter()
        .find(|e| e.supports(source, target))
}

pub(crate) fn not_installed(
    capability: Capability,
    source: &LanguageTag,
    target: &LanguageTag,
) -> TranslationError {
    TranslationError::new(
        TranslationErrorCode::ResourceNotInstalled,
        format!("没有可用的 {} 引擎支持 {} → {}", capability, source, target),
    )
    .with_suggestions([format!(
        "package.{}.{}-{}",
        capability.as_str(),
        source,
        target
    )])
}

/// 依据策略与语言对选择引擎。
///
/// `text` 是**规则处理后的**待翻译文本（不是 `req.text`）：
/// 规则可能把全角字母转成半角、把缩写展开，这会影响
/// 「是不是一个单词」的判断，进而影响该不该走词典。
pub fn select(
    registry: &PluginRegistry,
    req: &TranslateRequest,
    source: &LanguageTag,
    text: &str,
    prefer_dictionary: bool,
) -> Result<RouteOutcome, TranslationError> {
    use translator_types::TranslationPolicy as P;

    let target = &req.target;

    let outcome = match &req.policy {
        P::ExplicitEngine(id) => {
            let engine = registry.find_engine(id).ok_or_else(|| {
                TranslationError::new(
                    TranslationErrorCode::CapabilityUnavailable,
                    format!("指定的引擎不存在：{id}"),
                )
            })?;
            RouteOutcome {
                decision: RouteDecision {
                    engine_id: id.clone(),
                    capability: Capability::Nmt,
                    reason: "策略指定了具体引擎".to_string(),
                },
                engine,
            }
        }

        P::DictionaryOnly => {
            let engine = pick(registry, Capability::Dictionary, source, target)
                .ok_or_else(|| not_installed(Capability::Dictionary, source, target))?;
            RouteOutcome {
                decision: RouteDecision {
                    engine_id: engine.descriptor().id.to_string(),
                    capability: Capability::Dictionary,
                    reason: "策略为 DictionaryOnly".to_string(),
                },
                engine,
            }
        }

        P::LocalOnly => {
            let engine = pick(registry, Capability::Nmt, source, target)
                .ok_or_else(|| not_installed(Capability::Nmt, source, target))?;
            RouteOutcome {
                decision: RouteDecision {
                    engine_id: engine.descriptor().id.to_string(),
                    capability: Capability::Nmt,
                    reason: "策略为 LocalOnly".to_string(),
                },
                engine,
            }
        }

        P::RemoteOnly => {
            let engine = pick(registry, Capability::Remote, source, target)
                .ok_or_else(|| not_installed(Capability::Remote, source, target))?;
            RouteOutcome {
                decision: RouteDecision {
                    engine_id: engine.descriptor().id.to_string(),
                    capability: Capability::Remote,
                    reason: "策略为 RemoteOnly（需宿主已授权联网）".to_string(),
                },
                engine,
            }
        }

        P::AutoLocalFirst => {
            // 只有"像单词"的输入才优先查词典，避免用词条释义冒充整句翻译。
            if prefer_dictionary || text::looks_like_single_word(text) {
                if let Some(engine) = pick(registry, Capability::Dictionary, source, target) {
                    return Ok(RouteOutcome {
                        decision: RouteDecision {
                            engine_id: engine.descriptor().id.to_string(),
                            capability: Capability::Dictionary,
                            reason: if prefer_dictionary {
                                "文本类型为 Word，优先精确词典匹配".to_string()
                            } else {
                                "输入呈单词形态，优先精确词典匹配".to_string()
                            },
                        },
                        engine,
                    });
                }
            }

            // 其余一律交给本地 NMT；**不**自动回退远程（ADR-004）。
            let engine = pick(registry, Capability::Nmt, source, target)
                .ok_or_else(|| not_installed(Capability::Nmt, source, target))?;

            RouteOutcome {
                decision: RouteDecision {
                    engine_id: engine.descriptor().id.to_string(),
                    capability: Capability::Nmt,
                    reason: if text::looks_like_single_word(text) {
                        "单词形态但无词典可覆盖该语言对，退化到本地 NMT".to_string()
                    } else {
                        "输入为短语/句子，使用本地 NMT".to_string()
                    },
                },
                engine,
            }
        }
    };

    Ok(outcome)
}

/// 词典未命中时的回退选择。仅在 `AutoLocalFirst` 下调用。
pub fn fallback_to_nmt(
    registry: &PluginRegistry,
    source: &LanguageTag,
    target: &LanguageTag,
) -> Option<RouteOutcome> {
    pick(registry, Capability::Nmt, source, target).map(|engine| RouteOutcome {
        decision: RouteDecision {
            engine_id: engine.descriptor().id.to_string(),
            capability: Capability::Nmt,
            reason: "词典未命中，回退到本地 NMT".to_string(),
        },
        engine,
    })
}
