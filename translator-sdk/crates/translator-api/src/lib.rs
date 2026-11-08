//! 唯一受支持的对外入口。
//!
//! 设计文档 §0 的交付判据：
//! > 即使移除 Bergamot、ncnn、Mozilla 模型以及全部第三方词库，
//! > `translator-api` 仍应能以 Mock/空资源模式编译运行。
//!
//! 本 crate 因此**不依赖任何 native runtime、不依赖任何 HTTP 客户端、不依赖 SQLite**。

use std::sync::Arc;

use translator_core::{
    quality::QualityReport, BudgetReservation, Coordinator, CoordinatorConfig, MemoryBudget,
    MemoryReport,
};

// 插件契约：`pub use` 同时满足内部使用与对外导出，避免重复导入。
pub use translator_plugin_api::{
    BoxFuture, DictionaryProvider, InferenceBackend, LoadedModel, PluginRegistry, TranslationEngine,
};
// 类型层：请求 / 结果 / 错误 / 语言标签 等全部公开
pub use translator_types::*;

#[cfg(feature = "mocks")]
pub use translator_plugin_api::mocks;

#[cfg(feature = "dictionary")]
pub use translator_dictionary as dictionary;

#[cfg(feature = "rules")]
pub use translator_rules as rules;

#[cfg(feature = "nmt-bergamot")]
pub use translator_nmt_bergamot as nmt_bergamot;
#[cfg(feature = "nmt-ort")]
pub use translator_nmt_ort as nmt_ort;

// 规则契约：即使不开 `rules` feature，宿主也可以自己实现 `TextRules` 注入。
pub use translator_core::rules::{NoRules, RuleOutcome, RulePreview, TextRules};
// 术语契约（P2.4）：同上，宿主可自己实现 `TargetTermGuard`。
pub use translator_core::terms::{
    NoTermGuard, TargetTermGuard, TermOutcome, TermPolicy, TermViolation,
};
pub use translator_core::{LanguageIdentifier, MorphologyAnalyzer};
// 受保护跨度（P2.4 起出现在诊断里，因此必须公开）
pub use translator_core::text::{ProtectedKind, ProtectedSpan};
// 诊断（P2.4）：`translate_with_diagnostics` 的返回类型。
pub use translator_core::diagnostics::{
    RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics,
};

/// 对外门面。构造后不可变，可跨线程共享（`Send + Sync`）。
pub struct Translator {
    coordinator: Coordinator,
    /// 默认执行选项（模式/文本类型/段落策略/上下文）。
    default_options: TranslateOptions,
    /// 内存预算池。记账凭据必须活到 `Translator` 结束，否则预留会被立刻归还。
    budget: Arc<MemoryBudget>,
    /// 已挂载资源的内存预留凭据（字段名带下划线：只用于保活，不直接读取）。
    _reservations: Vec<BudgetReservation>,
    /// 从资源仓库挂载时持有的租约：`Translator` 活着，磁盘目录就不会被删（P2.3）。
    #[cfg(feature = "resource")]
    _leases: Vec<translator_resource::ResourceLease>,
    /// 真实 NMT 引擎句柄（`docs/17` §5：加载/卸载必须能被 SDK 侧观测）。
    #[cfg(feature = "nmt-bergamot")]
    nmt_engines: Vec<Arc<translator_nmt_bergamot::BergamotNmtEngine>>,
    #[cfg(feature = "nmt-ort")]
    ort_engines: Vec<Arc<translator_nmt_ort::OrtNmtEngine>>,
}

impl Translator {
    pub fn builder() -> TranslatorBuilder {
        TranslatorBuilder::new()
    }

    /// 翻译入口：默认 Auto 文本类型 + Precise 模式（docs/62）。
    ///
    /// 单句/单词仍走原有单路径；多句/段落/文章会由 `TextStructureAnalyzer`
    /// 规划成多个翻译单元，再按原结构还原。
    pub async fn translate(
        &self,
        req: &TranslateRequest,
    ) -> Result<TranslationResult, TranslationError> {
        let options = self.default_options.clone();
        self.translate_with_options(req, &options).await
    }

    /// 当前默认执行选项。
    pub fn default_options(&self) -> TranslateOptions {
        self.default_options.clone()
    }

    /// 文本类型（Builder 默认值；单请求可通过 `TranslateOptions` 覆盖）。
    pub fn text_kind(&self) -> TextKind {
        self.default_options.text_kind
    }

    /// 指定完整选项翻译（模式 + 文本类型 + 段落策略 + 上下文）。
    pub async fn translate_with_options(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
    ) -> Result<TranslationResult, TranslationError> {
        self.translate_with_diagnostics_options(req, options)
            .await
            .map(|out| out.result)
    }

    /// 指定完整选项 + 诊断。
    pub async fn translate_with_diagnostics_options(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
    ) -> Result<TranslateOutput, TranslationError> {
        self.run_options(req, options).await
    }

