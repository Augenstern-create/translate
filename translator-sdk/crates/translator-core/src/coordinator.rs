//! 协调器：串起校验 → 规范化 → 语言识别 → 占位符保护 → 路由 → 执行 → 还原 → 术语约束 → 缓存。
//!
//! 对应设计文档 §6 的流水线。Coordinator 不认识任何具体引擎，
//! 只依赖 `translator-plugin-api` 的 trait，因此替换后端不需要改这里。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use translator_plugin_api::{BoxFuture, PluginRegistry};
use translator_types::{
    CancelToken, Capability, CapabilityInfo, DictionaryEntry, LookupRequest, RouteDecision,
    TranslateRequest, TranslationError, TranslationErrorCode, TranslationPolicy, TranslationResult,
    TranslationWarning,
};

use crate::cache::{CacheEntry, CacheKey, QueryCache};
use crate::diagnostics::{RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics};
use crate::routing;
use crate::rules::{RulePreview, TextRules};
use crate::terms::{TargetTermGuard, TermContext, TermOutcome, TermPolicy};
use crate::text;

/// 协调器配置。
///
/// 手写 `Debug`：`term_guard` 是 trait object，不该为了可打印而逼所有实现者
/// 额外实现 `Debug`。这里只打印它的 id/version。
#[derive(Clone)]
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
    /// 输出侧术语约束（P2.4）。`None` 表示不约束。
    pub term_guard: Option<Arc<dyn TargetTermGuard>>,
    /// 术语约束策略。仅在配置了 `term_guard` 时有效。
    ///
    /// 默认 [`TermPolicy::Warn`]：只报告不改写（P2-S / S-03）。
    pub term_policy: TermPolicy,
}

impl std::fmt::Debug for CoordinatorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let term = self
            .term_guard
            .as_ref()
            .map(|g| format!("{}@{}", g.id(), g.version()));
        f.debug_struct("CoordinatorConfig")
            .field("cache_capacity", &self.cache_capacity)
            .field("rules_version", &self.rules_version)
            .field("rules", &self.config_rules_label())
            .field(
                "strict_placeholder_restore",
                &self.strict_placeholder_restore,
            )
            .field("term_guard", &term)
            .field("term_policy", &self.term_policy)
            .finish()
    }
}

impl CoordinatorConfig {
    fn config_rules_label(&self) -> Option<String> {
        self.rules
            .as_ref()
            .map(|r| format!("{}@{}", r.id(), r.version()))
    }
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 256,
            rules_version: 1,
            rules: None,
            strict_placeholder_restore: false,
            term_guard: None,
            term_policy: TermPolicy::default(),
        }
    }
}

pub struct Coordinator {
    registry: Arc<PluginRegistry>,
    cache: Mutex<QueryCache>,
    config: CoordinatorConfig,
    /// 实际参与缓存键的规则版本（规则包自报版本优先）。
    effective_rules_version: u32,
    /// 实际参与缓存键的资源指纹（挂载的词典/规则包/术语表 id@version）。
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

