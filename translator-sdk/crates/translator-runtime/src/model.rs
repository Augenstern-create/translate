//! 模型描述与**加载前**校验。
//!
//! `docs/12` §2.2 的核心要求：四个文件是一个**不可分割的集合**，进 native 之前就要证明
//! 「在、大小对、摘要对、不是符号链接」。P3.0 已经证明不这么做的后果——
//! 缺文件时参考运行时 `abort()`、权重被截断时 SIGSEGV。
//!
//! 这里刻意不依赖 `translator-resource`：模型包将来会长在资源仓库上（P3.1-D），
//! 但传输层不该反过来依赖仓库 crate，否则依赖方向就乱了。

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::RuntimeError;

/// 模型集合中的文件角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    Model,
    SourceVocab,
    TargetVocab,
    LexicalShortlist,
    /// 可选：ssplit 的 non-breaking prefix 表（P3.0 的可用配置里存在）。
    /// 它不是"四文件"的一部分，缺了也能启动；有就用，没有就不生成那个键。
    SplitPrefix,
}

impl FileRole {
    /// **必需**集合（`docs/14` §3.5 的"四文件"）。
    pub const REQUIRED: [FileRole; 4] = [
        FileRole::Model,
        FileRole::SourceVocab,
        FileRole::TargetVocab,
        FileRole::LexicalShortlist,
    ];

    /// 全部角色（含可选）。
    pub const ALL: [FileRole; 5] = [
        FileRole::Model,
        FileRole::SourceVocab,
        FileRole::TargetVocab,
        FileRole::LexicalShortlist,
        FileRole::SplitPrefix,
    ];

    pub fn required(self) -> bool {
        !matches!(self, Self::SplitPrefix)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::SourceVocab => "src_vocab",
            Self::TargetVocab => "trg_vocab",
            Self::LexicalShortlist => "lex_shortlist",
            Self::SplitPrefix => "split_prefix",
        }
    }
}

impl fmt::Display for FileRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 单个文件的期望属性。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSpec {
    pub role: FileRole,
    pub path: PathBuf,
    pub bytes: u64,
    /// 小写十六进制 sha256
    pub sha256: String,
}

/// 一次加载要用的模型集合。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelManifest {
    pub package_id: String,
    pub version: String,
    pub files: BTreeMap<FileRole, FileSpec>,
}