    /// 指定模式翻译（单请求覆盖 Builder 默认值）。
    ///
    /// 模式会进入缓存键：同一句话的 Precise 结果不会被 Fast 请求命中，反之亦然。
    pub async fn translate_with_mode(
        &self,
        req: &TranslateRequest,
        mode: TranslationMode,
    ) -> Result<TranslationResult, TranslationError> {
        let mut options = self.default_options.clone();
        options.mode = mode;
        self.translate_with_options(req, &options).await
    }

    /// 带诊断的指定模式翻译。
    pub async fn translate_with_diagnostics_mode(
        &self,
        req: &TranslateRequest,
        mode: TranslationMode,
    ) -> Result<TranslateOutput, TranslationError> {
        let mut options = self.default_options.clone();
        options.mode = mode;
        self.translate_with_diagnostics_options(req, &options).await
    }

    /// 可取消的翻译入口（G06）。
    ///
    /// 令牌可在任意线程 `cancel()`；取消后返回 `Cancelled`，不写缓存。
    /// 对真实 Bergamot 后端，取消会终止独占 worker 进程（下次请求需要重新 load）。
    pub async fn translate_cancellable(
        &self,
        req: &TranslateRequest,
        cancel: &CancelToken,
    ) -> Result<TranslationResult, TranslationError> {
        self.coordinator.translate_cancellable(req, cancel).await
    }

    /// 带诊断的可取消翻译入口。
    pub async fn translate_with_diagnostics_cancellable(
        &self,
        req: &TranslateRequest,
        cancel: &CancelToken,
    ) -> Result<TranslateOutput, TranslationError> {
        self.coordinator
            .translate_with_diagnostics_cancellable(req, cancel)
            .await
    }

    /// 带结构化诊断的翻译入口（P2.4）。
    ///
    /// 与 [`Translator::translate`] 走**同一条**流水线，因此不会出现
    /// "诊断里说改了两处、实际译文没改"这类不一致。多出来的信息是：
    /// 路由理由、规则命中、受保护跨度、占位符还原缺失、术语替换明细、是否缓存命中。
    ///
    /// ⚠️ 诊断含用户原文片段（URL / 模板变量），只应回给发起请求的调用方，
    /// 不要原样写入集中式日志（设计文档 §4.1）。
    pub async fn translate_with_diagnostics(
        &self,
        req: &TranslateRequest,
    ) -> Result<TranslateOutput, TranslationError> {
        self.coordinator.translate_with_diagnostics(req).await
    }

