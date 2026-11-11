//! 插件契约：翻译引擎、词典提供者、推理后端，以及插件注册表。
//!
//! 对应设计文档 `docs/01_翻译SDK完整开发设计.md` §5「插件契约、注册与替换机制」。
//!
//! 设计原则（原文摘录）：
//! - 插件首先是**能力 Trait + 编译期依赖注入**，不急于动态加载 Rust `.so`；
//! - 原生库句柄未必 `Sync`，不能盲目包进 `Arc<dyn LoadedModel + Send + Sync>`；
//! - 兼容关系由 `CompatibilityReport` 显式声明，**不允许通过字符串猜测**。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use translator_types::{
    BackendDescriptor, CancelToken, Capability, CapabilityInfo, CompatibilityReport,
    DictionaryEntry, EngineDescriptor, LanguageTag, LookupRequest, ModelManifest, NmtInput,
    NmtOutput, PluginApiVersion, ProviderDescriptor, ResourceLimits, TermConstraint, TextKind,
    TokenUsage, TranslateRequest, TranslationError, TranslationErrorCode, TranslationMode,
    TranslationResult,
};

/// 插件异步返回类型。
///
/// 用 boxed future 而非 `async fn in trait`，是为了在 MSRV 与对象安全性之间
/// 取得可控的平衡（`async fn in trait` 在 `dyn` 场景仍有额外限制）。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ============================================================
// 引擎契约
// ============================================================

// ============================================================
// 执行任务（引擎看到的"这一次翻译"）
// ============================================================

/// 交给引擎的一次翻译任务。
///
/// 与 [`TranslateRequest`] 的区别：这是**协调器内部**的执行契约，带上了
/// 协调器已经算好的东西（受保护文本、命中的术语约束、截止时间）。
///
/// 为什么不让引擎直接读 `TranslateRequest`：
///
/// - 术语约束必须在**生成之前**进入提示词（§6.1）。若只通过 `TranslateRequest`
///   传递，就得给公开结构加字段，等于把内部策略暴露给宿主。
/// - 截止时间必须是绝对时刻，否则每次重试都会重新计时，整体超时失效。
pub struct TranslationJob<'a> {
    /// 要翻译的文本。可能包含 `⟦P1⟧` 形式的保护标记。
    pub text: &'a str,
    /// 送进模型前的源文（未加保护标记），供后端做术语/上下文判断。
    pub original_text: &'a str,
    pub source: &'a LanguageTag,
    pub target: &'a LanguageTag,
    pub mode: TranslationMode,
    pub text_kind: TextKind,
    /// 本次源文**实际命中**的术语约束（已按优先级排好，可能为空）。
    pub glossary: &'a [TermConstraint],
    /// 文本中出现的保护标记（如 `⟦P1⟧`）。逐字告知模型比"让它自己发现"可靠得多。
    pub placeholders: &'a [String],
    /// 上一次尝试失败时的纠正提示（内容完整性重试时使用）。
    pub retry_hint: Option<&'a str>,
    /// 绝对截止时间：引擎必须在此时刻前返回或报 `Timeout`。
    pub deadline: Option<std::time::Instant>,
    /// 取消令牌。可在任意阶段检查，取消后不得继续重试。
    pub cancel: Option<&'a CancelToken>,
    /// 调用方要求的输出上限（token）。`None` 表示用后端默认值。
    pub max_output_tokens: Option<u32>,
}

impl std::fmt::Debug for TranslationJob<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不打印原文：日志里不得出现用户文本（设计文档 §4.1）。
        f.debug_struct("TranslationJob")
            .field("text_chars", &self.text.chars().count())
            .field("source", &self.source.as_str())
            .field("target", &self.target.as_str())
            .field("mode", &self.mode.as_str())
            .field("glossary", &self.glossary.len())
            .field("placeholders", &self.placeholders.len())
            .field("retry_hint", &self.retry_hint.is_some())
            .field("deadline", &self.deadline.is_some())
            .finish()
    }
}

/// 引擎产出的结果。
///
/// 比 [`TranslationResult`] 多了后端身份与用量：协调器要拿 `model_revision`
/// 参与缓存身份，拿 `usage` 做成本统计。
#[derive(Clone, Debug)]
pub struct EngineOutput {
    pub text: String,
    pub model_id: Option<String>,
    /// 模型/部署的**真实**版本标识。远程服务在别名不变时也可能替换模型，
    /// 因此不能用 `model_id` 当作永久缓存身份（§11.2）。
    pub model_revision: Option<String>,
    pub usage: Option<TokenUsage>,
    pub warnings: Vec<translator_types::TranslationWarning>,
}

impl EngineOutput {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            model_id: None,
            model_revision: None,
            usage: None,
            warnings: Vec::new(),
        }
    }
}