/// 流式算 sha256（44 MB 权重不进内存，也不阻塞太久）。
pub fn sha256_file(path: &Path) -> Result<(u64, String), RuntimeError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| RuntimeError::NotInstalled(format!("{}：{e}", path.display())))?;
    let meta = file
        .metadata()
        .map_err(|e| RuntimeError::NotInstalled(format!("{}：{e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|e| {
            RuntimeError::IntegrityFailure(format!("读取 {} 失败：{e}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok((meta.len(), format!("{:x}", hasher.finalize())))
}

impl ModelManifest {
    /// 逐文件校验：存在 → 是普通文件（非符号链接）→ 大小 → sha256。
    ///
    /// 顺序不是随意的：符号链接要在打开前拒绝（否则"校验的是链接目标、用的是被换掉的目标"），
    /// 大小在摘要之前（大文件摘要很贵，先用便宜的指标挡住明显不对的输入）。
    pub fn verify(&self) -> Result<(), RuntimeError> {
        let mut missing = Vec::new();
        for role in FileRole::REQUIRED {
            let Some(spec) = self.files.get(&role) else {
                missing.push(role.as_str());
                continue;
            };
            let link_meta = std::fs::symlink_metadata(&spec.path).map_err(|e| {
                RuntimeError::NotInstalled(format!("{}（{}）：{e}", spec.path.display(), role))
            })?;
            if link_meta.file_type().is_symlink() {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{}（{}）是符号链接，模型集合不接受链接",
                    spec.path.display(),
                    role
                )));
            }
            if !link_meta.is_file() {
                return Err(RuntimeError::NotInstalled(format!(
                    "{}（{}）不是普通文件",
                    spec.path.display(),
                    role
                )));
            }
            if link_meta.len() != spec.bytes {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{}（{}）大小不符：期望 {} 字节，实际 {}",
                    spec.path.display(),
                    role,
                    spec.bytes,
                    link_meta.len()
                )));
            }
        }
        if !missing.is_empty() {
            return Err(RuntimeError::IntegrityFailure(format!(
                "模型集合不完整，缺少：{}",
                missing.join(", ")
            )));
        }
        // 摘要最后做，且要做
        for role in FileRole::ALL {
            let Some(spec) = self.files.get(&role) else {
                continue;
            };
            let (_, actual) = sha256_file(&spec.path)?;
            if !actual.eq_ignore_ascii_case(&spec.sha256) {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{}（{}）sha256 不符：期望 {}，实际 {}",
                    spec.path.display(),
                    role,
                    spec.sha256,
                    actual
                )));
            }
        }
        Ok(())
    }

    /// 内容摘要：进入缓存/诊断的"模型身份"（`docs/12` §2.2 第 4 条）。
    ///
    /// 用"主模型 sha256 前 16 字节 + package@version"而不是全量拼接：
    /// 权重才是决定译文的东西，词表/短名单的变化已经在 bundle 语义里被版本覆盖；
    /// 等 P3.1-D 的 `ResolvedExecutionPlan` 上来后再按需扩展成全字段哈希。
    pub fn content_digest(&self) -> String {
        let model = self
            .files
            .get(&FileRole::Model)
            .map(|f| f.sha256.as_str())
            .unwrap_or("missing");
        let short: String = model.chars().take(32).collect();
        format!("{}@{}#{}", self.package_id, self.version, short)
    }
}

/// 单个文件的期望属性（进入协议：worker 要在**打开之前**再核一次）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDigest {
    pub bytes: u64,
    pub sha256: String,
}

/// 传给 worker 的加载参数（要能 JSON 序列化，所以路径用 String）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub package_id: String,
    pub version: String,
    pub model_path: String,
    pub src_vocab: String,
    pub trg_vocab: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lex_shortlist: Option<String>,
    /// 可选的 ssplit non-breaking prefix 表
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_prefix: Option<String>,
    pub threads: u16,
    pub beam_size: u8,
    /// 进入诊断与缓存键的内容摘要（`ModelManifest::content_digest()`）
    pub model_revision: String,
    /// 每个角色的期望大小与 sha256。worker 侧在 spawn native 之前**重新核验**，
    /// 这样"宿主校验过"和"native 实际打开的文件"之间不再只靠信任传递。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub digests: BTreeMap<FileRole, FileDigest>,
}