    /// 预览术语约束会怎么改一段**译文**（不翻译、不写缓存）。
    ///
    /// 返回 `None` 表示没配置术语表；`Some` 且 `violations` 为空表示"没问题"。
    pub fn explain_terms(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> Option<TermOutcome> {
        self.coordinator.explain_terms(text, source, target)
    }

    /// 当前术语策略。
    pub fn term_policy(&self) -> TermPolicy {
        self.coordinator.term_policy()
    }

    /// 查词入口。与整句翻译分开建模（ADR-003）。
    pub async fn lookup(
        &self,
        req: &LookupRequest,
    ) -> Result<Vec<DictionaryEntry>, TranslationError> {
        self.coordinator.lookup(req).await
    }

    /// 语言识别。短文本必须返回多个候选，不强行判定。
    pub async fn detect_language(
        &self,
        text: &str,
    ) -> Result<Vec<LanguageGuess>, TranslationError> {
        let normalized = translator_core::text::normalize(text);
        if normalized.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "文本为空",
            ));
        }

        // 注入的 LID 优先；未注入时保持 P1 的启发式行为（P94-01）。
        if let Some(mut guesses) = self.coordinator.identify_language(&normalized, 5) {
            if guesses.is_empty() {
                return Err(TranslationError::new(
                    TranslationErrorCode::UnsupportedLanguage,
                    "无法从文本判断语言",
                ));
            }
            guesses.truncate(5);
            return Ok(guesses);
        }

        let primary = translator_core::text::guess_language(&normalized).ok_or_else(|| {
            TranslationError::new(
                TranslationErrorCode::UnsupportedLanguage,
                "无法从文本判断语言",
            )
        })?;

        let mut guesses = vec![LanguageGuess {
            language: primary.clone(),
            confidence: 0.6,
        }];

        // 汉字为中/日共享，补充一个候选而不是硬判
        let has_han = normalized
            .chars()
            .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));
        if has_han && primary.as_str() == "zh-Hans" {
            if let Ok(ja) = LanguageTag::new("ja") {
                guesses.push(LanguageGuess {
                    language: ja,
                    confidence: 0.25,
                });
            }
        }

        Ok(guesses)
    }

    /// 当前默认翻译模式。
    pub fn translation_mode(&self) -> TranslationMode {
        self.coordinator.default_mode()
    }

    pub fn capabilities(&self) -> Vec<CapabilityInfo> {
        self.coordinator.capabilities()
    }

    /// 预览规则包对一段输入做了什么（不执行翻译、不写缓存）。
    ///
    /// 规则 id 不进 `TranslationResult`：给公开枚举加变体会破坏宿主的穷尽匹配，
    /// 因此诊断入口单独提供。
    pub fn explain_rules(
        &self,
        text: &str,
        source: &LanguageTag,
        target: &LanguageTag,
    ) -> RulePreview {
        self.coordinator.explain_rules(text, source, target)
    }

    /// 只给出路由决策，不执行翻译（调试/诊断用）。
    pub fn explain_route(&self, req: &TranslateRequest) -> Result<RouteDecision, TranslationError> {
        self.coordinator.explain_route(req)
    }

    /// 真实 NMT 引擎句柄（Bergamot）：加载/卸载/缓存统计都得能被宿主观测。
    #[cfg(feature = "nmt-bergamot")]
    pub fn nmt_engines(&self) -> &[Arc<translator_nmt_bergamot::BergamotNmtEngine>] {
        &self.nmt_engines
    }

    /// 第二后端（ORT）的引擎句柄：加载/卸载/身份观测用，不参与路由决策。
    #[cfg(feature = "nmt-ort")]
    pub fn nmt_ort_engines(&self) -> &[Arc<translator_nmt_ort::OrtNmtEngine>] {
        &self.ort_engines
    }

    /// 真实 NMT 引擎的加载状态（没挂真实引擎时是空表）。
    #[cfg(feature = "nmt-bergamot")]
    pub fn nmt_states(&self) -> Vec<translator_nmt_bergamot::LoadState> {
        self.nmt_engines.iter().map(|e| e.state()).collect()
    }

    /// 真实 NMT 引擎的缓存统计。
    #[cfg(feature = "nmt-bergamot")]
    pub fn nmt_cache_stats(&self) -> Vec<translator_core::plan_cache::PlanCacheStats> {
        self.nmt_engines.iter().map(|e| e.cache_stats()).collect()
    }

    /// 按空闲 TTL 回收真实模型；返回实际卸载的引擎数。
    ///
    /// 宿主应当在**没有请求进来的时候**调用（例如自己的空闲定时器）。
    #[cfg(feature = "nmt-bergamot")]
    pub fn evict_idle_nmt(&self) -> Result<usize, TranslationError> {
        let mut evicted = 0;
        for engine in &self.nmt_engines {
            if engine.evict_if_idle_configured()? {
                evicted += 1;
            }
        }
        Ok(evicted)
    }

    /// 显式卸载全部真实模型（内存立刻还回去；引擎对象仍可继续用）。
    #[cfg(feature = "nmt-bergamot")]
    pub fn unload_nmt(&self) -> Result<(), TranslationError> {
        for engine in &self.nmt_engines {
            engine.unload()?;
        }
        Ok(())
    }

    /// 显式卸载 ORT 模型（会话 drop 后 ORT arena 随之释放）。
    #[cfg(feature = "nmt-ort")]
    pub fn unload_nmt_ort(&self) -> Result<(), TranslationError> {
        for engine in &self.ort_engines {
            engine.unload()?;
        }
        Ok(())
    }

    pub fn registry(&self) -> &PluginRegistry {
        self.coordinator.registry()
    }

    pub fn clear_cache(&self) {
        self.coordinator.clear_cache();
    }

    /// 内存预算快照：上限、已用、逐项明细（P2.3）。
    ///
    /// 数值来自各组件自报的**下界估计**，不是进程 RSS；
    /// 判断"是否够用"请看 `docs/04` §6.2 的实测数据，不要把这里的数字当保证。
    /// 当前资源指纹（词典/规则/术语的 `id@version#digest`）；诊断与测试用。
    pub fn resource_fingerprint(&self) -> &str {
        self.coordinator.resource_fingerprint()
    }

    pub fn memory_report(&self) -> MemoryReport {
        self.budget.report()
    }

    /// 释放资源。P0 无可释放资源，保留稳定签名供后续实现。
    pub async fn shutdown(&self) -> Result<(), TranslationError> {
        self.coordinator.clear_cache();
        Ok(())
    }
}

