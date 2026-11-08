//! # translator-nmt-ort
//!
//! P4 的第二本地 NMT 后端：**ONNX Runtime + opus-mt-en-zh（Marian int8）**。
//!
//! ## 它证明什么、不证明什么
//!
//! 证明的是**插件替换**：同一套 `TranslationEngine` 契约、同一个 `Translator::translate()`，
//! 换一个完全不同的 native 运行时（ONNX Runtime）照样能出真实译文，且 `translator-core` 的
//! 结构保护（`protect-seg-v1`）与路由逻辑一行都不用改。
//!
//! 不证明的是性能：这个候选模型与 Bergamot 那份权重**不是同一个模型**，
//! 因此只能做 B 类比较（`docs/23` §3.3），不能把差异归因于运行时。
//!
//! ## 三个来自可行性门禁的硬约束（`docs/24` §1.4）
//!
//! 1. **模型的 id 空间是 `vocab.json` 的联合词表，不是 SentencePiece 的 piece id**。
//!    所以本 crate 用 `tokenizer.json`（按联合词表构造的 Unigram），而不是直接调 `*.spm`。
//!    用错不会报错，只会输出通顺但错误的中文——门禁阶段就是这么被骗过一次。
//! 2. merged decoder 首步（`use_cache_branch=false`）的 **encoder 侧 K/V 必须给真实 `src_len`**，
//!    否则 ORT 在 cross-attention 的 Reshape 上直接失败。
//! 3. 带缓存步输出的 `present.*.encoder.*` 是 **0 长度占位**，不能回喂；encoder 缓存只在首步后固定一次。
//!
//! ## 当前边界（必须如实说）
//!
//! - **推理跑在宿主进程内**，没有像 Bergamot 那样再套一层 worker 进程（ADR-027 的隔离在这里还没落地）。
//!   ORT 的失败模式与 Marian CLI 不同（它不 `abort`），但"崩溃会带走宿主"这条风险仍然存在；
//! - 没有缓存（`PlanCache` 未接）、没有后台空闲回收、没有取消在途请求；
//! - `ProtectionMode::Marker` 未实现，显式返回 `CapabilityUnavailable`，不假装支持；
//! - 没有缓存、没有取消在途请求。
//!
//! ## `timeout_ms` 的现状（必须对宿主说清楚）
//!
//! `TranslateRequest.timeout_ms` 会被**接受但不生效**：本后端没有取消点（解码是串行的 FFI 调用，
//! ORT 侧也没有可用的中断手段）。宿主若要超时，只能在外层自己 `select` 或丢弃结果——
//! 而"宿主不再等待"并不等于"底层停止计算"，那段 CPU 仍会跑完。
//! 因此：**不要**把 `timeout_ms=1` 仍然成功返回读成"超时已实现"；等真正实现时，
//! `crates/translator-nmt-ort/tests/p4_contract.rs` 里的 `lc04_超时字段当前被忽略` 会故意失败，
//! 提醒把这条文档、`docs/26` D-63 与能力声明一起改掉。
//!
//! ## 排障开关
//!
//! 设 `NMT_ORT_DEBUG=1` 会在 stderr 打出每一段的生成 id 与截断结果。
//! 它**默认关闭**，也不改任何行为——只为"这段为什么译成这样"提供证据（`docs/25` §1.3）。
//!
//! ## 解码上界（不是调质量的旋钮）
//!
//! 纯贪心 + 短片段会退化成周期性重复（实测 97 次「你好」，同一模型在 Python 参考实现里同样如此）。
//! 因此本后端固定四条上界：长度 `min(max_new_tokens, src_len*8/5+12)`、无重复 3-gram、
//! 连续 3 个相同 token 即停、尾部同周期块收敛到一轮；触发次数记在 `degeneracy_count()` 上。

use std::collections::HashMap;
#[cfg(feature = "ort-runtime")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use translator_core::protection::{ProtectionMode, ProtectionPlan};
use translator_plugin_api::{BoxFuture, TranslationEngine};
use translator_types::{
    Capability, EngineDescriptor, LanguageTag, PluginApiVersion, TranslateRequest,
    TranslationError, TranslationErrorCode, TranslationResult,
};

/// 引擎 id（与 Bergamot 的 `nmt.bergamot.cli` 不同，路由可据此显式选择）。
pub const ENGINE_ID: &str = "nmt.ort.opus-mt";
/// 运行时标识（进执行计划的 `runtime` 字段）。
pub const RUNTIME: &str = "onnxruntime";
/// 模型目录里必需的文件。
pub const ENCODER_FILE: &str = "encoder_model_quantized.onnx";
pub const DECODER_FILE: &str = "decoder_model_merged_quantized.onnx";
pub const TOKENIZER_FILE: &str = "tokenizer.json";

/// P13 模型精度变体：同一份权重的 int8 / fp16 / fp32 ONNX 导出。
///
/// 同集 100 句：int8 chrF 0.4535、fp16 0.4648、fp32 0.4724；fp32 稳态延迟
/// 反而低于 int8（量化算子开销），但峰值 RSS 约 1.13 GB vs 0.58 GB。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModelVariant {
    /// 默认：int8 量化（低内存）。
    #[default]
    Int8,
    /// fp16（CPU 上通常要 upcast，内存最高，暂不推荐）。
    Fp16,
    /// fp32：质量最好、稳态最快，内存约为 int8 的 2 倍。
    Fp32,
}

impl ModelVariant {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Int8 => "int8",
            Self::Fp16 => "fp16",
            Self::Fp32 => "fp32",
        }
    }

    pub const fn files(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Int8 => (ENCODER_FILE, DECODER_FILE, TOKENIZER_FILE),
            Self::Fp16 => (
                "encoder_model_fp16.onnx",
                "decoder_model_merged_fp16.onnx",
                TOKENIZER_FILE,
            ),
            Self::Fp32 => (
                "encoder_model.onnx",
                "decoder_model_merged.onnx",
                TOKENIZER_FILE,
            ),
        }
    }
}

// Marian（opus-mt-en-zh）的固定形状：改模型必须改这些常量，或者让它们来自 config。
#[cfg(feature = "ort-runtime")]
const LAYERS: usize = 6;
#[cfg(feature = "ort-runtime")]
const HEADS: usize = 8;
#[cfg(feature = "ort-runtime")]
const HEAD_DIM: usize = 64;
#[cfg(feature = "ort-runtime")]
const EOS_ID: i64 = 0;
const PAD_ID: i64 = 65000;

/// 一层/一侧的 past 张量列表（名字、形状、数据）。
type DecoderPast = Vec<(String, Vec<i64>, Vec<f32>)>;

