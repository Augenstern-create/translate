//! 协调器：串起校验 → 规范化 → 语言识别 → 占位符保护 → 路由 → 执行 → 还原 → 术语约束 → 缓存。
//!
//! 对应设计文档 §6 的流水线。Coordinator 不认识任何具体引擎，
//! 只依赖 `translator-plugin-api` 的 trait，因此替换后端不需要改这里。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use translator_plugin_api::{BoxFuture, PluginRegistry};
use translator_types::{
    CancelToken, Capability, CapabilityInfo, DictionaryEntry, LookupRequest, RouteDecision,
    TextKind, TranslateOptions, TranslateRequest, TranslationError, TranslationErrorCode,
    TranslationMode, TranslationPolicy, TranslationResult, TranslationWarning,
};

use crate::cache::{CacheEntry, CacheKey, QueryCache};
use crate::diagnostics::{RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics};
use crate::quality;
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
    /// 可选 LID 实现。`None` 时保持原有 Unicode 启发式，默认构建不引入模型。
    pub language_identifier: Option<Arc<dyn translator_analysis::LanguageIdentifier>>,
    /// 可选形态分析器；只用于**查词未命中后的候选**，不改写翻译输入。
    pub lemmatizer: Option<Arc<dyn translator_analysis::MorphologyAnalyzer>>,
    /// 完整句子模板（P16-R / R2）。只有整句精确匹配才直接返回。
    pub templates: Option<Arc<dyn crate::templates::SentenceTemplates>>,
    /// 默认翻译模式（docs/57 §4）。默认 [`TranslationMode::Precise`]。
    pub default_mode: TranslationMode,
    /// 关键完整性检查失败时是否拒绝返回译文。
    ///
    /// 默认 `true`：关键数字丢失、受保护标记残留、空译文都返回 `IntegrityFailure`，
    /// 而不是把一份缺内容的译文交给宿主。设 `false` 时这些问题只进诊断。
    pub strict_integrity: bool,
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
            .field(
                "language_identifier",
                &self.language_identifier.as_ref().map(|x| x.name()),
            )
            .field("lemmatizer", &self.lemmatizer.as_ref().map(|x| x.name()))
            .field("templates", &self.templates.as_ref().map(|x| x.id()))
            .field("default_mode", &self.default_mode)
            .field("strict_integrity", &self.strict_integrity)
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
            language_identifier: None,
            lemmatizer: None,
            templates: None,
            default_mode: TranslationMode::Precise,
            strict_integrity: true,
        }
    }
}

#[derive(Default)]
struct RouteCounters {
    executions: AtomicU64,
    dictionary_hits: AtomicU64,
    rule_matches: AtomicU64,
    nmt_calls: AtomicU64,
    nmt_fallbacks: AtomicU64,
    cache_hits: AtomicU64,
    best_candidates: AtomicU64,
    failures: AtomicU64,
}

pub struct Coordinator {
    registry: Arc<PluginRegistry>,
    cache: Mutex<QueryCache>,
    config: CoordinatorConfig,
    /// 实际参与缓存键的规则版本（规则包自报版本优先）。
    effective_rules_version: u32,
    /// 实际参与缓存键的资源指纹（挂载的词典/规则包/术语表 id@version）。
    resource_fingerprint: String,
    stats: RouteCounters,
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
            stats: RouteCounters::default(),
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

    /// 默认翻译模式。
    pub fn default_mode(&self) -> TranslationMode {
        self.config.default_mode
    }

    /// 路由/模型调用统计快照（P16-R / R5）。
    pub fn route_stats(&self) -> translator_types::RouteStats {
        translator_types::RouteStats {
            executions: self.stats.executions.load(Ordering::Relaxed),
            dictionary_hits: self.stats.dictionary_hits.load(Ordering::Relaxed),
            rule_matches: self.stats.rule_matches.load(Ordering::Relaxed),
            nmt_calls: self.stats.nmt_calls.load(Ordering::Relaxed),
            nmt_fallbacks: self.stats.nmt_fallbacks.load(Ordering::Relaxed),
            cache_hits: self.stats.cache_hits.load(Ordering::Relaxed),
            best_candidates: self.stats.best_candidates.load(Ordering::Relaxed),
            failures: self.stats.failures.load(Ordering::Relaxed),
        }
    }

