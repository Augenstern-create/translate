//! 资源仓库：安装 → 校验 → 原子激活 → 回滚 → 安全清理 → 租约（P2.2 + P2.3）。
//!
//! # 目录布局
//!
//! ```text
//! <root>/
//! ├── state.json                     # 唯一可变文件：每个 id 的激活版本与历史
//! ├── state.lock                     # 跨进程排他锁（只在改 state.json 期间存在）
//! ├── installed/<id>/<version>/…     # 已安装版本，落盘后视为不可变
//! ├── staging/<id>-<version>-<pid>-<nonce>/# 安装中目录；进程崩溃后由 recover() 清理
//! └── leases/<id>/<version>/<pid>-<nonce>.json # 每个使用方一个租约文件
//! ```
//!
//! # 四条不变量
//!
//! 1. **安装失败不影响已装版本**：所有校验都在 `staging/` 里做完，只有全部通过才
//!    `rename` 到 `installed/`；`state.json` 最后写，且是"写临时文件 + 原子 rename"。
//! 2. **激活版本不可删除**：`uninstall` 对当前激活版本返回 `ResourceInUse`。
//! 3. **有有效租约的版本不可删除**：`uninstall` / `cleanup` 返回 `ResourceLeased`，
//!    并在错误里列出持有者 pid 与标识（P2.3）。
//! 4. **不做隐式热切换**：安装新版本默认不改变激活版本（首次安装除外），
//!    切换必须显式调用 `activate`。
//!
//! # 锁的层次
//!
//! - **进程内**：`Mutex<()>`，避免两个线程同时改状态。
//! - **跨进程**：`state.lock`（`O_EXCL` 排他创建），只在"落位 + 写 state.json"这段持有，
//!   不覆盖数 MB 的复制校验——否则一次安装会让所有进程等几秒。
//! - **租约**：每个使用方一个文件，登记/释放互不干扰，检查就是列目录。
//!
//! # 已知限制
//!
//! - 租约是**协作式**的：绕过本 SDK 直接打开资源文件的程序检测不到（见 `leases.rs`）。
//! - 不做磁盘空间预检：空间不足会以 IO 错误形式返回（staging 未落位，安全）。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::compat::{check, ensure_compatible, HostSupport};
use crate::error::{ResourceError, ResourceResult};
use crate::leases::{self, LeaseInfo, ResourceLease};
use crate::lockfile::{self, StateLock, DEFAULT_LOCK_TIMEOUT};
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
    /// 回收的失效租约数（持有进程已死）。
    pub reclaimed_leases: usize,
    /// 清理的 `state.json.tmp-*` 残留数（写状态时崩溃留下）。
    pub removed_temp_files: usize,
    /// 清理的孤儿安装目录数（目录在、state 无记录：rename 后写状态前崩溃）。
    pub removed_orphans: usize,
    /// 丢弃的失效记录数（state 有记录、目录不在）。
    pub dropped_records: usize,
}

/// 一个已安装（或疑似已安装）的版本引用，用于对账报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledRef {
    pub id: String,
    pub version: String,
    pub path: PathBuf,
}

impl std::fmt::Display for InstalledRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}（{}）", self.id, self.version, self.path.display())
    }
}

/// `state.json` 与磁盘的三方对账结果（P2-S / S-05）。
///
/// 只报告、不自动删除：`state.json` 说"已安装"而目录不在，可能是人为删除、
/// 也可能是断电丢了目录项——两者的正确处理不同，程序不该替用户决定。
/// 唯一可以自动清理的是 `staging/` 与 `state.json.tmp-*`（它们按定义是半成品）。
#[derive(Clone, Debug, Default)]
pub struct StateAudit {
    /// `state` 里有记录，磁盘上目录不存在。
    pub missing_dirs: Vec<InstalledRef>,
    /// 磁盘上有目录，`state` 里没有记录（安装到一半 / 状态被回滚）。
    pub orphan_dirs: Vec<InstalledRef>,
    /// 激活版本的目录缺失。这是最严重的一类：任何挂载都会失败。
    pub active_missing: Vec<String>,
    /// `state.json.tmp-*` 残留（写状态时崩溃）。
    pub stale_tmp_files: Vec<String>,
    /// `staging/` 里的半成品目录。
    pub staging_dirs: Vec<String>,
    /// 租约文件总数与其中已失效的数量。
    pub lease_files: usize,
    pub stale_leases: usize,
    /// 已安装包的依赖声明指向"没有安装的版本"（依赖缺口）。
    pub broken_dependencies: Vec<String>,
}