/// 解码时的重复 n-gram 抑制阶数（0 = 关闭）。
///
/// 这不是"调质量"的旋钮，而是**上界约束**：greedy + 无抑制时，极短片段
/// （受保护跨度分段后常见）会退化成 `你好 你好 你好 …` 直到撞上 token 上限，
/// 实测（docs/24 §5）同一模型在 Python 参考实现里也是这样，属于模型行为不是解码 bug。
pub const NO_REPEAT_NGRAM: usize = 3;
/// 生成长度相对源 token 的上界：`src * 8 / 5 + 12`（再与 `max_new_tokens` 取小）。
///
/// 系数 1.6 的来历：en→zh 的实际 token 比在实测样本上约 0.8–1.2，
/// 但**纯贪心没有长度归一化**，短片段容易一路膨胀，所以上界不能只按源长度给；
/// 1.6 留出余量又不至于让退化片段跑满 `max_new_tokens`。
const LENGTH_RATIO_NUM: usize = 8;
const LENGTH_RATIO_DEN: usize = 5;
const LENGTH_SLACK: usize = 12;

/// 解码策略标识：**换策略必须换这个串**。
///
/// 它是给"身份"用的（现在是 `OrtIdentity.decode_policy`，将来要进 `plan-v1` 摘要）：
/// 同一份权重、不同解码策略会产出不同译文，只摘要权重不够（ADR-026）。
/// 这里刻意写成"人可读 + 机器可 diff"的一行，避免用版本号掩盖到底改了什么。
pub const DECODE_POLICY: &str =
    "greedy;no-repeat-3gram=3;stop-on-3-run;tail-collapse(>=3 cycles,period<=4);len-cap=min(max_new_tokens,src*8/5+12)";

/// 解码策略标签（进身份/缓存键）。beam=1 与历史贪心标签保持完全一致。
pub fn decode_policy_label(beam_width: usize) -> String {
    if beam_width <= 1 {
        DECODE_POLICY.to_string()
    } else {
        format!("beam={beam_width};no-repeat-3gram=3;len-cap=min(max_new_tokens,src*8/5+12)")
    }
}

/// 单段解码的 token 上界：`min(max_new_tokens, src_len*8/5 + 12)`。
///
/// 公开它是因为**测量与被测代码必须用同一套口径**：`examples/p4_bench.rs` 要把 cap
/// 写进基线文件，如果那里自己抄一份公式，改了实现就会悄悄对不上（`docs/27` R-04）。
pub fn token_cap(src_len: usize, max_new_tokens: usize) -> usize {
    let ratio = src_len * LENGTH_RATIO_NUM / LENGTH_RATIO_DEN + LENGTH_SLACK;
    max_new_tokens.min(ratio).max(1)
}

/// 内存准入估计：ORT 运行库 + int8 权重 + 激活 + 会话 arena。
///
/// 600 MiB 来自 **Rust 进程内实测**（release、单线程、499 字符长句跑满后 PSS 574.8 / RSS 578.1 MiB，
/// 见 `fixtures/nmt/baseline-p4-ort-linux-x86_64.json`）。门禁阶段先写的 420 MiB 是照
/// Python 宿主 RSS 折算的估计值，实测偏低 —— 准入估计偏低会让宿主**高估**可用内存，
/// 所以这里按实测值上调，并在 `docs/25` 记为偏差。
pub const ORT_STEADY_BYTES: u64 = 600 * 1024 * 1024;

/// 引擎配置。
#[derive(Clone, Debug)]
pub struct OrtNmtConfig {
    /// 模型目录。
    pub model_dir: PathBuf,
    /// 编码器文件名（默认 int8 导出；P13 可换成 fp32/fp16 做同集对照）。
    pub encoder_file: String,
    /// 合并解码器文件名。
    pub decoder_file: String,
    /// 分词器文件名。
    pub tokenizer_file: String,
    /// 模型精度变体（进身份；改变体必须换文件名）。
    pub variant: ModelVariant,
    pub source: LanguageTag,
    pub target: LanguageTag,
    /// ONNX Runtime 的 intra-op 线程数（解码是逐 token 串行的，加线程收益很小，见 docs/24 §2.4）
    pub threads: usize,
    /// 单句最多生成多少 token
    pub max_new_tokens: usize,
    /// Beam 宽度（1 = 贪心；>1 = 实验性 beam search，逐候选重跑 decoder，较慢）。
    pub beam_width: usize,
    pub protection: ProtectionMode,
    /// 装配期的内存估计（下界）
    pub steady_bytes: u64,
}

impl OrtNmtConfig {
    pub fn new(model_dir: impl Into<PathBuf>, source: LanguageTag, target: LanguageTag) -> Self {
        Self {
            model_dir: model_dir.into(),
            encoder_file: ENCODER_FILE.to_string(),
            decoder_file: DECODER_FILE.to_string(),
            tokenizer_file: TOKENIZER_FILE.to_string(),
            variant: ModelVariant::Int8,
            source,
            target,
            threads: 1,
            max_new_tokens: 192,
            beam_width: 1,
            protection: ProtectionMode::Segment,
            steady_bytes: ORT_STEADY_BYTES,
        }
    }

    pub fn with_threads(mut self, threads: usize) -> Self {
        self.threads = threads.max(1);
        self
    }

    pub fn with_max_new_tokens(mut self, n: usize) -> Self {
        self.max_new_tokens = n.max(1);
        self
    }

    /// 选择 int8/fp16/fp32 变体（同时设置三个文件名）。
    pub fn with_variant(mut self, variant: ModelVariant) -> Self {
        let (encoder, decoder, tokenizer) = variant.files();
        self.encoder_file = encoder.to_string();
        self.decoder_file = decoder.to_string();
        self.tokenizer_file = tokenizer.to_string();
        self.variant = variant;
        self
    }

    /// 指定模型文件名（P13 模型候选：int8/fp32/fp16 共用同一套 schema）。
    pub fn with_files(
        mut self,
        encoder: impl Into<String>,
        decoder: impl Into<String>,
        tokenizer: impl Into<String>,
    ) -> Self {
        self.encoder_file = encoder.into();
        self.decoder_file = decoder.into();
        self.tokenizer_file = tokenizer.into();
        self
    }

    /// Beam 宽度（T3/W2 对照用；1 = 现有贪心路径）。
    pub fn with_beam_width(mut self, width: usize) -> Self {
        self.beam_width = width.max(1);
        self
    }

