//! 协调器：串起校验 → 规范化 → 语言识别 → 占位符保护 → 路由 → 执行 → 还原 → 术语约束 → 缓存。
//!
//! 对应设计文档 §6 的流水线。Coordinator 不认识任何具体引擎，
//! 只依赖 `translator-plugin-api` 的 trait，因此替换后端不需要改这里。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use translator_plugin_api::{BoxFuture, PluginRegistry};
use translator_types::{
    CancelToken, Capability, CapabilityInfo, DictionaryEntry, FallbackReason, LookupRequest,
    RemoteFallbackPolicy, RouteDecision, TextKind, TranslateOptions, TranslateRequest,
    TranslationError, TranslationErrorCode, TranslationMode, TranslationPolicy, TranslationResult,
    TranslationRoute, TranslationWarning,
};

use crate::cache::{CacheEntry, CacheKey, QueryCache};
use crate::diagnostics::{
    RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics, UnitExecutionTrace,
};
use crate::evidence::{LanguageEvidence, SenseCandidate, TextSpan, UnitKind};
use crate::planner::{self, TranslationPlanner};
use crate::quality;
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
    /// 组合式日常句型规则（P19-G）。`None` 时不启用。
    pub composer: Option<Arc<crate::composer::CompositionEngine>>,
    /// 语义组合/中文生成层（P20-C）。`None` 时不启用。
    pub semantic_composer: Option<Arc<crate::semantic::SemanticCompositionEngine>>,
    /// 默认翻译模式（docs/57 §4）。默认 [`TranslationMode::Precise`]。
    pub default_mode: TranslationMode,
    /// 关键完整性检查失败时是否拒绝返回译文。
    ///
    /// 默认 `true`：关键数字丢失、受保护标记残留、空译文都返回 `IntegrityFailure`，
    /// 而不是把一份缺内容的译文交给宿主。设 `false` 时这些问题只进诊断。
    pub strict_integrity: bool,
    /// 本地模型不可用/失败时是否允许自动回退到远程模型。
    ///
    /// 默认 [`RemoteFallbackPolicy::Disabled`]：远程回退是独立于“本地学习”的
    /// 显式授权，不允许因为注册了远程插件就静默上传原文。
    pub remote_fallback: RemoteFallbackPolicy,
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
            .field(
                "composer",
                &self
                    .composer
                    .as_ref()
                    .map(|x| format!("{}@{}", x.id(), x.version())),
            )
            .field(
                "semantic_composer",
                &self
                    .semantic_composer
                    .as_ref()
                    .map(|x| format!("{}@{}", x.id(), x.version())),
            )
            .field("default_mode", &self.default_mode)
            .field("strict_integrity", &self.strict_integrity)
            .field("remote_fallback", &self.remote_fallback)
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
            composer: None,
            semantic_composer: None,
            default_mode: TranslationMode::Precise,
            strict_integrity: true,
            remote_fallback: RemoteFallbackPolicy::Disabled,
        }
    }
}

#[derive(Default)]
struct RouteCounters {
    executions: AtomicU64,
    dictionary_hits: AtomicU64,
    rule_matches: AtomicU64,
    nmt_calls: AtomicU64,
    remote_calls: AtomicU64,
    nmt_fallbacks: AtomicU64,
    remote_fallbacks: AtomicU64,
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
            remote_calls: self.stats.remote_calls.load(Ordering::Relaxed),
            nmt_fallbacks: self.stats.nmt_fallbacks.load(Ordering::Relaxed),
            remote_fallbacks: self.stats.remote_fallbacks.load(Ordering::Relaxed),
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
            &self.stats.remote_calls,
            &self.stats.nmt_fallbacks,
            &self.stats.remote_fallbacks,
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

