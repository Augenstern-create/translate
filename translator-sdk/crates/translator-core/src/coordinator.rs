//! 协调器：一条固定顺序的翻译流水线。
//!
//! ```text
//! 校验输入 → 规范化 + 语言识别 → 整句模板匹配
//!   → 保护文本 → 提取命中术语 → 计算执行身份
//!   → 缓存查询 → 选择后端 → 执行 → 还原受保护内容
//!   → 术语/数字/完整性校验 →（必要时一次受限重试）→ 写缓存 → 返回
//! ```
//!
//! # 为什么重写
//!
//! 旧协调器有 1800 行，同时承担"翻译策略研究、产品路由、执行控制、结果处理"。
//! 它先试整句模板、再试语义组合、再试普通组合、再试上下文词典选义，
//! 最后才轮到真正能用的模型后端。重构后这里只做四件事：
//!
//! 1. **按固定顺序调用**（顺序写死，不能靠"碰运气命中某条规则"）；
//! 2. **传播错误**（每一类失败都有明确错误码）；
//! 3. **守住完整性门禁**（不通过校验的译文不得返回、不得入缓存）；
//! 4. **写诊断**（可解释但不泄露原文到日志）。
//!
//! 具体能力都在独立模块里：[`crate::protection`]、[`crate::quality`]、
//! [`crate::terms`]、[`crate::templates`]、[`crate::cache`]、[`crate::routing`]。
//!
//! # 重试预算（§5.5 / §12.3）
//!
//! 一次逻辑请求最多 `1 + content_retry` 次模型调用（默认 2 次）。
//!
//! - 传输层重试由后端自己负责（`translator-http` 内部有界重试）；
//! - 这里只重试**内容完整性**失败，且第二次会换策略或加纠正提示；
//! - 重试用尽仍然失败 → `IntegrityFailure`，**绝不**把原文当译文返回。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use translator_plugin_api::{BoxFuture, PluginRegistry, TranslationJob};
use translator_types::{
    CancelToken, CapabilityInfo, DictionaryEntry, LanguageTag, LookupRequest, ParagraphStrategy,
    RemoteFallbackPolicy, RouteDecision, RouteStats, TermConstraint, TokenUsage, TranslateOptions,
    TranslateRequest, TranslationError, TranslationErrorCode, TranslationMode, TranslationResult,
    TranslationRoute, TranslationWarning,
};

use crate::cache::{CacheEntry, CacheIdentity, CacheKey, CacheStats, QueryCache};
use crate::diagnostics::{RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics};
use crate::language::{HeuristicIdentifier, LanguageIdentifier, MorphologyAnalyzer};
use crate::protection::{ProtectionPlan, ProtectionPolicy, ProtectionStrategy, PROTECTION_VERSION};
use crate::quality::{self, QualityReport};
use crate::routing::{BackendRouter, RouteOutcome, RoutingConfig};
use crate::rules::{RuleOutcome, TextRules};
use crate::templates::{SentenceTemplates, TemplateScope};
use crate::terms::{Glossary, TermOutcome, TermPolicy};
use crate::text;

/// 协调器配置。
#[derive(Clone)]
pub struct CoordinatorConfig {
    /// 查询缓存容量（0 = 禁用）。
    pub cache_capacity: usize,
    /// 保护策略（默认自动：标记优先，冲突回退分段）。
    pub protection: ProtectionPolicy,
    /// 术语表。`None` 表示不使用术语约束。
    pub glossary: Option<Arc<Glossary>>,
    /// 术语策略（默认只报告不改写）。
    pub term_policy: TermPolicy,
    /// 确定性文本规则包（NFKC、术语替换等）。`None` 表示不做规则处理。
    ///
    /// 规则**只能**做确定性变换；任何"句型翻译"都不得回到这里（§19 风险一）。
    pub rules: Option<Arc<dyn TextRules>>,
    /// 固定 UI 文案模板。
    pub templates: Option<Arc<dyn SentenceTemplates>>,
    /// 模板作用域（应用/页面）。
    pub template_scope: TemplateScope,
    /// 默认翻译模式。
    pub default_mode: TranslationMode,
    /// 关键完整性检查失败时是否拒绝返回译文。
    ///
    /// 默认 `true`：受保护内容丢失、编号被改、空译文都返回 `IntegrityFailure`。
    /// 设 `false` 只用于诊断/研究，不得用于产品。
    pub strict_integrity: bool,
    /// 远程回退授权（默认禁止）。
    pub remote_fallback: RemoteFallbackPolicy,
    /// 默认后端 id（如 `http.remote`）。
    pub preferred_backend: Option<String>,
    /// 单次请求最大输入字符数（成本/延迟保护）。
    pub max_input_chars: usize,
    /// 默认超时（毫秒）。请求可用 `TranslateRequest::timeout_ms` 覆盖。
    pub default_timeout_ms: u64,
    /// 内容完整性失败后的**额外**尝试次数（默认 1）。
    pub content_retry: u32,
    /// 语言识别器；`None` 时使用内置启发式。
    pub language_identifier: Option<Arc<dyn LanguageIdentifier>>,
    /// 形态分析器（仅用于查词未命中后的候选）。
    pub lemmatizer: Option<Arc<dyn MorphologyAnalyzer>>,
    /// 隐私隔离域：不同域之间不共享缓存。
    pub privacy_scope: String,
    /// 术语表的默认领域。
    pub domain: Option<String>,
    /// 资源指纹（进缓存键与诊断，例如词典 `id@version`）。
    pub resource_fingerprint: String,
}

