//! 资源仓库：安装 → 校验 → 原子激活 → 回滚 → 安全清理（P2.2）。
//!
//! # 目录布局
//!
//! ```text
//! <root>/
//! ├── state.json                     # 唯一可变文件：每个 id 的激活版本与历史
//! ├── installed/<id>/<version>/…     # 已安装版本，落盘后视为不可变
//! └── staging/<id>-<version>-<nonce>/# 安装中目录；进程崩溃后由 recover() 清理
//! ```
//!
//! # 三条不变量
//!
//! 1. **安装失败不影响已装版本**：所有校验都在 `staging/` 里做完，只有全部通过才
//!    `rename` 到 `installed/`；`state.json` 最后写，且是"写临时文件 + 原子 rename"。
//! 2. **激活版本不可删除**：`uninstall` 对当前激活版本返回 `ResourceInUse`。
//! 3. **不做隐式热切换**：安装新版本默认不改变激活版本（首次安装除外），
//!    切换必须显式调用 `activate`。P2.3 引入租约之前，这是避免"活跃请求读到半新半旧"
//!    的唯一保守做法。
//!
//! # 已知限制（P2.3 会改）
//!
//! - 没有租约：无法知道"某个旧版本是否仍被打开的 SQLite/推理句柄使用"。
//!   Unix 上 `unlink` 对 mmap 是安全的，Windows 上若文件仍被打开，删除会失败并返回 IO 错误；
//!   因此清理旧版本属于**显式动作**，不自动发生。
//! - 没有跨进程锁：同一仓库被两个进程同时写 `state.json` 不受保护（P2.3 加文件锁）。
//! - 不做磁盘空间预检：`OutOfBudget`/空间不足会以 IO 错误形式返回。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::compat::{check, ensure_compatible, HostSupport};
use crate::error::{ResourceError, ResourceResult};
use crate::manifest::{is_unsafe_relative, ResourceKind, ResourceManifest};

const STATE_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default = "state_schema")]
    schema: u32,
    #[serde(default)]
    resources: BTreeMap<String, Entry>,
}

