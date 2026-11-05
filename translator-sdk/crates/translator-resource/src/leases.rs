//! 资源租约（P2.3）。
//!
//! # 为什么需要租约
//!
//! P2.2 只能保证「激活版本不可删除」，但它不知道**谁在用**那个目录：
//!
//! - 宿主 `mount` 了某个版本，进程正常退出后被删除 —— 没问题；
//! - 但如果是**另一个进程**正在 mmap 那个 `index.fst`、或握着 `entries.sqlite`
//!   连接，此时删除目录：Unix 上 `unlink` 对已打开的 fd 安全（数据直到关闭才释放），
//!   Windows 上直接拒绝（`ERROR_SHARING_VIOLATION`）——同一个动作两种后果，
//!   都不能靠"试试看"来决定。
//!
//! 于是引入**显式租约**：使用方在挂载前先登记，卸载时释放；仓库在删除前检查。
//!
//! # 为什么一个租约一个文件
//!
//! 租约必须跨进程可见，而 `state.json` 是单文件、单写者语义。把租约塞进
//! `state.json` 会让「A 进程挂载资源」变成「A 进程写全局状态」，进而要求
//! 所有挂载者串行化——代价远大于收益。
//!
//! 每个租约独立成文件（`leases/<id>/<version>/<pid>-<nonce>.json`）后：
//!
//! - 登记/释放只碰自己的文件，天然无竞争；
//! - 崩溃残留可被逐个识别与回收（见 [`process_alive`]）；
//! - 检查"有没有人在用"就是列一次目录，不需要任何共享锁。
//!
//! # 诚实说明：这不是内核文件锁
//!
//! 租约是**协作式**的：只有通过 `ResourceStore::acquire_lease` 登记的进程才会被看到。
//! 一个绕过本 SDK、直接 `open()` 资源文件的程序不会被检测到。要做强保证，
//! 需要在 Windows 上以 `FILE_SHARE_READ` 打开、在 Unix 上用 `flock`——
//! 前者与「多进程只读共享」直接冲突，后者是 advisory 锁，绕开同样无效。
//! 因此本 SDK 选择**可见的协作式租约 + 明确的拒绝删除**，而不是半吊子的强制锁。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{ResourceError, ResourceResult};

/// 租约文件内容。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub pid: u32,
    /// 人类可读的持有者标识（`translator-api` 会填 `Translator`，CLI 填命令名）。
    pub holder: String,
    /// Unix 秒。用于排查，不参与判定（判定只看 pid 存活）。
    pub acquired_at_unix: u64,
}

/// 已登记的租约快照。
#[derive(Clone, Debug)]
pub struct LeaseInfo {
    pub id: String,
    pub version: String,
    pub pid: u32,
    pub holder: String,
    pub acquired_at_unix: u64,
    /// 进程是否仍存活。`None` 表示平台无法判定（保守当作存活）。
    pub alive: Option<bool>,
    pub path: PathBuf,
}

impl LeaseInfo {
    /// 是否应被视为"占用中"。
    ///
    /// 判定规则：**只有确定进程已死才算失效**。无法判定时按占用处理——
    /// 代价是偶尔要求用户手动 `reclaim`，而不是偶发删除正在被 mmap 的文件。
    pub fn is_active(&self) -> bool {
        self.alive != Some(false)
    }

    pub fn describe(&self) -> String {
        format!(
            "pid={} holder={} 起始=unix:{}",
            self.pid, self.holder, self.acquired_at_unix
        )
    }
}

/// 进程存活判定。
///
/// - Linux/Android：`/proc/<pid>` 存在即存活。这是本机唯一零依赖且可靠的判定。
/// - 其它平台：返回 `None`（无法判定）。不猜，也不假装知道。
///
/// 注意 pid 会被复用：一个刚崩溃的进程的 pid 可能马上被新进程占用，
/// 于是残留租约看起来"活着"。这是本机制的已知误判方向（偏保守，不偏危险）。
pub fn process_alive(pid: u32) -> Option<bool> {
    if pid == 0 {
        return Some(false);
    }
    #[cfg(target_os = "linux")]
    {
        Some(Path::new("/proc").join(pid.to_string()).exists())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // 明确返回"不知道"：没有平台 API 就无法判定，绝不按存活/已死去猜。
        let _ = pid;
        None
    }
}

/// 租约目录：`<root>/leases/<id>/<version>/`。
pub(crate) fn lease_dir(root: &Path, id: &str, version: &str) -> PathBuf {
    root.join("leases").join(id).join(version)
}

/// 列出某个版本下的全部租约（含已失效的）。
pub(crate) fn list_leases(root: &Path, id: &str, version: &str) -> ResourceResult<Vec<LeaseInfo>> {
    let dir = lease_dir(root, id, version);
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            // 读不到就当没有：租约文件是纯附加信息，读失败不该让 uninstall 直接爆炸；
            // 但下面的解析失败同样跳过——只在真正占用时才拒绝删除。
            Err(_) => continue,
        };
        let record: LeaseRecord = match serde_json::from_str(&text) {
            Ok(r) => r,
            // 半截文件（写入时崩溃）不构成占用
            Err(_) => continue,
        };
        out.push(LeaseInfo {
            id: id.to_string(),
            version: version.to_string(),
            pid: record.pid,
            holder: record.holder,
            acquired_at_unix: record.acquired_at_unix,
            alive: process_alive(record.pid),
            path,
        });
    }
    // 稳定顺序：pid 升序，便于输出比对
    out.sort_by_key(|l| (l.pid, l.acquired_at_unix, l.holder.clone()));
    Ok(out)
}

