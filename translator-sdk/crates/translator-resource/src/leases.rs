//! 资源租约（P2.3，P2-S 增补进程身份）。
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
//! - 登记/释放只碰自己的文件；
//! - 崩溃残留可被逐个识别与回收（见 [`probe`]）；
//! - 检查"有没有人在用"就是列一次目录，不需要任何共享锁。
//!
//! # 与 `uninstall` 的原子性（P2-S / S-01）
//!
//! 租约文件本身是独立的，但**"版本是否已安装"这个判断必须与删除互斥**，
//! 否则会出现：A 读到"已安装" → B 卸载删除 → A 写入租约 → 版本已不存在却有租约。
//!
//! 因此 [`crate::store::ResourceStore::acquire_lease`] 会在**进程内 Mutex +
//! 跨进程 `state.lock`** 的临界区内重新检查安装状态，与 `uninstall` / `cleanup`
//! 使用同一把锁、同样的加锁顺序（先 Mutex 后文件锁），不存在"检查完就放开"的窗口。
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
    /// 进程启动标识（Linux：`/proc/<pid>/stat` 的 starttime）。
    ///
    /// 有了它才能区分"同一个进程"与"pid 被回收后复用的新进程"。
    /// 旧记录没有这个字段时按 `None` 处理（退回"只看 pid 是否存在"）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_id: Option<u64>,
    /// 人类可读的持有者标识（`translator-api` 会填 `translator-api`，CLI 填命令名）。
    pub holder: String,
    /// Unix 秒。用于排查，不参与存活判定。
    pub acquired_at_unix: u64,
}

/// 进程存活探测结果。
///
/// 刻意不是 `bool`：**"不知道"必须能被表达出来**。把未知当成"已死"会导致
/// 删除正在被使用的目录；把未知当成"存活"只会多留一个文件，等人工 reclaim。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessLiveness {
    /// 进程存在，附启动标识（平台不支持时为空）。
    Alive(Option<u64>),
    /// 进程确定不存在。
    Dead,
    /// 平台无法判定（非 Linux，或读取失败但 `/proc` 存在）。
    Unknown,
}