impl Translator {
    /// 结构分析 + 规划 + 逐单元执行。
    async fn run_options(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
    ) -> Result<TranslateOutput, TranslationError> {
        if req.text.trim().is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "请求文本为空",
            ));
        }
        let plan = translator_core::planner::plan(&req.text, options);
        if plan.units.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "文本结构分析没有得到可翻译单元",
            ));
        }

        let single = plan.units.len() == 1
            && plan.units[0].text.is_some()
            && plan.kind != TextKind::Document;
        if single {
            let mut seg_options = options.clone();
            seg_options.text_kind = plan.kind;
            seg_options.strategy = Some(plan.strategy.to_string());
            return self
                .coordinator
                .translate_with_diagnostics_options(req, &seg_options)
                .await;
        }

        self.run_plan(req, options, &plan).await
    }

    async fn run_plan(
        &self,
        req: &TranslateRequest,
        options: &TranslateOptions,
        plan: &translator_core::planner::TranslationPlan,
    ) -> Result<TranslateOutput, TranslationError> {
        let mut text = String::new();
        let mut warnings: Vec<TranslationWarning> = Vec::new();
        let mut quality = QualityReport {
            mode: options.mode,
            ..QualityReport::default()
        };
        let mut first: Option<TranslateOutput> = None;
        let mut elapsed_ms = 0u64;
        let mut all_cache_hit = true;
        let mut any_translated = false;
        let mut mixed_engine = false;
        let mut first_engine = String::new();
        let mut first_model: Option<String> = None;

        for unit in &plan.units {
            text.push_str(&unit.separator);
            if let Some(literal) = &unit.literal {
                text.push_str(literal);
                text.push_str(&unit.suffix);
                continue;
            }
            let Some(source_text) = &unit.text else {
                text.push_str(&unit.suffix);
                continue;
            };

            let mut sub_req = req.clone();
            sub_req.text = source_text.clone();
            let mut seg_options = options.clone();
            seg_options.text_kind = unit.kind;
            seg_options.strategy = Some(plan.strategy.to_string());
            if unit.kind == TextKind::Paragraph {
                seg_options.paragraph_strategy = options.paragraph_strategy;
                seg_options.strategy = None;
            }

            let outcome = if unit.kind == TextKind::Paragraph {
                Box::pin(self.run_options(&sub_req, &seg_options)).await
            } else {
                self.coordinator
                    .translate_with_diagnostics_options(&sub_req, &seg_options)
                    .await
            };

            match outcome {
                Ok(out) => {
                    any_translated = true;
                    text.push_str(&out.result.text);
                    elapsed_ms = elapsed_ms.saturating_add(out.result.elapsed_ms);
                    warnings.extend(out.result.warnings.iter().cloned());
                    if let Some(q) = &out.diagnostics.quality {
                        quality.merge(q.clone());
                    }
                    all_cache_hit &= out.diagnostics.cache_hit;
                    if first.is_none() {
                        first_engine = out.result.engine_id.clone();
                        first_model = out.result.model_id.clone();
                        first = Some(out);
                    } else if first_engine != out.result.engine_id {
                        mixed_engine = true;
                    }
                }
                Err(err) if plan.kind == TextKind::Document => {
                    // 文档段落失败时保留原文与其他成功段落（T7.4）。
                    text.push_str(source_text);
                    warnings.push(TranslationWarning::SegmentFailed(format!(
                        "{}:{}",
                        unit.kind, err.code
                    )));
                }
                Err(err) => return Err(err),
            }
            text.push_str(&unit.suffix);
        }

        let engine_id = if !any_translated {
            "planner.passthrough".to_string()
        } else if mixed_engine {
            "planner.mixed".to_string()
        } else {
            first_engine
        };

        if let Some(first_out) = first {
            let mut diagnostics = first_out.diagnostics;
            diagnostics.engine_id = engine_id.clone();
            diagnostics.route_reason = format!(
                "planner:{} units={} kind={}",
                plan.strategy,
                plan.units.len(),
                plan.kind
            );
            diagnostics.cache_hit = all_cache_hit;
            diagnostics.text_kind = plan.kind;
            diagnostics.strategy = plan.strategy.to_string();
            diagnostics.context_id = options.context_id.clone();
            diagnostics.quality = Some(quality);
            diagnostics.protected_spans.clear();
            let source = first_out.result.source;
            Ok(TranslateOutput {
                result: TranslationResult {
                    text,
                    source: source.clone(),
                    target: req.target.clone(),
                    engine_id,
                    model_id: first_model,
                    elapsed_ms,
                    warnings,
                },
                diagnostics,
            })
        } else {
            let source = req
                .source
                .clone()
                .unwrap_or_else(|| LanguageTag::new("en").expect("静态语言标签合法"));
            let diagnostics = TranslationDiagnostics {
                engine_id: engine_id.clone(),
                capability: Capability::Nmt,
                route_reason: format!("planner:{} units={}", plan.strategy, plan.units.len()),
                cache_hit: false,
                source_assumed: req.source.is_none(),
                rules_version: 0,
                resource_fingerprint: self.coordinator.resource_fingerprint().to_string(),
                mode: options.mode,
                text_kind: plan.kind,
                strategy: plan.strategy.to_string(),
                context_id: options.context_id.clone(),
                quality: Some(quality),
                rules: None,
                protected_spans: Vec::new(),
                restore_missing: Vec::new(),
                term: None,
            };
            Ok(TranslateOutput {
                result: TranslationResult {
                    text,
                    source,
                    target: req.target.clone(),
                    engine_id,
                    model_id: None,
                    elapsed_ms,
                    warnings,
                },
                diagnostics,
            })
        }
    }
}

impl std::fmt::Debug for Translator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let registry = self.coordinator.registry();
        f.debug_struct("Translator")
            .field("engines", &registry.engines().len())
            .field("providers", &registry.providers().len())
            .field("backends", &registry.backends().len())
            .field("memory", &self.budget.report())
            .finish()
    }
}