fn state_schema() -> u32 {
    STATE_SCHEMA
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Entry {
    #[serde(default)]
    active: Option<String>,
    /// 最近激活过的旧版本（栈顶在前）。
    #[serde(default)]
    history: Vec<String>,
    #[serde(default)]
    installed: BTreeMap<String, Meta>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Meta {
    kind: String,
    payload_schema: String,
    installed_at: String,
}

/// 已安装的一个版本。
#[derive(Clone, Debug)]
pub struct InstalledResource {
    pub id: String,
    pub version: String,
    pub kind: ResourceKind,
    pub payload_schema: String,
    pub path: PathBuf,
    pub active: bool,
    pub installed_at: String,
}

/// 一个资源的完整状态。
#[derive(Clone, Debug)]
pub struct ResourceStatus {
    pub id: String,
    pub active: Option<String>,
    pub history: Vec<String>,
    pub versions: Vec<InstalledResource>,
}

/// 安装结果。
#[derive(Clone, Debug)]
pub struct InstallOutcome {
    pub resource: InstalledResource,
    /// 是否顺带激活（仅当该 id 之前没有任何激活版本）。
    pub activated: bool,
    pub previous_active: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RecoverReport {
    pub removed_staging: usize,
}

pub struct ResourceStore {
    root: PathBuf,
    /// 进程内串行化，避免两个线程同时改 `state.json`。
    lock: Mutex<()>,
}

impl std::fmt::Debug for ResourceStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceStore")
            .field("root", &self.root)
            .finish()
    }
}

impl ResourceStore {
    /// 打开（必要时创建）仓库根目录。
    pub fn open(root: impl AsRef<Path>) -> ResourceResult<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("installed"))?;
        fs::create_dir_all(root.join("staging"))?;
        let store = Self {
            root,
            lock: Mutex::new(()),
        };
        // 打开即解析一次状态文件：损坏要在启动时暴露，而不是等到安装到一半
        let _ = store.read_state()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 某个版本的目录。
    pub fn version_dir(&self, id: &str, version: &str) -> ResourceResult<PathBuf> {
        validate_component("id", id)?;
        validate_component("version", version)?;
        Ok(self.root.join("installed").join(id).join(version))
    }

    /// 安装一个资源目录（必须含 `manifest.json`）。
    ///
    /// 步骤：读清单 → 兼容性检查 → 复制到 staging → 逐文件 size+sha256 校验 →
    /// 原子 rename 到 `installed/` → 写状态。任何一步失败都会清掉 staging。
    pub fn install_from_dir(&self, src: impl AsRef<Path>) -> ResourceResult<InstallOutcome> {
        let src = src.as_ref();
        let manifest = read_manifest_from(src)?;

        validate_component("id", &manifest.id)?;
        validate_component("version", &manifest.version)?;

        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.read_state()?;

        // 已安装集合（含激活版本）用于依赖检查
        let installed_versions = self.installed_versions(&state, &manifest.id);
        if installed_versions.contains(&manifest.version) {
            return Err(ResourceError::AlreadyInstalled {
                id: manifest.id.clone(),
                version: manifest.version.clone(),
            });
        }

        let available: BTreeMap<String, String> = state
            .resources
            .iter()
            .filter_map(|(id, entry)| entry.active.clone().map(|v| (id.clone(), v)))
            .collect();
        ensure_compatible(&check(&manifest, &HostSupport::current(), &available))?;

        // 1) 复制到 staging
        let staging = self.staging_dir(&manifest.id, &manifest.version);
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;

        let result = (|| -> ResourceResult<()> {
            copy_tree(src, &staging)?;
            // 2) 逐文件校验（在 staging 上做，避免污染源目录/已装目录）
            verify_files(&staging, &manifest)?;
            Ok(())
        })();

        if let Err(err) = result {
            let _ = fs::remove_dir_all(&staging);
            return Err(err);
        }

        // 3) 原子落位
        let target = self.version_dir(&manifest.id, &manifest.version)?;
        if target.exists() {
            let _ = fs::remove_dir_all(&staging);
            return Err(ResourceError::AlreadyInstalled {
                id: manifest.id.clone(),
                version: manifest.version.clone(),
            });
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Err(err) = fs::rename(&staging, &target) {
            let _ = fs::remove_dir_all(&staging);
            return Err(err.into());
        }

        // 4) 写状态：新版本默认不夺取激活位（首次安装除外）
        let entry = state.resources.entry(manifest.id.clone()).or_default();
        let previous_active = entry.active.clone();
        let activated = previous_active.is_none();
        entry.installed.insert(
            manifest.version.clone(),
            Meta {
                kind: manifest.kind.as_str().to_string(),
                payload_schema: manifest.payload_schema.as_str().to_string(),
                installed_at: now_string(),
            },
        );
        if activated {
            entry.active = Some(manifest.version.clone());
        }
        // 状态写失败时不能让磁盘上留下一个"已安装但无记录"的目录
        if let Err(err) = self.write_state(&state) {
            let _ = fs::remove_dir_all(&target);
            return Err(err);
        }

        Ok(InstallOutcome {
            resource: InstalledResource {
                id: manifest.id,
                version: manifest.version,
                kind: manifest.kind,
                payload_schema: manifest.payload_schema.as_str().to_string(),
                path: target,
                active: activated,
                installed_at: now_string(),
            },
            activated,
            previous_active,
        })
    }

    /// 激活某个已安装版本。**不删除任何东西**，旧版本进入 history。
    pub fn activate(&self, id: &str, version: &str) -> ResourceResult<()> {
        validate_component("id", id)?;
        validate_component("version", version)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.read_state()?;

        let entry = state
            .resources
            .get_mut(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;
        if !entry.installed.contains_key(version) {
            return Err(ResourceError::NotInstalled {
                id: id.to_string(),
                version: version.to_string(),
            });
        }
        if entry.active.as_deref() == Some(version) {
            return Ok(());
        }

        if let Some(prev) = entry.active.replace(version.to_string()) {
            entry.history.retain(|v| v != &prev);
            entry.history.insert(0, prev);
        }
        // 被激活的版本不应再留在 history 里
        entry.history.retain(|v| v != version);

        self.write_state(&state)
    }

    /// 回滚到上一个激活过的版本，返回回滚到的版本号。
    pub fn rollback(&self, id: &str) -> ResourceResult<String> {
        validate_component("id", id)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.read_state()?;

        let entry = state
            .resources
            .get_mut(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;
        let previous = entry
            .history
            .first()
            .cloned()
            .ok_or_else(|| ResourceError::NotFound(format!("{id} 没有可回滚的历史版本")))?;
        if !entry.installed.contains_key(&previous) {
            return Err(ResourceError::NotInstalled {
                id: id.to_string(),
                version: previous,
            });
        }

        entry.history.remove(0);
        if let Some(current) = entry.active.replace(previous.clone()) {
            entry.history.retain(|v| v != &current);
            entry.history.insert(0, current);
        }
        entry.history.retain(|v| v != &previous);

        self.write_state(&state)?;
        Ok(previous)
    }

    /// 删除一个非激活版本。
    ///
    /// 激活版本一律拒绝（`ResourceInUse`）；P2.3 引入租约后会进一步拒绝"仍被持有"的版本。
    pub fn uninstall(&self, id: &str, version: &str) -> ResourceResult<()> {
        validate_component("id", id)?;
        validate_component("version", version)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.read_state()?;

        let entry = state
            .resources
            .get_mut(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;
        if !entry.installed.contains_key(version) {
            return Err(ResourceError::NotInstalled {
                id: id.to_string(),
                version: version.to_string(),
            });
        }
        if entry.active.as_deref() == Some(version) {
            return Err(ResourceError::ResourceInUse {
                id: id.to_string(),
                version: version.to_string(),
            });
        }

        let dir = self.version_dir(id, version)?;
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        entry.installed.remove(version);
        entry.history.retain(|v| v != version);
        if entry.installed.is_empty() {
            state.resources.remove(id);
        }

        self.write_state(&state)
    }

    /// 保留激活版本 + 最近 `keep` 个历史版本，删除更旧的版本。
    ///
    /// 返回被删除的版本号。这是"安全清理"：绝不会删激活版本，也不碰 `keep` 之内的版本。
    ///
    /// 注意：这里**不能**在持锁状态下调用 `uninstall`——`std::sync::Mutex` 不可重入，
    /// 那样会直接死锁（本项目在集成测试里踩到过一次，保留注释以免复现）。
    pub fn cleanup(&self, id: &str, keep: usize) -> ResourceResult<Vec<String>> {
        validate_component("id", id)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.read_state()?;

        let entry = state
            .resources
            .get(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;

        let active = entry.active.clone();
        let keep_set: std::collections::BTreeSet<String> =
            entry.history.iter().take(keep).cloned().collect();

        let victims: Vec<String> = entry
            .installed
            .keys()
            .filter(|v| Some((*v).clone()) != active && !keep_set.contains(*v))
            .cloned()
            .collect();

        if victims.is_empty() {
            return Ok(Vec::new());
        }

        // 先删目录：删不掉就整体失败，状态保持不变（不会出现"记录没了但文件还在"）
        for version in &victims {
            let dir = self.version_dir(id, version)?;
            if dir.exists() {
                fs::remove_dir_all(&dir)?;
            }
        }

        let entry = state
            .resources
            .get_mut(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;
        for version in &victims {
            entry.installed.remove(version);
            entry.history.retain(|v| v != version);
        }

        self.write_state(&state)?;
        Ok(victims)
    }

    pub fn list(&self) -> ResourceResult<Vec<ResourceStatus>> {
        let state = self.read_state()?;
        let mut out = Vec::new();
        for (id, entry) in &state.resources {
            out.push(self.status_of(id, entry)?);
        }
        Ok(out)
    }

    pub fn inspect(&self, id: &str) -> ResourceResult<ResourceStatus> {
        let state = self.read_state()?;
        let entry = state
            .resources
            .get(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;
        self.status_of(id, entry)
    }

    /// 当前激活版本的目录（给 `translator-api` 挂载用）。
    pub fn active_dir(&self, id: &str) -> ResourceResult<PathBuf> {
        let state = self.read_state()?;
        let entry = state
            .resources
            .get(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;
        let active = entry
            .active
            .as_ref()
            .ok_or_else(|| ResourceError::NotFound(format!("{id} 没有激活版本")))?;
        self.version_dir(id, active)
    }

    /// 读取当前激活版本的清单。
    pub fn active_manifest(&self, id: &str) -> ResourceResult<ResourceManifest> {
        let dir = self.active_dir(id)?;
        read_manifest_from(&dir)
    }

    /// 清理遗留的 staging 目录（上次安装被中断时留下）。
    pub fn recover(&self) -> ResourceResult<RecoverReport> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let staging_root = self.root.join("staging");
        let mut report = RecoverReport::default();
        if !staging_root.exists() {
            return Ok(report);
        }
        for entry in fs::read_dir(&staging_root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
                report.removed_staging += 1;
            }
        }
        Ok(report)
    }

    // ---------------- 内部 ----------------

    fn status_of(&self, id: &str, entry: &Entry) -> ResourceResult<ResourceStatus> {
        let mut versions = Vec::new();
        for (version, meta) in &entry.installed {
            versions.push(InstalledResource {
                id: id.to_string(),
                version: version.clone(),
                kind: ResourceKind::Unknown(meta.kind.clone()),
                payload_schema: meta.payload_schema.clone(),
                path: self.version_dir(id, version)?,
                active: entry.active.as_deref() == Some(version.as_str()),
                installed_at: meta.installed_at.clone(),
            });
        }
        Ok(ResourceStatus {
            id: id.to_string(),
            active: entry.active.clone(),
            history: entry.history.clone(),
            versions,
        })
    }

    fn installed_versions(&self, state: &State, id: &str) -> Vec<String> {
        state
            .resources
            .get(id)
            .map(|e| e.installed.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn staging_dir(&self, id: &str, version: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        self.root
            .join("staging")
            .join(format!("{id}-{version}-{}-{nonce}", std::process::id()))
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    fn read_state(&self) -> ResourceResult<State> {
        let path = self.state_path();
        if !path.exists() {
            return Ok(State {
                schema: STATE_SCHEMA,
                resources: BTreeMap::new(),
            });
        }
        let text = fs::read_to_string(&path)?;
        if text.trim().is_empty() {
            return Ok(State {
                schema: STATE_SCHEMA,
                resources: BTreeMap::new(),
            });
        }
        let state: State = serde_json::from_str(&text)
            .map_err(|e| ResourceError::CorruptState(format!("{}: {e}", path.display())))?;
        if state.schema != STATE_SCHEMA {
            return Err(ResourceError::CorruptState(format!(
                "不支持的 state schema={}（本 SDK 为 {STATE_SCHEMA}）",
                state.schema
            )));
        }
        Ok(state)
    }

    /// 原子写：临时文件 + fsync + rename，避免断电留下半个 JSON。
    fn write_state(&self, state: &State) -> ResourceResult<()> {
        let path = self.state_path();
        let tmp = self.root.join(format!(
            "state.json.tmp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let json = serde_json::to_string_pretty(state)?;
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// 从目录读取清单（不做文件校验）。
pub fn read_manifest_from(dir: impl AsRef<Path>) -> ResourceResult<ResourceManifest> {
    let path = dir.as_ref().join("manifest.json");
    let text = fs::read_to_string(&path)
        .map_err(|e| ResourceError::InvalidManifest(format!("无法读取 {}: {e}", path.display())))?;
    ResourceManifest::parse(&text)
}

/// 校验清单列出的每个文件：存在、大小一致、sha256 一致。
pub fn verify_files(dir: impl AsRef<Path>, manifest: &ResourceManifest) -> ResourceResult<()> {
    let dir = dir.as_ref();
    for file in &manifest.files {
        if is_unsafe_relative(&file.path) {
            return Err(ResourceError::UnsafePath(file.path.clone()));
        }
        let path = dir.join(&file.path);
        let meta = fs::metadata(&path).map_err(|_| ResourceError::IntegrityFailure {
            path: file.path.clone(),
            expected: format!("{} 字节", file.bytes),
            actual: "文件缺失".to_string(),
        })?;
        if meta.len() != file.bytes {
            return Err(ResourceError::IntegrityFailure {
                path: file.path.clone(),
                expected: format!("{} 字节", file.bytes),
                actual: format!("{} 字节", meta.len()),
            });
        }
        let actual = sha256_file(&path)?;
        if actual != file.sha256 {
            return Err(ResourceError::IntegrityFailure {
                path: file.path.clone(),
                expected: file.sha256.clone(),
                actual,
            });
        }
    }
    Ok(())
}

pub fn sha256_file(path: impl AsRef<Path>) -> ResourceResult<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = fs::File::open(path.as_ref())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 复制目录树。**不跟随符号链接**：资源包里出现链接一律拒绝。
fn copy_tree(src: &Path, dst: &Path) -> ResourceResult<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_symlink() {
            return Err(ResourceError::SymlinkNotAllowed(
                entry.path().display().to_string(),
            ));
        }
        if ty.is_dir() {
            fs::create_dir_all(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), &target)?;
        }
        // 其它类型（管道、socket）直接忽略：资源包里不该出现
    }
    Ok(())
}

fn validate_component(field: &str, value: &str) -> ResourceResult<()> {
    if value.trim().is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value.contains("..")
        || value == "."
    {
        return Err(ResourceError::InvalidVersion(format!(
            "{field}={value:?} 不能用作目录名"
        )));
    }
    Ok(())
}

fn now_string() -> String {
    // 不引入 chrono：把 Unix 秒直接写成十进制，排序与比较够用
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("unix:{}", d.as_secs()),
        Err(_) => "unix:0".to_string(),
    }
}