/// 仍然有效的租约。
pub(crate) fn active_leases(
    root: &Path,
    id: &str,
    version: &str,
) -> ResourceResult<Vec<LeaseInfo>> {
    Ok(list_leases(root, id, version)?
        .into_iter()
        .filter(LeaseInfo::is_active)
        .collect())
}

/// 删除已失效的租约文件，返回删除数量。
pub(crate) fn reclaim_stale(root: &Path) -> ResourceResult<usize> {
    let leases_root = root.join("leases");
    let mut removed = 0;
    if !leases_root.exists() {
        return Ok(removed);
    }
    for id_entry in fs::read_dir(&leases_root)? {
        let id_entry = id_entry?;
        if !id_entry.file_type()?.is_dir() {
            continue;
        }
        for version_entry in fs::read_dir(id_entry.path())? {
            let version_entry = version_entry?;
            if !version_entry.file_type()?.is_dir() {
                continue;
            }
            for lease in fs::read_dir(version_entry.path())? {
                let lease = lease_entry_path(lease?);
                let Some(lease) = lease else { continue };
                if let Some(record) = read_record(&lease) {
                    // 只有"确定进程已死"才回收：无法判定存活时按占用处理
                    if process_alive(record.pid) == Some(false) && fs::remove_file(&lease).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
    }
    Ok(removed)
}

fn lease_entry_path(entry: fs::DirEntry) -> Option<PathBuf> {
    if entry.file_type().ok()?.is_file() {
        Some(entry.path())
    } else {
        None
    }
}

fn read_record(path: &Path) -> Option<LeaseRecord> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 一个已登记的租约。drop 即释放。
///
/// 不实现 `Clone`：复制租约等于让"一次挂载"变成"两次占用"，
/// 释放时还会互相踩（同一个文件被删两次）。
#[derive(Debug)]
pub struct ResourceLease {
    id: String,
    version: String,
    path: PathBuf,
    /// 记录根目录，drop 时清理空目录用。
    root: PathBuf,
    released: bool,
}

impl ResourceLease {
    /// 登记一个新租约。
    pub(crate) fn acquire(
        root: &Path,
        id: &str,
        version: &str,
        holder: &str,
    ) -> ResourceResult<Self> {
        let dir = lease_dir(root, id, version);
        fs::create_dir_all(&dir)?;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!("{}-{nonce}.json", std::process::id()));
        let record = LeaseRecord {
            pid: std::process::id(),
            holder: holder.to_string(),
            acquired_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let json = serde_json::to_string(&record)?;

        // create_new：同一进程内两次 acquire 也不该覆盖同一个文件
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(json.as_bytes())?;
        // 不 fsync：租约是易失信息，崩溃后由 `recover()` 回收，没有任何东西
        // 依赖它的持久性。为它付一次磁盘同步的代价是纯粹的浪费。

        Ok(Self {
            id: id.to_string(),
            version: version.to_string(),
            path,
            root: root.to_path_buf(),
            released: false,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// 显式释放。等价于 drop，但可以早于作用域结束。
    pub fn release(mut self) -> ResourceResult<()> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> ResourceResult<()> {
        if self.released {
            return Ok(());
        }
        self.released = true;
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            // 已经被 `reclaim_stale` 或人工清掉：释放动作本身算成功
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        // 顺手清理空目录，避免仓库里堆一排空壳。非空则说明还有别的租约，正常。
        let dir = lease_dir(&self.root, &self.id, &self.version);
        let _ = fs::remove_dir(&dir);
        if let Some(parent) = dir.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }
}

impl Drop for ResourceLease {
    fn drop(&mut self) {
        // drop 里不能返回错误：释放失败的后果只是多一个残留文件，
        // 下次 `recover()` 会把它清掉（进程退出后 pid 判定为死）。
        let _ = self.release_inner();
    }
}

/// 把"被占用"渲染成给人看的列表。
pub(crate) fn describe_holders(leases: &[LeaseInfo]) -> Vec<String> {
    leases.iter().map(LeaseInfo::describe).collect()
}

/// 供 `uninstall` / `cleanup` 使用的占用检查。
pub(crate) fn ensure_not_leased(root: &Path, id: &str, version: &str) -> ResourceResult<()> {
    let active = active_leases(root, id, version)?;
    if active.is_empty() {
        return Ok(());
    }
    Err(ResourceError::ResourceLeased {
        id: id.to_string(),
        version: version.to_string(),
        holders: describe_holders(&active),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 当前进程必然存活() {
        assert_eq!(process_alive(std::process::id()), Some(true));
    }

    #[test]
    fn pid为零视为已死() {
        assert_eq!(process_alive(0), Some(false));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn 不存在的pid视为已死() {
        // pid 上限通常远小于 4194304；这里取一个几乎不可能存在的值
        assert_eq!(process_alive(4_194_303), Some(false));
    }

    #[test]
    fn 垃圾租约文件不被当成占用() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = lease_dir(tmp.path(), "a", "1.0.0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("junk.json"), "{ 这不是 JSON").unwrap();
        let list = list_leases(tmp.path(), "a", "1.0.0").unwrap();
        assert!(list.is_empty(), "半截文件不该构成占用");
    }

    #[test]
    fn 无法判定存活时按占用处理() {
        let info = LeaseInfo {
            id: "a".into(),
            version: "1".into(),
            pid: 12345,
            holder: "x".into(),
            acquired_at_unix: 0,
            alive: None,
            path: PathBuf::from("/dev/null"),
        };
        assert!(info.is_active(), "未知必须按占用处理，不能按已死处理");
    }
}
