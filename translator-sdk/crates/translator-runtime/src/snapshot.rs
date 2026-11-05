//! 模型**隔离快照**：把已校验的文件复制进一个私有目录，之后 native 只认这个目录。
//!
//! `docs/14` §3.5 的结论是明确的：`sha256(path)` 之后再 `open(path)` 存在 TOCTOU 窗口，
//! "更频繁地检查路径"不是修复。真正的修复是把**身份**绑定到一个不会被别人改的副本上：
//!
//! 1. 先对源文件做完整校验（存在/非链接/大小/摘要）；
//! 2. 复制到 `snap-<pid>-<seq>-<digest8>/`（0o700 私有目录，文件 0o400 只读）；
//! 3. **在副本上重新算一遍**摘要，与清单比对——这一步证明"复制过程中没有被换"；
//! 4. 之后 worker 只拿到副本路径，宿主也不再碰源文件；
//! 5. worker 打开之前再用 `ModelSpec::verify_digests()` 核一次（见 `model.rs`）。
//!
//! 诚实边界（不能靠文件权限假装拥有）：同一用户下的恶意进程仍可改写这些文件；
//! 0o400 只挡"自己手滑"，不是安全沙箱。真正的跨用户隔离要靠目录权限/容器。
//!
//! 代价：约 51 MB 的复制 I/O 与磁盘占用（P3.0 四文件 43.8 + 0.8 + 0.8 + 6.5 MB）。
//! 这个代价在 `docs/15` 里有实测；D 阶段可换成内容寻址仓库避免每次复制。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::error::RuntimeError;
use crate::model::{FileDigest, FileRole, ModelManifest, ModelSpec};

/// 一个已经落盘并通过校验的模型副本。
pub struct ModelSnapshot {
    dir: PathBuf,
    manifest: ModelManifest,
    spec: ModelSpec,
    digests: BTreeMap<FileRole, FileDigest>,
    bytes: u64,
    prepare_ms: f64,
    keep: bool,
}