    /// 缺文件时一次说清缺哪些，而不是等到加载才报第一个。
    pub fn missing_files(&self) -> Vec<&str> {
        [
            self.encoder_file.as_str(),
            self.decoder_file.as_str(),
            self.tokenizer_file.as_str(),
        ]
        .into_iter()
        .filter(|name| !self.model_dir.join(name).is_file())
        .collect()
    }
}

/// 一次加载的身份（进执行计划 / 诊断）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrtIdentity {
    pub engine_id: String,
    pub model_revision: String,
    pub runtime: String,
    pub runtime_build: String,
    pub threads: usize,
    pub max_new_tokens: usize,
    pub beam_width: usize,
    pub model_variant: String,
    /// 解码策略标识（`DECODE_POLICY`）：同一权重换策略就是另一个身份。
    pub decode_policy: String,
}

/// ORT 引擎。
pub struct OrtNmtEngine {
    config: OrtNmtConfig,
    identity: Mutex<Option<OrtIdentity>>,
    loaded: Mutex<Option<Loaded>>,
    /// 构造/首次查询时算出的执行身份（文件内容 + 解码策略），供协调器缓存键使用。
    #[cfg_attr(not(feature = "ort-runtime"), allow(dead_code))]
    cache_identity: Mutex<Option<String>>,
    /// 退化片段计数：截断/重复抑制触发的次数（可观测，但不改公开警告枚举）。
    degeneracy: AtomicU64,
}

type EnginePool = Mutex<HashMap<String, Weak<OrtNmtEngine>>>;

static POOL: OnceLock<EnginePool> = OnceLock::new();

fn pool() -> &'static EnginePool {
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn canonical(path: &std::path::Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// 共享键：模型三文件内容 + 所有会影响解码/内存的配置。
fn pool_key(config: &OrtNmtConfig, revision: &str) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        canonical(&config.model_dir),
        revision,
        config.source.as_str(),
        config.target.as_str(),
        config.threads,
        config.max_new_tokens,
        config.beam_width,
        config.variant.as_str(),
        config.protection.as_str(),
        config.steady_bytes,
        decode_policy_label(config.beam_width),
    )
}

impl std::fmt::Debug for OrtNmtEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrtNmtEngine")
            .field("engine_id", &ENGINE_ID)
            .field("model_dir", &self.config.model_dir)
            .finish()
    }
}

impl OrtNmtEngine {
    /// 构造只做**文件存在性**检查，不加载权重（与 Bergamot 引擎一致：构造要便宜）。
    pub fn new(config: OrtNmtConfig) -> Result<Self, TranslationError> {
        let missing = config.missing_files();
        if !missing.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::ResourceNotInstalled,
                format!(
                    "ORT 模型目录 {} 缺文件：{}（用 tools/p4/build-tokenizer.py 生成 tokenizer.json）",
                    config.model_dir.display(),
                    missing.join("、")
                ),
            ));
        }
        if config.protection == ProtectionMode::Marker {
            return Err(TranslationError::new(
                TranslationErrorCode::CapabilityUnavailable,
                "ORT 后端暂未实现标记模式（protect-mark-v1）；请使用默认的安全分段（protect-seg-v1）",
            ));
        }
        Ok(Self {
            config,
            identity: Mutex::new(None),
            loaded: Mutex::new(None),
            cache_identity: Mutex::new(None),
            degeneracy: AtomicU64::new(0),
        })
    }

    /// 进程内共享同一个 ORT 会话（相同模型内容/配置）。
    ///
    /// 池只存 `Weak`：最后一个 `Translator` 释放后会话/arena 自然释放。
    pub fn shared(config: OrtNmtConfig) -> Result<Arc<Self>, TranslationError> {
        let revision = revision_of(&config)?;
        let key = pool_key(&config, &revision);
        let pool = pool();
        let mut map = pool.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, weak| weak.strong_count() > 0);
        if let Some(existing) = map.get(&key).and_then(Weak::upgrade) {
            return Ok(existing);
        }
        let engine = Arc::new(Self::new(config)?);
        map.insert(key, Arc::downgrade(&engine));
        Ok(engine)
    }

    /// 当前池里仍存活的 ORT 引擎数（诊断/测试用）。
    pub fn shared_engine_count() -> usize {
        let pool = pool();
        let mut map = pool.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, weak| weak.strong_count() > 0);
        map.len()
    }

    pub fn config(&self) -> &OrtNmtConfig {
        &self.config
    }

    pub fn identity(&self) -> Option<OrtIdentity> {
        self.identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 退化片段计数（重复抑制 / 长度截断触发次数）。
    ///
    /// 为什么不是 `TranslationWarning`：给公开警告枚举加变体会破坏宿主的穷尽匹配，
    /// 所以退化的可观测性放在引擎句柄上，宿主可自行决定如何暴露。
    pub fn degeneracy_count(&self) -> u64 {
        self.degeneracy.load(Ordering::Relaxed)
    }

    /// 内存准入估计（下界）。
    pub fn memory_footprint_bytes(&self) -> u64 {
        self.config.steady_bytes
    }

    /// 加载（幂等）。重复调用只返回同一份身份。
    pub fn load(&self) -> Result<OrtIdentity, TranslationError> {
        if let Some(id) = self.identity() {
            return Ok(id);
        }
        let mut guard = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = self.identity() {
            return Ok(id);
        }
        let loaded = Loaded::open(&self.config)?;
        let identity = loaded.identity.clone();
        *guard = Some(loaded);
        *self.identity.lock().unwrap_or_else(|e| e.into_inner()) = Some(identity.clone());
        Ok(identity)
    }

    /// 卸载：drop 会话与分词器，并把内存还回去（ORT 的 arena 随会话释放）。
    pub fn unload(&self) -> Result<(), TranslationError> {
        let mut guard = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
        *self.identity.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    pub fn is_loaded(&self) -> bool {
        self.identity().is_some()
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

        // 路由层会先问 `supports()`，但引擎自己也要把住：
        // 直接持有引擎句柄的宿主可以绕过 Coordinator，此时语言对不符必须报错——
        // 否则会把 en→zh 模型的结果标成别的语言对，是"看起来成功"的错。
        if req.target != self.config.target {
            return Err(TranslationError::new(
                TranslationErrorCode::CapabilityUnavailable,
                format!(
                    "本引擎只做 {} → {}，收到目标语言 {}",
                    self.config.source, self.config.target, req.target
                ),
            ));
        }
        if let Some(source) = &req.source {
            if source != &self.config.source {
                return Err(TranslationError::new(
                    TranslationErrorCode::CapabilityUnavailable,
                    format!(
                        "本引擎只做 {} → {}，收到源语言 {}",
                        self.config.source, self.config.target, source
                    ),
                ));
            }
        }

        // 先确保加载（幂等），再取锁——避免持锁重入
        self.load()?;
        let mut degeneracy = 0u64;

        // 结构保护由 translator-core 负责，与后端无关：换运行时不能让受保护片段语义变化
        let plan = ProtectionPlan::build(&req.text);
        let mut parts: Vec<String> = Vec::new();
        {
            let mut guard = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
            let loaded = guard.as_mut().ok_or_else(|| {
                TranslationError::new(
                    TranslationErrorCode::RuntimeInitFailed,
                    "ORT 引擎加载后仍然没有会话（内部状态错误）",
                )
            })?;
            for part in plan.plain_texts() {
                if part.trim().is_empty() {
                    parts.push(part.to_string());
                    continue;
                }
                let (translated, bounded) = loaded.translate_segment(part)?;
                if bounded {
                    degeneracy += 1;
                }
                parts.push(translated);
            }
        }
        if degeneracy > 0 {
            self.degeneracy.fetch_add(degeneracy, Ordering::Relaxed);
        }

        let identity = self.identity().expect("load() 之后必然有身份");
        let text = plan.assemble(&parts)?;
        Ok(TranslationResult {
            text,
            source: self.config.source.clone(),
            target: self.config.target.clone(),
            engine_id: ENGINE_ID.to_string(),
            model_id: Some(identity.model_revision),
            elapsed_ms: started.elapsed().as_millis() as u64,
            warnings: Vec::new(),
        })
    }
}

impl TranslationEngine for OrtNmtEngine {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: ENGINE_ID,
            capabilities: &[Capability::Nmt],
            // 比 Bergamot 低：同一语言对上默认仍选已验证更久的参考实现，
            // 只有宿主显式 `engine:nmt.ort.opus-mt` 或禁用 Bergamot 时才会选中它。
            priority: 90,
            plugin_api: PluginApiVersion::CURRENT,
            license: "Apache-2.0 (model + runtime)",
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

    #[cfg(feature = "ort-runtime")]
    /// 本后端用 `ProtectionPlan` 自己处理受保护跨度，协调器必须交出原文。
    fn handles_protected_spans(&self) -> bool {
        true
    }

    fn cache_identity(&self) -> Option<String> {
        if let Some(id) = self
            .cache_identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Some(id);
        }
        let revision = revision_of(&self.config).ok()?;
        let identity = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            ENGINE_ID,
            revision,
            RUNTIME,
            self.config.threads,
            self.config.max_new_tokens,
            self.config.beam_width,
            self.config.protection.as_str(),
            decode_policy_label(self.config.beam_width),
        );
        *self
            .cache_identity
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(identity.clone());
        Some(identity)
    }
}