impl ModelSpec {
    /// 从已校验的清单构造。**不在校验前调用**——顺序由类型之外保证，这里只做映射。
    pub fn from_manifest(manifest: &ModelManifest, threads: u16, beam_size: u8) -> Self {
        let path_of = |role: FileRole| {
            manifest
                .files
                .get(&role)
                .map(|f| f.path.display().to_string())
                .unwrap_or_default()
        };
        Self {
            package_id: manifest.package_id.clone(),
            version: manifest.version.clone(),
            model_path: path_of(FileRole::Model),
            src_vocab: path_of(FileRole::SourceVocab),
            trg_vocab: path_of(FileRole::TargetVocab),
            lex_shortlist: manifest
                .files
                .get(&FileRole::LexicalShortlist)
                .map(|f| f.path.display().to_string()),
            split_prefix: manifest
                .files
                .get(&FileRole::SplitPrefix)
                .map(|f| f.path.display().to_string()),
            threads,
            beam_size,
            model_revision: manifest.content_digest(),
            digests: manifest
                .files
                .iter()
                .map(|(role, spec)| {
                    (
                        *role,
                        FileDigest {
                            bytes: spec.bytes,
                            sha256: spec.sha256.clone(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// 轻量前置检查：文件在、是普通文件、非空。
    ///
    /// 传输层在 **spawn worker 之前**调用它——哪怕调用方忘了 `ModelManifest::verify()`，
    /// 也不会把"文件不存在"这种错误送进 native 去触发 abort。
    pub fn verify_paths(&self) -> Result<(), RuntimeError> {
        let mut required = vec![
            (FileRole::Model, &self.model_path),
            (FileRole::SourceVocab, &self.src_vocab),
            (FileRole::TargetVocab, &self.trg_vocab),
        ];
        if let Some(lex) = &self.lex_shortlist {
            required.push((FileRole::LexicalShortlist, lex));
        }
        for (role, path) in required {
            let meta = std::fs::symlink_metadata(path)
                .map_err(|e| RuntimeError::NotInstalled(format!("{path}（{role}）：{e}")))?;
            if meta.file_type().is_symlink() {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{path}（{role}）是符号链接，模型集合不接受链接"
                )));
            }
            if !meta.is_file() {
                return Err(RuntimeError::NotInstalled(format!(
                    "{path}（{role}）不是普通文件"
                )));
            }
            if meta.len() == 0 {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{path}（{role}）是空文件"
                )));
            }
        }
        Ok(())
    }

    /// **worker 侧**的加载前核验：按 `digests` 重新算一遍大小与 sha256。
    ///
    /// 与宿主的 `verify_paths()` 的区别：宿主只能证明"路径上的东西现在对"，
    /// 真正的信任链要在**打开之前、由将要打开它的人**再确认一次（`docs/14` §3.5 第 5 条）。
    /// 摘要为空时不校验（mock worker / 旧调用方）。
    pub fn verify_digests(&self) -> Result<(), RuntimeError> {
        if self.digests.is_empty() {
            return Ok(());
        }
        for role in FileRole::ALL {
            let Some(expected) = self.digests.get(&role) else {
                continue;
            };
            let path = self.path_of(role).ok_or_else(|| {
                RuntimeError::IntegrityFailure(format!("{role} 在 digests 里有、路径却缺失"))
            })?;
            let (bytes, sha256) = sha256_file(Path::new(path))?;
            if bytes != expected.bytes {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{path}（{role}）大小不符：期望 {}，实际 {bytes}",
                    expected.bytes
                )));
            }
            if !sha256.eq_ignore_ascii_case(&expected.sha256) {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{path}（{role}）sha256 不符：期望 {}，实际 {sha256}",
                    expected.sha256
                )));
            }
        }
        Ok(())
    }

    /// 角色 → 当前 spec 里的路径。
    pub fn path_of(&self, role: FileRole) -> Option<&str> {
        match role {
            FileRole::Model => Some(&self.model_path),
            FileRole::SourceVocab => Some(&self.src_vocab),
            FileRole::TargetVocab => Some(&self.trg_vocab),
            FileRole::LexicalShortlist => self.lex_shortlist.as_deref(),
            FileRole::SplitPrefix => self.split_prefix.as_deref(),
        }
    }

    /// 字节总量（用于预算的**已知部分**；不含 native 工作区）。
    pub fn declared_bytes(&self) -> u64 {
        [
            Some(&self.model_path),
            Some(&self.src_vocab),
            Some(&self.trg_vocab),
            self.lex_shortlist.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum()
    }
}

/// 翻译任务。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationJob {
    pub text: String,
    pub target: String,
}

/// 翻译结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationReply {
    pub text: String,
    /// worker 侧计时（不含 IPC 往返）
    pub worker_elapsed_ms: u64,
    /// 产生这条译文的 worker 世代
    pub worker_epoch: u64,
    /// 产生这条译文的模型内容摘要
    pub model_revision: String,
}

