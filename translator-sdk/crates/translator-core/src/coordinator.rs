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
use crate::rules::{RulePreview, TextRules};
use crate::text;

/// 协调器配置。
#[derive(Clone, Debug)]
pub struct CoordinatorConfig {
    /// 查询缓存容量（0 表示禁用）。
    pub cache_capacity: usize,
    /// 规则包版本。规则变更必须让旧缓存失效，因此进入缓存键。
    ///
    /// 配置了 `rules` 时以规则包自报的版本为准，这里的值只作为
    /// 「无规则包时的手动版本号」使用（例如宿主自己实现规则、只改版本）。
    pub rules_version: u32,
    /// 文本规则包。`None` 表示不做任何规则处理。
    pub rules: Option<Arc<dyn TextRules>>,
    /// 占位符恢复失败时是否直接报错。
    ///
    /// 默认 `false`：只发 `PlaceholderRestoreFailed` 警告（P0 行为）。
    /// 置 `true` 时，只要有关键占位符（URL/邮箱/代码/模板变量）没能还原，
    /// 就返回 `IntegrityFailure` 错误而不是返回一份缺内容的译文。
    /// 判断"关键"的依据来自 `translate` 拿到的警告与 span 数量，不依赖引擎配合。
    pub strict_placeholder_restore: bool,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 256,
            rules_version: 1,
            rules: None,
            strict_placeholder_restore: false,
        }
    }
}

pub struct Coordinator {
    registry: Arc<PluginRegistry>,
    cache: Mutex<QueryCache>,
    config: CoordinatorConfig,
    /// 实际参与缓存键的规则版本（规则包自报版本优先）。
    effective_rules_version: u32,
    /// 实际参与缓存键的资源指纹（挂载的词典/规则包 id@version）。
    resource_fingerprint: String,
}

impl Coordinator {
    pub fn new(registry: Arc<PluginRegistry>, config: CoordinatorConfig) -> Self {
        let capacity = config.cache_capacity;
        let effective_rules_version = config
            .rules
            .as_ref()
            .map(|rules| rules.version())
            .unwrap_or(config.rules_version);
        Self {
            registry,
            cache: Mutex::new(QueryCache::new(capacity)),
            config,
            effective_rules_version,
            resource_fingerprint: String::new(),
        }
    }

    /// 规则包版本（参与缓存键与诊断输出）。
    pub fn rules_version(&self) -> u32 {
        self.effective_rules_version
    }

    /// 绑定已挂载资源的指纹（`id@version` 用 `;` 连接）。
    ///
    /// 资源版本变化必须让旧缓存失效（ADR-011）。目前资源在构建期绑定，
    /// 换版本等于换 `Translator`（自然是新缓存）；这里仍把指纹写进键，
    /// 是为了将来支持热替换时不会静默读到旧结果。
    pub fn bind_resource_fingerprint(&mut self, fingerprint: impl Into<String>) {
        let fingerprint = fingerprint.into();
        if !fingerprint.is_empty() {
            self.resource_fingerprint = fingerprint;
        }
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// 预览规则包对输入文本的作用，不执行翻译。
    pub fn explain_rules(
        &self,
        text: &str,
        source: &translator_types::LanguageTag,
        target: &translator_types::LanguageTag,
    ) -> RulePreview {
        let normalized = text::normalize(text);
        match &self.config.rules {
            None => RulePreview::none(normalized),
            Some(rules) => {
                // 与真实翻译走同一条路径（含受保护片段跳过），否则预览会骗人
                let (text, applied) = self
                    .apply_rules(&normalized, source, target)
                    .unwrap_or_else(|_| (normalized.clone(), Vec::new()));
                RulePreview {
                    rules_id: Some(rules.id().to_string()),
                    version: rules.version(),
                    text,
                    applied,
                    rule_count: rules.rule_count(),
                }
            }
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
        // 规则可能改变文本形态（全角→半角、术语替换），路由必须看到处理后的文本
        let work_text = self.apply_rules(&normalized, &source, &req.target)?.0;
        let outcome = routing::select(&self.registry, req, &source, &work_text)?;
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

        // 3) 规则包（声明式、可编译校验；详见 translator-rules）
        let (work_text, rule_applied) = self.apply_rules(&normalized, &source, &req.target)?;
        if work_text.trim().is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "规则处理后文本为空",
            ));
        }

        // 4) 占位符保护
        let protected = text::protect(&work_text);

        // 5) 路由（用的是规则处理后的文本）
        let outcome = routing::select(&self.registry, req, &source, &protected.text)?;

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
            self.effective_rules_version,
            &self.resource_fingerprint,
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
        if !missing.is_empty() && self.config.strict_placeholder_restore {
            // 严格模式：宁可不给译文，也不返回缺内容的译文
            return Err(TranslationError::new(
                TranslationErrorCode::IntegrityFailure,
                format!(
                    "占位符恢复失败（严格模式）：{} 个受保护片段未被引擎保留：{}",
                    missing.len(),
                    missing.join(", ")
                ),
            ));
        }
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
        // 规则 trace 不进 TranslationResult：给公开枚举加变体会破坏宿主的穷尽匹配，
        // 因此只保留内部应用（见 `explain_rules`）。这里显式忽略以避免误用。
        let _ = rule_applied;

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

    /// 应用规则包。返回 (处理后文本, 实际生效的规则 id)。
    ///
    /// 受保护跨度（URL / 邮箱 / `{模板变量}` / 反引号代码）的跳过由规则引擎自己保证
    /// （`TextRules::apply` 的契约），这里只负责调用与去重。这样"金样本校验的行为"
    /// 与"线上行为"是同一套实现，不会出现样例通过、线上改坏的情况。
    fn apply_rules(
        &self,
        text: &str,
        source: &translator_types::LanguageTag,
        target: &translator_types::LanguageTag,
    ) -> Result<(String, Vec<String>), TranslationError> {
        let Some(rules) = &self.config.rules else {
            return Ok((text.to_string(), Vec::new()));
        };
        let outcome = rules.apply(text, source, target);
        Ok((outcome.text, outcome.applied))
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