// ============================================================
// 加载后的会话（feature `ort-runtime` 打开时才有真实实现）
// ============================================================

#[cfg(feature = "ort-runtime")]
struct Loaded {
    encoder: ort::session::Session,
    decoder: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    identity: OrtIdentity,
}

#[cfg(feature = "ort-runtime")]
impl Loaded {
    fn open(config: &OrtNmtConfig) -> Result<Self, TranslationError> {
        let env = environment()?;

        let encoder = build_session(
            env,
            &config.model_dir.join(&config.encoder_file),
            config.threads,
        )?;
        let decoder = build_session(
            env,
            &config.model_dir.join(&config.decoder_file),
            config.threads,
        )?;
        let tokenizer =
            tokenizers::Tokenizer::from_file(config.model_dir.join(&config.tokenizer_file))
                .map_err(|e| {
                    TranslationError::new(
                        TranslationErrorCode::RuntimeInitFailed,
                        format!("分词器 {} 加载失败：{e}", config.tokenizer_file),
                    )
                })?;

        let revision = revision_of(config)?;
        Ok(Self {
            encoder,
            decoder,
            tokenizer,
            identity: OrtIdentity {
                engine_id: ENGINE_ID.to_string(),
                model_revision: revision,
                runtime: RUNTIME.to_string(),
                runtime_build: format!(
                    "onnxruntime api 1.{}（ort 2.0.0-rc.15 prebuilt）",
                    ort::MINOR_VERSION
                ),
                threads: config.threads,
                max_new_tokens: config.max_new_tokens,
                beam_width: config.beam_width,
                model_variant: config.variant.as_str().to_string(),
                decode_policy: decode_policy_label(config.beam_width),
            },
        })
    }

