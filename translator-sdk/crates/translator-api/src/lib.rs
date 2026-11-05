//! 唯一受支持的对外入口。
//!
//! 设计文档 §0 的交付判据：
//! > 即使移除 Bergamot、ncnn、Mozilla 模型以及全部第三方词库，
//! > `translator-api` 仍应能以 Mock/空资源模式编译运行。
//!
//! 本 crate 因此**不依赖任何 native runtime、不依赖任何 HTTP 客户端、不依赖 SQLite**。

use std::sync::Arc;

use translator_core::{
    BudgetReservation, Coordinator, CoordinatorConfig, MemoryBudget, MemoryReport,
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

// 规则契约：即使不开 `rules` feature，宿主也可以自己实现 `TextRules` 注入。
pub use translator_core::rules::{NoRules, RuleOutcome, RulePreview, TextRules};
// 术语契约（P2.4）：同上，宿主可自己实现 `TargetTermGuard`。
pub use translator_core::terms::{
    NoTermGuard, TargetTermGuard, TermOutcome, TermPolicy, TermViolation,
};
// 受保护跨度（P2.4 起出现在诊断里，因此必须公开）
pub use translator_core::text::{ProtectedKind, ProtectedSpan};
// 诊断（P2.4）：`translate_with_diagnostics` 的返回类型。
pub use translator_core::diagnostics::{
    RuleTrace, TermTrace, TranslateOutput, TranslationDiagnostics,
};

/// 对外门面。构造后不可变，可跨线程共享（`Send + Sync`）。
pub struct Translator {
    coordinator: Coordinator,
    /// 内存预算池。记账凭据必须活到 `Translator` 结束，否则预留会被立刻归还。
    budget: Arc<MemoryBudget>,
    /// 已挂载资源的内存预留凭据（字段名带下划线：只用于保活，不直接读取）。
    _reservations: Vec<BudgetReservation>,
    /// 从资源仓库挂载时持有的租约：`Translator` 活着，磁盘目录就不会被删（P2.3）。
    #[cfg(feature = "resource")]
    _leases: Vec<translator_resource::ResourceLease>,
}

impl Translator {
    pub fn builder() -> TranslatorBuilder {
        TranslatorBuilder::new()
    }

    /// 翻译入口。
    pub async fn translate(
        &self,
        req: &TranslateRequest,
    ) -> Result<TranslationResult, TranslationError> {
        self.coordinator.translate(req).await
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
    pub fn memory_report(&self) -> MemoryReport {
        self.budget.report()
    }

    /// 释放资源。P0 无可释放资源，保留稳定签名供后续实现。
    pub async fn shutdown(&self) -> Result<(), TranslationError> {
        self.coordinator.clear_cache();
        Ok(())
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
    term_guard: Option<Arc<dyn TargetTermGuard>>,
    term_policy: TermPolicy,
    /// 峰值内存上限（字节）。`None` 表示不限制但仍记账。
    memory_limit: Option<u64>,
    /// 已登记的资源租约，`build()` 时移交给 `Translator`。
    #[cfg(feature = "resource")]
    leases: Vec<translator_resource::ResourceLease>,
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
            term_guard: None,
            term_policy: TermPolicy::default(),
            memory_limit: None,
            #[cfg(feature = "resource")]
            leases: Vec::new(),
        }
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
        match translator_rules::RuleEngine::load(path) {
            Ok(engine) => self.rules = Some(Arc::new(engine)),
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
        match translator_rules::terms::TargetTermTable::from_file(path) {
            Ok(table) => self.term_guard = Some(Arc::new(table)),
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
                                fingerprint.push(format!("{}@{}", manifest.id, manifest.version));
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

        if let Some(err) = self.errors.into_iter().next() {
            return Err(err);
        }

        if let Some(rules) = &self.rules {
            fingerprint.push(format!("{}@{}", rules.id(), rules.version()));
        }
        if let Some(guard) = &self.term_guard {
            // 术语表变化必须让旧译文失效：同一句话在不同术语表下结果不同
            fingerprint.push(format!("{}@{}", guard.id(), guard.version()));
        }
        self.config.rules = self.rules.take();
        self.config.term_guard = self.term_guard.take();
        self.config.term_policy = self.term_policy;

        let mut coordinator = Coordinator::new(Arc::new(self.registry), self.config);
        coordinator.bind_resource_fingerprint(fingerprint.join(";"));
        Ok(Translator {
            coordinator,
            budget,
            _reservations: reservations,
            #[cfg(feature = "resource")]
            _leases: self.leases,
        })
    }
}

/// 常用类型的预导入。
pub mod prelude {
    pub use crate::{
        TargetTermGuard, TermPolicy, TermViolation, TranslateOutput, TranslationDiagnostics,
        Translator,
    };
    pub use translator_types::{
        Capability, LanguageTag, LookupRequest, TranslateRequest, TranslationError,
        TranslationErrorCode, TranslationPolicy, TranslationResult,
    };
}