/// 加载完成后的模型信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub engine_id: String,
    pub model_revision: String,
    pub threads: u16,
    pub beam_size: u8,
    pub worker_epoch: u64,
    /// worker 内部真正跑推理的东西（`mock` / `bergamot-cli`），**不是**对外承诺
    pub runtime: Option<String>,
    /// 运行时版本（Bergamot commit / Marian 子模块），未知时 `None`
    pub runtime_build: Option<String>,
    /// 平台三元组
    pub platform: Option<String>,
    /// native 进程 pid（`docs/14` §3.7 的可核对身份；仅在 Linux/有 native 时存在）
    pub native_pid: Option<u32>,
}

/// worker 心跳结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerHealth {
    pub alive: bool,
    pub requests_served: u64,
    /// worker 进程的 RSS（Linux 读 `/proc/<pid>/status`；其它平台为 `None`）
    pub worker_rss_bytes: Option<u64>,
    pub worker_pid: u32,
    pub worker_epoch: u64,
}

/// 传输层指标（`docs/12` §2.4：宿主与 worker 分开记）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransportMetrics {
    pub spawns: u32,
    pub respawns: u32,
    pub epoch: u64,
    pub host_rss_bytes: Option<u64>,
    pub worker_rss_bytes: Option<u64>,
    pub worker_pid: Option<u32>,
    /// 最近一次 `model_preparation_ms`（复制 + 校验快照）
    pub model_preparation_ms: Option<u64>,
    /// 当前快照占用字节数
    pub snapshot_bytes: Option<u64>,
    /// 本次加载前清理掉的崩溃残留快照数（`docs/17` §4.1.6）。
    pub stale_snapshots_removed: u32,
    /// 因为持有者仍存活而保留的残留快照数。
    pub stale_snapshots_kept: u32,
}

impl TransportMetrics {
    /// 宿主 + worker 的合计 RSS。
    ///
    /// 注意：这是**两个进程 RSS 的相加**，共享的文件页会被重复计入，
    /// 不等于系统物理内存增量（`docs/12` §2.4）。要精确必须看 PSS。
    pub fn combined_rss_bytes(&self) -> Option<u64> {
        match (self.host_rss_bytes, self.worker_rss_bytes) {
            (Some(h), Some(w)) => Some(h + w),
            (Some(h), None) => Some(h),
            (None, Some(w)) => Some(w),
            (None, None) => None,
        }
    }
}

/// 当前进程 RSS（Linux）。非 Linux 返回 `None`，**不猜**。
pub fn process_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/self/status").ok()?;
        parse_status_rss(&text)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// 任意 pid 的 **峰值** RSS（`VmHWM`，Linux）。进程退出后不可读。