/// 探测某个 pid 的存活状态与启动标识。
///
/// - Linux：`/proc/<pid>` 存在且可读 → `Alive(starttime)`；不存在 → `Dead`。
/// - 其它平台：`Unknown`（不猜）。
pub fn probe(pid: u32) -> ProcessLiveness {
    if pid == 0 {
        return ProcessLiveness::Dead;
    }
    #[cfg(target_os = "linux")]
    {
        let dir = Path::new("/proc").join(pid.to_string());
        if !dir.exists() {
            return ProcessLiveness::Dead;
        }
        match process_start_id(pid) {
            Some(start) => ProcessLiveness::Alive(Some(start)),
            // /proc 存在但读不出 starttime（权限、极短命进程）：存在即按存活处理
            None => ProcessLiveness::Alive(None),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // 明确返回"不知道"：没有平台 API 就无法判定，绝不按存活/已死去猜。
        let _ = pid;
        ProcessLiveness::Unknown
    }
}

/// 兼容旧接口：只看"是否可能存在"。`None` 表示无法判定。
///
/// 新代码应优先用 [`probe`]——它能识别 pid 复用。
pub fn process_alive(pid: u32) -> Option<bool> {
    match probe(pid) {
        ProcessLiveness::Alive(_) => Some(true),
        ProcessLiveness::Dead => Some(false),
        ProcessLiveness::Unknown => None,
    }
}

/// Linux 进程启动标识（`/proc/<pid>/stat` 第 22 个字段，单位 jiffies）。
///
/// 解析要点：`comm` 字段（第 2 个）用括号包住且**可以包含空格与括号**，
/// 所以不能按空白简单切分——必须先定位最后一个 `)`，再数它之后的字段。
#[cfg(target_os = "linux")]
pub fn process_start_id(pid: u32) -> Option<u64> {
    let text = fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
    let tail = &text[text.rfind(')')? + 1..];
    // tail 以 " S 1 ..." 开头：字段 3(state) 起算，starttime 是第 22 个 → tail 里下标 19
    tail.split_whitespace().nth(19)?.parse().ok()
}

/// 非 Linux 平台没有可移植的启动标识 API。
#[cfg(not(target_os = "linux"))]
pub fn process_start_id(_pid: u32) -> Option<u64> {
    None
}

/// 判定一条（pid, start_id）记录是否仍代表**同一个活着的进程**。
///
/// 规则：
/// - `Dead` → 失效；
/// - `Unknown` → 视为存活（保守）；
/// - `Alive(current)`：
///   - 记录里有 `start_id` 且能读到当前值 → 相等才算同一进程（**识别 pid 复用**）；
///   - 记录里没有 `start_id`（旧格式）或读不到当前值 → 视为存活。
pub fn record_is_live(pid: u32, recorded_start: Option<u64>) -> bool {
    match probe(pid) {
        ProcessLiveness::Dead => false,
        ProcessLiveness::Unknown => true,
        ProcessLiveness::Alive(current) => match (recorded_start, current) {
            (Some(rec), Some(cur)) => rec == cur,
            _ => true,
        },
    }
}

/// 已登记的租约快照。
#[derive(Clone, Debug)]
pub struct LeaseInfo {
    pub id: String,
    pub version: String,
    pub pid: u32,
    /// 记录里的启动标识（旧记录为 `None`）。
    pub start_id: Option<u64>,
    pub holder: String,
    pub acquired_at_unix: u64,
    /// 进程是否仍存活（且是同一个进程）。`None` 表示平台无法判定。
    pub alive: Option<bool>,
    /// pid 存在但启动标识不匹配 → pid 被复用，这条租约属于已死的旧进程。
    pub pid_reused: bool,
    pub path: PathBuf,
}

impl LeaseInfo {
    /// 是否应被视为"占用中"。
    ///
    /// 判定规则：**只有确定进程已死（或 pid 已被别的进程复用）才算失效**。
    /// 无法判定时按占用处理——代价是偶尔要求用户手动 `reclaim`，
    /// 而不是偶发删除正在被 mmap 的文件。
    pub fn is_active(&self) -> bool {
        self.alive != Some(false)
    }

    pub fn describe(&self) -> String {
        let mut text = format!(
            "pid={} holder={} 起始=unix:{}",
            self.pid, self.holder, self.acquired_at_unix
        );
        if self.pid_reused {
            text.push_str("（pid 已被复用）");
        }
        text
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
        let liveness = probe(record.pid);
        let alive = match liveness {
            ProcessLiveness::Dead => Some(false),
            ProcessLiveness::Unknown => None,
            ProcessLiveness::Alive(current) => match (record.start_id, current) {
                (Some(rec), Some(cur)) if rec != cur => Some(false),
                _ => Some(true),
            },
        };
        let pid_reused = matches!(
            (record.start_id, liveness),
            (Some(rec), ProcessLiveness::Alive(Some(cur))) if rec != cur
        );
        out.push(LeaseInfo {
            id: id.to_string(),
            version: version.to_string(),
            pid: record.pid,
            start_id: record.start_id,
            holder: record.holder,
            acquired_at_unix: record.acquired_at_unix,
            alive,
            pid_reused,
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
                    // 只有"确定进程已死 / pid 已复用"才回收：无法判定时按占用处理
                    if !record_is_live(record.pid, record.start_id)
                        && fs::remove_file(&lease).is_ok()
                    {
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
    ///
    /// **调用方必须持有仓库临界区**（见模块文档 S-01）：这里只负责写文件，
    /// "版本是否已安装"的判断由 `ResourceStore::acquire_lease` 在锁内完成。
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
        let pid = std::process::id();
        let path = dir.join(format!("{pid}-{nonce}.json"));
        let record = LeaseRecord {
            pid,
            start_id: process_start_id(pid),
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
        // Linux 读 /proc 能给出确定答案；其它平台没有可移植 API，`probe()` 明确返回 Unknown
        // （保守语义：租约视为仍被占用，绝不误回收）。所以这里钉的是**平台语义**而不是
        // "进程一定活着"：Windows 的真实存活探测还没实现（P5.4 工作包），不许假装知道。
        #[cfg(target_os = "linux")]
        assert_eq!(process_alive(std::process::id()), Some(true));
        #[cfg(not(target_os = "linux"))]
        assert_eq!(process_alive(std::process::id()), None);
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

    #[cfg(target_os = "linux")]
    #[test]
    fn 能读到当前进程的启动标识() {
        let start = process_start_id(std::process::id());
        assert!(start.is_some(), "Linux 上应能读到 starttime");
        // 启动标识必须稳定（同一进程多读几次结果相同）
        assert_eq!(start, process_start_id(std::process::id()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pid复用会被识别为失效() {
        let me = std::process::id();
        let real = process_start_id(me).expect("应能读到 starttime");
        assert!(record_is_live(me, Some(real)), "同一进程必须算存活");
        assert!(
            !record_is_live(me, Some(real.wrapping_add(1))),
            "启动标识不同 → pid 被复用 → 旧记录失效"
        );
        assert!(
            record_is_live(me, None),
            "旧记录没有启动标识时只能按存活处理"
        );
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
            start_id: None,
            holder: "x".into(),
            acquired_at_unix: 0,
            alive: None,
            pid_reused: false,
            path: PathBuf::from("/dev/null"),
        };
        assert!(info.is_active(), "未知必须按占用处理，不能按已死处理");
    }

    #[test]
    fn 旧格式租约仍然可读() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = lease_dir(tmp.path(), "a", "1.0.0");
        fs::create_dir_all(&dir).unwrap();
        // 没有 start_id 字段（P2.3 写的记录）
        fs::write(
            dir.join(format!("{}-1.json", std::process::id())),
            format!(
                r#"{{"pid":{},"holder":"old","acquired_at_unix":1}}"#,
                std::process::id()
            ),
        )
        .unwrap();
        let list = list_leases(tmp.path(), "a", "1.0.0").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].start_id, None);
        assert!(list[0].is_active(), "本进程的旧租约仍算有效");
        assert!(!list[0].pid_reused);
    }
}