    /// Beam search（实验性）：每个候选每步重跑一次 decoder，不共享缓存。
    ///
    /// 这样做的代价是约 `beam_width` 倍延迟，但实现不依赖 batch/cache 布局；
    /// 质量对照必须用同一金样本与贪心比较后再决定是否进入 Precise 默认。
    fn translate_segment_beam(&mut self, text: &str) -> Result<(String, bool), TranslationError> {
        let ids = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| {
                TranslationError::new(
                    TranslationErrorCode::InvalidRequest,
                    format!("分词失败：{e}"),
                )
            })?
            .get_ids()
            .to_vec();
        if ids.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "分词结果为空（输入没有可用 token）",
            ));
        }
        let mut input: Vec<i64> = ids.into_iter().map(|i| i as i64).collect();
        input.push(EOS_ID);
        let src_len = input.len();
        let hidden = {
            let feed: Vec<(String, ort::session::SessionInputValue)> = vec![
                (
                    "input_ids".to_string(),
                    ort_tensor_i64(vec![1, src_len as i64], input)?.into(),
                ),
                (
                    "attention_mask".to_string(),
                    ort_tensor_i64(vec![1, src_len as i64], vec![1i64; src_len])?.into(),
                ),
            ];
            let outputs = self
                .encoder
                .run(feed)
                .map_err(|e| runtime_error("编码器推理失败", e))?;
            let (shape, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| runtime_error("编码器输出解析失败", e))?;
            let shape: Vec<i64> = shape.iter().copied().collect();
            (shape, data.to_vec())
        };

        let cap = token_cap(src_len, self.identity.max_new_tokens);
        let beam_width = self.identity.beam_width.max(2);

        #[derive(Clone)]
        struct Beam {
            tokens: Vec<u32>,
            score: f32,
            past: DecoderPast,
        }

        // 首步：一次 decoder，得到 logits 与 present；后续每个 beam 只跑 `cur=[last]`。
        let (logits0, past0, enc_past) = self.decoder_step(
            &hidden,
            src_len,
            &[PAD_ID],
            &initial_past(src_len),
            &Vec::new(),
            false,
        )?;

        let mut active: Vec<Beam> = Vec::new();
        let mut completed: Vec<(Vec<u32>, f32)> = Vec::new();
        for (id, score) in top_candidates(&logits0, &[], beam_width * 2) {
            if id as i64 == EOS_ID {
                completed.push((Vec::new(), score));
            } else {
                active.push(Beam {
                    tokens: vec![id],
                    score,
                    past: past0.clone(),
                });
            }
        }
        active.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        active.truncate(beam_width);

        let mut bounded = false;
        for step in 1..cap {
            if active.is_empty() {
                break;
            }
            let mut candidates: Vec<Beam> = Vec::new();
            for beam in &active {
                let last = *beam.tokens.last().unwrap_or(&0);
                let (logits, next_past, _) = self.decoder_step(
                    &hidden,
                    src_len,
                    &[last as i64],
                    &beam.past,
                    &enc_past,
                    true,
                )?;
                for (id, lp) in top_candidates(&logits, &beam.tokens, beam_width * 2) {
                    let mut tokens = beam.tokens.clone();
                    let score = beam.score + lp;
                    if id as i64 == EOS_ID {
                        completed.push((tokens, score));
                    } else {
                        tokens.push(id);
                        candidates.push(Beam {
                            tokens,
                            score,
                            past: next_past.clone(),
                        });
                    }
                }
            }
            candidates.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            candidates.truncate(beam_width);
            active = candidates;
            if step + 1 == cap {
                bounded = true;
            }
        }

        let mut pool = completed;
        pool.extend(active.into_iter().map(|b| (b.tokens, b.score)));
        let (best, _) = pool
            .into_iter()
            .max_by(|a, b| {
                let la = a.0.len().max(1) as f32;
                let lb = b.0.len().max(1) as f32;
                (a.1 / la.powf(0.7))
                    .partial_cmp(&(b.1 / lb.powf(0.7)))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or_else(|| {
                TranslationError::new(TranslationErrorCode::BackendFailed, "beam 没有候选")
            })?;
        let (kept, truncated) = truncate_repeated_tail(&best);
        let text = self
            .tokenizer
            .decode(kept, true)
            .map_err(|e| runtime_error("反分词失败", e))?;
        Ok((unescape_metaspace(&text), bounded || truncated))
    }

    /// 逐步 decoder（带/不带 cache）。返回 `(last_logits, next_decoder_past, encoder_past)`。
    fn decoder_step(
        &mut self,
        hidden: &(Vec<i64>, Vec<f32>),
        src_len: usize,
        cur: &[i64],
        past: &DecoderPast,
        enc_past_in: &DecoderPast,
        use_cache: bool,
    ) -> Result<(Vec<f32>, DecoderPast, DecoderPast), TranslationError> {
        let mut feed: Vec<(String, ort::session::SessionInputValue)> = vec![
            (
                "input_ids".to_string(),
                ort_tensor_i64(vec![1, cur.len() as i64], cur.to_vec())?.into(),
            ),
            (
                "encoder_attention_mask".to_string(),
                ort_tensor_i64(vec![1, src_len as i64], vec![1i64; src_len])?.into(),
            ),
            (
                "encoder_hidden_states".to_string(),
                ort_tensor_f32(hidden.0.clone(), hidden.1.clone())?.into(),
            ),
            (
                "use_cache_branch".to_string(),
                ort_tensor_bool(vec![1], vec![use_cache])?.into(),
            ),
        ];
        for (name, shape, data) in past.iter().chain(enc_past_in.iter()) {
            feed.push((
                name.clone(),
                ort_tensor_f32(shape.clone(), data.clone())?.into(),
            ));
        }
        let outputs = self
            .decoder
            .run(feed)
            .map_err(|e| runtime_error("beam decoder 推理失败", e))?;
        let (shape, logits) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| runtime_error("beam logits 解析失败", e))?;
        let vocab = *shape.last().unwrap_or(&0) as usize;
        if vocab == 0 || logits.len() < vocab {
            return Err(TranslationError::new(
                TranslationErrorCode::BackendFailed,
                "beam decoder 输出词表维度异常",
            ));
        }
        let last = logits[logits.len() - vocab..].to_vec();

        let names: Vec<String> = outputs.keys().map(|k| k.to_string()).collect();
        let mut next: Vec<(String, Vec<i64>, Vec<f32>)> = Vec::new();
        let mut enc_past: Vec<(String, Vec<i64>, Vec<f32>)> = Vec::new();
        for name in names {
            if name == "logits" {
                continue;
            }
            let past_name = name.replace("present.", "past_key_values.");
            if past_name.contains(".encoder.") {
                if !use_cache {
                    let (shape, data) = outputs[&*name]
                        .try_extract_tensor::<f32>()
                        .map_err(|e| runtime_error("beam encoder past 解析失败", e))?;
                    enc_past.push((past_name, shape.iter().copied().collect(), data.to_vec()));
                }
                continue;
            }
            let (shape, data) = outputs[&*name]
                .try_extract_tensor::<f32>()
                .map_err(|e| runtime_error("beam present 解析失败", e))?;
            next.push((past_name, shape.iter().copied().collect(), data.to_vec()));
        }
        Ok((last, next, enc_past))
    }

    fn translate_segment(&mut self, text: &str) -> Result<(String, bool), TranslationError> {
        if self.identity.beam_width > 1 {
            return self.translate_segment_beam(text);
        }
        let ids = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| {
                TranslationError::new(
                    TranslationErrorCode::InvalidRequest,
                    format!("分词失败：{e}"),
                )
            })?
            .get_ids()
            .to_vec();
        if ids.is_empty() {
            return Err(TranslationError::new(
                TranslationErrorCode::InvalidRequest,
                "分词结果为空（输入没有可用 token）",
            ));
        }
        let mut input: Vec<i64> = ids.into_iter().map(|i| i as i64).collect();
        input.push(EOS_ID); // Marian：句末 </s>

        let src_len = input.len();
        let hidden = {
            let feed: Vec<(String, ort::session::SessionInputValue)> = vec![
                (
                    "input_ids".to_string(),
                    ort_tensor_i64(vec![1, src_len as i64], input)?.into(),
                ),
                (
                    "attention_mask".to_string(),
                    ort_tensor_i64(vec![1, src_len as i64], vec![1i64; src_len])?.into(),
                ),
            ];
            let outputs = self
                .encoder
                .run(feed)
                .map_err(|e| runtime_error("编码器推理失败", e))?;
            let (shape, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| runtime_error("编码器输出解析失败", e))?;
            let shape: Vec<i64> = shape.iter().copied().collect();
            (shape, data.to_vec())
        };

        let mut past = initial_past(src_len);
        // encoder 侧缓存首步之后**必须原样继续喂**，但**不再更新**：
        // 合并解码器把它声明成图输入（If 两分支共用），少喂一个就直接
        // `Missing Input: past_key_values.0.encoder.key`；
        // 而缓存步输出的 `present.*.encoder.*` 只是 0 长度占位，回喂会崩（模块文档第 3 条）。
        let mut enc_past: Vec<(String, Vec<i64>, Vec<f32>)> = Vec::new();
        let mut cur: Vec<i64> = vec![PAD_ID];
        let mut out_ids: Vec<u32> = Vec::new();

        // 长度上界：`max_new_tokens` 是绝对上限，逐段再按源长度收紧。
        // 没有这一条，一个 "Hello " 片段会一路重复到撞上 96 个 token。
        let cap = token_cap(src_len, self.identity.max_new_tokens);
        let mut bounded = false;

        for step in 0..cap {
            let mut feed: Vec<(String, ort::session::SessionInputValue)> = vec![
                (
                    "input_ids".to_string(),
                    ort_tensor_i64(vec![1, cur.len() as i64], cur.clone())?.into(),
                ),
                (
                    "encoder_attention_mask".to_string(),
                    ort_tensor_i64(vec![1, src_len as i64], vec![1i64; src_len])?.into(),
                ),
                (
                    "encoder_hidden_states".to_string(),
                    ort_tensor_f32(hidden.0.clone(), hidden.1.clone())?.into(),
                ),
                (
                    "use_cache_branch".to_string(),
                    ort_tensor_bool(vec![1], vec![step > 0])?.into(),
                ),
            ];
            for (name, shape, data) in past.iter().chain(enc_past.iter()) {
                feed.push((
                    name.clone(),
                    ort_tensor_f32(shape.clone(), data.clone())?.into(),
                ));
            }

            let outputs = self.decoder.run(feed).map_err(|e| {
                runtime_error(
                    &format!(
                        "解码器推理失败（step {step}，cur_len={}，src_len={src_len}，encoder past {} 项）",
                        cur.len(),
                        enc_past.len()
                    ),
                    e,
                )
            })?;

            let (logits_shape, logits) = outputs["logits"]
                .try_extract_tensor::<f32>()
                .map_err(|e| runtime_error("解码器输出解析失败", e))?;
            let vocab = *logits_shape.last().unwrap_or(&0) as usize;
            if vocab == 0 || logits.is_empty() {
                return Err(TranslationError::new(
                    TranslationErrorCode::BackendFailed,
                    "解码器输出的词表维度为 0",
                ));
            }
            let last = &logits[logits.len() - vocab..];
            let mut best = 0usize;
            let mut best_v = f32::NEG_INFINITY;
            let mut fallback = 0usize;
            let mut fallback_v = f32::NEG_INFINITY;
            for (i, v) in last.iter().enumerate() {
                if i as i64 == PAD_ID {
                    continue; // bad_words_ids：禁止生成 pad
                }
                if *v > fallback_v {
                    fallback_v = *v;
                    fallback = i;
                }
                // 重复 n-gram 抑制：候选只有在**不撞重**时才允许胜出；
                // 全程都被禁时退回原始 argmax（宁可退化，也不能无候选）。
                if *v > best_v && !repeats_ngram(&out_ids, i as u32, NO_REPEAT_NGRAM) {
                    best_v = *v;
                    best = i;
                }
            }
            if best_v == f32::NEG_INFINITY {
                best = fallback;
                bounded = true; // 所有候选都被抑制 → 说明这段已经在该循环里了
            }
            if best as i64 == EOS_ID {
                break;
            }
            out_ids.push(best as u32);
            cur = vec![best as i64];
            // 三个相同的 token 连着出现 = 已经进入退化循环，立刻停：
            // 再解下去只会把 CPU 烧在重复上，且输出不会更好（见 `truncate_repeated_tail`）。
            if out_ids.len() >= 3 {
                let n = out_ids.len();
                if out_ids[n - 1] == out_ids[n - 2] && out_ids[n - 2] == out_ids[n - 3] {
                    bounded = true;
                    break;
                }
            }
            if step + 1 == cap {
                bounded = true; // 撞上长度上界仍未吐 EOS
            }

            // 只更新 decoder 侧缓存；encoder 侧只在首步取一次，之后固定（模块文档第 3 条）
            let names: Vec<String> = outputs.keys().map(|k| k.to_string()).collect();
            let mut next: Vec<(String, Vec<i64>, Vec<f32>)> = Vec::new();
            for name in names {
                if name == "logits" {
                    continue;
                }
                let past_name = name.replace("present.", "past_key_values.");
                if past_name.contains(".encoder.") {
                    if step == 0 {
                        let (shape, data) = outputs[&*name]
                            .try_extract_tensor::<f32>()
                            .map_err(|e| runtime_error("encoder past 解析失败", e))?;
                        let shape: Vec<i64> = shape.iter().copied().collect();
                        enc_past.push((past_name, shape, data.to_vec()));
                    }
                    continue; // 缓存步的 encoder 输出是 0 长度占位，不回喂
                }
                let (shape, data) = outputs[&*name]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| runtime_error("present 张量解析失败", e))?;
                let shape: Vec<i64> = shape.iter().copied().collect();
                next.push((past_name, shape, data.to_vec()));
            }
            past = next;
        }

        let (kept, truncated) = truncate_repeated_tail(&out_ids);
        if std::env::var_os("NMT_ORT_DEBUG").is_some() {
            eprintln!(
                "[p4-debug] segment={text:?} generated={out_ids:?} kept={} truncated={truncated}",
                kept.len()
            );
        }
        let text = self
            .tokenizer
            .decode(kept, true)
            .map_err(|e| runtime_error("反分词失败", e))?;
        Ok((unescape_metaspace(&text), bounded || truncated))
    }
}