impl ModelSnapshot {
    /// 复制 + 校验。`root` 必须是已存在且可写的目录（由调用方决定放哪）。
    pub fn materialize(
        manifest: &ModelManifest,
        root: &Path,
        threads: u16,
        beam_size: u8,
    ) -> Result<Self, RuntimeError> {
        let started = Instant::now();
        let digest = manifest.content_digest();
        let short = digest.rsplit('#').next().unwrap_or("nosha");
        let name = format!(
            "snap-{}-{}-{}",
            std::process::id(),
            next_seq(),
            &short[..short.len().min(8)]
        );
        let dir = root.join(&name);

        // 1) 便宜的预检查：缺文件/截断/符号链接在这里就失败，不做无用的复制。
        //    （真正的**权威**校验是第 2 步"对复制出来的字节算摘要"，见模块注释。）
        for role in FileRole::ALL {
            let Some(source) = manifest.files.get(&role) else {
                continue;
            };
            let meta = std::fs::symlink_metadata(&source.path).map_err(|e| {
                RuntimeError::NotInstalled(format!("{}（{role}）：{e}", source.path.display()))
            })?;
            if meta.file_type().is_symlink() {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{}（{role}）是符号链接，模型集合不接受链接",
                    source.path.display()
                )));
            }
            if !meta.is_file() {
                return Err(RuntimeError::NotInstalled(format!(
                    "{}（{role}）不是普通文件",
                    source.path.display()
                )));
            }
            if meta.len() != source.bytes {
                return Err(RuntimeError::IntegrityFailure(format!(
                    "{}（{role}）大小不符：期望 {}，实际 {}",
                    source.path.display(),
                    source.bytes,
                    meta.len()
                )));
            }
        }

        std::fs::create_dir_all(&dir).map_err(|e| {
            RuntimeError::InitFailed(format!("无法创建快照目录 {}：{e}", dir.display()))
        })?;
        restrict_dir(&dir)?;

        // 2) 复制与摘要同一次读：复制出来的字节就是被校验的字节。
        //    先"校验源文件、再复制"中间有窗口；这样写窗口不存在。
        let mut files = BTreeMap::new();
        let mut bytes = 0u64;
        for role in FileRole::ALL {
            let Some(source) = manifest.files.get(&role) else {
                continue;
            };
            let ext = source
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("bin");
            let target = dir.join(format!("{}.{}", role.as_str(), ext));
            let (actual_bytes, actual_sha) = match copy_with_sha256(&source.path, &target) {
                Ok(pair) => pair,
                Err(err) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(err);
                }
            };
            if actual_bytes != source.bytes || !actual_sha.eq_ignore_ascii_case(&source.sha256) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(RuntimeError::IntegrityFailure(format!(
                    "快照复制后校验失败：{}（{role}）期望 {} 字节 / {}，实际 {} 字节 / {}",
                    target.display(),
                    source.bytes,
                    source.sha256,
                    actual_bytes,
                    actual_sha
                )));
            }
            if let Err(e) = restrict_file(&target) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
            bytes += actual_bytes;
            files.insert(
                role,
                crate::model::FileSpec {
                    role,
                    path: target,
                    bytes: actual_bytes,
                    sha256: actual_sha,
                },
            );
        }

        // 以快照为准构造清单：之后所有 digest 都指向副本
        let snap_manifest = ModelManifest {
            package_id: manifest.package_id.clone(),
            version: manifest.version.clone(),
            files,
        };
        for role in FileRole::REQUIRED {
            debug_assert!(snap_manifest.files.contains_key(&role), "缺 {role}");
        }

        let digests = snap_manifest
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
            .collect();
        let spec = ModelSpec::from_manifest(&snap_manifest, threads, beam_size);

        Ok(Self {
            dir,
            manifest: snap_manifest,
            spec,
            digests,
            bytes,
            prepare_ms: started.elapsed().as_secs_f64() * 1000.0,
            keep: false,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    pub fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    pub fn digests(&self) -> &BTreeMap<FileRole, FileDigest> {
        &self.digests
    }

    /// 快照占用的字节数（= 四文件 + 可选文件）。
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// 复制 + 校验耗时（`model_preparation_ms`，`docs/14` §6 第 1 项）。
    pub fn prepare_ms(&self) -> f64 {
        self.prepare_ms
    }

    /// 保留目录（调试/取证用；正常路径应在 Drop 时清掉）。
    pub fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for ModelSnapshot {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        // 尽力而为：文件是 0o400，但目录是 0o700，删除可行
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 清理**所有者已死**的快照目录。命名里带 pid，所以判定不需要额外元数据。
///
/// 这是崩溃残留的兜底：正常 Drop 会自己删；只有 worker/宿主被 SIGKILL 时才会留下。
pub fn prune_stale_snapshots(root: &Path) -> Result<usize, RuntimeError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(RuntimeError::InitFailed(format!(
                "无法读取快照根 {}：{e}",
                root.display()
            )))
        }
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix("snap-") else {
            continue;
        };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if pid_alive(pid) {
            continue;
        }
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// 复制的同时算 sha256：一次读、一次写，返回 (字节数, 十六进制摘要)。
fn copy_with_sha256(source: &Path, target: &Path) -> Result<(u64, String), RuntimeError> {
    use sha2::{Digest, Sha256};

    let mut input = std::fs::File::open(source)
        .map_err(|e| RuntimeError::NotInstalled(format!("{}：{e}", source.display())))?;
    let mut output = std::fs::File::create(target)
        .map_err(|e| RuntimeError::InitFailed(format!("无法创建 {}：{e}", target.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let read = std::io::Read::read(&mut input, &mut buf).map_err(|e| {
            RuntimeError::IntegrityFailure(format!("读取 {} 失败：{e}", source.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        std::io::Write::write_all(&mut output, &buf[..read]).map_err(|e| {
            RuntimeError::InitFailed(format!("写入 {} 失败：{e}", target.display()))
        })?;
        total += read as u64;
    }
    std::io::Write::flush(&mut output).ok();
    Ok((total, format!("{:x}", hasher.finalize())))
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true // 判定不了就当活着，绝不误删
    }
}

fn next_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) -> Result<(), RuntimeError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| RuntimeError::InitFailed(format!("无法收紧 {} 权限：{e}", dir.display())))
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), RuntimeError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))
        .map_err(|e| RuntimeError::InitFailed(format!("无法收紧 {} 权限：{e}", path.display())))
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) -> Result<(), RuntimeError> {
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), RuntimeError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{sha256_file, FileRole, FileSpec};

    fn manifest(dir: &Path) -> ModelManifest {
        let mut files = BTreeMap::new();
        for role in [
            FileRole::Model,
            FileRole::SourceVocab,
            FileRole::TargetVocab,
            FileRole::LexicalShortlist,
        ] {
            let path = dir.join(format!("{}.bin", role.as_str()));
            std::fs::write(&path, format!("payload-of-{role}")).unwrap();
            let (bytes, sha256) = sha256_file(&path).unwrap();
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
        ModelManifest {
            package_id: "nmt.snap.test".into(),
            version: "1.0.0".into(),
            files,
        }
    }

    #[test]
    fn 快照把文件复制到私有目录并给出副本路径() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let manifest = manifest(src.path());

        let snapshot = ModelSnapshot::materialize(&manifest, root.path(), 1, 1).unwrap();
        assert!(snapshot.dir().starts_with(root.path()));
        for role in [
            FileRole::Model,
            FileRole::SourceVocab,
            FileRole::TargetVocab,
        ] {
            let path = snapshot.spec().path_of(role).unwrap();
            assert!(path.starts_with(snapshot.dir().to_str().unwrap()), "{path}");
            assert!(!path.starts_with(src.path().to_str().unwrap()), "{path}");
            assert!(Path::new(path).exists());
        }
        assert_eq!(snapshot.digests().len(), 4);
        assert!(snapshot.bytes() > 0);
        assert!(snapshot.prepare_ms() >= 0.0);
        // 副本的 revision 与清单一致（同一批字节）
        assert_eq!(snapshot.spec().model_revision, manifest.content_digest());
    }

    #[test]
    fn 源文件被替换后快照内容仍然是校验过的那份() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut manifest = manifest(src.path());
        let expected = manifest.content_digest();
        let snapshot = ModelSnapshot::materialize(&manifest, root.path(), 1, 1).unwrap();

        // 校验/复制之后篡改源文件：快照不应受影响
        let model = &mut manifest.files.get_mut(&FileRole::Model).unwrap().path;
        let mut body = std::fs::read(&*model).unwrap();
        body[0] ^= 0xff;
        std::fs::write(&*model, &body).unwrap();

        let snap_model = snapshot.spec().path_of(FileRole::Model).unwrap();
        let snap_body = std::fs::read(snap_model).unwrap();
        assert_ne!(snap_body[0], body[0], "快照内容不该跟着源文件变");
        assert_eq!(snapshot.spec().model_revision, expected);
    }

    #[test]
    fn 源文件摘要不符时拒绝复制() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let manifest = manifest(src.path());
        let model = manifest.files[&FileRole::Model].path.clone();
        std::fs::write(&model, b"tampered").unwrap();

        let err = match ModelSnapshot::materialize(&manifest, root.path(), 1, 1) {
            Ok(_) => panic!("篡改后的模型不该生成快照"),
            Err(err) => err,
        };
        assert_eq!(
            err.code(),
            translator_types::TranslationErrorCode::IntegrityFailure,
            "{err}"
        );
        // 没留下半个快照目录
        let left: Vec<_> = std::fs::read_dir(root.path()).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn 快照里的文件是只读的() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let manifest = manifest(src.path());
        let snapshot = ModelSnapshot::materialize(&manifest, root.path(), 1, 1).unwrap();
        let path = snapshot.spec().path_of(FileRole::TargetVocab).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o400, "快照文件应只读，实际 {mode:o}");
            assert!(std::fs::write(path, b"x").is_err(), "只读文件不该可写");
        }
    }

    #[test]
    fn 进程活着时不清理它的快照() {
        let src = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let manifest = manifest(src.path());
        {
            let mut snapshot = ModelSnapshot::materialize(&manifest, root.path(), 1, 1).unwrap();
            snapshot.keep();
        }
        // 目录名里的 pid 是当前进程 → 活着 → 不清
        assert_eq!(prune_stale_snapshots(root.path()).unwrap(), 0);
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_some());
    }
}