///
/// `docs/14` §6 要求峰值与稳态分开报：`VmRSS` 是"现在多少"，`VmHWM` 是"最多到过多少"。
pub fn vmhwm_bytes_of(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("VmHWM:") {
                let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kib * 1024);
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// 任意 pid 的 RSS（Linux）。
pub fn rss_bytes_of(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        parse_status_rss(&text)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "linux")]
fn parse_status_rss(text: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kib * 1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn spec_for(role: FileRole, path: &Path, body: &[u8]) -> FileSpec {
        let (bytes, sha256) = sha256_file(path).unwrap();
        assert_eq!(bytes, body.len() as u64);
        FileSpec {
            role,
            path: path.to_path_buf(),
            bytes,
            sha256,
        }
    }

    fn manifest_of(dir: &Path) -> ModelManifest {
        let mut files = BTreeMap::new();
        for role in FileRole::ALL {
            let body = format!("body-of-{role}").into_bytes();
            let path = temp_file(dir, &format!("{role}.bin"), &body);
            files.insert(role, spec_for(role, &path, &body));
        }
        ModelManifest {
            package_id: "test.model".into(),
            version: "1.0.0".into(),
            files,
        }
    }

    #[test]
    fn 四文件齐全时校验通过且摘要稳定() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = manifest_of(dir.path());
        manifest.verify().unwrap();
        let digest = manifest.content_digest();
        assert!(digest.starts_with("test.model@1.0.0#"), "{digest}");
        assert_eq!(
            digest,
            manifest.content_digest(),
            "同一份文件必须给出同一摘要"
        );

        let spec = ModelSpec::from_manifest(&manifest, 1, 1);
        spec.verify_paths().unwrap();
        assert_eq!(spec.threads, 1);
        assert!(spec.lex_shortlist.is_some());
        assert_eq!(spec.model_revision, digest);
    }

    #[test]
    fn 缺一个文件时报不完整而不是等在native里崩() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = manifest_of(dir.path());
        manifest.files.remove(&FileRole::LexicalShortlist);
        let err = manifest.verify().unwrap_err();
        assert_eq!(
            err.code(),
            translator_types::TranslationErrorCode::IntegrityFailure
        );
        assert!(err.to_string().contains("不完整"), "{err}");
    }

    #[test]
    fn 文件被改动后摘要不符() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = manifest_of(dir.path());
        // 改一个字节，大小不变 → 只能靠摘要发现
        let model = &manifest.files[&FileRole::Model];
        let mut body = std::fs::read(&model.path).unwrap();
        let last = body.len() - 1;
        body[last] ^= 0xff;
        std::fs::write(&model.path, &body).unwrap();

        let err = manifest.verify().unwrap_err();
        assert_eq!(
            err.code(),
            translator_types::TranslationErrorCode::IntegrityFailure
        );
        assert!(err.to_string().contains("sha256 不符"), "{err}");
    }

    #[test]
    fn 大小不符在算摘要之前就被拒绝() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = manifest_of(dir.path());
        let model = &manifest.files[&FileRole::Model];
        std::fs::write(&model.path, b"short").unwrap();
        let err = manifest.verify().unwrap_err();
        assert!(err.to_string().contains("大小不符"), "{err}");
    }

    #[test]
    fn 路径缺失报未安装() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = manifest_of(dir.path());
        let model = manifest.files.get_mut(&FileRole::Model).unwrap();
        model.path = dir.path().join("nope.bin");
        let err = manifest.files[&FileRole::Model].clone();
        let err = match std::fs::symlink_metadata(&err.path) {
            Ok(_) => panic!("文件不该存在"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

        let err = manifest.verify().unwrap_err();
        assert_eq!(
            err.code(),
            translator_types::TranslationErrorCode::ResourceNotInstalled
        );
    }

    #[cfg(unix)]
    #[test]
    fn 符号链接被拒绝() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = manifest_of(dir.path());
        let real = manifest.files[&FileRole::TargetVocab].path.clone();
        let link = dir.path().join("link-to-vocab.bin");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let entry = manifest.files.get_mut(&FileRole::TargetVocab).unwrap();
        entry.path = link;
        let err = manifest.verify().unwrap_err();
        assert_eq!(
            err.code(),
            translator_types::TranslationErrorCode::IntegrityFailure
        );
        assert!(err.to_string().contains("符号链接"), "{err}");
    }

    #[test]
    fn 空文件不通过路径检查() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = manifest_of(dir.path());
        let spec_path = dir.path().join("empty.bin");
        std::fs::write(&spec_path, b"").unwrap();
        let mut spec = ModelSpec::from_manifest(&manifest, 1, 1);
        spec.model_path = spec_path.display().to_string();
        let err = spec.verify_paths().unwrap_err();
        assert!(err.to_string().contains("空文件"), "{err}");
    }

    #[test]
    fn 当前进程能读到rss或明确为none() {
        match process_rss_bytes() {
            Some(bytes) => assert!(bytes > 1024 * 1024, "RSS 至少几 MB：{bytes}"),
            None => {
                // 非 Linux：必须返回 None，不能编一个 0。这里不需要断言——
                // 走到这个分支本身就是被验证的行为（`assert!(true)` 只会惹 clippy）。
                #[cfg(target_os = "linux")]
                panic!("Linux 上应当能读到 /proc/self/status");
            }
        }
    }
}