/// 组装器（composition root 的一部分：把插件注入 Coordinator）。
pub struct TranslatorBuilder {
    registry: PluginRegistry,
    config: CoordinatorConfig,
    errors: Vec<TranslationError>,
    #[cfg(feature = "dictionary")]
    dictionary_roots: Vec<std::path::PathBuf>,
    rules: Option<Arc<dyn TextRules>>,
    /// 规则文件内容摘要（仅 `rules_file` 路径有）；P8 EP-02。
    rules_digest: Option<String>,
    /// 宿主直接注入 Arc 时提供的规则内容摘要（可选）。
    rules_fingerprint: Option<String>,
    term_guard: Option<Arc<dyn TargetTermGuard>>,
    /// 术语文件内容摘要（仅 `term_table_file` 路径有）；P8 EP-02。
    term_digest: Option<String>,
    /// 宿主直接注入 Arc 时提供的术语内容摘要（可选）。
    term_fingerprint: Option<String>,
    term_policy: TermPolicy,
    language_identifier: Option<Arc<dyn LanguageIdentifier>>,
    lemmatizer: Option<Arc<dyn MorphologyAnalyzer>>,
    /// 峰值内存上限（字节）。`None` 表示不限制但仍记账。
    memory_limit: Option<u64>,
    /// 默认翻译模式（docs/57 §4）：默认 Precise。
    translation_mode: TranslationMode,
    /// 默认文本类型（docs/62 §3）：默认 Auto。
    default_text_kind: TextKind,
    /// 默认段落策略：Auto。
    default_paragraph_strategy: ParagraphStrategy,
    /// 已登记的资源租约，`build()` 时移交给 `Translator`。
    #[cfg(feature = "resource")]
    leases: Vec<translator_resource::ResourceLease>,
    /// 真实 NMT 引擎句柄与被拒绝的构造错误。
    #[cfg(feature = "nmt-bergamot")]
    nmt_engines: Vec<Arc<translator_nmt_bergamot::BergamotNmtEngine>>,
    #[cfg(feature = "nmt-ort")]
    ort_engines: Vec<Arc<translator_nmt_ort::OrtNmtEngine>>,
}

impl Default for TranslatorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TranslatorBuilder {
    pub fn new() -> Self {
        Self {
            registry: PluginRegistry::new(),
            config: CoordinatorConfig::default(),
            errors: Vec::new(),
            #[cfg(feature = "dictionary")]
            dictionary_roots: Vec::new(),
            rules: None,
            rules_digest: None,
            rules_fingerprint: None,
            term_guard: None,
            term_digest: None,
            term_fingerprint: None,
            term_policy: TermPolicy::default(),
            language_identifier: None,
            lemmatizer: None,
            memory_limit: None,
            translation_mode: TranslationMode::Precise,
            default_text_kind: TextKind::Auto,
            default_paragraph_strategy: ParagraphStrategy::Auto,
            #[cfg(feature = "resource")]
            leases: Vec::new(),
            #[cfg(feature = "nmt-bergamot")]
            nmt_engines: Vec::new(),
            #[cfg(feature = "nmt-ort")]
            ort_engines: Vec::new(),
        }
    }

    /// 挂载真实本地 NMT 引擎（需启用 `nmt-bergamot` feature）。
    ///
    /// 构造期只做**文件存在性与摘要校验**，不 spawn 进程、不加载权重：
    /// 首次翻译（或宿主显式调用 `engine.load()`）才会真正加载。
    /// 构造失败计入构建错误，`build()` 时返回，不会带病运行。
    #[cfg(feature = "nmt-bergamot")]
    pub fn nmt_bergamot(mut self, config: translator_nmt_bergamot::BergamotNmtConfig) -> Self {
        // P8.2：相同模型内容/配置的宿主共享同一个 engine/worker（池里只存 Weak）。
        match translator_nmt_bergamot::BergamotNmtEngine::shared(config) {
            Ok(engine) => {
                // 句柄要留一份给 `Translator`：加载/卸载/缓存统计得能被观测（docs/17 §5）
                self.nmt_engines.push(engine.clone());
                self.engine(engine)
            }
            Err(err) => {
                self.errors.push(err);
                self
            }
        }
    }

    /// 挂载第二个本地 NMT 引擎（ONNX Runtime + opus-mt，需启用 `nmt-ort` feature）。
    ///
    /// P4 的意义：**同一个 `Translator` 契约下换掉整个 native 运行时**。
    /// 构造期只检查文件与配置，不加载权重；默认 feature 下这段代码不会进依赖树。
    #[cfg(feature = "nmt-ort")]
    pub fn nmt_ort(mut self, config: translator_nmt_ort::OrtNmtConfig) -> Self {
        // P8.2：相同 ORT 模型/配置的宿主共享同一个会话（池里只存 Weak）。
        match translator_nmt_ort::OrtNmtEngine::shared(config) {
            Ok(engine) => {
                self.ort_engines.push(engine.clone());
                self.engine(engine)
            }
            Err(err) => {
                self.errors.push(err);
                self
            }
        }
    }

    /// 注入 LID 实现（可选）。默认不注入，保持零模型启发式；只有宿主显式提供
    /// `FastTextIdentifier` 等实现时才启用正式模型。
    pub fn language_identifier(mut self, identifier: Arc<dyn LanguageIdentifier>) -> Self {
        self.language_identifier = Some(identifier);
        self
    }

