//! 本地 NMT 引擎（真实 Bergamot）——`TranslationEngine` 的实现。
//!
//! 这一层是 **P3.1-E 的关键接线**：`translator-api` 只认识
//! [`translator_plugin_api::TranslationEngine`]，不认识 Bergamot、不认识进程、不认识帧协议。
//!
//! ```text
//! Translator::translate
//!   → LocalNmtEngine (本 crate)
//!      → ProtectionPlan（translator-core，纯 Rust）
//!      → ProcessTransport（translator-runtime，进程隔离）
//!         → nmt-worker → bergamot
//!      → 严格还原（失败即 IntegrityFailure）
//! ```
//!
//! 三条硬约束：
//! 1. **不静默降级**：本地失败就返回错误，绝不改走远程（ADR-004）；
//! 2. **不静默坏文本**：受保护片段丢失/重复 → `IntegrityFailure`，不返回"看起来正常"的译文；
//! 3. **不在 API 里泄漏实现**：不出现 pid、socket、C++ 类型；只暴露 `ModelIdentity`。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use translator_core::plan::ResolvedExecutionPlan;
use translator_core::plan_cache::{PlanCache, PlanCacheKey, PlanCacheStats};
use translator_core::protection::{ProtectionPlan, STRATEGY_VERSION};
// 宿主选保护策略要能拿到这个类型的名字，因此在 crate 根部重新导出。
pub use translator_core::protection::ProtectionMode;
// 平台门禁与运行时同源：引擎能不能用，取决于 `translator-runtime` 的证据表。
use translator_plugin_api::BoxFuture;
use translator_plugin_api::TranslationEngine;
use translator_runtime::model::{
    sha256_file, FileRole, FileSpec, ModelInfo, ModelManifest, TranslationJob,
};
pub use translator_runtime::platform;
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::WorkerConfig;
use translator_types::{
    CancelToken, Capability, EngineDescriptor, LanguageTag, PluginApiVersion, TranslateRequest,
    TranslationError, TranslationErrorCode, TranslationResult,
};

/// 引擎 id（进入诊断与缓存键）。改实现语义必须换 id。
pub const ENGINE_ID: &str = "nmt.bergamot.cli";

/// native 稳态 RSS 实测值（`docs/15` §2.4：341.9 MiB，1 线程 beam=1）。
///
/// 用途：内存准入的**下界**估计。它不等于"整机会多占这么多物理内存"
/// （共享库页、页缓存都会被重复计入），也不承诺是上限。
pub const NATIVE_STEADY_BYTES: u64 = 358_507_315; // ≈ 341.9 MiB

/// native 峰值（VmHWM）实测值（`docs/15` §2.4：413.8 MiB）。
pub const NATIVE_PEAK_BYTES: u64 = 433_867_571; // ≈ 413.8 MiB

/// 加载/卸载状态：真实模型的生命周期必须能被 SDK 侧观测到（`docs/17` §5）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    Unloaded,
    Loaded {
        identity: ModelIdentity,
        snapshot_bytes: u64,
        preparation_ms: u64,
        /// 已加载多久（毫秒）。
        uptime_ms: u64,
    },
}

impl LoadState {
    pub fn is_loaded(&self) -> bool {
        matches!(self, Self::Loaded { .. })
    }
}

/// 模型文件清单：与 `fixtures/nmt/models.lock.json` 一致。
const MODEL_FILES: [(FileRole, &str); 5] = [
    (FileRole::Model, "model.enzh.intgemm.alphas.bin"),
    (FileRole::SourceVocab, "srcvocab.enzh.spm"),
    (FileRole::TargetVocab, "trgvocab.enzh.spm"),
    (FileRole::LexicalShortlist, "lex.50.50.enzh.s2t.bin"),
    (FileRole::SplitPrefix, "nonbreaking_prefix.en"),
];