/// 引擎的模型身份：参与缓存键的"这到底是哪个模型/哪个提示词"。
///
/// 单独成一个结构而不是一个拼接字符串，是为了让缓存键的每个维度都可读：
/// 出问题时能直接看出"是模型换了还是提示词换了"（§11.2）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub model_id: String,
    /// 部署版本。远程服务可能在别名不变时替换模型，因此这一项**不能**留空。
    pub model_revision: Option<String>,
    pub prompt_version: String,
    /// 温度/输出上限/扩展字段等的稳定摘要。
    pub generation_config_hash: String,
}

/// 翻译引擎。词典、本地 NMT、远程 LLM 都实现此 trait。
pub trait TranslationEngine: Send + Sync {
    fn descriptor(&self) -> EngineDescriptor;

    /// 声明是否支持某语言对。**不得**在这里做耗时的模型加载。
    fn supports(&self, source: &LanguageTag, target: &LanguageTag) -> bool;

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>>;

    /// 可取消版本。默认实现忽略令牌并调用 [`TranslationEngine::translate`]；
    /// 能在执行中观察令牌的后端（如进程 worker）应覆盖它。
    ///
    /// 契约：已在取消状态时**必须尽快**返回 `Cancelled`，不得产生新任务。
    fn translate_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        let _ = cancel;
        self.translate(req)
    }

    /// 带术语约束与截止时间的执行入口。协调器走这条路径。
    ///
    /// 默认实现把 job 退化成一个 [`TranslateRequest`] 再调用 [`Self::translate`]，
    /// 因此已有引擎不需要改动；能利用术语约束的 LLM 后端应覆盖它。
    fn translate_job<'a>(
        &'a self,
        job: &'a TranslationJob<'a>,
    ) -> BoxFuture<'a, Result<EngineOutput, TranslationError>> {
        let request = TranslateRequest {
            text: job.text.to_string(),
            source: Some(job.source.clone()),
            target: job.target.clone(),
            ..TranslateRequest::new(job.text.to_string(), job.target.clone())
        };
        Box::pin(async move {
            let result = self.translate(&request).await?;
            Ok(EngineOutput {
                text: result.text,
                model_id: result.model_id,
                model_revision: None,
                usage: None,
                warnings: result.warnings,
            })
        })
    }

    /// 模型身份（可选）。能提供的后端应当提供：缓存键需要它。
    fn model_identity(&self) -> Option<ModelIdentity> {
        None
    }

    /// 影响译文的执行身份（模型/词表/解码/保护策略的稳定摘要）。
    ///
    /// 默认 `None`：协调器只用 engine_id + 资源指纹。能提供
    /// `ResolvedExecutionPlan` 的后端应覆盖它，避免"同 engine_id 不同模型内容"
    /// 命中旧缓存（P8.3 / EP-01）。
    fn cache_identity(&self) -> Option<String> {
        None
    }

    /// 引擎是否**自己**完成受保护跨度的切分与还原（结构化保护）。
    ///
    /// 默认 `false`：协调器会把 URL / 邮箱 / `{变量}` / 反引号代码替换成
    /// `⟦Pn⟧` 标记，再交给引擎翻译，最后按标记还原。这是 P0 的通用做法。
    ///
    /// 真实 NMT 后端（Bergamot / ORT）实现的是 `ProtectionPlan` 分段策略：
    /// 受保护片段**根本不进模型**。如果协调器先把原文换成标记，模型看到的
    /// 就是 `⟦P0⟧` 这种它不认识的符号，P3.0 已实测标记会被吞掉——于是
    /// 同一层保护被做了两次，第二次反而破坏了第一次。这些后端必须覆盖本方法
    /// 返回 `true`，让协调器把**原始文本**交给它。
    fn handles_protected_spans(&self) -> bool {
        false
    }
}

/// 词典/语言学提供者。与翻译引擎分开建模，
/// 因为「查词」与「整句翻译」是两种不同能力（设计文档 ADR-003）。
pub trait DictionaryProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    fn lookup<'a>(
        &'a self,
        req: &'a LookupRequest,
    ) -> BoxFuture<'a, Result<Vec<DictionaryEntry>, TranslationError>>;
}

/// 推理后端。Bergamot / ncnn / ONNX Runtime 都实现此 trait。
pub trait InferenceBackend: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    /// 显式声明模型兼容性。**不允许**通过模型名字符串猜测。
    fn supports_model(&self, model: &ModelManifest) -> CompatibilityReport;

    fn load<'a>(
        &'a self,
        model: &'a ModelManifest,
        limits: &'a ResourceLimits,
    ) -> BoxFuture<'a, Result<Arc<dyn LoadedModel>, TranslationError>>;
}