impl StateAudit {
    /// 没有任何需要人工处理的问题（staging / tmp 残留不算，`recover()` 会自动清）。
    pub fn is_clean(&self) -> bool {
        self.missing_dirs.is_empty()
            && self.orphan_dirs.is_empty()
            && self.active_missing.is_empty()
            && self.stale_tmp_files.is_empty()
            && self.staging_dirs.is_empty()
            && self.stale_leases == 0
            && self.broken_dependencies.is_empty()
    }

    /// 是否有需要人工介入的项（缺目录 / 孤儿目录 / 激活版本缺失）。
    pub fn needs_attention(&self) -> bool {
        !self.missing_dirs.is_empty()
            || !self.orphan_dirs.is_empty()
            || !self.active_missing.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.is_clean() {
            return "仓库状态一致：state / installed / staging 三方无差异".to_string();
        }
        let mut parts = Vec::new();
        if !self.active_missing.is_empty() {
            parts.push(format!(
                "激活版本目录缺失：{}",
                self.active_missing.join(", ")
            ));
        }
        if !self.missing_dirs.is_empty() {
            parts.push(format!("{} 个版本有记录但无目录", self.missing_dirs.len()));
        }
        if !self.orphan_dirs.is_empty() {
            parts.push(format!("{} 个目录无记录（孤儿）", self.orphan_dirs.len()));
        }
        if !self.staging_dirs.is_empty() {
            parts.push(format!("{} 个 staging 残留", self.staging_dirs.len()));
        }
        if !self.stale_tmp_files.is_empty() {
            parts.push(format!("{} 个状态临时文件", self.stale_tmp_files.len()));
        }
        if self.stale_leases > 0 {
            parts.push(format!("{} 个失效租约", self.stale_leases));
        }
        if !self.broken_dependencies.is_empty() {
            parts.push(format!(
                "{} 个依赖缺口（{}）",
                self.broken_dependencies.len(),
                self.broken_dependencies.join("、")
            ));
        }
        parts.join("；")
    }
}

/// `cleanup` 的结果。
///
/// 返回结构而不是"删掉的版本列表"：被跳过的版本必须**可见**——
/// 静默保留和静默删除一样有害。
#[derive(Clone, Debug, Default)]
pub struct CleanupReport {
    /// 实际删除的版本。
    pub removed: Vec<String>,
    /// 因为有有效租约而保留的版本（`(version, holders)`）。
    pub kept_leased: Vec<(String, Vec<String>)>,
    /// 因为被其他已安装包依赖而保留的版本（`(version, dependents)`）。
    pub kept_dependents: Vec<(String, Vec<String>)>,
}

pub struct ResourceStore {
    root: PathBuf,
    /// 进程内串行化，避免两个线程同时改 `state.json`。
    lock: Mutex<()>,
    /// 等跨进程锁的时长。
    timeout: Duration,
}

impl std::fmt::Debug for ResourceStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceStore")
            .field("root", &self.root)
            .finish()
    }
}

impl ResourceStore {
    /// 打开（必要时创建）仓库根目录，使用默认锁超时。
    pub fn open(root: impl AsRef<Path>) -> ResourceResult<Self> {
        Self::open_with_timeout(root, DEFAULT_LOCK_TIMEOUT)
    }