/// 引擎配置。全部来自宿主显式传入，**不读隐含默认路径**。
#[derive(Clone, Debug)]
pub struct BergamotNmtConfig {
    /// `nmt-worker` 可执行文件
    pub worker_binary: PathBuf,
    /// `bergamot` native 可执行文件
    pub native_binary: PathBuf,
    /// 模型目录（四文件 + 可选 ssplit 前缀表）
    pub model_dir: PathBuf,
    /// 快照根目录（需要可写；快照在 unload 时删除）
    pub snapshot_root: PathBuf,
    pub source: LanguageTag,
    pub target: LanguageTag,
    pub cpu_threads: u16,
    pub beam_size: u8,
    /// 保护策略：默认分段（安全），标记策略为可选
    pub protection: ProtectionMode,
    /// native 构建标识（Bergamot commit），进执行计划
    pub runtime_build: String,
    /// 单次请求默认超时
    pub request_timeout: Duration,
    /// 结果缓存容量（条）。0 = 关闭缓存。键是执行计划摘要（ADR-038）。
    pub cache_capacity: usize,
    /// 空闲多久后允许回收模型（`None` = 不自动回收，只能显式 unload）。
    pub idle_ttl: Option<Duration>,
}

impl BergamotNmtConfig {
    /// 显式构造：三个路径必须给出，其余取保守默认（1 线程、beam 1、分段保护、15 s 超时）。
    pub fn new(
        worker_binary: impl Into<PathBuf>,
        native_binary: impl Into<PathBuf>,
        model_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            worker_binary: worker_binary.into(),
            native_binary: native_binary.into(),
            model_dir: model_dir.into(),
            snapshot_root: std::env::temp_dir().join("nmt-snapshots"),
            source: LanguageTag::new("en").expect("en 是合法标签"),
            target: LanguageTag::new("zh-Hans").expect("zh-Hans 是合法标签"),
            cpu_threads: 1,
            beam_size: 1,
            protection: ProtectionMode::Segment,
            runtime_build: "unknown".to_string(),
            request_timeout: Duration::from_secs(15),
            cache_capacity: 256,
            idle_ttl: None,
        }
    }

    /// 从环境变量读一份常见配置（CLI 与集成测试用）。
    ///
    /// 认的三个变量：`NMT_WORKER_BIN`（可选，默认取同目录的 `nmt-worker`）、
    /// `NMT_BERGAMOT_BIN`、`NMT_MODEL_DIR`；`NMT_SNAPSHOT_ROOT` 可选。
    pub fn from_env() -> Result<Self, TranslationError> {
        let native = std::env::var("NMT_BERGAMOT_BIN").map_err(|_| {
            TranslationError::new(
                TranslationErrorCode::RuntimeInitFailed,
                "缺少 NMT_BERGAMOT_BIN（真实模型需要显式指定 native 可执行文件）",
            )
        })?;
        let model_dir = std::env::var("NMT_MODEL_DIR").map_err(|_| {
            TranslationError::new(
                TranslationErrorCode::RuntimeInitFailed,
                "缺少 NMT_MODEL_DIR（真实模型目录）",
            )
        })?;
        let worker = std::env::var("NMT_WORKER_BIN").unwrap_or_else(|_| "nmt-worker".to_string());
        let mut config = Self::new(worker, native, model_dir);
        if let Ok(root) = std::env::var("NMT_SNAPSHOT_ROOT") {
            config.snapshot_root = PathBuf::from(root);
        }
        Ok(config)
    }
}

/// 已经加载的模型身份（给诊断/CLI 展示用，不含 pid/socket）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub engine_id: String,
    pub model_revision: String,
    pub runtime: Option<String>,
    pub runtime_build: Option<String>,
    pub threads: u16,
    pub beam_size: u8,
}

/// 本地 NMT 引擎。
pub struct BergamotNmtEngine {
    config: BergamotNmtConfig,
    transport: ProcessTransport,
    manifest: ModelManifest,
    identity: Mutex<Option<ModelIdentity>>,
    load_gate: Mutex<()>,
    /// 执行计划摘要作键的结果缓存（`docs/17` §4.1.1）。
    cache: PlanCache,
    /// 加载时算好的执行计划摘要：让缓存键在 unload 之后仍然可用（不必为了查缓存而重载模型）。
    plan_digest: Mutex<Option<String>>,
    /// 最近一次翻译/加载的时刻，供空闲回收判断。
    last_used: Mutex<Instant>,
    /// 加载完成时刻（`ModelIdentity` 存在时才有意义）。
    loaded_at: Mutex<Option<Instant>>,
}