impl std::fmt::Debug for CoordinatorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoordinatorConfig")
            .field("cache_capacity", &self.cache_capacity)
            .field("protection", &self.protection)
            .field(
                "glossary",
                &self
                    .glossary
                    .as_ref()
                    .map(|g| format!("{}@{}", g.id(), g.version())),
            )
            .field("term_policy", &self.term_policy)
            .field(
                "rules",
                &self
                    .rules
                    .as_ref()
                    .map(|r| format!("{}@{}", r.id(), r.version())),
            )
            .field("templates", &self.templates.as_ref().map(|t| t.id()))
            .field("template_scope", &self.template_scope)
            .field("default_mode", &self.default_mode)
            .field("strict_integrity", &self.strict_integrity)
            .field("remote_fallback", &self.remote_fallback)
            .field("preferred_backend", &self.preferred_backend)
            .field("max_input_chars", &self.max_input_chars)
            .field("content_retry", &self.content_retry)
            .field("privacy_scope", &self.privacy_scope)
            .field("domain", &self.domain)
            .finish()
    }
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 256,
            protection: ProtectionPolicy::default(),
            glossary: None,
            term_policy: TermPolicy::default(),
            rules: None,
            templates: None,
            template_scope: TemplateScope::none(),
            default_mode: TranslationMode::Precise,
            strict_integrity: true,
            remote_fallback: RemoteFallbackPolicy::Disabled,
            preferred_backend: None,
            max_input_chars: 20_000,
            default_timeout_ms: 30_000,
            content_retry: 1,
            language_identifier: None,
            lemmatizer: None,
            privacy_scope: "default".to_string(),
            domain: None,
            resource_fingerprint: String::new(),
        }
    }
}

/// 路由与执行计数。
#[derive(Default)]
struct Counters {
    executions: AtomicU64,
    model_calls: AtomicU64,
    cache_hits: AtomicU64,
    template_hits: AtomicU64,
    failures: AtomicU64,
    retries: AtomicU64,
}

/// 协调器。
pub struct Coordinator {
    registry: Arc<PluginRegistry>,
    config: CoordinatorConfig,
    cache: Mutex<QueryCache>,
    language_identifier: Arc<dyn LanguageIdentifier>,
    counters: Counters,
}

impl Coordinator {
    pub fn new(registry: Arc<PluginRegistry>, config: CoordinatorConfig) -> Self {
        let cache = QueryCache::new(config.cache_capacity);
        let language_identifier = config
            .language_identifier
            .clone()
            .unwrap_or_else(|| Arc::new(HeuristicIdentifier::new()));
        Self {
            registry,
            config,
            cache: Mutex::new(cache),
            language_identifier,
            counters: Counters::default(),
        }
    }

    // ---------------- 只读访问 ----------------

    pub fn config(&self) -> &CoordinatorConfig {
        &self.config
    }

    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    pub fn default_mode(&self) -> TranslationMode {
        self.config.default_mode
    }

    pub fn term_policy(&self) -> TermPolicy {
        self.config.term_policy
    }

    pub fn glossary(&self) -> Option<&Arc<Glossary>> {
        self.config.glossary.as_ref()
    }