/// 从 logits 取 top-k 候选（log-softmax 分值），跳过 pad 与重复 n-gram。
#[cfg_attr(not(feature = "ort-runtime"), allow(dead_code))]
fn top_candidates(logits: &[f32], tokens: &[u32], k: usize) -> Vec<(u32, f32)> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let sum: f32 = logits.iter().map(|v| (v - max).exp()).sum();
    let logsum = max + sum.ln();
    let mut top: Vec<(u32, f32)> = Vec::with_capacity(k);
    for (i, v) in logits.iter().enumerate() {
        if i as i64 == PAD_ID {
            continue;
        }
        if repeats_ngram(tokens, i as u32, NO_REPEAT_NGRAM) {
            continue;
        }
        let lp = *v - logsum;
        if top.len() < k {
            top.push((i as u32, lp));
            top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        } else if lp > top[k - 1].1 {
            top[k - 1] = (i as u32, lp);
            top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
    top
}

/// 去掉**尾部**连续重复的周期块，只保留一轮。
///
/// 为什么需要它：`protect-seg-v1` 会把句子切成片段，`Hello ` 这种片段在以
/// `max_new_tokens` 为界的贪心解码下会退化成 `你好 你好 你好 …`（同一模型在
/// Python 参考实现里也一样，见 `docs/24` §5）。Bergamot 那条链路不出这个问题，
/// 说明这是**本候选实现**的解码缺陷，而不是"中文就长这样"。
///
/// 规则是确定的、不猜内容：只有当同一个周期块连续出现 **≥3 次**、且周期 ≤4 个 token 时，
/// 才砍到只剩一轮。出现两次的（例如「哈哈」）不动，避免把正常重复也当退化。
#[cfg_attr(not(feature = "ort-runtime"), allow(dead_code))]
fn truncate_repeated_tail(ids: &[u32]) -> (&[u32], bool) {
    const MIN_CYCLES: usize = 3;
    const MAX_PERIOD: usize = 4;
    let mut end = ids.len();

    // 第一刀：找**最早**出现的连续 3 个相同 token，砍到只剩 1 个。
    // 这一步处理"重复 → 被抑制 → 又回到重复"的锯齿形输出（实测就有这种形状）。
    for i in 0..end.saturating_sub(MIN_CYCLES - 1) {
        if ids[i..i + MIN_CYCLES].iter().all(|t| *t == ids[i]) {
            end = i + 1;
            break;
        }
    }
    loop {
        let mut cut = None;
        let max_p = (end / MIN_CYCLES).min(MAX_PERIOD);
        for p in 1..=max_p {
            let tail = &ids[end - p..end];
            let repeated = (1..MIN_CYCLES).all(|k| ids[end - p * (k + 1)..end - p * k] == *tail);
            if repeated {
                cut = Some(p);
            }
        }
        match cut {
            Some(p) => end -= p * (MIN_CYCLES - 1),
            None => break,
        }
    }
    (&ids[..end], end != ids.len())
}

/// 把 `token` 接到 `history` 后面是否会复现一个长度 `n` 的既有 n-gram。
#[cfg_attr(not(feature = "ort-runtime"), allow(dead_code))]
fn repeats_ngram(history: &[u32], token: u32, n: usize) -> bool {
    if n < 2 || history.len() + 1 < n {
        return false;
    }
    let mut seq: Vec<u32> = Vec::with_capacity(history.len() + 1);
    seq.extend_from_slice(history);
    seq.push(token);
    if seq.len() < n {
        return false;
    }
    let tail = &seq[seq.len() - n..];
    // 只和**更早**的位置比（`seq[..len-1]` 保证最后一个窗口不是 tail 自己）
    seq[..seq.len() - 1].windows(n).any(|w| w == tail)
}

#[cfg(not(feature = "ort-runtime"))]
struct Loaded {
    identity: OrtIdentity,
}

#[cfg(not(feature = "ort-runtime"))]
impl Loaded {
    /// 未链接 ORT 时 `open` 必然失败，所以这里只是给编译器一个同形状的实现；
    /// 真被调用到就返回能力不可用，而不是 panic。
    fn translate_segment(&mut self, _text: &str) -> Result<(String, bool), TranslationError> {
        Err(TranslationError::new(
            TranslationErrorCode::CapabilityUnavailable,
            "本构建未启用 ort-runtime feature",
        ))
    }

    fn open(_config: &OrtNmtConfig) -> Result<Self, TranslationError> {
        Err(TranslationError::new(
            TranslationErrorCode::CapabilityUnavailable,
            "本构建未启用 translator-nmt-ort 的 ort-runtime feature：没有链接 ONNX Runtime",
        ))
    }
}

// ============================================================
// 小工具
// ============================================================

#[cfg_attr(not(feature = "ort-runtime"), allow(dead_code))]
fn init_error(e: impl std::fmt::Display) -> TranslationError {
    TranslationError::new(
        TranslationErrorCode::RuntimeInitFailed,
        format!("ONNX Runtime 环境初始化失败：{e}"),
    )
}

#[cfg_attr(not(feature = "ort-runtime"), allow(dead_code))]
fn runtime_error(what: &str, e: impl std::fmt::Display) -> TranslationError {
    TranslationError::new(TranslationErrorCode::BackendFailed, format!("{what}：{e}"))
}

/// SentencePiece 的 `▁` 在部分文本里会残留成 U+2581；按空格还原（与 Python 侧一致）。
#[cfg_attr(not(feature = "ort-runtime"), allow(dead_code))]
fn unescape_metaspace(text: &str) -> String {
    text.replace('\u{2581}', " ").trim().to_string()
}

/// 进程级 ORT 环境。
///
/// ONNX Runtime 只允许**每个进程一个** environment：`ort::init()` 第二次调用会
/// 直接报 `only one environment is allowed per process`。宿主可能挂多个引擎、
/// 也可能先卸载再加载，所以这里必须做进程级单例，而不是每次 `load()` 都建一个。
#[cfg(feature = "ort-runtime")]
fn environment() -> Result<&'static ort::environment::Environment, TranslationError> {
    static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> =
        std::sync::OnceLock::new();
    match ENV.get_or_init(|| {
        ort::init()
            .with_name("translator-nmt-ort")
            .build()
            .map_err(|e| e.to_string())
    }) {
        Ok(env) => Ok(env),
        Err(msg) => Err(TranslationError::new(
            TranslationErrorCode::RuntimeInitFailed,
            format!("ONNX Runtime 环境初始化失败：{msg}"),
        )),
    }
}