    /// 注入形态分析器（可选）：只在 `lookup` 原词未命中时尝试 lemma 候选。
    pub fn lemmatizer(mut self, lemmatizer: Arc<dyn MorphologyAnalyzer>) -> Self {
        self.lemmatizer = Some(lemmatizer);
        self
    }

    pub fn engine(mut self, engine: Arc<dyn TranslationEngine>) -> Self {
        if let Err(err) = self.registry.register_engine(engine) {
            self.errors.push(err);
        }
        self
    }

    pub fn provider(mut self, provider: Arc<dyn DictionaryProvider>) -> Self {
        if let Err(err) = self.registry.register_provider(provider) {
            self.errors.push(err);
        }
        self
    }

    pub fn backend(mut self, backend: Arc<dyn InferenceBackend>) -> Self {
        if let Err(err) = self.registry.register_backend(backend) {
            self.errors.push(err);
        }
        self
    }

    /// 挂载一个离线词典包目录（需启用 `dictionary` feature）。
    ///
    /// 包在 `build()` 时才真正打开并校验；打开失败会计入构建错误，
    /// 不会静默退化。
    #[cfg(feature = "dictionary")]
    pub fn dictionary_package(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.dictionary_roots.push(root.into());
        self
    }

    /// 为**直接注入**的规则 Arc 提供内容摘要。版本号不变但内容变化时，
    /// 宿主应传新的摘要；不传则只按 `id@version` 计入缓存键（明确边界）。
    pub fn rules_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.rules_fingerprint = Some(fingerprint.into());
        self
    }

    /// 为**直接注入**的术语 Arc 提供内容摘要；语义同 [`Self::rules_fingerprint`]。
    pub fn term_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.term_fingerprint = Some(fingerprint.into());
        self
    }

    /// 注入文本规则包（不需要 `rules` feature：`TextRules` 是公开契约）。
    ///
    /// 规则包的 `version()` 会自动进入缓存键，因此**换规则必须换版本**，
    /// 否则旧缓存会被错误复用。
    pub fn rules(mut self, rules: Arc<dyn TextRules>) -> Self {
        self.rules = Some(rules);
        self
    }

    /// 从文件加载规则包（JSON 或二进制形态，需启用 `rules` feature）。
    ///
    /// 加载失败计入构建错误，不会静默退化成「没有规则」——
    /// 规则是为了纠正译文，静默失效比报错更危险。
    #[cfg(feature = "rules")]
    pub fn rules_file(mut self, path: impl AsRef<std::path::Path>) -> Self {
        let path_buf = path.as_ref().to_path_buf();
        match translator_rules::RuleEngine::load(&path_buf) {
            Ok(engine) => {
                self.rules = Some(Arc::new(engine));
                self.rules_digest = sha256_file(&path_buf);
            }
            Err(e) => self.errors.push(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!("规则包加载失败：{e}"),
            )),
        }
        self
    }

    /// 从资源仓库挂载某个资源的**激活版本**，并持有租约（P2.3）。
    ///
    /// 与 `dictionary_package(dir)` 的区别：
    ///
    /// - 目录由仓库解析，宿主不用自己拼路径；
    /// - 登记租约，因此在 `Translator` 存续期间该版本**不会被 `uninstall`/`cleanup` 删掉**
    ///   （跨进程也成立，前提是对方式遵守同一套租约协议）；
    /// - 仓库里换激活版本不影响已经挂载的这一份（快照语义）。
    #[cfg(all(feature = "dictionary", feature = "resource"))]
    pub fn dictionary_from_store(
        mut self,
        store: &translator_resource::ResourceStore,
        id: &str,
    ) -> Self {
        let status = match store.inspect(id) {
            Ok(status) => status,
            Err(e) => {
                self.errors.push(TranslationError::new(
                    TranslationErrorCode::ResourceNotInstalled,
                    e.to_string(),
                ));
                return self;
            }
        };
        let Some(version) = status.active else {
            self.errors.push(TranslationError::new(
                TranslationErrorCode::ResourceNotInstalled,
                format!("资源 {id} 没有激活版本"),
            ));
            return self;
        };
        // 先拿租约再取目录：中间任何一步失败，租约随作用域释放，
        // 不会留下"锁住了但没挂载"的悬空租约。
        let lease = match store.acquire_lease(id, &version, "translator-api") {
            Ok(lease) => lease,
            Err(e) => {
                self.errors.push(TranslationError::new(
                    TranslationErrorCode::RuntimeInitFailed,
                    e.to_string(),
                ));
                return self;
            }
        };
        match store.version_dir(id, &version) {
            Ok(dir) => {
                self.dictionary_roots.push(dir);
                self.leases.push(lease);
            }
            Err(e) => {
                self.errors.push(TranslationError::new(
                    TranslationErrorCode::RuntimeInitFailed,
                    e.to_string(),
                ));
            }
        }
        self
    }

    /// 默认翻译模式（docs/57 §4）。默认 [`TranslationMode::Precise`]；
    /// Fast 必须显式选择，且两种模式不会共用缓存。
    ///
    /// 单请求覆盖用 [`Translator::translate_with_mode`] 等入口，
    /// 因此**不需要**给 `TranslateRequest` 增加字段，旧的结构体字面量仍然编译。
    pub fn translation_mode(mut self, mode: TranslationMode) -> Self {
        self.translation_mode = mode;
        self
    }

    /// 默认文本类型（docs/62 §3）：默认 [`TextKind::Auto`]。
    ///
    /// 宿主可用它把同一 `Translator` 固定为 Word/Sentence/Paragraph/Document；
    /// 单请求覆盖用 [`Translator::translate_with_options`]。
    pub fn text_kind(mut self, kind: TextKind) -> Self {
        self.default_text_kind = kind;
        self
    }

    /// 默认段落策略（仅 `TextKind::Paragraph` 时有意义）。
    pub fn paragraph_strategy(mut self, strategy: ParagraphStrategy) -> Self {
        self.default_paragraph_strategy = strategy;
        self
    }

    /// 关键完整性检查失败时是否拒绝返回译文（默认 `true`）。
    ///
    /// 关闭后，数字丢失/标记残留只写入诊断，译文照常返回——仅在宿主自己做修复时使用。
    pub fn strict_integrity(mut self, strict: bool) -> Self {
        self.config.strict_integrity = strict;
        self
    }

    /// 峰值内存上限（字节）。`None` 之外还想表达"不限制"，用 `MemoryBudget::unlimited` 的语义，
    /// 这里传 `None` 即可。
    ///
    /// 判定发生在 **`build()` 装配期**：超过上限直接返回 `OutOfBudget`，不会带着
    /// "大概放不下"的状态跑起来。运行期不按请求限流（见 `translator_core::budget` 的说明）。
    pub fn memory_budget(mut self, bytes: Option<u64>) -> Self {
        self.memory_limit = bytes;
        self
    }

    /// 用 `ResourceLimits` 配置预算（`max_peak_ram_bytes` 为上限）。
    pub fn limits(mut self, limits: translator_types::ResourceLimits) -> Self {
        self.memory_limit = limits.max_peak_ram_bytes;
        self
    }

    /// 注入输出侧术语表（不需要 `rules` feature：`TargetTermGuard` 是公开契约）。
    ///
    /// 术语表版本会自动进入缓存键与诊断，**改术语表必须改 `version()`**，
    /// 否则旧缓存会被错误复用。
    pub fn term_guard(mut self, guard: Arc<dyn TargetTermGuard>) -> Self {
        self.term_guard = Some(guard);
        self
    }

    /// 从文件加载术语表（JSON，需启用 `rules` feature）。
    ///
    /// 加载失败计入构建错误：术语表写错了要立刻知道，静默退化成"没有术语约束"
    /// 会让用户以为约束生效了。
    #[cfg(feature = "rules")]
    pub fn term_table_file(mut self, path: impl AsRef<std::path::Path>) -> Self {
        let path_buf = path.as_ref().to_path_buf();
        match translator_rules::terms::TargetTermTable::from_file(&path_buf) {
            Ok(table) => {
                self.term_guard = Some(Arc::new(table));
                self.term_digest = sha256_file(&path_buf);
            }
            Err(e) => self.errors.push(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!("术语表加载失败：{e}"),
            )),
        }
        self
    }

    /// 术语约束策略，默认 [`TermPolicy::Enforce`]。
    pub fn term_policy(mut self, policy: TermPolicy) -> Self {
        self.term_policy = policy;
        self
    }

    pub fn cache_capacity(mut self, capacity: usize) -> Self {
        self.config.cache_capacity = capacity;
        self
    }

    /// 严格占位符恢复：关键占位符（URL/邮箱/代码/模板变量）没能还原时，
    /// 直接返回 `IntegrityFailure` 错误，而不是给出缺内容的译文。
    ///
    /// 默认关闭（与 P0/P1 行为一致）。开启后不再"降级返回"，见 ADR-009。
    pub fn strict_placeholder_restore(mut self, strict: bool) -> Self {
        self.config.strict_placeholder_restore = strict;
        self
    }

    /// 手动指定规则版本。仅在**没有**注入 `rules` 时生效
    /// （注入了规则包时以规则包自报版本为准）。
    pub fn rules_version(mut self, version: u32) -> Self {
        self.config.rules_version = version;
        self
    }

    /// 构造门面。注册阶段积累的错误在这里一次性暴露，
    /// 避免把错误分散到每个 builder 方法里。
    pub fn build(mut self) -> Result<Translator, TranslationError> {
        // 已挂载资源的 `id@version`，用于缓存隔离（ADR-011）
        let mut fingerprint: Vec<String> = Vec::new();
        // 内存预算池与已预留凭据（凭据必须活到 Translator 结束）
        let budget = Arc::new(MemoryBudget::new(self.memory_limit));
        #[cfg_attr(not(feature = "dictionary"), allow(unused_mut))]
        let mut reservations: Vec<BudgetReservation> = Vec::new();

        // 先挂载词典包：把包内引擎与查词提供者注册进同一个注册表
        #[cfg(feature = "dictionary")]
        {
            let roots = std::mem::take(&mut self.dictionary_roots);
            for root in roots {
                match translator_dictionary::open_as_plugins(&root) {
                    Ok((engine, provider)) => {
                        // 指纹取自清单（只读 manifest.json，不重复打开索引/数据库）
                        match translator_dictionary::read_manifest(&root) {
                            Ok(manifest) => {
                                // P8.3：同 id@version 重打包成不同内容也必须换指纹。
                                fingerprint.push(format!(
                                    "{}@{}#{}",
                                    manifest.id,
                                    manifest.version,
                                    manifest.content_digest()
                                ));
                            }
                            Err(e) => self.errors.push(TranslationError::from(e)),
                        }
                        // 内存预算：按"下界估计"记账，超限即在装配期失败（P2.3）
                        let footprint = engine.package().memory_footprint();
                        let label = format!("dictionary:{}", engine.package().manifest().id);
                        match budget.try_reserve(label, footprint.accounting_bytes()) {
                            Ok(reservation) => reservations.push(reservation),
                            Err(e) => self.errors.push(TranslationError::from(e)),
                        }
                        if let Err(e) = self.registry.register_engine(engine) {
                            self.errors.push(e);
                        }
                        if let Err(e) = self.registry.register_provider(provider) {
                            self.errors.push(e);
                        }
                    }
                    Err(e) => self.errors.push(TranslationError::from(e)),
                }
            }
        }

        // 真实 NMT 的内存准入（docs/17 §4.1.8）：用实测下界估计记账，
        // 超限在装配期就失败，而不是加载到一半 OOM。
        #[cfg(feature = "nmt-bergamot")]
        for engine in &self.nmt_engines {
            let footprint = engine.memory_footprint_bytes();
            let label = format!("nmt:{}", translator_nmt_bergamot::ENGINE_ID);
            match budget.try_reserve(label, footprint) {
                Ok(reservation) => reservations.push(reservation),
                Err(e) => self.errors.push(TranslationError::from(e)),
            }
        }

        #[cfg(feature = "nmt-ort")]
        for engine in &self.ort_engines {
            let footprint = engine.memory_footprint_bytes();
            let label = format!("nmt:{}", translator_nmt_ort::ENGINE_ID);
            match budget.try_reserve(label, footprint) {
                Ok(reservation) => reservations.push(reservation),
                Err(e) => self.errors.push(TranslationError::from(e)),
            }
        }

        if let Some(err) = self.errors.into_iter().next() {
            return Err(err);
        }

        if let Some(rules) = &self.rules {
            let mut item = format!("{}@{}", rules.id(), rules.version());
            if let Some(digest) = self
                .rules_digest
                .as_ref()
                .or(self.rules_fingerprint.as_ref())
            {
                item.push('#');
                item.push_str(digest);
            }
            fingerprint.push(item);
        }
        if let Some(guard) = &self.term_guard {
            // 术语表变化必须让旧译文失效：同一句话在不同术语表下结果不同
            let mut item = format!("{}@{}", guard.id(), guard.version());
            if let Some(digest) = self.term_digest.as_ref().or(self.term_fingerprint.as_ref()) {
                item.push('#');
                item.push_str(digest);
            }
            fingerprint.push(item);
        }
        self.config.rules = self.rules.take();
        self.config.term_guard = self.term_guard.take();
        self.config.term_policy = self.term_policy;
        self.config.language_identifier = self.language_identifier.take();
        self.config.lemmatizer = self.lemmatizer.take();
        self.config.default_mode = self.translation_mode;

        let default_options = TranslateOptions {
            mode: self.translation_mode,
            text_kind: self.default_text_kind,
            paragraph_strategy: self.default_paragraph_strategy,
            strategy: None,
            context_id: None,
        };
        let mut coordinator = Coordinator::new(Arc::new(self.registry), self.config);
        coordinator.bind_resource_fingerprint(fingerprint.join(";"));
        Ok(Translator {
            coordinator,
            default_options,
            budget,
            _reservations: reservations,
            #[cfg(feature = "resource")]
            _leases: self.leases,
            #[cfg(feature = "nmt-bergamot")]
            nmt_engines: self.nmt_engines,
            #[cfg(feature = "nmt-ort")]
            ort_engines: self.ort_engines,
        })
    }
}

#[cfg(feature = "rules")]
fn sha256_file(path: &std::path::Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// 常用类型的预导入。
pub mod prelude {
    pub use crate::{
        TargetTermGuard, TermPolicy, TermViolation, TranslateOutput, TranslationDiagnostics,
        Translator,
    };
    pub use translator_types::{
        Capability, LanguageTag, LookupRequest, TranslateRequest, TranslationError,
        TranslationErrorCode, TranslationMode, TranslationPolicy, TranslationResult,
    };
}
