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
use translator_core::protection::{ProtectionPlan, STRATEGY_VERSION};
// 宿主选保护策略要能拿到这个类型的名字，因此在 crate 根部重新导出。
pub use translator_core::protection::ProtectionMode;
use translator_plugin_api::BoxFuture;
use translator_plugin_api::TranslationEngine;
use translator_runtime::model::{
    sha256_file, FileRole, FileSpec, ModelInfo, ModelManifest, TranslationJob,
};
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::WorkerConfig;
use translator_types::{
    Capability, EngineDescriptor, LanguageTag, PluginApiVersion, TranslateRequest,
    TranslationError, TranslationErrorCode, TranslationResult,
};

/// 引擎 id（进入诊断与缓存键）。改实现语义必须换 id。
pub const ENGINE_ID: &str = "nmt.bergamot.cli";

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
            config,
            transport,
            manifest,
            identity: Mutex::new(None),
            load_gate: Mutex::new(()),
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
        Ok(identity)
    }

    /// 卸载模型（保留 worker 进程，native 退出）。
    pub fn unload(&self) -> Result<(), TranslationError> {
        self.transport.unload().map_err(to_translation_error)?;
        *self.identity.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
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
    ) -> Result<TranslationResult, TranslationError> {
        let started = Instant::now();
        if req.text.trim().is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "空文本不进入 NMT（避免把空串当译文返回）",
            ));
        }
        let identity = self.load()?;
        let deadline = req
            .timeout_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms))
            .unwrap_or_else(|| Instant::now() + self.config.request_timeout);

        let plan = match self.config.protection {
            ProtectionMode::Segment => ProtectionPlan::build(&req.text),
            ProtectionMode::Marker => ProtectionPlan::build_marker(&req.text),
        };

        let (text, warnings) = match plan.mode() {
            ProtectionMode::Segment => {
                let mut translated = Vec::new();
                for part in plan.plain_texts() {
                    if part.trim().is_empty() {
                        translated.push(part.to_string());
                        continue;
                    }
                    let reply = self
                        .transport
                        .translate_with_deadline(&job_of(part, &self.config.target), deadline)
                        .map_err(to_translation_error)?;
                    translated.push(reply.text);
                }
                (plan.assemble(&translated)?, Vec::new())
            }
            ProtectionMode::Marker => {
                let reply = self
                    .transport
                    .translate_with_deadline(&job_of(plan.masked(), &self.config.target), deadline)
                    .map_err(to_translation_error)?;
                (plan.restore(&reply.text)?, Vec::new())
            }
        };

        Ok(TranslationResult {
            text,
            source: self.config.source.clone(),
            target: self.config.target.clone(),
            engine_id: ENGINE_ID.to_string(),
            model_id: Some(identity.model_revision),
            elapsed_ms: started.elapsed().as_millis() as u64,
            warnings,
        })
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
        Box::pin(async move { self.translate_blocking(req) })
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

    #[test]
    fn 引擎声明本地nmt能力且只支持配置的语言对() {
        let config = BergamotNmtConfig {
            worker_binary: PathBuf::from("/bin/true"),
            native_binary: PathBuf::from("/bin/true"),
            model_dir: PathBuf::from("/tmp/does-not-matter"),
            snapshot_root: PathBuf::from("/tmp"),
            source: LanguageTag::new("en").unwrap(),
            target: LanguageTag::new("zh-Hans").unwrap(),
            cpu_threads: 1,
            beam_size: 1,
            protection: ProtectionMode::Segment,
            runtime_build: "test".into(),
            request_timeout: Duration::from_secs(1),
        };
        let engine = BergamotNmtEngine {
            config: config.clone(),
            transport: ProcessTransport::new(WorkerConfig::new("/bin/true"), 1),
            manifest: ModelManifest {
                package_id: "x".into(),
                version: "1".into(),
                files: Default::default(),
            },
            identity: Mutex::new(None),
            load_gate: Mutex::new(()),
        };
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
        let engine = BergamotNmtEngine {
            config: BergamotNmtConfig {
                worker_binary: PathBuf::from("/bin/true"),
                native_binary: PathBuf::from("/bin/true"),
                model_dir: PathBuf::from("/tmp"),
                snapshot_root: PathBuf::from("/tmp"),
                source: LanguageTag::new("en").unwrap(),
                target: LanguageTag::new("zh-Hans").unwrap(),
                cpu_threads: 1,
                beam_size: 1,
                protection: ProtectionMode::Segment,
                runtime_build: "test".into(),
                request_timeout: Duration::from_secs(1),
            },
            transport: ProcessTransport::new(WorkerConfig::new("/bin/true"), 1),
            manifest: ModelManifest {
                package_id: "x".into(),
                version: "1".into(),
                files: Default::default(),
            },
            identity: Mutex::new(None),
            load_gate: Mutex::new(()),
        };
        let stats = engine.describe_protection("Hi {a} see https://x.example/a");
        assert!(stats.contains(&("template_var", 1)), "{stats:?}");
        assert!(stats.contains(&("url", 1)), "{stats:?}");
    }
}