#[cfg(feature = "ort-runtime")]
fn build_session(
    env: &ort::environment::Environment,
    path: &Path,
    threads: usize,
) -> Result<ort::session::Session, TranslationError> {
    let mut builder = ort::session::Session::builder(env).map_err(init_error)?;
    builder = builder.with_intra_threads(threads).map_err(init_error)?;
    builder
        .commit_from_file(path)
        .map_err(|e| init_error(format!("{} 加载失败：{e}", path.display())))
}

fn revision_of(config: &OrtNmtConfig) -> Result<String, TranslationError> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for name in [
        &config.encoder_file,
        &config.decoder_file,
        &config.tokenizer_file,
    ] {
        let path = config.model_dir.join(name.as_str());
        let mut file = std::fs::File::open(&path).map_err(|e| {
            TranslationError::new(
                TranslationErrorCode::ResourceNotInstalled,
                format!("读取 {} 失败：{e}", path.display()),
            )
        })?;
        std::io::copy(&mut file, &mut hasher).map_err(|e| {
            TranslationError::new(
                TranslationErrorCode::RuntimeInitFailed,
                format!("计算 {} 摘要失败：{e}", path.display()),
            )
        })?;
    }
    let digest = format!("{:x}", hasher.finalize());
    Ok(format!("nmt.ort.opus-mt#{}", &digest[..16]))
}