    fn record_model_call(&self, route: TranslationRoute) {
        match route {
            TranslationRoute::LocalNmt => {
                self.stats.nmt_calls.fetch_add(1, Ordering::Relaxed);
            }
            TranslationRoute::Remote => {
                self.stats.remote_calls.fetch_add(1, Ordering::Relaxed);
            }
            TranslationRoute::Dictionary | TranslationRoute::Rule | TranslationRoute::Mixed => {}
        }
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
        let outcome = TranslationPlanner::new(&self.registry, self.config.remote_fallback)
            .resolve(req, &source, &work_text, false)?;
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

    /// 收集一个单元可共享的语言证据（P17-A / A1）。
    ///
    /// 词形/词义候选只来自词典 provider；查询失败不会影响翻译主链，只会让
    /// 证据为空。证据本身**不决定**引擎，Planner 仍按策略与结构选择。
    pub async fn language_evidence(
        &self,
        text: &str,
        source: &translator_types::LanguageTag,
        target: &translator_types::LanguageTag,
        kind: TextKind,
        context_text: Option<&str>,
        glossary_id: Option<&str>,
    ) -> LanguageEvidence {
        let mut evidence = LanguageEvidence::for_text(
            TextSpan::new(0, text.len()),
            UnitKind::from_text_kind(kind, false),
            text,
        );
        if let Some(id) = glossary_id {
            evidence
                .constraints
                .push(crate::evidence::TranslationConstraint::Glossary { id: id.to_string() });
        }
        if let Some(context) = context_text.filter(|c| !c.trim().is_empty()) {
            evidence.context = Some(crate::evidence::ContextRef {
                id: "provided-context".to_string(),
                chars: context.chars().count(),
            });
        }

        // 只有 Word/Phrase 才查词义候选；整句预查会浪费一次词典访问，
        // 也会把“句子中某个词出现在词典里”误当成整句可词典翻译。
        if kind != TextKind::Word {
            return evidence;
        }

        evidence.dictionary_available = self
            .registry
            .engines_with(Capability::Dictionary)
            .iter()
            .any(|engine| engine.supports(source, target));
        let lookup = LookupRequest {
            term: text.to_string(),
            language: Some(source.clone()),
            limit: 8,
        };
        let mut found = Vec::new();
        for provider in self.registry.providers() {
            evidence.dictionary_available = true;
            match provider.lookup(&lookup).await {
                Ok(entries) => {
                    for entry in entries {
                        let pos = entry.parts_of_speech.first().cloned();
                        for (idx, sense) in entry.senses.into_iter().take(3).enumerate() {
                            let confidence = ((entry.confidence.clamp(0.0, 1.0) * 100.0) as u8)
                                .saturating_sub(idx as u8 * 10);
                            found.push(SenseCandidate {
                                headword: entry.headword.clone(),
                                definition: sense.definition,
                                part_of_speech: pos.clone(),
                                source_id: sense.source_id,
                                confidence,
                            });
                        }
                    }
                }
                Err(_) => {
                    // 证据是可选增强；查询失败不能阻塞实际翻译。
                }
            }
            if found.len() >= 8 {
                break;
            }
        }
        let _ = target;
        evidence.add_senses(found);
        evidence
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

    /// P20-C：语义组合/中文生成层。只处理局部高级结构；失败后继续 P19/NMT。
    async fn try_semantic_composition(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
        source: &translator_types::LanguageTag,
        work_text: &str,
        rule_applied: &[String],
        source_assumed: bool,
    ) -> Result<Option<TranslateOutput>, TranslationError> {
        let Some(engine) = self.config.semantic_composer.as_ref().cloned() else {
            return Ok(None);
        };
        if !engine.is_candidate(work_text) {
            return Ok(None);
        }
        let candidates = engine.lookup_candidates(work_text);
        if candidates.is_empty() {
            return Ok(None);
        }
        let mut entries = std::collections::HashMap::new();
        for word in candidates {
            let lookup = LookupRequest {
                term: word.clone(),
                language: Some(source.clone()),
                limit: 8,
            };
            if let Ok(found) = self.lookup_term(&lookup, &word).await {
                if !found.is_empty() {
                    entries.insert(word, found);
                }
            }
        }
        let Some(semantic) = engine.compose(work_text, source, &req.target, &entries) else {
            return Ok(None);
        };
        let outcome = crate::composer::CompositionOutcome {
            text: semantic.text,
            rule_id: semantic.rule_id,
            pattern: semantic.pattern,
            evidence: semantic.evidence,
        };
        let engine_id = format!("semantic:{}", engine.id());
        let engine_identity = format!("{}@{}", engine.id(), engine.version());
        self.finish_composition(
            req,
            options,
            source.clone(),
            work_text,
            &outcome,
            &engine_id,
            &engine_identity,
            rule_applied,
            source_assumed,
        )
        .map(Some)
    }

    /// P19-G：尝试组合式日常简单句。失败不产生错误，只返回 `None` 交给 NMT。
    async fn try_composition(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
        source: &translator_types::LanguageTag,
        work_text: &str,
        rule_applied: &[String],
        source_assumed: bool,
    ) -> Result<Option<TranslateOutput>, TranslationError> {
        let Some(engine) = self.config.composer.as_ref().cloned() else {
            return Ok(None);
        };
        let candidates = engine.lookup_candidates(work_text);
        if candidates.is_empty() {
            return Ok(None);
        }
        let mut entries = std::collections::HashMap::new();
        for word in candidates {
            let lookup = LookupRequest {
                term: word.clone(),
                language: Some(source.clone()),
                limit: 8,
            };
            // 证据查询失败不能让主链失败；没有完整词典证据时组合会拒绝。
            if let Ok(found) = self.lookup_term(&lookup, &word).await {
                if !found.is_empty() {
                    entries.insert(word, found);
                }
            }
        }
        let Some(outcome) = engine.compose(work_text, source, &req.target, &entries) else {
            return Ok(None);
        };
        let engine_id = format!("compose:{}", engine.id());
        let engine_identity = format!("{}@{}", engine.id(), engine.version());
        self.finish_composition(
            req,
            options,
            source.clone(),
            work_text,
            &outcome,
            &engine_id,
            &engine_identity,
            rule_applied,
            source_assumed,
        )
        .map(Some)
    }

    /// 组合规则结果的收尾：与模板走同一术语/质量/缓存路径。
    #[allow(clippy::too_many_arguments)]
    fn finish_composition(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
        source: translator_types::LanguageTag,
        work_text: &str,
        outcome: &crate::composer::CompositionOutcome,
        engine_id: &str,
        engine_identity: &str,
        rule_applied: &[String],
        source_assumed: bool,
    ) -> Result<TranslateOutput, TranslationError> {
        let engine_id = engine_id.to_string();
        let spans = text::protected_spans(work_text);
        let key = CacheKey::new_with_options(
            work_text,
            req,
            source.as_str(),
            &engine_id,
            Some(engine_identity),
            self.effective_rules_version,
            &self.resource_fingerprint,
            options,
            "composition",
            TranslationRoute::Rule,
        );
        if let Some(hit) = self.lookup_cache(&key, &spans, &None, &outcome.evidence) {
            return Ok(hit);
        }

        let mut result = TranslationResult {
            text: outcome.text.clone(),
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
                format!("组合规则译文未通过完整性检查：{}", quality.summary()),
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
            route_reason: format!("组合规则命中：{}（{}）", engine_identity, outcome.rule_id),
            cache_hit: false,
            source_assumed,
            rules_version: self.effective_rules_version,
            resource_fingerprint: self.resource_fingerprint.clone(),
            mode: options.mode,
            text_kind: TextKind::Sentence,
            route: TranslationRoute::Rule,
            model_invoked: false,
            dictionary_hit: false,
            rule_matched: true,
            fallback_reason: None,
            evidence: Some(outcome.evidence.clone()),
            units: vec![UnitExecutionTrace {
                index: 0,
                span: TextSpan::new(0, work_text.len()),
                kind: UnitKind::Sentence,
                route: TranslationRoute::Rule,
                engine_id: engine_id.clone(),
                capability: Capability::Nmt,
                reason: format!("组合规则：{}", outcome.pattern),
                model_invoked: false,
                cache_hit: false,
                fallback_reason: None,
            }],
            strategy: "composition".to_string(),
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

    /// P16-R R-03 / P17-A A1：带明确搭配线索的单词/短语选义。
    ///
    /// 该路径在 Coordinator 内完成，不再由 API 旁路执行：这样策略、规则、缓存、
    /// 术语约束、质量门禁与诊断都走同一套逻辑。只有 Planner 先判定“能走词典”
    /// 才会进入。
    #[allow(clippy::too_many_arguments)]
    async fn try_context_word(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
        source: &translator_types::LanguageTag,
        work_text: &str,
        rule_applied: &[String],
        source_assumed: bool,
    ) -> Result<Option<TranslateOutput>, TranslationError> {
        if options.text_kind != TextKind::Word || options.context_text.is_none() {
            return Ok(None);
        }
        if !matches!(
            req.policy,
            TranslationPolicy::AutoLocalFirst | TranslationPolicy::DictionaryOnly
        ) {
            return Ok(None);
        }
        let planned = TranslationPlanner::new(&self.registry, self.config.remote_fallback)
            .resolve(req, source, work_text, true)?;
        if planned.decision.capability != Capability::Dictionary {
            return Ok(None);
        }

        let entries = match self
            .lookup_term(
                &LookupRequest {
                    term: work_text.to_string(),
                    language: Some(source.clone()),
                    limit: 10,
                },
                work_text,
            )
            .await
        {
            Ok(entries) => entries,
            // 证据/上下文增强失败不能阻塞正常词典/NMT 路径。
            Err(_) => return Ok(None),
        };
        if entries.is_empty() {
            return Ok(None);
        }
        let context = options.context_text.as_deref().unwrap_or("");
        let selected =
            crate::context_sense::select_by_context(work_text, context, &entries, &req.target)
                .or_else(|| crate::context_sense::compose_senses(&entries, 3));
        let Some(selected) = selected else {
            return Ok(None);
        };

        let spans = text::protected_spans(work_text);
        let mut evidence = LanguageEvidence::for_text(
            TextSpan::new(0, work_text.len()),
            UnitKind::Word,
            work_text,
        );
        evidence.context = Some(crate::evidence::ContextRef {
            id: options
                .context_id
                .clone()
                .unwrap_or_else(|| "provided-context".to_string()),
            chars: context.chars().count(),
        });
        evidence.dictionary_available = true;
        evidence.add_senses(entries.iter().flat_map(|entry| {
            let pos = entry.parts_of_speech.first().cloned();
            entry.senses.iter().map(move |sense| SenseCandidate {
                headword: entry.headword.clone(),
                definition: sense.definition.clone(),
                part_of_speech: pos.clone(),
                source_id: sense.source_id.clone(),
                confidence: (entry.confidence.clamp(0.0, 1.0) * 100.0) as u8,
            })
        }));
        let rules_trace = self.config.rules.as_ref().map(|rules| RuleTrace {
            id: rules.id().to_string(),
            version: rules.version(),
            applied: rule_applied.to_vec(),
        });

        // 上下文原文必须进入缓存身份，否则同一 context_id 下换上下文会命中错误译法。
        let mut key_opts = options.clone();
        let digest = context_digest(options.context_id.as_deref(), context);
        key_opts.context_id = Some(digest);
        let source_id = entries
            .first()
            .map(|e| e.source_id.clone())
            .unwrap_or_else(|| "dictionary".to_string());
        let key = CacheKey::new_with_options(
            work_text,
            req,
            source.as_str(),
            "dictionary.context",
            Some(&source_id),
            self.effective_rules_version,
            &self.resource_fingerprint,
            &key_opts,
            "word-context",
            TranslationRoute::Dictionary,
        );
        if let Some(hit) = self.lookup_cache(&key, &spans, &rules_trace, &evidence) {
            return Ok(Some(hit));
        }

        let mut result = TranslationResult {
            text: selected,
            source: source.clone(),
            target: req.target.clone(),
            engine_id: "dictionary.context".to_string(),
            model_id: Some(source_id),
            elapsed_ms: 0,
            warnings: Vec::new(),
        };
        let term_trace = self.apply_term_guard(work_text, source, &req.target, &mut result);
        result.text = quality::repair_output(&result.text);
        let quality = quality::check(work_text, &result.text, &req.target, options.mode);
        if self.config.strict_integrity && quality.has_fatal() {
            return Err(TranslationError::new(
                TranslationErrorCode::IntegrityFailure,
                format!("上下文选义词条未通过完整性检查：{}", quality.summary()),
            ));
        }
        let diagnostics = TranslationDiagnostics {
            engine_id: "dictionary.context".to_string(),
            capability: Capability::Dictionary,
            route_reason: "上下文线索选义（P16-R / R-03）".to_string(),
            cache_hit: false,
            source_assumed,
            rules_version: self.effective_rules_version,
            resource_fingerprint: self.resource_fingerprint.clone(),
            mode: options.mode,
            text_kind: TextKind::Word,
            route: TranslationRoute::Dictionary,
            model_invoked: false,
            dictionary_hit: true,
            rule_matched: false,
            fallback_reason: None,
            evidence: Some(evidence),
            units: vec![UnitExecutionTrace {
                index: 0,
                span: TextSpan::new(0, work_text.len()),
                kind: UnitKind::Word,
                route: TranslationRoute::Dictionary,
                engine_id: "dictionary.context".to_string(),
                capability: Capability::Dictionary,
                reason: "上下文线索选义".to_string(),
                model_invoked: false,
                cache_hit: false,
                fallback_reason: None,
            }],
            strategy: "word-context".to_string(),
            context_id: options.context_id.clone(),
            rules: rules_trace,
            protected_spans: spans,
            restore_missing: Vec::new(),
            term: term_trace,
            quality: Some(quality),
        };
        self.stats.dictionary_hits.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                key,
                CacheEntry {
                    result: result.clone(),
                    diagnostics: diagnostics.clone(),
                },
            );
        }
        Ok(Some(TranslateOutput {
            result,
            diagnostics,
        }))
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
        // 3.7) 组合式简单句规则（P19-G）。仅在策略允许词典/规则直接完成时尝试；
        //     未知成分或复杂结构立即返回 None，继续现有 Planner/NMT。
        if options.text_kind == TextKind::Sentence
            && matches!(
                req.policy,
                TranslationPolicy::AutoLocalFirst | TranslationPolicy::DictionaryOnly
            )
        {
            if let Some(out) = self
                .try_semantic_composition(
                    req,
                    options,
                    &source,
                    &work_text,
                    &rule_applied,
                    source_assumed,
                )
                .await?
            {
                return Ok(out);
            }
            if let Some(out) = self
                .try_composition(
                    req,
                    options,
                    &source,
                    &work_text,
                    &rule_applied,
                    source_assumed,
                )
                .await?
            {
                return Ok(out);
            }
        }

        // 3.8) 上下文选义：只在 Planner 确认可走词典时进入，仍走同一缓存/术语/质量链。
        if let Some(out) = self
            .try_context_word(
                req,
                options,
                &source,
                &work_text,
                &rule_applied,
                source_assumed,
            )
            .await?
        {
            return Ok(out);
        }

        // 4) 统一 Planner 选择引擎。词典/规则/本地模型/远程模型都在这里决定；
        //    协调器不再自己写第二套路由分支。
        let planner = TranslationPlanner::new(&self.registry, self.config.remote_fallback);
        let mut outcome = planner.resolve(
            req,
            &source,
            &work_text,
            options.text_kind == TextKind::Word,
        )?;

        // 4.5) 共享语言证据。Word/Phrase 才查词义候选；失败不影响主链。
        let evidence = self
            .language_evidence(
                &work_text,
                &source,
                &req.target,
                options.text_kind,
                options.context_text.as_deref(),
                req.glossary_id.as_deref(),
            )
            .await;

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

        // 6) 缓存查询（模式/策略/执行路由必须进键）
        let mut actual_capability = outcome.decision.capability;
        let mut actual_route = route_for_capability(actual_capability);
        let mut actual_engine_id = outcome.decision.engine_id.clone();
        let mut actual_reason = outcome.decision.reason.clone();
        let mut fallback_reason =
            planner::classify_fallback(req, &work_text, options.text_kind, actual_capability);
        if actual_route == TranslationRoute::Remote
            && matches!(req.policy, TranslationPolicy::AutoLocalFirst)
        {
            fallback_reason = Some(FallbackReason::EngineFallback);
        }
        let mut key = CacheKey::new_with_options(
            &protected,
            req,
            source.as_str(),
            &actual_engine_id,
            outcome.engine.cache_identity().as_deref(),
            self.effective_rules_version,
            &self.resource_fingerprint,
            options,
            options.strategy.as_deref().unwrap_or("single"),
            actual_route,
        );

        if let Some(hit) = self.lookup_cache(&key, &spans, &rules_trace, &evidence) {
            return Ok(hit);
        }

        // 7) 交给引擎的请求：文本已保护、源语言已确定
        let resolved = TranslateRequest {
            text: protected.clone(),
            source: Some(source.clone()),
            ..req.clone()
        };

        check_cancelled(cancel)?;
        let started = Instant::now();
        let mut attempts = 0usize;
        let mut result = loop {
            attempts += 1;
            if attempts > 3 {
                return Err(TranslationError::new(
                    TranslationErrorCode::BackendFailed,
                    "引擎回退次数超过上限",
                ));
            }
            self.record_model_call(actual_route);
            match outcome
                .engine
                .translate_cancellable(&resolved, cancel.unwrap_or_else(|| never_cancelled()))
                .await
            {
                Ok(result) => break result,
                Err(err) if self.should_fallback(req, actual_capability, &err) => {
                    let Some(fallback) =
                        planner.fallback_after_dictionary_miss(&source, &req.target)
                    else {
                        return Err(planner::not_installed(
                            Capability::Nmt,
                            &source,
                            &req.target,
                        ));
                    };
                    if fallback.decision.capability == Capability::Nmt {
                        self.stats.nmt_fallbacks.fetch_add(1, Ordering::Relaxed);
                    } else {
                        self.stats.remote_fallbacks.fetch_add(1, Ordering::Relaxed);
                    }
                    actual_capability = fallback.decision.capability;
                    actual_route = route_for_capability(actual_capability);
                    actual_engine_id = fallback.decision.engine_id.clone();
                    actual_reason = fallback.decision.reason.clone();
                    fallback_reason = if actual_route == TranslationRoute::Remote {
                        Some(FallbackReason::EngineFallback)
                    } else {
                        planner::classify_fallback(
                            req,
                            &work_text,
                            options.text_kind,
                            actual_capability,
                        )
                    };
                    key = CacheKey::new_with_options(
                        &protected,
                        req,
                        source.as_str(),
                        &actual_engine_id,
                        fallback.engine.cache_identity().as_deref(),
                        self.effective_rules_version,
                        &self.resource_fingerprint,
                        options,
                        options.strategy.as_deref().unwrap_or("single"),
                        actual_route,
                    );
                    if let Some(hit) = self.lookup_cache(&key, &spans, &rules_trace, &evidence) {
                        return Ok(hit);
                    }
                    outcome = fallback;
                    continue;
                }
                Err(err) if actual_route == TranslationRoute::LocalNmt => {
                    let Some(fallback) = planner.fallback_after_local_failure(&source, &req.target)
                    else {
                        return Err(err);
                    };
                    self.stats.remote_fallbacks.fetch_add(1, Ordering::Relaxed);
                    actual_capability = fallback.decision.capability;
                    actual_route = route_for_capability(actual_capability);
                    actual_engine_id = fallback.decision.engine_id.clone();
                    actual_reason = fallback.decision.reason.clone();
                    fallback_reason = Some(FallbackReason::EngineFallback);
                    key = CacheKey::new_with_options(
                        &protected,
                        req,
                        source.as_str(),
                        &actual_engine_id,
                        fallback.engine.cache_identity().as_deref(),
                        self.effective_rules_version,
                        &self.resource_fingerprint,
                        options,
                        options.strategy.as_deref().unwrap_or("single"),
                        actual_route,
                    );
                    if let Some(hit) = self.lookup_cache(&key, &spans, &rules_trace, &evidence) {
                        return Ok(hit);
                    }
                    outcome = fallback;
                    continue;
                }
                Err(err) => return Err(err),
            }
        };
        check_cancelled(cancel)?;
        if actual_route == TranslationRoute::Dictionary {
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

        let dictionary_hit = actual_route == TranslationRoute::Dictionary;
        let model_invoked = matches!(
            actual_route,
            TranslationRoute::LocalNmt | TranslationRoute::Remote
        );
        let unit_kind = UnitKind::from_text_kind(options.text_kind, false);
        let unit_trace = UnitExecutionTrace {
            index: 0,
            span: TextSpan::new(0, work_text.len()),
            kind: unit_kind,
            route: actual_route,
            engine_id: actual_engine_id.clone(),
            capability: actual_capability,
            reason: actual_reason.clone(),
            model_invoked,
            cache_hit: false,
            fallback_reason,
        };
        let diagnostics = TranslationDiagnostics {
            engine_id: actual_engine_id.clone(),
            capability: actual_capability,
            route_reason: actual_reason.clone(),
            cache_hit: false,
            source_assumed,
            rules_version: self.effective_rules_version,
            resource_fingerprint: self.resource_fingerprint.clone(),
            mode: options.mode,
            text_kind: options.text_kind,
            route: actual_route,
            model_invoked,
            dictionary_hit,
            rule_matched: false,
            fallback_reason,
            evidence: Some(evidence),
            units: vec![unit_trace],
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
        let mut evidence = LanguageEvidence::for_text(
            TextSpan::new(0, work_text.len()),
            UnitKind::Sentence,
            work_text,
        );
        evidence.add_pattern(crate::evidence::PatternMatch {
            kind: crate::evidence::PatternKind::Template,
            id: format!("{}@{}", templates.id(), templates.version()),
            span: TextSpan::new(0, work_text.len()),
            complete: true,
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
            fallback_reason: None,
            evidence: Some(evidence),
            units: vec![UnitExecutionTrace {
                index: 0,
                span: TextSpan::new(0, work_text.len()),
                kind: UnitKind::Sentence,
                route: TranslationRoute::Rule,
                engine_id: engine_id.clone(),
                capability: Capability::Nmt,
                reason: format!(
                    "完整句子模板命中：{}@{}",
                    templates.id(),
                    templates.version()
                ),
                model_invoked: false,
                cache_hit: false,
                fallback_reason: None,
            }],
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

impl Coordinator {
    /// 缓存读回与易变字段重置。回退路径也必须走这里，否则“词典 miss -> NMT”
    /// 每次都会重新推理（P16 Q-065），远程回退还会重复计费。
    fn lookup_cache(
        &self,
        key: &CacheKey,
        spans: &[crate::text::ProtectedSpan],
        rules: &Option<RuleTrace>,
        evidence: &LanguageEvidence,
    ) -> Option<TranslateOutput> {
        let mut entry = {
            let cache = self.cache.lock().ok()?;
            cache.get(key)?.clone()
        };
        entry.result.warnings.push(TranslationWarning::CacheHit);
        entry.result.elapsed_ms = 0;
        entry.diagnostics.mark_cached();
        entry.diagnostics.protected_spans = spans.to_vec();
        entry.diagnostics.evidence = Some(evidence.clone());
        if entry.diagnostics.rules.is_none() {
            entry.diagnostics.rules = rules.clone();
        }
        self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
        Some(TranslateOutput {
            result: entry.result,
            diagnostics: entry.diagnostics,
        })
    }
}

fn context_digest(context_id: Option<&str>, context_text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(context_id.unwrap_or("").as_bytes());
    hasher.update(b"\0");
    hasher.update(context_text.as_bytes());
    format!("word-ctx:{:x}", hasher.finalize())
}

fn route_for_capability(capability: Capability) -> TranslationRoute {
    match capability {
        Capability::Dictionary => TranslationRoute::Dictionary,
        Capability::Nmt => TranslationRoute::LocalNmt,
        Capability::Remote => TranslationRoute::Remote,
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
