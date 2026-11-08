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
    NmtOutput, PluginApiVersion, ProviderDescriptor, ResourceLimits, TranslateRequest,
    TranslationError, TranslationErrorCode, TranslationResult,
};

/// 插件异步返回类型。
///
/// 用 boxed future 而非 `async fn in trait`，是为了在 MSRV 与对象安全性之间
/// 取得可控的平衡（`async fn in trait` 在 `dyn` 场景仍有额外限制）。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ============================================================
// 引擎契约
// ============================================================

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
            db.priority.cmp(&da.priority).then_with(|| da.id.cmp(db.id))
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