#[cfg(feature = "ort-runtime")]
fn ort_tensor_i64(
    shape: Vec<i64>,
    data: Vec<i64>,
) -> Result<ort::value::Tensor<i64>, TranslationError> {
    ort::value::Tensor::from_array((shape, data))
        .map_err(|e| runtime_error("构造 int64 张量失败", e))
}

#[cfg(feature = "ort-runtime")]
fn ort_tensor_f32(
    shape: Vec<i64>,
    data: Vec<f32>,
) -> Result<ort::value::Tensor<f32>, TranslationError> {
    ort::value::Tensor::from_array((shape, data)).map_err(|e| runtime_error("构造 f32 张量失败", e))
}

#[cfg(feature = "ort-runtime")]
fn ort_tensor_bool(
    shape: Vec<i64>,
    data: Vec<bool>,
) -> Result<ort::value::Tensor<bool>, TranslationError> {
    ort::value::Tensor::from_array((shape, data))
        .map_err(|e| runtime_error("构造 bool 张量失败", e))
}

/// 首步的 past：decoder 侧 0 长度，encoder 侧真实 `src_len`（陷阱 2）。
#[cfg(feature = "ort-runtime")]
fn initial_past(src_len: usize) -> Vec<(String, Vec<i64>, Vec<f32>)> {
    let mut out = Vec::new();
    for layer in 0..LAYERS {
        for side in ["decoder", "encoder"] {
            for kind in ["key", "value"] {
                let seq = if side == "encoder" { src_len as i64 } else { 0 };
                let shape = vec![1, HEADS as i64, seq, HEAD_DIM as i64];
                let len = (shape.iter().product::<i64>()).max(0) as usize;
                out.push((
                    format!("past_key_values.{layer}.{side}.{kind}"),
                    shape,
                    vec![0.0f32; len],
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn cfg(dir: &Path) -> OrtNmtConfig {
        OrtNmtConfig::new(
            dir,
            LanguageTag::new("en").unwrap(),
            LanguageTag::new("zh-Hans").unwrap(),
        )
    }

    #[test]
    fn 模型变体切换文件名() {
        let dir = tempfile::tempdir().unwrap();
        let config = cfg(dir.path()).with_variant(ModelVariant::Fp32);
        assert_eq!(config.encoder_file, "encoder_model.onnx");
        assert_eq!(config.decoder_file, "decoder_model_merged.onnx");
        assert_eq!(config.variant, ModelVariant::Fp32);
        let fp16 = cfg(dir.path()).with_variant(ModelVariant::Fp16);
        assert_eq!(fp16.encoder_file, "encoder_model_fp16.onnx");
    }

    #[test]
    fn 缺文件时一次报全() {
        let dir = tempfile::tempdir().unwrap();
        let config = cfg(dir.path());
        let missing = config.missing_files();
        assert_eq!(missing.len(), 3, "{missing:?}");
        let err = OrtNmtEngine::new(config).unwrap_err();
        assert_eq!(err.code, TranslationErrorCode::ResourceNotInstalled);
        assert!(err.message.contains("tokenizer.json"), "{}", err.message);
    }

    #[test]
    fn 标记模式显式不支持而不是静默降级() {
        let dir = tempfile::tempdir().unwrap();
        for name in [ENCODER_FILE, DECODER_FILE, TOKENIZER_FILE] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let mut config = cfg(dir.path());
        config.protection = ProtectionMode::Marker;
        let err = OrtNmtEngine::new(config).unwrap_err();
        assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
        assert!(err.message.contains("protect-seg-v1"), "{}", err.message);
    }

    #[test]
    fn 引擎身份与能力声明() {
        let dir = tempfile::tempdir().unwrap();
        for name in [ENCODER_FILE, DECODER_FILE, TOKENIZER_FILE] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let engine = OrtNmtEngine::new(cfg(dir.path())).unwrap();
        let desc = engine.descriptor();
        assert_eq!(desc.id, ENGINE_ID);
        assert_eq!(desc.capabilities, &[Capability::Nmt]);
        assert!(desc.license.contains("Apache-2.0"));
        let en = LanguageTag::new("en").unwrap();
        let zh = LanguageTag::new("zh-Hans").unwrap();
        let ja = LanguageTag::new("ja").unwrap();
        assert!(engine.supports(&en, &zh));
        assert!(!engine.supports(&ja, &zh), "不支持的语言对必须明确拒绝");
    }

    #[test]
    fn 尾部重复被截断到一轮() {
        // 单 token 周期重复三次 → 只留一轮
        assert_eq!(
            truncate_repeated_tail(&[7, 9, 9, 9]),
            (&[7u32, 9][..], true)
        );
        // 两 token 周期重复三次 → 只留一轮
        assert_eq!(
            truncate_repeated_tail(&[1, 5, 6, 5, 6, 5, 6]),
            (&[1u32, 5, 6][..], true)
        );
        // 只重复两次（如「哈哈」）不动：避免把正常重复当退化
        assert_eq!(
            truncate_repeated_tail(&[7, 9, 9]),
            (&[7u32, 9, 9][..], false)
        );
        // 没有重复：原样返回
        assert_eq!(
            truncate_repeated_tail(&[1, 2, 3, 4]),
            (&[1u32, 2, 3, 4][..], false)
        );
        // 五次重复也收敛到一轮
        assert_eq!(
            truncate_repeated_tail(&[3, 4, 4, 4, 4, 4]),
            (&[3u32, 4][..], true)
        );
    }

    #[test]
    fn 空文本不进入模型() {
        let dir = tempfile::tempdir().unwrap();
        for name in [ENCODER_FILE, DECODER_FILE, TOKENIZER_FILE] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let engine = OrtNmtEngine::new(cfg(dir.path())).unwrap();
        let req = TranslateRequest::new("   ", LanguageTag::new("zh-Hans").unwrap())
            .with_source(LanguageTag::new("en").unwrap());
        let err = pollster::block_on(engine.translate(&req)).unwrap_err();
        assert_eq!(err.code, TranslationErrorCode::InvalidRequest);
    }

    #[test]
    fn metaspace残留被还原() {
        assert_eq!(unescape_metaspace("▁你好▁世界"), "你好 世界");
        assert_eq!(unescape_metaspace(" 猫在桌子上 "), "猫在桌子上");
    }
}