    /// 打开仓库并指定等待跨进程锁的时长。
    pub fn open_with_timeout(root: impl AsRef<Path>, timeout: Duration) -> ResourceResult<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("installed"))?;
        fs::create_dir_all(root.join("staging"))?;
        fs::create_dir_all(root.join("leases"))?;
        let store = Self {
            root,
            lock: Mutex::new(()),
            timeout,
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

    /// 显式独占状态：给外部工具（备份、迁移、人工修复）用。
    ///
    /// 持有期间本进程内的安装/激活等操作会**等锁直到超时**（`Mutex` 与文件锁不是同一把），
    /// 因此只应在"准备离线改仓库"时短暂持有。
    pub fn lock_state(&self, holder: &str) -> ResourceResult<StateLock> {
        lockfile::acquire(&self.root, self.timeout, holder)
    }

    // ---------------- 租约 ----------------

    /// 登记租约：表示"我要用这个版本的目录"。
    ///
    /// # 为什么要持有仓库锁（P2-S / S-01）
    ///
    /// "版本已安装"与"版本被删除"必须互斥。否则会出现这个交错：
    ///
    /// ```text
    /// A: 读 state → 1.0.0 已安装
    /// B: 拿锁 → 检查租约（无）→ 删目录 → 写 state → 放锁
    /// A: 写入租约 → 成功  ← 但版本已经不存在了
    /// ```
    ///
    /// 结果是"租约登记成功、资源已被删除"，即 C-01 描述的非法状态。
    /// 所以这里与 `uninstall` / `cleanup` 使用**同一把进程内 Mutex + 同一个跨进程
    /// `state.lock`**，并在锁内**重新读状态**再判断。加锁顺序两处一致（先 Mutex 后文件锁），
    /// 不会互相死锁。
    ///
    /// 代价：登记租约要拿一次跨进程锁。挂载是构建期动作（`build()` 里逐个资源），
    /// 不在热路径上，这个代价可以接受。
    pub fn acquire_lease(
        &self,
        id: &str,
        version: &str,
        holder: &str,
    ) -> ResourceResult<ResourceLease> {
        validate_component("id", id)?;
        validate_component("version", version)?;

        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _state_lock = self.acquire_state_lock()?;
        let state = self.read_state()?;
        let installed = state
            .resources
            .get(id)
            .map(|e| e.installed.contains_key(version))
            .unwrap_or(false);
        if !installed {
            return Err(ResourceError::NotInstalled {
                id: id.to_string(),
                version: version.to_string(),
            });
        }
        ResourceLease::acquire(&self.root, id, version, holder)
    }

    /// 某个版本的全部租约（含已失效的）。
    pub fn leases(&self, id: &str, version: &str) -> ResourceResult<Vec<LeaseInfo>> {
        validate_component("id", id)?;
        validate_component("version", version)?;
        leases::list_leases(&self.root, id, version)
    }

    /// 仓库里所有租约，按 id / version / pid 排序。
    pub fn all_leases(&self) -> ResourceResult<Vec<LeaseInfo>> {
        let leases_root = self.root.join("leases");
        let mut out = Vec::new();
        if !leases_root.exists() {
            return Ok(out);
        }
        for id_entry in fs::read_dir(&leases_root)? {
            let id_entry = id_entry?;
            if !id_entry.file_type()?.is_dir() {
                continue;
            }
            let Some(id) = id_entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            for version_entry in fs::read_dir(id_entry.path())? {
                let version_entry = version_entry?;
                if !version_entry.file_type()?.is_dir() {
                    continue;
                }
                let Some(version) = version_entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                out.extend(leases::list_leases(&self.root, &id, &version)?);
            }
        }
        out.sort_by(|a, b| {
            (&a.id, &a.version, a.pid, a.acquired_at_unix).cmp(&(
                &b.id,
                &b.version,
                b.pid,
                b.acquired_at_unix,
            ))
        });
        Ok(out)
    }

    /// 回收失效租约（持有进程已死），返回数量。
    pub fn reclaim_stale_leases(&self) -> ResourceResult<usize> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        leases::reclaim_stale(&self.root)
    }

    // ---------------- 安装 / 激活 / 回滚 / 删除 ----------------

    /// 安装一个资源目录（必须含 `manifest.json`）。
    ///
    /// 分两段，锁的粒度不同：
    ///
    /// 1. **无锁**：读清单 → 快速预检 → 复制到 staging → 逐文件 size+sha256 校验。
    ///    这一段最慢（真实词典包 19 MB），且不碰任何共享状态。
    /// 2. **持锁**（进程内 + 跨进程）：重新读状态并再检查一次 → 原子 rename 落位 →
    ///    写状态。复制期间别的进程可能已经装好同一版本，所以检查必须做第二遍。
    pub fn install_from_dir(&self, src: impl AsRef<Path>) -> ResourceResult<InstallOutcome> {
        let src = src.as_ref();
        let manifest = read_manifest_from(src)?;

        validate_component("id", &manifest.id)?;
        validate_component("version", &manifest.version)?;

        {
            let state = self.read_state()?;
            self.precheck_install(&state, &manifest)?;
        }

        let staging = self.staging_dir(&manifest.id, &manifest.version);
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;

        let staged = (|| -> ResourceResult<()> {
            copy_tree(src, &staging)?;
            verify_files(&staging, &manifest)?;
            sync_dir(&staging)?;
            crash_point("after-staging");
            Ok(())
        })();
        if let Err(err) = staged {
            let _ = fs::remove_dir_all(&staging);
            return Err(err);
        }

        let outcome = self.commit_install(&manifest, &staging, &mut |_| {});
        if outcome.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        outcome
    }

    fn precheck_install(&self, state: &State, manifest: &ResourceManifest) -> ResourceResult<()> {
        if self
            .installed_versions(state, &manifest.id)
            .contains(&manifest.version)
        {
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
        ensure_compatible(&check(manifest, &HostSupport::current(), &available))
    }

    /// 持锁落位 + 记账。`on_staged` 仅用于测试钩子（当前实现不使用）。
    fn commit_install(
        &self,
        manifest: &ResourceManifest,
        staging: &Path,
        on_staged: &mut dyn FnMut(&Path),
    ) -> ResourceResult<InstallOutcome> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _state_lock = self.acquire_state_lock()?;
        let mut state = self.read_state()?;
        // 复制期间可能已有别的进程装好了同一个版本
        self.precheck_install(&state, manifest)?;

        let target = self.version_dir(&manifest.id, &manifest.version)?;
        if target.exists() {
            return Err(ResourceError::AlreadyInstalled {
                id: manifest.id.clone(),
                version: manifest.version.clone(),
            });
        }
        on_staged(staging);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        crash_point("before-rename");
        fs::rename(staging, &target)?;
        // rename 之后立刻把父目录目录项落盘：否则断电后可能"目录没名字"
        if let Some(parent) = target.parent() {
            sync_dir(parent)?;
        }
        crash_point("after-rename");

        let installed_at = now_string();
        let entry = state.resources.entry(manifest.id.clone()).or_default();
        let previous_active = entry.active.clone();
        let activated = previous_active.is_none();
        entry.installed.insert(
            manifest.version.clone(),
            Meta {
                kind: manifest.kind.as_str().to_string(),
                payload_schema: manifest.payload_schema.as_str().to_string(),
                installed_at: installed_at.clone(),
            },
        );
        if activated {
            entry.active = Some(manifest.version.clone());
        }
        crash_point("before-state");
        // 状态写失败时不能让磁盘上留下一个"已安装但无记录"的目录
        if let Err(err) = self.write_state(&state) {
            let _ = fs::remove_dir_all(&target);
            return Err(err);
        }

        crash_point("after-state");
        Ok(InstallOutcome {
            resource: InstalledResource {
                id: manifest.id.clone(),
                version: manifest.version.clone(),
                kind: manifest.kind.clone(),
                payload_schema: manifest.payload_schema.as_str().to_string(),
                path: target,
                active: activated,
                installed_at,
            },
            activated,
            previous_active,
        })
    }

    /// 激活某个已安装版本。**不删除任何东西**，旧版本进入 history。
    ///
    /// 不需要等租约：切换激活版本不影响已经持有旧版本租约的使用方（它们继续用自己的目录）。
    pub fn activate(&self, id: &str, version: &str) -> ResourceResult<()> {
        validate_component("id", id)?;
        validate_component("version", version)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _state_lock = self.acquire_state_lock()?;
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
        let _state_lock = self.acquire_state_lock()?;
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

    /// 删除一个版本。激活版本 → `ResourceInUse`；有有效租约 → `ResourceLeased`。
    pub fn uninstall(&self, id: &str, version: &str) -> ResourceResult<()> {
        validate_component("id", id)?;
        validate_component("version", version)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _state_lock = self.acquire_state_lock()?;
        let mut state = self.read_state()?;
        // 反向依赖要在拿到可变借用之前算（`dependents_of` 需要读整个 state）
        let dependents = self.dependents_of(&state, id, version);

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
        // 激活检查之后再看租约：激活版本报 ResourceInUse 更具体
        leases::ensure_not_leased(&self.root, id, version)?;
        // 反向依赖：别的已安装包在清单里点名要这个版本，删了它就是永久破坏（LC-07）
        if !dependents.is_empty() {
            return Err(ResourceError::ReversedDependency {
                id: id.to_string(),
                version: version.to_string(),
                dependents,
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
    /// 有有效租约的版本**一律保留**并记入 [`CleanupReport::kept_leased`]：
    /// 清理是"腾空间"，不该把正在用的东西删掉，也不该因为别人在用就让整次清理失败。
    ///
    /// 注意：这里**不能**在持锁状态下调用 `uninstall`——`std::sync::Mutex` 不可重入，
    /// 那样会直接死锁（本项目在集成测试里踩到过一次，保留注释以免复现）。
    pub fn cleanup(&self, id: &str, keep: usize) -> ResourceResult<CleanupReport> {
        validate_component("id", id)?;
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _state_lock = self.acquire_state_lock()?;
        let mut state = self.read_state()?;

        let entry = state
            .resources
            .get(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;

        let active = entry.active.clone();
        let keep_set: std::collections::BTreeSet<String> =
            entry.history.iter().take(keep).cloned().collect();

        let candidates: Vec<String> = entry
            .installed
            .keys()
            .filter(|v| Some((*v).clone()) != active && !keep_set.contains(*v))
            .cloned()
            .collect();

        if candidates.is_empty() {
            return Ok(CleanupReport::default());
        }

        let mut report = CleanupReport::default();
        for version in &candidates {
            let leased = leases::active_leases(&self.root, id, version)?;
            if !leased.is_empty() {
                report.kept_leased.push((
                    version.clone(),
                    leased.iter().map(LeaseInfo::describe).collect(),
                ));
                continue;
            }
            // 被其他已安装包依赖的版本一律保留：清理是"腾空间"，不是"拆别人地基"
            let dependents = self.dependents_of(&state, id, version);
            if !dependents.is_empty() {
                report.kept_dependents.push((version.clone(), dependents));
                continue;
            }
            let dir = self.version_dir(id, version)?;
            if dir.exists() {
                // 先删目录：删不掉就整体失败，状态保持不变（不会出现"记录没了但文件还在"）
                fs::remove_dir_all(&dir)?;
            }
            report.removed.push(version.clone());
        }

        if report.removed.is_empty() {
            return Ok(report);
        }

        let entry = state
            .resources
            .get_mut(id)
            .ok_or_else(|| ResourceError::NotFound(id.to_string()))?;
        for version in &report.removed {
            entry.installed.remove(version);
            entry.history.retain(|v| v != version);
        }

        self.write_state(&state)?;
        Ok(report)
    }

    /// 三方对账：`state.json` ↔ `installed/` ↔ `staging/`，外加租约。
    ///
    /// 只读，不改任何东西（`recover()` 才动手清理半成品）。
    pub fn audit(&self) -> ResourceResult<StateAudit> {
        let state = self.read_state()?;
        let mut audit = StateAudit::default();

        // 1) state 里记录的版本 → 目录是否真的在
        for (id, entry) in &state.resources {
            for version in entry.installed.keys() {
                let dir = self.version_dir(id, version)?;
                if !dir.exists() {
                    audit.missing_dirs.push(InstalledRef {
                        id: id.clone(),
                        version: version.clone(),
                        path: dir,
                    });
                }
            }
            if let Some(active) = &entry.active {
                let dir = self.version_dir(id, active)?;
                if !dir.exists() {
                    audit.active_missing.push(format!("{id}@{active}"));
                }
            }
        }

        // 2) 依赖缺口：已安装包的依赖声明是否还能被满足
        for (id, entry) in &state.resources {
            for version in entry.installed.keys() {
                let dir = self.version_dir(id, version)?;
                if !dir.exists() {
                    continue;
                }
                let Ok(manifest) = read_manifest_from(&dir) else {
                    continue;
                };
                for dep in &manifest.dependencies {
                    // "满足"必须是"记录在 + 目录在"：只看 state 会把外部误删当成功
                    let satisfied = match &dep.version {
                        Some(required) => {
                            state
                                .resources
                                .get(&dep.id)
                                .map(|e| e.installed.contains_key(required))
                                .unwrap_or(false)
                                && self.version_dir(&dep.id, required)?.exists()
                        }
                        None => match state.resources.get(&dep.id) {
                            None => false,
                            Some(e) => e.installed.keys().any(|v| {
                                self.root.join("installed").join(&dep.id).join(v).exists()
                            }),
                        },
                    };
                    if !satisfied {
                        audit.broken_dependencies.push(format!(
                            "{id}@{version} → {}@{}",
                            dep.id,
                            dep.version.clone().unwrap_or_else(|| "(任意)".to_string())
                        ));
                    }
                }
            }
        }

        // 3) 磁盘上有目录但 state 里没记录 → 孤儿
        let installed_root = self.root.join("installed");
        if installed_root.exists() {
            for id_entry in fs::read_dir(&installed_root)? {
                let id_entry = id_entry?;
                if !id_entry.file_type()?.is_dir() {
                    continue;
                }
                let id = id_entry.file_name().to_string_lossy().to_string();
                for version_entry in fs::read_dir(id_entry.path())? {
                    let version_entry = version_entry?;
                    if !version_entry.file_type()?.is_dir() {
                        continue;
                    }
                    let version = version_entry.file_name().to_string_lossy().to_string();
                    let known = state
                        .resources
                        .get(&id)
                        .map(|e| e.installed.contains_key(&version))
                        .unwrap_or(false);
                    if !known {
                        audit.orphan_dirs.push(InstalledRef {
                            id: id.clone(),
                            version,
                            path: version_entry.path(),
                        });
                    }
                }
            }
        }

        // 3) 半成品
        let staging_root = self.root.join("staging");
        if staging_root.exists() {
            for entry in fs::read_dir(&staging_root)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    audit
                        .staging_dirs
                        .push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("state.json.tmp-") && entry.file_type()?.is_file() {
                audit.stale_tmp_files.push(name);
            }
        }

        // 4) 租约
        let leases = self.all_leases()?;
        audit.lease_files = leases.len();
        audit.stale_leases = leases.iter().filter(|l| !l.is_active()).count();

        audit
            .orphan_dirs
            .sort_by(|a, b| (&a.id, &a.version).cmp(&(&b.id, &b.version)));
        audit
            .missing_dirs
            .sort_by(|a, b| (&a.id, &a.version).cmp(&(&b.id, &b.version)));

        Ok(audit)
    }

    /// 删除孤儿目录（磁盘上有、`state` 里没有的版本目录）。
    ///
    /// 显式动作：返回被删除的条目。有有效租约的版本不会被删（先报 `ResourceLeased`）。
    /// 缺失方向（有记录无目录）不在这里处理——那是数据丢失，需要人决定是重新安装还是清记录。
    pub fn prune_orphans(&self) -> ResourceResult<Vec<InstalledRef>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _state_lock = self.acquire_state_lock()?;
        let audit = self.audit()?;

        let mut removed = Vec::new();
        for orphan in audit.orphan_dirs {
            // 孤儿目录也可能被别人 mmap 着，仍然要尊重租约
            if !leases::active_leases(&self.root, &orphan.id, &orphan.version)?.is_empty() {
                continue;
            }
            if orphan.path.exists() {
                fs::remove_dir_all(&orphan.path)?;
                removed.push(orphan);
            }
        }
        // 顺手把空的 id 目录删掉
        if let Some(parent) = removed.first().and_then(|r| r.path.parent()) {
            let _ = fs::remove_dir(parent);
        }
        Ok(removed)
    }

    // ---------------- 查询 ----------------

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

    /// 清理上次中断留下的残骸：staging 目录、`state.json.tmp-*`、失效租约。
    pub fn recover(&self) -> ResourceResult<RecoverReport> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut report = RecoverReport::default();

        let staging_root = self.root.join("staging");
        if staging_root.exists() {
            for entry in fs::read_dir(&staging_root)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    fs::remove_dir_all(entry.path())?;
                    report.removed_staging += 1;
                }
            }
        }

        // state.json.tmp-<pid>-<nonce>：写状态时崩溃留下的临时文件
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("state.json.tmp-") && entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
                report.removed_temp_files += 1;
            }
        }

        // 孤儿目录（rename 成功、写状态前崩溃）：目录在、state 无记录。
        // 这种目录**不可能**是"合法已安装"——合法安装必然先写状态再返回成功，
        // 而写状态失败时 `commit_install` 自己会删目录。因此可以安全清理。
        let installed_root = self.root.join("installed");
        if installed_root.exists() {
            let state = self.read_state()?;
            for id_entry in fs::read_dir(&installed_root)? {
                let id_entry = id_entry?;
                if !id_entry.file_type()?.is_dir() {
                    continue;
                }
                let id = id_entry.file_name().to_string_lossy().to_string();
                for version_entry in fs::read_dir(id_entry.path())? {
                    let version_entry = version_entry?;
                    if !version_entry.file_type()?.is_dir() {
                        continue;
                    }
                    let version = version_entry.file_name().to_string_lossy().to_string();
                    let recorded = state
                        .resources
                        .get(&id)
                        .map(|e| e.installed.contains_key(&version))
                        .unwrap_or(false);
                    if !recorded {
                        fs::remove_dir_all(version_entry.path())?;
                        report.removed_orphans += 1;
                    }
                }
            }
        }

        // 失效记录（state 有记录、目录不在）：丢掉记录，避免"看起来装好了但根本打不开"
        {
            let _state_lock = self.acquire_state_lock()?;
            let mut state = self.read_state()?;
            let mut dropped_any = false;
            let ids: Vec<String> = state.resources.keys().cloned().collect();
            for id in ids {
                let Some(entry) = state.resources.get_mut(&id) else {
                    continue;
                };
                let gone: Vec<String> = entry
                    .installed
                    .keys()
                    .filter(|v| !self.root.join("installed").join(&id).join(v).exists())
                    .cloned()
                    .collect();
                if gone.is_empty() {
                    continue;
                }
                for version in &gone {
                    entry.installed.remove(version);
                    entry.history.retain(|v| v != version);
                    report.dropped_records += 1;
                }
                if entry.active.as_ref().is_some_and(|a| gone.contains(a)) {
                    // 激活版本没了：退回最近一个还在的版本，没有就置空
                    let next = entry
                        .history
                        .iter()
                        .find(|v| entry.installed.contains_key(*v))
                        .cloned();
                    entry.active = next;
                }
                if entry.installed.is_empty() {
                    state.resources.remove(&id);
                }
                dropped_any = true;
            }
            if dropped_any {
                self.write_state(&state)?;
            }
        }

        report.reclaimed_leases = leases::reclaim_stale(&self.root)?;
        Ok(report)
    }

    // ---------------- 内部 ----------------

    fn acquire_state_lock(&self) -> ResourceResult<StateLock> {
        lockfile::acquire(
            &self.root,
            self.timeout,
            &format!("ResourceStore(pid={})", std::process::id()),
        )
    }

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

    /// 找出所有"清单里点名依赖 `id@version`"的已安装包（`docs/17` LC-07）。
    ///
    /// 语义：
    /// - 依赖写死了版本 → 只有删**那个**版本才算破坏；
    /// - 依赖只写 id（`version: None`）→ 删到该 id **一个版本都不剩**才算破坏。
    ///
    /// 读不到清单的目录直接跳过：那是 `audit()`/`recover()` 的活，不在这里替它判定。
    fn dependents_of(&self, state: &State, id: &str, version: &str) -> Vec<String> {
        let remaining = state
            .resources
            .get(id)
            .map(|e| e.installed.len())
            .unwrap_or(0);
        let last_version = remaining <= 1;

        let mut dependents = Vec::new();
        for (other_id, entry) in &state.resources {
            for other_version in entry.installed.keys() {
                if other_id == id && other_version == version {
                    continue;
                }
                let Ok(dir) = self.version_dir(other_id, other_version) else {
                    continue;
                };
                if !dir.exists() {
                    continue;
                }
                let Ok(manifest) = read_manifest_from(&dir) else {
                    continue;
                };
                for dep in &manifest.dependencies {
                    if dep.id != id {
                        continue;
                    }
                    let blocks = match &dep.version {
                        Some(required) => required == version,
                        None => last_version,
                    };
                    if blocks {
                        dependents.push(format!("{other_id}@{other_version}"));
                        break;
                    }
                }
            }
        }
        dependents.sort();
        dependents.dedup();
        dependents
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
    ///
    /// 调用方必须持 [`StateLock`]，否则两个进程会互相覆盖。
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
        // 目录项落盘：rename 改的是根目录，不 fsync 会留下"内容在、名字不在"
        sync_dir(&self.root)?;
        Ok(())
    }
}