    /// 术语约束策略（诊断用）。
    pub fn term_policy(&self) -> TermPolicy {
        self.config.term_policy
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

    /// 当前资源指纹（诊断用）。
    pub fn resource_fingerprint(&self) -> &str {
        &self.resource_fingerprint
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

    /// 预览术语约束会怎么改一段**译文**（不翻译、不写缓存）。
    ///
    /// 返回 `None` 表示没有配置术语表（与"配了但无违规"区分开）。
    pub fn explain_terms(
        &self,
        text: &str,
        source: &translator_types::LanguageTag,
        target: &translator_types::LanguageTag,
    ) -> Option<TermOutcome> {
        let guard = self.config.term_guard.as_ref()?;
        // 预览没有源文：把预览文本当作源文传入，因此需要源触发条件的规则会被跳过，
        // 这一点必须在返回值里看得出来（预览不是"线上效果"的等价物）。
        Some(guard.apply_with_context(&TermContext {
            source_text: text,
            target_text: text,
            source,
            target,
        }))
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

    /// 仅返回译文（对外主路径）。
    pub fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move { self.run(req).await.map(|out| out.result) })
    }

    /// 可取消的译文入口。
    ///
    /// 取消在阶段边界检查；引擎若能中断（Bergamot worker）会在执行中返回
    /// `Cancelled`，否则最迟在本次引擎调用返回后生效。取消不写缓存。
    pub fn translate_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            self.run_inner(req, Some(cancel))
                .await
                .map(|out| out.result)
        })
    }

    /// 返回译文 + 结构化诊断（P2.4）。
    ///
    /// 与 [`Self::translate`] 走**同一条**实现路径，只是多带出一份诊断，
    /// 因此两者不可能给出不同译文。
    pub fn translate_with_diagnostics<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        Box::pin(async move { self.run(req).await })
    }

    /// 带诊断的可取消入口（与 [`Self::translate_cancellable`] 同一实现）。
    pub fn translate_with_diagnostics_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        Box::pin(async move { self.run_inner(req, Some(cancel)).await })
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

    async fn run(&self, req: &TranslateRequest) -> Result<TranslateOutput, TranslationError> {
        self.run_inner(req, None).await
    }

    async fn run_inner(
        &self,
        req: &TranslateRequest,
        cancel: Option<&CancelToken>,
    ) -> Result<TranslateOutput, TranslationError> {
        check_cancelled(cancel)?;
        // 1) 校验 + 规范化
        let normalized = text::normalize(&req.text);
        if normalized.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "请求文本为空",
            ));
        }

        // 2) 源语言解析（记录"是猜的"还是"用户指定的"）
        let source_assumed = req.source.is_none();
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

        check_cancelled(cancel)?;
        // 4) 占位符保护
        let protected = text::protect(&work_text);
        let spans = text::protected_spans(&work_text);

        // 5) 路由（用的是规则处理后的文本）
        let outcome = routing::select(&self.registry, req, &source, &protected.text)?;

        let rules_trace = self.config.rules.as_ref().map(|rules| RuleTrace {
            id: rules.id().to_string(),
            version: rules.version(),
            applied: rule_applied,
        });

        // 6) 缓存查询
        let key = CacheKey::new(
            &protected.text,
            req,
            source.as_str(),
            &outcome.decision.engine_id,
            self.effective_rules_version,
            &self.resource_fingerprint,
        );

        if let Ok(cache) = self.cache.lock() {
            if let Some(hit) = cache.get(&key) {
                let mut entry = hit.clone();
                entry.result.warnings.push(TranslationWarning::CacheHit);
                entry.result.elapsed_ms = 0;
                entry.diagnostics.mark_cached();
                // 跨度与规则命中随当前输入重算（与首次一致，因为键包含文本哈希）
                entry.diagnostics.protected_spans = spans;
                if entry.diagnostics.rules.is_none() {
                    entry.diagnostics.rules = rules_trace;
                }
                return Ok(TranslateOutput {
                    result: entry.result,
                    diagnostics: entry.diagnostics,
                });
            }
        }

        // 7) 交给引擎的请求：文本已保护、源语言已确定
        let resolved = TranslateRequest {
            text: protected.text.clone(),
            source: Some(source.clone()),
            ..req.clone()
        };

        check_cancelled(cancel)?;
        let started = Instant::now();
        let mut result = match outcome
            .engine
            .translate_cancellable(&resolved, cancel.unwrap_or_else(|| never_cancelled()))
            .await
        {
            Ok(result) => result,
            Err(err) if self.should_fallback(req, outcome.decision.capability, &err) => {
                match routing::fallback_to_nmt(&self.registry, &source, &req.target) {
                    Some(fallback) => {
                        fallback
                            .engine
                            .translate_cancellable(
                                &resolved,
                                cancel.unwrap_or_else(|| never_cancelled()),
                            )
                            .await?
                    }
                    None => return Err(err),
                }
            }
            Err(err) => return Err(err),
        };
        check_cancelled(cancel)?;

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

        // 9) 输出侧术语约束（P2.4）：只在译文上做，且不得触碰受保护跨度
        let term_trace = self.apply_term_guard(&work_text, &source, &req.target, &mut result);

        // 10) 补全结果元信息
        result.source = source;
        result.target = req.target.clone();
        result.elapsed_ms = started.elapsed().as_millis() as u64;

        let diagnostics = TranslationDiagnostics {
            engine_id: outcome.decision.engine_id.clone(),
            capability: outcome.decision.capability,
            route_reason: outcome.decision.reason.clone(),
            cache_hit: false,
            source_assumed,
            rules_version: self.effective_rules_version,
            resource_fingerprint: self.resource_fingerprint.clone(),
            rules: rules_trace,
            protected_spans: spans,
            restore_missing: missing,
            term: term_trace,
        };

        // 11) 写缓存（缓存副本不含 CacheHit 警告与耗时）
        if let Ok(mut cache) = self.cache.lock() {
            let mut cached = CacheEntry {
                result: result.clone(),
                diagnostics: diagnostics.clone(),
            };
            cached
                .result
                .warnings
                .retain(|w| !matches!(w, TranslationWarning::CacheHit));
            cached.result.elapsed_ms = 0;
            cache.insert(key, cached);
        }

        Ok(TranslateOutput {
            result,
            diagnostics,
        })
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

    /// 输出侧术语约束。返回 `None` 表示"没跑"（未配置或策略为 `Off`）。
    ///
    /// 传入源文，让实现者能按"源文是否触发"决定要不要检查（P2-S / S-03）。
    fn apply_term_guard(
        &self,
        source_text: &str,
        source: &translator_types::LanguageTag,
        target: &translator_types::LanguageTag,
        result: &mut TranslationResult,
    ) -> Option<TermTrace> {
        if !self.config.term_policy.runs() {
            return None;
        }
        let guard = self.config.term_guard.as_ref()?;
        let outcome = guard.apply_with_context(&TermContext {
            source_text,
            target_text: &result.text,
            source,
            target,
        });

        let enforced = self.config.term_policy.applies_text();
        if enforced {
            result.text = outcome.text;
        }

        if !outcome.violations.is_empty() {
            // 已有枚举变体（P0 就存在），不新增变体：结构化信息走诊断
            let head: Vec<String> = outcome
                .violations
                .iter()
                .take(3)
                .map(|v| v.to_string())
                .collect();
            let more = outcome.violations.len().saturating_sub(head.len());
            result
                .warnings
                .push(TranslationWarning::TermMismatch(format!(
                    "术语不一致 {} 处（策略 {}）：{}{}",
                    outcome.violations.len(),
                    self.config.term_policy.as_str(),
                    head.join("；"),
                    if more > 0 {
                        format!("；另有 {more} 处")
                    } else {
                        String::new()
                    }
                )));
        }

        Some(TermTrace {
            id: guard.id().to_string(),
            version: guard.version(),
            policy: self.config.term_policy,
            replaced: if enforced {
                outcome.violations.len()
            } else {
                0
            },
            violations: outcome.violations,
        })
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

/// 取消检查点。`None` 表示调用方没要求取消，零成本。
fn check_cancelled(cancel: Option<&CancelToken>) -> Result<(), TranslationError> {
    match cancel {
        Some(token) if token.is_cancelled() => Err(TranslationError::cancelled()),
        _ => Ok(()),
    }
}

/// 未传令牌时给引擎的占位令牌（永远不取消）。
///
/// 用 `OnceLock` 而不是每个请求新建：它只读、无状态；真正需要取消时
/// 调用方总会显式传入令牌。
static NEVER: std::sync::OnceLock<CancelToken> = std::sync::OnceLock::new();

fn never_cancelled() -> &'static CancelToken {
    NEVER.get_or_init(CancelToken::new)
}