/// 已加载的模型句柄。
///
/// ⚠️ 实现提醒：真实原生句柄未必 `Sync`。若后端不满足线程安全，
/// 应当用专属 worker / actor 包装，让本 trait 只作为消息代理，
/// 而不是把裸 C++ 指针塞进来（设计文档 §5.1）。
pub trait LoadedModel: Send + Sync {
    fn infer<'a>(
        &'a self,
        input: &'a NmtInput,
    ) -> BoxFuture<'a, Result<NmtOutput, TranslationError>>;

    fn estimated_bytes(&self) -> u64;
}

// ============================================================
// 注册表
// ============================================================

/// 插件注册表。P0 为编译期注册；动态加载留给 v0.3。
#[derive(Default)]
pub struct PluginRegistry {
    engines: Vec<Arc<dyn TranslationEngine>>,
    providers: Vec<Arc<dyn DictionaryProvider>>,
    backends: Vec<Arc<dyn InferenceBackend>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册引擎。会校验插件 API 主版本一致与 id 唯一。
    pub fn register_engine(
        &mut self,
        engine: Arc<dyn TranslationEngine>,
    ) -> Result<(), TranslationError> {
        let desc = engine.descriptor();

        if !desc.plugin_api.accepts(PluginApiVersion::CURRENT) {
            return Err(TranslationError::new(
                TranslationErrorCode::CapabilityUnavailable,
                format!(
                    "插件 {} 的 API 版本 {} 与本 SDK 的 {} 不兼容",
                    desc.id,
                    desc.plugin_api,
                    PluginApiVersion::CURRENT
                ),
            ));
        }

        if self.engines.iter().any(|e| e.descriptor().id == desc.id) {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!("引擎 id 重复：{}", desc.id),
            ));
        }

        self.engines.push(engine);
        Ok(())
    }

    pub fn register_provider(
        &mut self,
        provider: Arc<dyn DictionaryProvider>,
    ) -> Result<(), TranslationError> {
        let desc = provider.descriptor();
        if !desc.plugin_api.accepts(PluginApiVersion::CURRENT) {
            return Err(TranslationError::new(
                TranslationErrorCode::CapabilityUnavailable,
                format!("词典插件 {} 的 API 版本不兼容", desc.id),
            ));
        }
        if self.providers.iter().any(|p| p.descriptor().id == desc.id) {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!("词典插件 id 重复：{}", desc.id),
            ));
        }
        self.providers.push(provider);
        Ok(())
    }

    pub fn register_backend(
        &mut self,
        backend: Arc<dyn InferenceBackend>,
    ) -> Result<(), TranslationError> {
        let desc = backend.descriptor();
        if !desc.plugin_api.accepts(PluginApiVersion::CURRENT) {
            return Err(TranslationError::new(
                TranslationErrorCode::CapabilityUnavailable,
                format!("推理后端 {} 的 API 版本不兼容", desc.id),
            ));
        }
        if self.backends.iter().any(|b| b.descriptor().id == desc.id) {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                format!("推理后端 id 重复：{}", desc.id),
            ));
        }
        self.backends.push(backend);
        Ok(())
    }

    pub fn engines(&self) -> &[Arc<dyn TranslationEngine>] {
        &self.engines
    }

    pub fn providers(&self) -> &[Arc<dyn DictionaryProvider>] {
        &self.providers
    }

    pub fn backends(&self) -> &[Arc<dyn InferenceBackend>] {
        &self.backends
    }

    pub fn find_engine(&self, id: &str) -> Option<Arc<dyn TranslationEngine>> {
        self.engines
            .iter()
            .find(|e| e.descriptor().id == id)
            .map(Arc::clone)
    }

    /// 返回具备某能力的引擎，按 `priority` 降序、id 升序排列（结果稳定）。
    ///
    /// `priority` **仅在可兼容插件之间**用于排序，不能绕过质量/内存要求。
    pub fn engines_with(&self, capability: Capability) -> Vec<Arc<dyn TranslationEngine>> {
        let mut found: Vec<Arc<dyn TranslationEngine>> = self
            .engines
            .iter()
            .filter(|e| e.descriptor().supports(capability))
            .map(Arc::clone)
            .collect();

        found.sort_by(|a, b| {
            let (da, db) = (a.descriptor(), b.descriptor());
            db.priority
                .cmp(&da.priority)
                .then_with(|| da.id.cmp(&db.id))
        });

        found
    }

    /// 对外暴露的能力清单。
    pub fn capability_infos(&self) -> Vec<CapabilityInfo> {
        self.engines
            .iter()
            .map(|e| {
                let d = e.descriptor();
                CapabilityInfo {
                    engine_id: d.id.to_string(),
                    capabilities: d.capabilities.to_vec(),
                    priority: d.priority,
                    license: d.license.to_string(),
                }
            })
            .collect()
    }
}

// ============================================================
// Mock 插件（feature = "mocks"）
// ============================================================

#[cfg(feature = "mocks")]
pub mod mocks;