impl BergamotNmtEngine {
    /// 构造引擎（**不加载模型**；第一次 `translate` 或显式 `load()` 才加载）。
    pub fn new(config: BergamotNmtConfig) -> Result<Self, TranslationError> {
        let manifest = build_manifest(&config.model_dir)?;
        let transport = ProcessTransport::new(
            WorkerConfig::new(&config.worker_binary)
                .with_args([
                    "--bergamot".to_string(),
                    "--native".to_string(),
                    config.native_binary.display().to_string(),
                    "--runtime-build".to_string(),
                    config.runtime_build.clone(),
                ])
                .with_translate_timeout(config.request_timeout)
                .with_load_timeout(Duration::from_secs(30)),
            1,
        );
        Ok(Self {
            config: config.clone(),
            transport,
            manifest,
            identity: Mutex::new(None),
            load_gate: Mutex::new(()),
            cache: PlanCache::new(config.cache_capacity),
            plan_digest: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            loaded_at: Mutex::new(None),
        })
    }

    /// 显式加载（幂等）。CLI 的 `--warmup` 与首次翻译都会走到这里。
    pub fn load(&self) -> Result<ModelIdentity, TranslationError> {
        let _gate = self.load_gate.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(identity) = self
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(identity);
        }
        std::fs::create_dir_all(&self.config.snapshot_root).map_err(|e| {
            TranslationError::new(
                TranslationErrorCode::RuntimeInitFailed,
                format!(
                    "无法创建快照根 {}：{e}",
                    self.config.snapshot_root.display()
                ),
            )
        })?;
        let info = self
            .transport
            .load_from_manifest(
                &self.manifest,
                &self.config.snapshot_root,
                self.config.cpu_threads,
                self.config.beam_size,
            )
            .map_err(to_translation_error)?;
        let identity = identity_of(&info);
        *self.identity.lock().unwrap_or_else(|e| e.into_inner()) = Some(identity.clone());
        *self.loaded_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        // 计划摘要在这里算一次，之后缓存键直接复用（unload 也不清掉）
        let resolved = self.plan(&identity, self.config.protection);
        *self.plan_digest.lock().unwrap_or_else(|e| e.into_inner()) = Some(resolved.digest());
        Ok(identity)
    }

    /// 卸载模型（保留 worker 进程，native 退出）。
    pub fn unload(&self) -> Result<(), TranslationError> {
        self.transport.unload().map_err(to_translation_error)?;
        *self.identity.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.loaded_at.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // 注意：**不清** plan_digest。模型内容没变，摘要就不该变；
        // 清掉它只会让"卸载后重查同一句"白跑一次加载。
        Ok(())
    }

    /// 当前加载状态（给 SDK 侧观测用，不含 pid/socket）。
    pub fn state(&self) -> LoadState {
        let identity = self
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(identity) = identity else {
            return LoadState::Unloaded;
        };
        let metrics = self.transport.metrics();
        let uptime_ms = self
            .loaded_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        LoadState::Loaded {
            identity,
            snapshot_bytes: metrics.snapshot_bytes.unwrap_or(0),
            preparation_ms: metrics.model_preparation_ms.unwrap_or(0),
            uptime_ms,
        }
    }

    /// 缓存统计（命中/未命中/条目/淘汰）。
    pub fn cache_stats(&self) -> PlanCacheStats {
        self.cache.stats()
    }

    /// 清空结果缓存（诊断/测试用；换模型时不需要，键里已经含计划摘要）。
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// 已空闲多久。
    pub fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }

    /// 空闲超过 TTL 时卸载模型（`docs/17` §4.1.8）。返回是否真的卸了。
    ///
    /// **有在飞请求时一律不动**：宁可多占一会儿内存，也不把正在用的 native 拆掉。
    pub fn evict_if_idle(&self, ttl: Duration) -> Result<bool, TranslationError> {
        if self.transport.active_requests() > 0 {
            return Ok(false);
        }
        if !self.state().is_loaded() {
            return Ok(false);
        }
        if self.idle_for() < ttl {
            return Ok(false);
        }
        self.unload()?;
        Ok(true)
    }

    /// 按配置里的 `idle_ttl` 尝试回收（宿主可在自己的定时器里调）。
    pub fn evict_if_idle_configured(&self) -> Result<bool, TranslationError> {
        match self.config.idle_ttl {
            Some(ttl) => self.evict_if_idle(ttl),
            None => Ok(false),
        }
    }

    /// 内存准入估计（下界，不是上限；见 [`NATIVE_STEADY_BYTES`]）。
    pub fn memory_footprint_bytes(&self) -> u64 {
        NATIVE_STEADY_BYTES + self.manifest.files.values().map(|f| f.bytes).sum::<u64>()
    }

    pub fn identity(&self) -> Option<ModelIdentity> {
        self.identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn config(&self) -> &BergamotNmtConfig {
        &self.config
    }

    /// 当前这次执行对应的计划（缓存键的一半；本 crate 不持有缓存）。
    pub fn plan(
        &self,
        identity: &ModelIdentity,
        protection: ProtectionMode,
    ) -> ResolvedExecutionPlan {
        let mut file_digests = std::collections::BTreeMap::new();
        for (role, spec) in &self.manifest.files {
            file_digests.insert(role.as_str().to_string(), spec.sha256.clone());
        }
        ResolvedExecutionPlan {
            engine_id: ENGINE_ID.to_string(),
            runtime: identity.runtime.clone().unwrap_or_else(|| "unknown".into()),
            runtime_build: identity
                .runtime_build
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            model_revision: identity.model_revision.clone(),
            file_digests,
            source: self.config.source.as_str().to_string(),
            target: self.config.target.as_str().to_string(),
            beam_size: identity.beam_size,
            cpu_threads: identity.threads,
            protection: protection.as_str().to_string(),
            rules_version: None,
            glossary_digest: None,
            pipeline_version: 2,
            privacy_domain: "local".to_string(),
        }
    }

    fn translate_blocking(
        &self,
        req: &TranslateRequest,
        cancel: Option<&CancelToken>,
    ) -> Result<TranslationResult, TranslationError> {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(TranslationError::cancelled());
        }
        let started = Instant::now();
        if req.text.trim().is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "空文本不进入 NMT（避免把空串当译文返回）",
            ));
        }
        // 缓存：键 = 执行计划摘要 + 目标语言 + 原文摘要（ADR-038）。
        // 计划摘要里含模型五文件摘要与解码参数，所以"模型内容变了"必然是未命中（LC-09）。
        //
        // 摘要来自上一次加载；**卸载之后仍然有效**——这样空闲回收不会把结果一起丢掉，
        // 也让"命中缓存"不必以"重新加载 342 MiB 模型"为代价。
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        if let Some(digest) = self
            .plan_digest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            let key = PlanCacheKey::new(digest, self.config.target.as_str(), &req.text);
            if let Some(mut hit) = self.cache.get(&key) {
                // 命中也要如实报告耗时：这里的 elapsed 是"取缓存"的耗时，不是原翻译耗时
                hit.elapsed_ms = started.elapsed().as_millis() as u64;
                return Ok(hit);
            }
        }

        let identity = self.load()?;
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(TranslationError::cancelled());
        }
        let deadline = req
            .timeout_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms))
            .unwrap_or_else(|| Instant::now() + self.config.request_timeout);
        let digest = self
            .plan_digest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("load() 之后必然有计划摘要");
        let key = PlanCacheKey::new(digest, self.config.target.as_str(), &req.text);

        let plan = match self.config.protection {
            ProtectionMode::Segment => ProtectionPlan::build(&req.text),
            ProtectionMode::Marker => ProtectionPlan::build_marker(&req.text),
        };

        let (text, warnings) = match plan.mode() {
            ProtectionMode::Segment => {
                let mut translated = Vec::new();
                for part in plan.plain_texts() {
                    if cancel.is_some_and(CancelToken::is_cancelled) {
                        return Err(TranslationError::cancelled());
                    }
                    if part.trim().is_empty() {
                        translated.push(part.to_string());
                        continue;
                    }
                    let reply = self
                        .transport
                        .translate_with_deadline_and_cancel(
                            &job_of(part, &self.config.target),
                            deadline,
                            cancel,
                        )
                        .map_err(to_translation_error)?;
                    translated.push(reply.text);
                }
                (plan.assemble(&translated)?, Vec::new())
            }
            ProtectionMode::Marker => {
                let reply = self
                    .transport
                    .translate_with_deadline_and_cancel(
                        &job_of(plan.masked(), &self.config.target),
                        deadline,
                        cancel,
                    )
                    .map_err(to_translation_error)?;
                (plan.restore(&reply.text)?, Vec::new())
            }
        };

        let result = TranslationResult {
            text,
            source: self.config.source.clone(),
            target: self.config.target.clone(),
            engine_id: ENGINE_ID.to_string(),
            model_id: Some(identity.model_revision),
            elapsed_ms: started.elapsed().as_millis() as u64,
            warnings,
        };
        // 只有成功结果入缓存：失败被缓存成成功是最坏的一类 bug
        self.cache.put(key, result.clone());
        Ok(result)
    }

    /// 诊断用的保护统计（类别与数量，**不含原文内容**）。
    pub fn describe_protection(&self, text: &str) -> Vec<(&'static str, usize)> {
        let plan = ProtectionPlan::build(text);
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for item in plan.items() {
            match counts.iter_mut().find(|(kind, _)| *kind == item.kind_str()) {
                Some((_, count)) => *count += 1,
                None => counts.push((item.kind_str(), 1)),
            }
        }
        counts
    }
}