    pub fn reset_route_stats(&self) {
        for c in [
            &self.stats.executions,
            &self.stats.dictionary_hits,
            &self.stats.rule_matches,
            &self.stats.nmt_calls,
            &self.stats.nmt_fallbacks,
            &self.stats.cache_hits,
            &self.stats.best_candidates,
            &self.stats.failures,
        ] {
            c.store(0, Ordering::Relaxed);
        }
    }

    pub fn record_dictionary_hit(&self) {
        self.stats.dictionary_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_execution(&self) {
        self.stats.executions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_best_candidates(&self, n: u64) {
        self.stats.best_candidates.fetch_add(n, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.stats.failures.fetch_add(1, Ordering::Relaxed);
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
        let outcome = routing::select(&self.registry, req, &source, &work_text, false)?;
        Ok(outcome.decision)
    }

    /// 默认执行选项（模式取配置，文本类型 Auto、单路径）。
    pub fn default_options(&self) -> TranslateOptions {
        TranslateOptions::new(self.default_mode())
    }

    /// 仅返回译文（对外主路径，使用默认模式）。
    pub fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            self.run(req, &self.default_options())
                .await
                .map(|out| out.result)
        })
    }

    /// 指定模式翻译（单请求覆盖 Builder 默认值）。
    pub fn translate_with_mode<'a>(
        &'a self,
        req: &'a TranslateRequest,
        mode: TranslationMode,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        let options = TranslateOptions::new(mode);
        Box::pin(async move { self.run(req, &options).await.map(|out| out.result) })
    }

    /// 指定执行选项（模式 + 文本类型 + 策略 + 上下文）。
    pub fn translate_with_options<'a>(
        &'a self,
        req: &'a TranslateRequest,
        options: &'a TranslateOptions,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move { self.run(req, options).await.map(|out| out.result) })
    }

    /// 可取消的译文入口（默认模式）。
    ///
    /// 取消在阶段边界检查；引擎若能中断（Bergamot worker）会在执行中返回
    /// `Cancelled`，否则最迟在本次引擎调用返回后生效。取消不写缓存。
    pub fn translate_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        let options = self.default_options();
        Box::pin(async move {
            self.run_inner(req, Some(cancel), &options)
                .await
                .map(|out| out.result)
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
            self.run_inner(req, Some(cancel), &options)
                .await
                .map(|out| out.result)
        })
    }

    /// 返回译文 + 结构化诊断（P2.4，默认模式）。
    ///
    /// 与 [`Self::translate`] 走**同一条**实现路径，只是多带出一份诊断，
    /// 因此两者不可能给出不同译文。
    pub fn translate_with_diagnostics<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        let options = self.default_options();
        Box::pin(async move { self.run(req, &options).await })
    }

    /// 指定模式的诊断入口。
    pub fn translate_with_diagnostics_mode<'a>(
        &'a self,
        req: &'a TranslateRequest,
        mode: TranslationMode,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        let options = TranslateOptions::new(mode);
        Box::pin(async move { self.run(req, &options).await })
    }

    /// 指定执行选项的诊断入口。
    pub fn translate_with_diagnostics_options<'a>(
        &'a self,
        req: &'a TranslateRequest,
        options: &'a TranslateOptions,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        Box::pin(async move { self.run(req, options).await })
    }

    /// 带诊断的可取消入口（与 [`Self::translate_cancellable`] 同一实现）。
    pub fn translate_with_diagnostics_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        let options = self.default_options();
        Box::pin(async move { self.run_inner(req, Some(cancel), &options).await })
    }

    /// 带诊断的可取消入口 + 指定模式。
    pub fn translate_with_diagnostics_cancellable_mode<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
        mode: TranslationMode,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        let options = TranslateOptions::new(mode);
        Box::pin(async move { self.run_inner(req, Some(cancel), &options).await })
    }

    /// 带诊断的可取消入口 + 完整选项。
    pub fn translate_with_diagnostics_cancellable_options<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
        options: &'a TranslateOptions,
    ) -> BoxFuture<'a, Result<TranslateOutput, TranslationError>> {
        Box::pin(async move { self.run_inner(req, Some(cancel), options).await })
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

            let mut all = self.lookup_term(req, &req.term).await?;
            if all.is_empty() {
                if let Some(lemmatizer) = &self.config.lemmatizer {
                    for lemma in lemmatizer.lemmas(&req.term).into_iter().take(3) {
                        let mut found = self.lookup_term(req, &lemma).await?;
                        for entry in found.drain(..) {
                            if !all.iter().any(|e| {
                                e.headword == entry.headword && e.source_id == entry.source_id
                            }) {
                                all.push(entry);
                                if all.len() >= req.limit {
                                    break;
                                }
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

    /// 注入的 LID（若配置）；返回 `None` 表示使用调用方原有启发式。
    pub fn identify_language(
        &self,
        text: &str,
        limit: usize,
    ) -> Option<Vec<translator_types::LanguageGuess>> {
        self.config
            .language_identifier
            .as_ref()
            .map(|id| id.identify(text, limit))
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

        let guessed = self
            .config
            .language_identifier
            .as_ref()
            .and_then(|id| id.identify(normalized, 1).into_iter().next())
            .map(|g| g.language)
            .or_else(|| text::guess_language(normalized));
        guessed.ok_or_else(|| {
            TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "无法自动识别源语言，请显式指定 source",
            )
        })
    }

    async fn run(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
    ) -> Result<TranslateOutput, TranslationError> {
        self.run_inner(req, None, options).await
    }

    async fn run_inner(
        &self,
        req: &TranslateRequest,
        cancel: Option<&CancelToken>,
        options: &TranslateOptions,
    ) -> Result<TranslateOutput, TranslationError> {
        self.stats.executions.fetch_add(1, Ordering::Relaxed);
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

        // 3.5) 完整句子模板（P16-R / R2）：只有整句精确匹配才直接返回。
        if options.text_kind == TextKind::Sentence {
            if let Some(templates) = &self.config.templates {
                if let Some(target_text) = templates.match_exact(&work_text, &source, &req.target) {
                    return self.finish_template(
                        req,
                        options,
                        source,
                        &work_text,
                        &target_text,
                        &rule_applied,
                        source_assumed,
                    );
                }
            }
        }

        check_cancelled(cancel)?;
        // 4) 路由（用的是规则处理后的文本）。先选引擎，才知道该由谁做保护：
        //    真实 NMT 后端自己按 `ProtectionPlan` 分段，协调器再打一次标记会破坏它。
        let outcome = routing::select(
            &self.registry,
            req,
            &source,
            &work_text,
            options.text_kind == TextKind::Word,
        )?;

        // 5) 占位符保护。结构化后端拿原文；通用后端拿标记化文本。
        let spans = text::protected_spans(&work_text);
        let (masked, protected) = if outcome.engine.handles_protected_spans() {
            (None, work_text.clone())
        } else {
            let p = text::protect(&work_text);
            (Some(p.clone()), p.text)
        };

        let rules_trace = self.config.rules.as_ref().map(|rules| RuleTrace {
            id: rules.id().to_string(),
            version: rules.version(),
            applied: rule_applied,
        });

        // 6) 缓存查询（模式必须进键：Precise 与 Fast 不得互相复用）
        let mut key = CacheKey::new_with_options(
            &protected,
            req,
            source.as_str(),
            &outcome.decision.engine_id,
            outcome.engine.cache_identity().as_deref(),
            self.effective_rules_version,
            &self.resource_fingerprint,
            options,
            options.strategy.as_deref().unwrap_or("single"),
            if outcome.decision.capability == Capability::Dictionary {
                translator_types::TranslationRoute::Dictionary
            } else {
                translator_types::TranslationRoute::LocalNmt
            },
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
                self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(TranslateOutput {
                    result: entry.result,
                    diagnostics: entry.diagnostics,
                });
            }
        }

        // 7) 交给引擎的请求：文本已保护、源语言已确定
        let resolved = TranslateRequest {
            text: protected.clone(),
            source: Some(source.clone()),
            ..req.clone()
        };

        check_cancelled(cancel)?;
        let started = Instant::now();
        let mut actual_engine_id = outcome.decision.engine_id.clone();
        let mut actual_route = if outcome.decision.capability == Capability::Dictionary {
            translator_types::TranslationRoute::Dictionary
        } else {
            translator_types::TranslationRoute::LocalNmt
        };
        let mut actual_reason = outcome.decision.reason.clone();
        let mut dictionary_hit = actual_route == translator_types::TranslationRoute::Dictionary;
        let mut nmt_invoked = actual_route == translator_types::TranslationRoute::LocalNmt;
        if nmt_invoked {
            self.stats.nmt_calls.fetch_add(1, Ordering::Relaxed);
        }

        let mut result = match outcome
            .engine
            .translate_cancellable(&resolved, cancel.unwrap_or_else(|| never_cancelled()))
            .await
        {
            Ok(result) => result,
            Err(err) if self.should_fallback(req, outcome.decision.capability, &err) => {
                dictionary_hit = false;
                match routing::fallback_to_nmt(&self.registry, &source, &req.target) {
                    Some(fallback) => {
                        self.stats.nmt_calls.fetch_add(1, Ordering::Relaxed);
                        self.stats.nmt_fallbacks.fetch_add(1, Ordering::Relaxed);
                        nmt_invoked = true;
                        actual_engine_id = fallback.decision.engine_id.clone();
                        actual_route = translator_types::TranslationRoute::LocalNmt;
                        actual_reason = fallback.decision.reason.clone();
                        // 回退后换执行身份，缓存键必须按实际引擎重算。
                        let fallback_identity = fallback.engine.cache_identity();
                        key = CacheKey::new_with_options(
                            &protected,
                            req,
                            source.as_str(),
                            &actual_engine_id,
                            fallback_identity.as_deref(),
                            self.effective_rules_version,
                            &self.resource_fingerprint,
                            options,
                            options.strategy.as_deref().unwrap_or("single"),
                            actual_route,
                        );
                        fallback
                            .engine
                            .translate_cancellable(
                                &resolved,
                                cancel.unwrap_or_else(|| never_cancelled()),
                            )
                            .await?
                    }
                    None => {
                        return Err(routing::not_installed(
                            Capability::Nmt,
                            &source,
                            &req.target,
                        ))
                    }
                }
            }
            Err(err) => return Err(err),
        };
        check_cancelled(cancel)?;
        if dictionary_hit {
            self.stats.dictionary_hits.fetch_add(1, Ordering::Relaxed);
        }

        // 8) 占位符还原；结构化后端已经还原过，不能再走一遍标记替换。
        let (restored, missing) = match &masked {
            Some(protected) => text::restore(protected, &result.text),
            None => (result.text.clone(), Vec::new()),
        };
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
        // 9.2) 受控格式修复：数字/范围词换位、重复标点；不改写语义。
        result.text = quality::repair_output(&result.text);

        // 9.5) 译后完整性检查（T2）。Fast 只查数字/占位符/空输出；Precise 再加
        //      否定、重复、目标语言、原文原样返回等启发式。
        let quality = quality::check(&work_text, &result.text, &req.target, options.mode);
        if self.config.strict_integrity && quality.has_fatal() {
            return Err(TranslationError::new(
                TranslationErrorCode::IntegrityFailure,
                format!("译后完整性检查未通过：{}", quality.summary()),
            ));
        }

        // 10) 补全结果元信息
        result.source = source;
        result.target = req.target.clone();
        result.elapsed_ms = started.elapsed().as_millis() as u64;

        let diagnostics = TranslationDiagnostics {
            engine_id: actual_engine_id.clone(),
            capability: outcome.decision.capability,
            route_reason: actual_reason.clone(),
            cache_hit: false,
            source_assumed,
            rules_version: self.effective_rules_version,
            resource_fingerprint: self.resource_fingerprint.clone(),
            mode: options.mode,
            text_kind: options.text_kind,
            route: actual_route,
            model_invoked: nmt_invoked,
            dictionary_hit,
            rule_matched: false,
            strategy: options
                .strategy
                .clone()
                .unwrap_or_else(|| "single".to_string()),
            context_id: options.context_id.clone(),
            quality: Some(quality),
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

    /// 模板完整匹配的收尾：术语、质量门禁、缓存、诊断。
    #[allow(clippy::too_many_arguments)]
    fn finish_template(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
        source: translator_types::LanguageTag,
        work_text: &str,
        target_text: &str,
        rule_applied: &[String],
        source_assumed: bool,
    ) -> Result<TranslateOutput, TranslationError> {
        let templates = self
            .config
            .templates
            .as_ref()
            .expect("finish_template 只在 templates 存在时调用");
        let engine_id = format!("template:{}", templates.id());
        let engine_identity = format!("{}@{}", templates.id(), templates.version());
        let spans = text::protected_spans(work_text);
        let key = CacheKey::new_with_options(
            work_text,
            req,
            source.as_str(),
            &engine_id,
            Some(&engine_identity),
            self.effective_rules_version,
            &self.resource_fingerprint,
            options,
            options.strategy.as_deref().unwrap_or("template"),
            translator_types::TranslationRoute::Rule,
        );
        if let Ok(cache) = self.cache.lock() {
            if let Some(hit) = cache.get(&key) {
                let mut entry = hit.clone();
                entry.result.warnings.push(TranslationWarning::CacheHit);
                entry.result.elapsed_ms = 0;
                entry.diagnostics.mark_cached();
                entry.diagnostics.protected_spans = spans;
                self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(TranslateOutput {
                    result: entry.result,
                    diagnostics: entry.diagnostics,
                });
            }
        }

        let mut result = TranslationResult {
            text: target_text.to_string(),
            source: source.clone(),
            target: req.target.clone(),
            engine_id: engine_id.clone(),
            model_id: None,
            elapsed_ms: 0,
            warnings: Vec::new(),
        };
        let term_trace = self.apply_term_guard(work_text, &source, &req.target, &mut result);
        result.text = quality::repair_output(&result.text);
        let quality = quality::check(work_text, &result.text, &req.target, options.mode);
        if self.config.strict_integrity && quality.has_fatal() {
            return Err(TranslationError::new(
                TranslationErrorCode::IntegrityFailure,
                format!("模板译文未通过完整性检查：{}", quality.summary()),
            ));
        }
        let rules_trace = self.config.rules.as_ref().map(|rules| RuleTrace {
            id: rules.id().to_string(),
            version: rules.version(),
            applied: rule_applied.to_vec(),
        });
        let diagnostics = TranslationDiagnostics {
            engine_id: engine_id.clone(),
            capability: Capability::Nmt,
            route_reason: format!(
                "完整句子模板命中：{}@{}",
                templates.id(),
                templates.version()
            ),
            cache_hit: false,
            source_assumed,
            rules_version: self.effective_rules_version,
            resource_fingerprint: self.resource_fingerprint.clone(),
            mode: options.mode,
            text_kind: TextKind::Sentence,
            route: translator_types::TranslationRoute::Rule,
            model_invoked: false,
            dictionary_hit: false,
            rule_matched: true,
            strategy: options
                .strategy
                .clone()
                .unwrap_or_else(|| "template".to_string()),
            context_id: options.context_id.clone(),
            rules: rules_trace,
            protected_spans: spans,
            restore_missing: Vec::new(),
            term: term_trace,
            quality: Some(quality),
        };
        self.stats.rule_matches.fetch_add(1, Ordering::Relaxed);
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

    /// 对聚合后的段落/文档译文做一次术语约束（P13-Q3 上下文术语一致性）。
    ///
    /// 源文传入**整段/整篇**：术语规则只在源文出现对应触发词时生效，因此
    /// 前文出现过的术语可以约束后文译文，而不需要把上下文塞进模型。
    pub fn enforce_terms_on_text(
        &self,
        source_text: &str,
        target: &translator_types::LanguageTag,
        text: &str,
    ) -> (String, Option<TermTrace>) {
        if !self.config.term_policy.runs() {
            return (text.to_string(), None);
        }
        let Some(guard) = self.config.term_guard.as_ref() else {
            return (text.to_string(), None);
        };
        let source_lang = self
            .config
            .language_identifier
            .as_ref()
            .and_then(|id| id.identify(source_text, 1).into_iter().next())
            .map(|g| g.language)
            .unwrap_or_else(|| translator_types::LanguageTag::new("en").expect("静态语言标签合法"));
        let outcome = guard.apply_with_context(&TermContext {
            source_text,
            target_text: text,
            source: &source_lang,
            target,
        });
        let applied = self.config.term_policy.applies_text();
        let result_text = if applied {
            outcome.text.clone()
        } else {
            text.to_string()
        };
        let trace = TermTrace {
            id: guard.id().to_string(),
            version: guard.version(),
            policy: self.config.term_policy,
            replaced: if applied { outcome.violations.len() } else { 0 },
            violations: outcome.violations,
        };
        (result_text, Some(trace))
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