/// 测试用的崩溃注入点。
///
/// 两种模式：
/// - 只设 `TRANSLATOR_TEST_CRASH_POINT`：立即 `abort()`（兼容老测试）；
/// - 同时设 `TRANSLATOR_TEST_CRASH_MARKER`：先写标记文件，然后**停在这里等外部
///   `SIGKILL`**（G08 要求"外部 kill -9"，而不是进程自己 abort）。30 s 内没人杀
///   才退化为 abort，避免测试挂死。
///
/// 生产路径上只是"读一次环境变量"的成本。
fn crash_point(name: &str) {
    if std::env::var("TRANSLATOR_TEST_CRASH_POINT").as_deref() != Ok(name) {
        return;
    }
    if let Ok(marker) = std::env::var("TRANSLATOR_TEST_CRASH_MARKER") {
        if let Ok(mut file) = fs::File::create(&marker) {
            let _ = file.write_all(name.as_bytes());
            let _ = file.sync_all();
        }
        // 等父进程从外部 SIGKILL。sleep 而不是自旋，避免测试机 CPU 飙高。
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // abort 而不是 exit：给"来不及清理"的最坏情况，不留 Drop 机会
    std::process::abort();
}

/// 目录项落盘（`fsync` 目录本身）。
///
/// 只 `fsync` 文件不够：`rename` 改的是**父目录**的目录项，父目录不 fsync
/// 时断电后可能出现"文件内容在、名字不在"。非 Unix 平台没有可移植的目录 fsync，
/// 这里显式跳过并在报告里注明（不假装跨平台持久化已保证）。
fn sync_dir(path: &Path) -> ResourceResult<()> {
    #[cfg(unix)]
    {
        let dir = fs::File::open(path)?;
        dir.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
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