impl TranslationEngine for BergamotNmtEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: ENGINE_ID,
            capabilities: &[Capability::Nmt],
            // 真实模型优先于 Mock；但路由不会因此绕过语言对/资源检查
            priority: 100,
            plugin_api: PluginApiVersion::CURRENT,
            license: "MPL-2.0 (runtime) / model weights pending",
        }
    }

    fn supports(&self, source: &LanguageTag, target: &LanguageTag) -> bool {
        source == &self.config.source && target == &self.config.target
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move { self.translate_blocking(req, None) })
    }

    fn translate_cancellable<'a>(
        &'a self,
        req: &'a TranslateRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move { self.translate_blocking(req, Some(cancel)) })
    }
}

fn job_of(text: &str, target: &LanguageTag) -> TranslationJob {
    TranslationJob {
        text: text.to_string(),
        target: target.as_str().to_string(),
    }
}

fn identity_of(info: &ModelInfo) -> ModelIdentity {
    ModelIdentity {
        engine_id: info.engine_id.clone(),
        model_revision: info.model_revision.clone(),
        runtime: info.runtime.clone(),
        runtime_build: info.runtime_build.clone(),
        threads: info.threads,
        beam_size: info.beam_size,
    }
}

fn to_translation_error(err: translator_runtime::RuntimeError) -> TranslationError {
    TranslationError::new(err.code(), err.to_string())
}