    pub fn capabilities(&self) -> Vec<CapabilityInfo> {
        self.registry.capability_infos()
    }

    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.lock().map(|c| c.stats()).unwrap_or_default()
    }

    pub fn route_stats(&self) -> RouteStats {
        RouteStats {
            executions: self.counters.executions.load(Ordering::Relaxed),
            dictionary_hits: 0,
            rule_matches: self.counters.template_hits.load(Ordering::Relaxed),
            nmt_calls: self.counters.model_calls.load(Ordering::Relaxed),
            remote_calls: self.counters.model_calls.load(Ordering::Relaxed),
            nmt_fallbacks: 0,
            remote_fallbacks: 0,
            cache_hits: self.counters.cache_hits.load(Ordering::Relaxed),
            best_candidates: 0,
            failures: self.counters.failures.load(Ordering::Relaxed),
        }
    }

    pub fn reset_route_stats(&self) {
        for counter in [
            &self.counters.executions,
            &self.counters.model_calls,
            &self.counters.cache_hits,
            &self.counters.template_hits,
            &self.counters.failures,
            &self.counters.retries,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }

    /// 默认执行选项。
    pub fn default_options(&self) -> TranslateOptions {
        TranslateOptions::new(self.config.default_mode)
    }

    // ---------------- 对外入口 ----------------

    /// 仅返回译文（默认选项）。
    pub fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        let options = self.default_options();
        Box::pin(async move { self.run(req, None, &options).await.map(|o| o.result) })
    }

    /// 指定模式。
    pub fn translate_with_mode<'a>(
        &'a self,
        req: &'a TranslateRequest,
        mode: TranslationMode,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        let options = TranslateOptions::new(mode);
        Box::pin(async move { self.run(req, None, &options).await.map(|o| o.result) })
    }

    /// 指定选项。
    pub fn translate_with_options<'a>(
        &'a self,
        req: &'a TranslateRequest,
        options: &'a TranslateOptions,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move { self.run(req, None, options).await.map(|o| o.result) })
    }

    /// 可取消。
    pub fn translate_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        let options = self.default_options();
        Box::pin(async move {
            self.run(req, Some(cancel), &options)
                .await
                .map(|o| o.result)
        })
    }

    /// 可取消 + 指定模式。
    pub fn translate_cancellable_with_mode<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
        mode: TranslationMode,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        let options = TranslateOptions::new(mode);
        Box::pin(async move {
            self.run(req, Some(cancel), &options)
                .await
                .map(|o| o.result)
        })
    }

    /// 带诊断（默认选项）。
    pub fn translate_with_diagnostics<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        let options = self.default_options();
        Box::pin(async move { self.run(req, None, &options).await })
    }

    /// 带诊断 + 指定选项。
    pub fn translate_with_diagnostics_options<'a>(
        &'a self,
        req: &'a TranslateRequest,
        options: &'a TranslateOptions,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        Box::pin(async move { self.run(req, None, options).await })
    }

    /// 带诊断 + 可取消。
    pub fn translate_with_diagnostics_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        let options = self.default_options();
        Box::pin(async move { self.run(req, Some(cancel), &options).await })
    }

    /// 词典查询（与句子翻译完全分开，§7.2）。
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
            if req.limit == 0 {
                return Err(TranslationError::new(
                    TranslationErrorCode::InvalidRequest,
                    "limit 必须大于 0",
                ));
            }
            if self.registry.providers().is_empty() {
                return Err(TranslationError::new(
                    TranslationErrorCode::DictionaryUnavailable,
                    "没有注册任何词典；请安装词典包或注册词典插件",
                ));
            }

            let mut all = self.lookup_term(req, &req.term).await?;

            // 词形未命中：尝试词形还原后的候选（只作补充，不覆盖原文结果）
            if all.is_empty() {
                if let Some(lemmatizer) = &self.config.lemmatizer {
                    for lemma in lemmatizer.lemmas(&req.term).into_iter().take(3) {
                        let found = self.lookup_term(req, &lemma).await?;
                        for entry in found {
                            if !all.iter().any(|e: &DictionaryEntry| {
                                e.headword == entry.headword && e.source_id == entry.source_id
                            }) {
                                all.push(entry);
                            }
                        }
                        if all.len() >= req.limit {
                            break;
                        }
                    }
                }
            }

            all.truncate(req.limit);
            Ok(all)
        })
    }

    async fn lookup_term(
        &self,
        req: &LookupRequest,
        term: &str,
    ) -> Result<Vec<DictionaryEntry>, TranslationError> {
        let mut all = Vec::new();
        let sub = LookupRequest {
            term: term.to_string(),
            language: req.language.clone(),
            limit: req.limit,
        };
        for provider in self.registry.providers() {
            let mut entries = provider.lookup(&sub).await?;
            all.append(&mut entries);
            if all.len() >= req.limit {
                break;
            }
        }
        Ok(all)
    }

    /// 只返回路由决策，不执行翻译、不记录原文。
    pub fn explain_route(&self, req: &TranslateRequest) -> Result<RouteDecision, TranslationError> {
        let normalized = self.validate_and_normalize(&req.text)?;
        let (source, _) = self.resolve_source(&normalized, req)?;
        let outcome = self.route(req, &source, &req.target)?;
        Ok(outcome.decision())
    }

    /// 预览术语判定（不改写、不翻译）。
    ///
    /// 返回 `None` 表示没有配置术语表（与"配了但没有命中"区分开）。
    pub fn explain_terms(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> Option<TermOutcome> {
        let glossary = self.config.glossary.as_ref()?;
        let verification = glossary.verify(
            text,
            text,
            source.as_str(),
            target.as_str(),
            self.config.domain.as_deref(),
        );
        Some(TermOutcome {
            text: text.to_string(),
            violations: verification.violations,
        })
    }

    // ---------------- 流水线 ----------------

    fn routing_config(&self) -> RoutingConfig {
        RoutingConfig {
            preferred_backend: self.config.preferred_backend.clone(),
            remote_fallback: self.config.remote_fallback,
        }
    }

    fn route(
        &self,
        req: &TranslateRequest,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> Result<RouteOutcome, TranslationError> {
        BackendRouter::new(&self.registry, &self.routing_config()).route(req, source, target)
    }

    fn validate_and_normalize(&self, raw: &str) -> Result<String, TranslationError> {
        if raw.trim().is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "请求文本为空",
            ));
        }
        let normalized = text::normalize(raw);
        let chars = normalized.chars().count();
        if chars > self.config.max_input_chars {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!(
                    "输入过长：{chars} 字符，上限 {}",
                    self.config.max_input_chars
                ),
            ));
        }
        Ok(normalized)
    }

    fn resolve_source(
        &self,
        normalized: &str,
        req: &TranslateRequest,
    ) -> Result<(LanguageTag, bool), TranslationError> {
        if let Some(source) = &req.source {
            return Ok((source.clone(), false));
        }
        let guessed = self
            .config
            .language_identifier
            .as_ref()
            .unwrap_or(&self.language_identifier)
            .identify(normalized, 1)
            .into_iter()
            .next()
            .map(|g| g.language);
        guessed.map(|tag| (tag, true)).ok_or_else(|| {
            TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "无法自动识别源语言，请显式指定 source",
            )
        })
    }

    fn check_cancel(&self, cancel: Option<&CancelToken>) -> Result<(), TranslationError> {
        match cancel {
            Some(token) if token.is_cancelled() => Err(TranslationError::cancelled()),
            _ => Ok(()),
        }
    }

    fn deadline(&self, req: &TranslateRequest) -> Option<Instant> {
        let ms = req.timeout_ms.unwrap_or(self.config.default_timeout_ms);
        (ms > 0).then(|| Instant::now() + Duration::from_millis(ms))
    }

    /// 一次请求的完整执行。
    async fn run(
        &self,
        req: &TranslateRequest,
        cancel: Option<&CancelToken>,
        options: &TranslateOptions,
    ) -> Result<TranslateOutput, TranslationError> {
        let started = Instant::now();
        self.counters.executions.fetch_add(1, Ordering::Relaxed);
        let result = self.run_inner(req, cancel, options, started).await;
        if result.is_err() {
            self.counters.failures.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    async fn run_inner(
        &self,
        req: &TranslateRequest,
        cancel: Option<&CancelToken>,
        options: &TranslateOptions,
        started: Instant,
    ) -> Result<TranslateOutput, TranslationError> {
        // 1. 校验输入
        let normalized = self.validate_and_normalize(&req.text)?;
        self.check_cancel(cancel)?;

        // 2. 语言识别
        let (source, source_assumed) = self.resolve_source(&normalized, req)?;
        if source == req.target {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "源语言与目标语言相同",
            ));
        }

        // 3. 确定性规则（规范化/术语替换）。规则变更必须换版本，否则缓存会串。
        let RuleOutcome {
            text: work_text,
            applied: rule_applied,
            warnings: rule_warnings,
        } = self.apply_rules(&normalized, &source, &req.target);

        // 4. 整句模板（确定性 UI 文案）
        if let Some(templates) = &self.config.templates {
            if let Some(hit) = templates.match_scoped(
                &work_text,
                &source,
                &req.target,
                &self.config.template_scope,
            ) {
                return self.finish_template(
                    req,
                    options,
                    &work_text,
                    &source,
                    hit,
                    source_assumed,
                    started,
                );
            }
        }

        // 5. 保护文本
        let mut plan = ProtectionPlan::prepare(&work_text, &self.config.protection)?;
        self.check_cancel(cancel)?;

        // 6. 命中术语（只发命中的，§6.5）
        let constraints: Vec<TermConstraint> = self
            .config
            .glossary
            .as_ref()
            .map(|g| {
                g.constraints(
                    &work_text,
                    source.as_str(),
                    req.target.as_str(),
                    self.config.domain.as_deref(),
                )
            })
            .unwrap_or_default();

        // 7. 路由（决定后端 id 与缓存身份）
        let route = self.route(req, &source, &req.target)?;

        // 8. 缓存查询
        let identity = self.cache_identity(&route, options, &source, &req.target);
        let key = CacheKey::new(&work_text, identity);
        if let Some(entry) = self.cache_get(&key) {
            self.counters.cache_hits.fetch_add(1, Ordering::Relaxed);
            let mut entry = entry;
            entry.result.elapsed_ms = started.elapsed().as_millis() as u64;
            entry.result.warnings.push(TranslationWarning::CacheHit);
            entry.diagnostics.cache_hit = true;
            entry.diagnostics.elapsed_ms = entry.result.elapsed_ms;
            entry.diagnostics.route_reason = format!("{}（命中缓存）", route.reason);
            entry.diagnostics.protection_strategy = plan.strategy().as_str().to_string();
            return Ok(TranslateOutput {
                result: entry.result,
                diagnostics: entry.diagnostics,
            });
        }

        // 9. 执行（含一次受限重试）
        let deadline = self.deadline(req);
        let max_attempts = 1 + self.config.content_retry;
        let mut attempt: u32 = 0;
        let mut retry_hint: Option<String> = None;
        let mut warnings: Vec<TranslationWarning> = rule_warnings;
        if source_assumed {
            warnings.push(TranslationWarning::LanguageAssumed(
                source.as_str().to_string(),
            ));
        }

        let (outcome, quality_report) = loop {
            attempt += 1;
            self.check_cancel(cancel)?;
            if attempt > 1 {
                self.counters.retries.fetch_add(1, Ordering::Relaxed);
            }

            let executed = self
                .execute(
                    &route,
                    &plan,
                    &constraints,
                    req,
                    options,
                    &source,
                    deadline,
                    cancel,
                    retry_hint.as_deref(),
                )
                .await;

            match executed {
                Ok(executed) => {
                    let report = quality::check_with_protection(
                        &plan,
                        &executed.text,
                        &req.target,
                        options.mode,
                    );
                    let fatal = report.has_fatal() && self.config.strict_integrity;
                    if !fatal {
                        break (executed, report);
                    }

                    if attempt >= max_attempts {
                        return Err(TranslationError::new(
                            TranslationErrorCode::IntegrityFailure,
                            format!(
                                "译文未通过完整性检查（尝试 {attempt} 次）：{}",
                                report.fatal_summary()
                            ),
                        ));
                    }

                    // 第一次失败：如果用的是标记策略，就换分段（结构上不可能丢标记）
                    if plan.strategy() == ProtectionStrategy::Marker && !plan.is_trivial() {
                        plan = ProtectionPlan::prepare(
                            &work_text,
                            &ProtectionPolicy {
                                preference: crate::protection::StrategyPreference::Segment,
                                spans: self.config.protection.spans,
                            },
                        )?;
                        retry_hint = Some(
                            "上一次输出没有完整保留受保护内容；这次受保护内容不会出现在输入里。"
                                .to_string(),
                        );
                    } else {
                        let missing = report.fatal_summary();
                        retry_hint = Some(format!(
                            "上一次输出未通过完整性检查（{missing}）。请逐字保留所有数字、编号与占位符。"
                        ));
                    }
                    continue;
                }
                Err(err) => {
                    let retryable = matches!(
                        err.code,
                        TranslationErrorCode::IntegrityFailure
                            | TranslationErrorCode::InvalidBackendResponse
                    );
                    if retryable && attempt < max_attempts {
                        if plan.strategy() == ProtectionStrategy::Marker && !plan.is_trivial() {
                            plan = ProtectionPlan::prepare(
                                &work_text,
                                &ProtectionPolicy {
                                    preference: crate::protection::StrategyPreference::Segment,
                                    spans: self.config.protection.spans,
                                },
                            )?;
                        }
                        retry_hint = Some(format!(
                            "上一次尝试失败：{}。请严格保留所有受保护内容与数字。",
                            err.message
                        ));
                        continue;
                    }
                    // 重试用尽：内容类失败一律归为完整性失败，避免宿主把
                    // "重试后仍然不完整"误当成普通后端故障。
                    let code = match err.code {
                        // "响应格式不对/空译文" 在重试用尽后就是内容不完整
                        TranslationErrorCode::InvalidBackendResponse => {
                            TranslationErrorCode::IntegrityFailure
                        }
                        other => other,
                    };
                    return Err(TranslationError::new(code, err.message));
                }
            }
        };

        // 10. 术语校验
        let term_verification = self.config.glossary.as_ref().map(|glossary| {
            glossary.verify(
                &work_text,
                &outcome.text,
                source.as_str(),
                req.target.as_str(),
                self.config.domain.as_deref(),
            )
        });

        if let Some(verification) = &term_verification {
            if self.config.term_policy.runs() {
                for violation in &verification.violations {
                    warnings.push(TranslationWarning::TermMismatch(violation.detail()));
                }
                if self.config.term_policy.rejects_unmet_strict()
                    && !verification.strict_satisfied()
                {
                    let detail: Vec<String> = verification
                        .violations
                        .iter()
                        .filter(|v| v.strict)
                        .map(|v| v.detail())
                        .collect();
                    return Err(TranslationError::new(
                        TranslationErrorCode::IntegrityFailure,
                        format!("术语约束未满足：{}", detail.join("；")),
                    ));
                }
            }
        }

        // 11. 组装结果 + 诊断
        let elapsed_ms = started.elapsed().as_millis() as u64;
        warnings.extend(outcome.warnings.iter().cloned());

        let mut result = TranslationResult {
            text: outcome.text.clone(),
            source: source.clone(),
            target: req.target.clone(),
            engine_id: outcome.engine_id.clone(),
            model_id: outcome.model_id.clone(),
            elapsed_ms,
            warnings,
        };
        // 输出层面的确定性修复（只做格式换位，不猜内容）。
        // 修复**不得**碰受保护内容：一旦修复会破坏它，就放弃修复。
        let repaired = quality::repair_output(&result.text);
        if plan
            .spans()
            .iter()
            .all(|span| repaired.contains(&span.original))
        {
            result.text = repaired;
        }

        let diagnostics = self.build_diagnostics(
            &route,
            options,
            &plan,
            &result,
            source_assumed,
            attempt,
            elapsed_ms,
            term_verification.as_ref(),
            Some(quality_report),
            outcome.usage,
            outcome.model_revision.clone(),
            outcome.prompt_version.clone(),
            self.rules_trace(&rule_applied),
        );

        // 12. 写缓存（只有通过校验的结果才会走到这里）
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
            cached.diagnostics.cache_hit = false;
            cached.diagnostics.elapsed_ms = 0;
            cache.insert(key, cached);
        }

        Ok(TranslateOutput {
            result,
            diagnostics,
        })
    }

    /// 应用确定性规则包。规则失败不影响主链（只记录未生效）。
    fn apply_rules(&self, text: &str, source: &LanguageTag, target: &LanguageTag) -> RuleOutcome {
        match &self.config.rules {
            Some(rules) => rules.apply(text, source, target),
            None => RuleOutcome::unchanged(text),
        }
    }

    fn rules_trace(&self, applied: &[String]) -> Option<RuleTrace> {
        self.config.rules.as_ref().map(|rules| RuleTrace {
            id: rules.id().to_string(),
            version: rules.version(),
            applied: applied.to_vec(),
        })
    }

    fn cache_get(&self, key: &CacheKey) -> Option<CacheEntry> {
        self.cache.lock().ok().and_then(|mut c| c.get(key).cloned())
    }

    fn cache_identity(
        &self,
        route: &RouteOutcome,
        options: &TranslateOptions,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> CacheIdentity {
        let engine_id = route.engine_id();
        let model = route.engine.model_identity();
        let prompt_version = model
            .as_ref()
            .map(|m| m.prompt_version.clone())
            .or_else(|| route.engine.cache_identity())
            .unwrap_or_else(|| "engine-default".to_string());

        let context_fingerprint = match (&options.context_id, &options.context_text) {
            (Some(id), _) => id.clone(),
            (None, Some(text)) => format!("{}:{}", text.chars().count(), hash8(text)),
            (None, None) => String::new(),
        };

        CacheIdentity::new(engine_id, options.mode)
            .with_languages(source.as_str(), target.as_str())
            .with_model(
                model.as_ref().map(|m| m.model_id.clone()),
                model.as_ref().and_then(|m| m.model_revision.clone()),
            )
            .with_prompt(prompt_version)
            .with_glossary(self.glossary_version())
            .with_protection(PROTECTION_VERSION)
            .with_kind(options.text_kind)
            .with_strategy(format!(
                "{}|rules:{}",
                strategy_label(options),
                self.config
                    .rules
                    .as_ref()
                    .map(|r| format!("{}@{}", r.id(), r.version()))
                    .unwrap_or_else(|| "none".to_string())
            ))
            .with_route(route.route)
            .with_context(context_fingerprint)
            .with_privacy_scope(self.config.privacy_scope.clone())
            .with_generation_config(
                model
                    .as_ref()
                    .map(|m| m.generation_config_hash.clone())
                    .unwrap_or_default(),
            )
    }

    fn glossary_version(&self) -> String {
        self.config
            .glossary
            .as_ref()
            .map(|g| format!("{}@{}", g.id(), g.version()))
            .unwrap_or_default()
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_template(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
        normalized: &str,
        source: &LanguageTag,
        hit: String,
        source_assumed: bool,
        started: Instant,
    ) -> Result<TranslateOutput, TranslationError> {
        self.counters.template_hits.fetch_add(1, Ordering::Relaxed);
        let elapsed_ms = started.elapsed().as_millis() as u64;

        // 模板结果同样要过完整性门禁（§8.4）：模板里可能带 `{filename}` 这类变量
        let plan = ProtectionPlan::prepare(normalized, &self.config.protection)?;
        let report = quality::check_with_protection(&plan, &hit, &req.target, options.mode);
        if report.has_fatal() && self.config.strict_integrity {
            return Err(TranslationError::new(
                TranslationErrorCode::IntegrityFailure,
                format!(
                    "模板译文未通过完整性检查（模板数据本身可能已过期）：{}",
                    report.fatal_summary()
                ),
            ));
        }

        let text = quality::repair_output(&hit);
        let mut result = TranslationResult {
            text,
            source: source.clone(),
            target: req.target.clone(),
            engine_id: format!(
                "template:{}",
                self.config
                    .templates
                    .as_ref()
                    .map(|t| t.id().to_string())
                    .unwrap_or_default()
            ),
            model_id: None,
            elapsed_ms,
            warnings: if source_assumed {
                vec![TranslationWarning::LanguageAssumed(
                    source.as_str().to_string(),
                )]
            } else {
                Vec::new()
            },
        };

        let identity = CacheIdentity::new(result.engine_id.clone(), options.mode)
            .with_languages(source.as_str(), req.target.as_str())
            .with_prompt(
                self.config
                    .templates
                    .as_ref()
                    .map(|t| format!("{}@{}", t.id(), t.version()))
                    .unwrap_or_default(),
            )
            .with_glossary(self.glossary_version())
            .with_protection(PROTECTION_VERSION)
            .with_kind(options.text_kind)
            .with_strategy("template")
            .with_route(TranslationRoute::Rule)
            .with_privacy_scope(self.config.privacy_scope.clone());
        let key = CacheKey::new(normalized, identity);

        let diagnostics = TranslationDiagnostics {
            engine_id: result.engine_id.clone(),
            route: TranslationRoute::Rule,
            mode: options.mode,
            text_kind: options.text_kind,
            cache_hit: false,
            model_invoked: false,
            source_assumed,
            rules_version: 0,
            resource_fingerprint: self.config.resource_fingerprint.clone(),
            route_reason: "整句模板命中".to_string(),
            protection_strategy: plan.strategy().as_str().to_string(),
            protected_spans: plan.spans().to_vec(),
            glossary: None,
            quality: Some(report),
            attempts: 1,
            elapsed_ms,
            prompt_version: String::new(),
            model_revision: None,
            usage: None,
            rules: self.config.templates.as_ref().map(|t| RuleTrace {
                id: t.id().to_string(),
                version: t.version(),
                applied: vec!["template.exact".to_string()],
            }),
        };

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                key,
                CacheEntry {
                    result: result.clone(),
                    diagnostics: diagnostics.clone(),
                },
            );
        }
        result.elapsed_ms = elapsed_ms;
        Ok(TranslateOutput {
            result,
            diagnostics,
        })
    }

    /// 调用后端执行一次翻译（不含质量门禁）。
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        &self,
        route: &RouteOutcome,
        plan: &ProtectionPlan,
        constraints: &[TermConstraint],
        req: &TranslateRequest,
        options: &TranslateOptions,
        source: &LanguageTag,
        deadline: Option<Instant>,
        cancel: Option<&CancelToken>,
        retry_hint: Option<&str>,
    ) -> Result<Executed, TranslationError> {
        self.counters.model_calls.fetch_add(1, Ordering::Relaxed);
        match plan.strategy() {
            ProtectionStrategy::Marker => {
                let placeholders = plan.marker_tokens();
                let job = TranslationJob {
                    text: plan.masked(),
                    original_text: plan.source(),
                    source,
                    target: &req.target,
                    mode: options.mode,
                    text_kind: options.text_kind,
                    glossary: constraints,
                    placeholders: &placeholders,
                    retry_hint,
                    deadline,
                    cancel,
                    max_output_tokens: None,
                };
                let output = route.engine.translate_job(&job).await?;
                let restored = plan.restore(&output.text)?;
                Ok(Executed {
                    text: restored,
                    engine_id: route.engine_id(),
                    model_id: output.model_id,
                    model_revision: output.model_revision,
                    prompt_version: route
                        .engine
                        .model_identity()
                        .map(|m| m.prompt_version)
                        .unwrap_or_default(),
                    usage: output.usage,
                    warnings: output.warnings,
                })
            }
            ProtectionStrategy::Segment => {
                // 分段：受保护片段不进模型，逐段翻译后按原顺序拼回。
                let segments = plan.plain_texts();
                let mut translated: Vec<String> = Vec::with_capacity(segments.len());
                let mut usage = TokenUsage::default();
                let mut used_usage = false;
                let mut model_id = None;
                let mut model_revision = None;
                let mut warnings = Vec::new();

                for segment in &segments {
                    self.check_cancel(cancel)?;
                    if segment.trim().is_empty() {
                        translated.push((*segment).to_string());
                        continue;
                    }
                    let job = TranslationJob {
                        text: segment,
                        original_text: segment,
                        source,
                        target: &req.target,
                        mode: options.mode,
                        text_kind: options.text_kind,
                        glossary: constraints,
                        placeholders: &[],
                        retry_hint,
                        deadline,
                        cancel,
                        max_output_tokens: None,
                    };
                    let output = route.engine.translate_job(&job).await?;
                    if output.text.trim().is_empty() {
                        // 分段策略下"模型返回空串"意味着这一段彻底没翻译。
                        // 拼回去仍然会包含受保护内容，看起来像成功——必须拦住。
                        return Err(TranslationError::new(
                            TranslationErrorCode::InvalidBackendResponse,
                            "模型对非空片段返回了空译文",
                        ));
                    }
                    if let Some(u) = output.usage {
                        usage.input_tokens += u.input_tokens;
                        usage.output_tokens += u.output_tokens;
                        used_usage = true;
                    }
                    model_id = output.model_id.or(model_id);
                    model_revision = output.model_revision.or(model_revision);
                    warnings.extend(output.warnings);
                    translated.push(output.text);
                }

                let assembled = plan.assemble(&translated)?;
                Ok(Executed {
                    text: assembled,
                    engine_id: route.engine_id(),
                    model_id,
                    model_revision,
                    prompt_version: route
                        .engine
                        .model_identity()
                        .map(|m| m.prompt_version)
                        .unwrap_or_default(),
                    usage: used_usage.then_some(usage),
                    warnings,
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_diagnostics(
        &self,
        route: &RouteOutcome,
        options: &TranslateOptions,
        plan: &ProtectionPlan,
        result: &TranslationResult,
        source_assumed: bool,
        attempt: u32,
        elapsed_ms: u64,
        terms: Option<&crate::terms::TermVerification>,
        quality: Option<QualityReport>,
        usage: Option<TokenUsage>,
        model_revision: Option<String>,
        prompt_version: String,
        rules: Option<RuleTrace>,
    ) -> TranslationDiagnostics {
        let glossary = terms.map(|v| TermTrace {
            id: self
                .config
                .glossary
                .as_ref()
                .map(|g| g.id().to_string())
                .unwrap_or_default(),
            version: self
                .config
                .glossary
                .as_ref()
                .map(|g| g.version())
                .unwrap_or(0),
            policy: self.config.term_policy,
            hits: v.hits.len(),
            satisfied: v.satisfied_count(),
            violations: v.violations.clone(),
        });

        TranslationDiagnostics {
            engine_id: result.engine_id.clone(),
            route: route.route,
            mode: options.mode,
            text_kind: options.text_kind,
            cache_hit: false,
            model_invoked: true,
            source_assumed,
            rules_version: 0,
            resource_fingerprint: self.config.resource_fingerprint.clone(),
            route_reason: route.reason.clone(),
            protection_strategy: plan.strategy().as_str().to_string(),
            protected_spans: plan.spans().to_vec(),
            glossary,
            quality,
            attempts: attempt,
            elapsed_ms,
            prompt_version,
            model_revision,
            usage,
            rules,
        }
    }
}

/// 执行结果（内部）。
struct Executed {
    text: String,
    engine_id: String,
    model_id: Option<String>,
    model_revision: Option<String>,
    prompt_version: String,
    usage: Option<TokenUsage>,
    warnings: Vec<TranslationWarning>,
}

fn strategy_label(options: &TranslateOptions) -> String {
    match &options.strategy {
        Some(strategy) => strategy.clone(),
        None => match options.paragraph_strategy {
            ParagraphStrategy::Sentence => "sentence".to_string(),
            ParagraphStrategy::Group => "group".to_string(),
            _ => "single".to_string(),
        },
    }
}

fn hash8(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}
