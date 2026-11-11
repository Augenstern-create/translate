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
    RemoteFallbackPolicy, RequestBudget, RouteDecision, RouteStats, TermConstraint, TokenUsage,
    TranslateOptions, TranslateRequest, TranslationError, TranslationErrorCode, TranslationMode,
    TranslationResult, TranslationRoute, TranslationWarning,
};

use crate::cache::{CacheEntry, CacheIdentity, CacheKey, CacheStats, QueryCache};
use crate::diagnostics::{RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics};
use crate::language::{HeuristicIdentifier, LanguageIdentifier, MorphologyAnalyzer};
use crate::protection::{ProtectionPlan, ProtectionPolicy, ProtectionStrategy, PROTECTION_VERSION};
use crate::quality::{self, QualityReport};
use crate::routing::{BackendRouter, RouteOutcome, RoutingConfig};
use crate::rules::{RuleOutcome, TextRules};
use crate::templates::{SentenceTemplates, TemplateScope};
use crate::terms::{Glossary, TermPolicy, TermVerification};
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
    ///
    /// 注意：这只限制"翻译尝试"，真正的费用上限是
    /// [`Self::max_http_calls_per_request`]。
    pub content_retry: u32,
    /// 一次逻辑请求允许发出的**物理 HTTP 调用**总数（默认 4）。
    ///
    /// 分段翻译、内容重试与传输层重试共享这一额度；超出即 `BudgetExceeded`。
    pub max_http_calls_per_request: u32,
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
            .field(
                "max_http_calls_per_request",
                &self.max_http_calls_per_request,
            )
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
            max_http_calls_per_request: 4,
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
    ) -> Option<TermVerification> {
        let glossary = self.config.glossary.as_ref()?;
        // 没有译文时只能做"源文命中"预览，满足情况一律记为未满足
        Some(glossary.verify(
            text,
            "",
            source.as_str(),
            target.as_str(),
            self.config.domain.as_deref(),
        ))
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
        // 预算按**逻辑请求**创建一次：分段与重试共享同一份额度
        let budget = Arc::new(RequestBudget::new(
            self.config.max_http_calls_per_request,
            deadline,
        ));
        let max_attempts = 1 + self.config.content_retry;
        let mut attempt: u32 = 0;
        let mut retry_hint: Option<String> = None;
        let mut warnings: Vec<TranslationWarning> = rule_warnings;
        if source_assumed {
            warnings.push(TranslationWarning::LanguageAssumed(
                source.as_str().to_string(),
            ));
        }

        let (outcome, finalized) = loop {
            attempt += 1;
            self.check_cancel(cancel)?;
            if attempt > 1 {
                self.counters.retries.fetch_add(1, Ordering::Relaxed);
                route.engine.record_content_retry();
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
                    &budget,
                )
                .await;

            match executed {
                Ok(executed) => {
                    // 终检：格式修复 + 完整性 + 术语，全部针对**最终文本**
                    match self.finalize_translation(
                        &work_text,
                        &executed.text,
                        &plan,
                        &source,
                        &req.target,
                        options.mode,
                    ) {
                        Ok(finalized) => break (executed, finalized),
                        Err(err) => {
                            if attempt >= max_attempts {
                                return Err(TranslationError::new(
                                    err.code,
                                    format!("尝试 {attempt} 次仍未通过：{}", err.message),
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
                                retry_hint = Some(format!(
                                    "上一次输出未通过校验（{}）。请逐字保留所有数字、编号、占位符与术语。",
                                    err.message
                                ));
                            }
                            continue;
                        }
                    }
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

        // 10. 组装结果 + 诊断（术语与完整性已在终检里对最终文本完成）
        let elapsed_ms = started.elapsed().as_millis() as u64;
        warnings.extend(outcome.warnings.iter().cloned());
        warnings.extend(finalized.warnings.iter().cloned());

        let result = TranslationResult {
            text: finalized.text.clone(),
            source: source.clone(),
            target: req.target.clone(),
            engine_id: outcome.engine_id.clone(),
            model_id: outcome.model_id.clone(),
            elapsed_ms,
            warnings,
        };

        let diagnostics = self.build_diagnostics(
            &route,
            options,
            &plan,
            &result,
            source_assumed,
            attempt,
            elapsed_ms,
            finalized.terms.as_ref(),
            Some(finalized.quality.clone()),
            outcome.usage,
            outcome.model_revision.clone(),
            outcome.prompt_version.clone(),
            self.rules_trace(&rule_applied),
            finalized.repair,
            budget.calls_started(),
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

    /// 统一终检：**先做安全格式修复，再对最终文本做完整校验**。
    ///
    /// 修复策略（S1-01）：
    ///
    /// 1. 先算修复后的文本；
    /// 2. 修复改变了文本时，用最终校验判定它是否可接受；
    /// 3. 若修复后不通过、但**原候选**通过，则退回原候选（不能因为"格式优化"
    ///    让一份本来合格的译文不可用）；
    /// 4. 两者都不通过 → 返回 `IntegrityFailure`，由调用方进入有界重试。
    ///
    /// 模板路径与模型路径**都必须**走这里，避免维护两套终检逻辑。
    fn finalize_translation(
        &self,
        source_text: &str,
        candidate: &str,
        plan: &ProtectionPlan,
        source: &LanguageTag,
        target: &LanguageTag,
        mode: TranslationMode,
    ) -> Result<FinalizedTranslation, TranslationError> {
        let proposed = quality::repair_output(candidate);

        // 修复没有改变文本：直接校验候选。
        if proposed == candidate {
            let (quality, terms, warnings) =
                self.validate_final(source_text, candidate, plan, source, target, mode)?;
            return Ok(FinalizedTranslation {
                text: candidate.to_string(),
                quality,
                terms,
                warnings,
                repair: RepairOutcome::Unchanged,
            });
        }

        // 修复会改动确定性数字（编号/版本/日期/百分比）：一律不采用。
        if !self.repair_is_safe(candidate, &proposed) {
            let (quality, terms, warnings) =
                self.validate_final(source_text, candidate, plan, source, target, mode)?;
            return Ok(FinalizedTranslation {
                text: candidate.to_string(),
                quality,
                terms,
                warnings,
                repair: RepairOutcome::Rejected,
            });
        }

        // 修复改变了文本但数字没变：只有在它能通过终检时才采用。
        match self.validate_final(source_text, &proposed, plan, source, target, mode) {
            Ok((quality, terms, warnings)) => Ok(FinalizedTranslation {
                text: proposed,
                quality,
                terms,
                warnings,
                repair: RepairOutcome::Applied,
            }),
            Err(_repaired_error) => {
                // 修复把译文弄坏了（例如受保护内容被拆开）：退回原候选，
                // 但原候选必须自己通过终检——不能拿"修复失败"当免检理由。
                let (quality, terms, warnings) =
                    self.validate_final(source_text, candidate, plan, source, target, mode)?;
                Ok(FinalizedTranslation {
                    text: candidate.to_string(),
                    quality,
                    terms,
                    warnings,
                    repair: RepairOutcome::Rejected,
                })
            }
        }
    }

    /// 修复前后"确定性数字"是否完全一致。
    ///
    /// 只比较 `is_strict()` 的 token（编号、版本号、日期、百分比）：
    /// 普通数量允许中英互转，格式修复也不应该动它们。
    fn repair_is_safe(&self, before: &str, after: &str) -> bool {
        let strict = |text: &str| {
            let mut tokens: Vec<String> = quality::number::tokenize(text)
                .into_iter()
                .filter(|t| t.kind.is_strict())
                .map(|t| t.raw)
                .collect();
            tokens.sort();
            tokens
        };
        strict(before) == strict(after)
    }

    /// 对**最终文本**跑完整性 + 术语校验。
    fn validate_final(
        &self,
        source_text: &str,
        text: &str,
        plan: &ProtectionPlan,
        source: &LanguageTag,
        target: &LanguageTag,
        mode: TranslationMode,
    ) -> Result<
        (
            QualityReport,
            Option<crate::terms::TermVerification>,
            Vec<TranslationWarning>,
        ),
        TranslationError,
    > {
        let quality = quality::check_with_protection(plan, text, target, mode);
        if self.config.strict_integrity && quality.has_fatal() {
            return Err(TranslationError::new(
                TranslationErrorCode::IntegrityFailure,
                format!("译文未通过完整性检查：{}", quality.fatal_summary()),
            ));
        }

        let mut warnings = Vec::new();
        let terms = self.config.glossary.as_ref().map(|glossary| {
            glossary.verify(
                source_text,
                text,
                source.as_str(),
                target.as_str(),
                self.config.domain.as_deref(),
            )
        });
        if let Some(verification) = &terms {
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

        Ok((quality, terms, warnings))
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

        // 模板结果走**同一套**终检（S1-01/§8.4）：模板里可能带 `{filename}` 这类变量，
        // 也可能与术语表冲突——"本地确定性数据"不是免检理由。
        let plan = ProtectionPlan::prepare(normalized, &self.config.protection)?;
        let finalized = self
            .finalize_translation(normalized, &hit, &plan, source, &req.target, options.mode)
            .map_err(|err| {
                TranslationError::new(
                    err.code,
                    format!("模板译文未通过终检（模板数据可能已过期）：{}", err.message),
                )
            })?;

        let mut result = TranslationResult {
            text: finalized.text.clone(),
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
            http_calls: 0,
            repair: finalized.repair.as_str().to_string(),
            protected_spans: plan.spans().to_vec(),
            glossary: self.term_trace(finalized.terms.as_ref()),
            quality: Some(finalized.quality.clone()),
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
        budget: &Arc<RequestBudget>,
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
                    budget: Some(Arc::clone(budget)),
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
                        budget: Some(Arc::clone(budget)),
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

    /// 术语诊断轨迹（模型路径与模板路径共用）。
    fn term_trace(&self, terms: Option<&crate::terms::TermVerification>) -> Option<TermTrace> {
        let verification = terms?;
        let glossary = self.config.glossary.as_ref()?;
        Some(TermTrace {
            id: glossary.id().to_string(),
            version: glossary.version(),
            policy: self.config.term_policy,
            hits: verification.hits.len(),
            satisfied: verification.satisfied_count(),
            violations: verification.violations.clone(),
        })
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
        repair: RepairOutcome,
        http_calls: u32,
    ) -> TranslationDiagnostics {
        let glossary = self.term_trace(terms);

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
            http_calls,
            repair: repair.as_str().to_string(),
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

/// 格式修复的处置结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairOutcome {
    /// 修复没有改变文本。
    Unchanged,
    /// 修复后的文本通过了终检，采用之。
    Applied,
    /// 修复后的文本未通过终检，退回原候选。
    Rejected,
}

impl RepairOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
        }
    }
}

/// 通过终检的最终译文。
///
/// 关键不变量：`text` 就是被 `quality` / `terms` 判定过的**同一份文本**。
/// 任何会改变译文的操作（格式修复、术语处理）都必须发生在生成它之前。
#[derive(Debug)]
struct FinalizedTranslation {
    text: String,
    quality: QualityReport,
    terms: Option<crate::terms::TermVerification>,
    warnings: Vec<TranslationWarning>,
    repair: RepairOutcome,
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

#[cfg(test)]
mod finalize_tests {
    //! S1-01：统一终检的白盒测试。
    //!
    //! 这些用例直接对 `finalize_translation` 断言，因为"修复是否被采用/退回"
    //! 在端到端层面不容易构造出自然的中文输入；机制本身必须被单独固定下来。

    use super::*;
    use translator_plugin_api::mocks::MockNmtEngine;

    fn coordinator() -> Coordinator {
        let mut registry = PluginRegistry::new();
        registry
            .register_engine(MockNmtEngine::new())
            .expect("注册 mock 引擎失败");
        Coordinator::new(Arc::new(registry), CoordinatorConfig::default())
    }

    fn plan(text: &str) -> ProtectionPlan {
        ProtectionPlan::prepare(text, &ProtectionPolicy::default()).expect("保护计划")
    }

    fn zh() -> (LanguageTag, LanguageTag) {
        (
            LanguageTag::new("en").unwrap(),
            LanguageTag::new("zh-CN").unwrap(),
        )
    }

    #[test]
    fn 修复不破坏内容时会被采用() {
        let c = coordinator();
        let (en, zh) = zh();
        let source = "The temperature is above 20 degrees.";
        let p = plan(source);
        assert!(p.is_trivial(), "该源文不应有受保护片段");

        let finalized = c
            .finalize_translation(
                source,
                "温度在以上20度。",
                &p,
                &en,
                &zh,
                TranslationMode::Precise,
            )
            .expect("应通过终检");
        assert_eq!(finalized.repair, RepairOutcome::Applied);
        assert_eq!(finalized.text, "温度在20度以上。");
    }

    #[test]
    fn 修复破坏受保护版本号时退回原候选() {
        let c = coordinator();
        let (en, zh) = zh();
        let source = "Version 2.1.3 or above is supported.";
        let p = plan(source);
        assert_eq!(p.spans().len(), 1, "2.1.3 应被保护");

        // 这个候选一旦被"格式修复"就会把 2.1.3 拆成 2.1以上.3
        let candidate = "版本 以上2.1.3";
        assert_ne!(
            quality::repair_output(candidate),
            candidate,
            "该候选确实会被修复改动"
        );

        let finalized = c
            .finalize_translation(source, candidate, &p, &en, &zh, TranslationMode::Precise)
            .expect("退回原候选后应通过");
        assert_eq!(finalized.repair, RepairOutcome::Rejected);
        assert_eq!(
            finalized.text, candidate,
            "必须退回原候选而不是被改坏的文本"
        );
        assert!(finalized.text.contains("2.1.3"));
    }

    #[test]
    fn 修复改动关键数字时不采用() {
        let c = coordinator();
        let (en, zh) = zh();
        // 版本号在候选里出现两次：一次被保护，一次是模型自己写的。
        // 修复会把模型那份 "2.1.3" 拆成 "2.1以上.3" —— 必须拒绝采用。
        let source = "Version 2.1.3 or above is supported.";
        let p = plan(source);
        let candidate = "版本 以上2.1.3（2.1.3）";
        assert_ne!(quality::repair_output(candidate), candidate);

        let finalized = c
            .finalize_translation(source, candidate, &p, &en, &zh, TranslationMode::Precise)
            .expect("候选本身应通过");
        assert_eq!(finalized.repair, RepairOutcome::Rejected);
        assert_eq!(finalized.text, candidate);
    }

    #[test]
    fn 候选与修复后都不通过时返回完整性错误() {
        let c = coordinator();
        let (en, zh) = zh();
        let source = "Order #10248 has shipped.";
        let p = plan(source);

        let err = c
            .finalize_translation(
                source,
                "订单 #99999 已发货。",
                &p,
                &en,
                &zh,
                TranslationMode::Precise,
            )
            .expect_err("编号被改必须拒绝");
        assert_eq!(err.code, TranslationErrorCode::IntegrityFailure);
    }

    #[test]
    fn 修复后退回候选时仍然校验候选本身() {
        let c = coordinator();
        let (en, zh) = zh();
        // 候选既会被修复改动，又缺少受保护内容 → 两条路都不通
        let source = "Open https://example.com and {file}.";
        let p = plan(source);
        let err = c
            .finalize_translation(
                source,
                "打开 以上2.1.3",
                &p,
                &en,
                &zh,
                TranslationMode::Precise,
            )
            .expect_err("缺少受保护内容必须拒绝");
        assert_eq!(err.code, TranslationErrorCode::IntegrityFailure);
    }

    #[test]
    fn 严格术语在最终文本上重新核验() {
        let glossary = Arc::new(
            Glossary::from_json(
                r#"{"schema_version":1,"id":"g","version":1,"entries":[
                    {"id":"t1","source":"large language model","target":"大语言模型",
                     "strict":true}]}"#,
            )
            .unwrap(),
        );
        let config = CoordinatorConfig {
            glossary: Some(glossary),
            term_policy: TermPolicy::Enforce,
            ..CoordinatorConfig::default()
        };
        let mut registry = PluginRegistry::new();
        registry.register_engine(MockNmtEngine::new()).unwrap();
        let c = Coordinator::new(Arc::new(registry), config);

        let (en, zh) = zh();
        let source = "A large language model is useful.";
        let p = plan(source);

        let ok = c
            .finalize_translation(
                source,
                "大语言模型很有用。",
                &p,
                &en,
                &zh,
                TranslationMode::Precise,
            )
            .expect("满足严格术语应通过");
        assert!(ok.terms.as_ref().is_some_and(|t| t.strict_satisfied()));

        let err = c
            .finalize_translation(
                source,
                "大型语言模型很有用。",
                &p,
                &en,
                &zh,
                TranslationMode::Precise,
            )
            .expect_err("未满足严格术语必须失败");
        assert_eq!(err.code, TranslationErrorCode::IntegrityFailure);
        assert!(err.message.contains("术语"), "{}", err.message);
    }
}