/// 构造模型清单：文件必须齐全（`ModelManifest::verify` 在加载时再核一次摘要）。
pub fn build_manifest(model_dir: &Path) -> Result<ModelManifest, TranslationError> {
    let mut files = std::collections::BTreeMap::new();
    for (role, name) in MODEL_FILES {
        let path = model_dir.join(name);
        if !path.exists() {
            if role.required() {
                return Err(TranslationError::new(
                    TranslationErrorCode::ResourceNotInstalled,
                    format!("模型文件缺失：{}", path.display()),
                ));
            }
            continue;
        }
        let (bytes, sha256) = sha256_file(&path).map_err(|e| {
            TranslationError::new(e.code(), format!("读取 {} 失败", path.display()))
        })?;
        files.insert(
            role,
            FileSpec {
                role,
                path,
                bytes,
                sha256,
            },
        );
    }
    let manifest = ModelManifest {
        package_id: "nmt.enzh.mozilla".to_string(),
        version: "3.0".to_string(),
        files,
    };
    manifest
        .verify()
        .map_err(|e| TranslationError::new(e.code(), e.to_string()))?;
    Ok(manifest)
}

/// 保护策略版本 → 诊断串（给 CLI/日志用，不含原文）。
pub fn protection_label() -> &'static str {
    STRATEGY_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用引擎：不读模型文件、不 spawn 进程（manifest 为空）。
    fn stub_engine(protection: ProtectionMode) -> BergamotNmtEngine {
        let mut config = BergamotNmtConfig::new("/bin/true", "/bin/true", "/tmp/does-not-matter");
        config.protection = protection;
        BergamotNmtEngine {
            cache: PlanCache::new(config.cache_capacity),
            plan_digest: Mutex::new(None),
            config,
            transport: ProcessTransport::new(WorkerConfig::new("/bin/true"), 1),
            manifest: ModelManifest {
                package_id: "x".into(),
                version: "1".into(),
                files: Default::default(),
            },
            identity: Mutex::new(None),
            load_gate: Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
            loaded_at: Mutex::new(None),
        }
    }

    #[test]
    fn 引擎声明本地nmt能力且只支持配置的语言对() {
        let engine = stub_engine(ProtectionMode::Segment);
        let descriptor = engine.descriptor();
        assert_eq!(descriptor.id, ENGINE_ID);
        assert!(descriptor.supports(Capability::Nmt));
        assert!(engine.supports(
            &LanguageTag::new("en").unwrap(),
            &LanguageTag::new("zh-Hans").unwrap()
        ));
        assert!(!engine.supports(
            &LanguageTag::new("en").unwrap(),
            &LanguageTag::new("ja").unwrap()
        ));
    }

    #[test]
    fn 缺少必需文件时构造清单失败() {
        let dir = tempfile::tempdir().unwrap();
        let err = build_manifest(dir.path()).unwrap_err();
        assert_eq!(err.code, TranslationErrorCode::ResourceNotInstalled);
    }

    #[test]
    fn 保护统计只给类别与数量() {
        let engine = stub_engine(ProtectionMode::Segment);
        let stats = engine.describe_protection("Hi {a} see https://x.example/a");
        assert!(stats.contains(&("template_var", 1)), "{stats:?}");
        assert!(stats.contains(&("url", 1)), "{stats:?}");
    }

    #[test]
    fn 未加载时状态是unloaded且空闲回收是空操作() {
        let engine = stub_engine(ProtectionMode::Segment);
        assert_eq!(engine.state(), LoadState::Unloaded);
        assert!(!engine.evict_if_idle(Duration::ZERO).unwrap());
        assert!(!engine.evict_if_idle_configured().unwrap());
    }

    #[test]
    fn 计划摘要把模型文件与解码参数分开() {
        let engine = stub_engine(ProtectionMode::Segment);
        let identity = ModelIdentity {
            engine_id: ENGINE_ID.into(),
            model_revision: "m@1#abc".into(),
            runtime: Some("bergamot-cli".into()),
            runtime_build: Some("9271618".into()),
            threads: 1,
            beam_size: 1,
        };
        let a = engine.plan(&identity, ProtectionMode::Segment);
        let b = engine.plan(&identity, ProtectionMode::Marker);
        assert_ne!(a.digest(), b.digest(), "保护策略必须进摘要");
        let mut other = identity.clone();
        other.beam_size = 4;
        assert_ne!(
            a.digest(),
            engine.plan(&other, ProtectionMode::Segment).digest()
        );
    }

    #[test]
    fn 内存估计是实测下界() {
        let engine = stub_engine(ProtectionMode::Segment);
        assert!(engine.memory_footprint_bytes() >= NATIVE_STEADY_BYTES);
        // 峰值必须大于稳态：两个常数是实测值，写反了说明抄错了
        const { assert!(NATIVE_PEAK_BYTES > NATIVE_STEADY_BYTES) };
    }
}
