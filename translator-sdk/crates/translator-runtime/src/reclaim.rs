//! 崩溃残留回收：快照目录与 worker 工作目录（`docs/17` §4.1.6）。
//!
//! 两条硬要求：
//!
//! 1. **清理前核实进程身份**。只看 `pid` 会被 PID 复用骗过去——一个已经死掉的宿主留下的
//!    `snap-1234-…`，恰好被新的 1234 号进程"继承"，就会被永远当成"还在用"；
//! 2. **判定不了就不删**。非 Linux 平台没有可移植的启动标识，此时一律返回"未知"并按
//!    "占用中"处理（宁可漏删，不误删正在被 native 打开的文件）。
//!
//! 身份来源：Linux `/proc/<pid>/stat` 第 22 个字段（starttime，单位 jiffies）。
//! 快照目录里写一份 `.owner`，内容是 `pid=<n>` 与 `start=<n>`；早期留下的目录没有
//! `.owner` 时退化为"只有 pid 能证明它死了才删"。

use std::path::{Path, PathBuf};

use crate::RuntimeError;

/// 一次回收的结果。**三个数字都要能报告**：删了多少、因为活着留了多少、判定不了多少。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// 已删除的残留目录数。
    pub removed: usize,
    /// 因为持有者仍然存活而保留的目录数。
    pub kept_alive: usize,
    /// 无法判定持有者身份而保留的目录数（非 Linux、或读不到 `/proc`）。
    pub unverifiable: usize,
}

impl SweepReport {
    pub fn merge(&mut self, other: SweepReport) {
        self.removed += other.removed;
        self.kept_alive += other.kept_alive;
        self.unverifiable += other.unverifiable;
    }
}

/// 清空快照根目录下**持有者已死**的快照。
pub fn sweep_snapshots(root: &Path) -> Result<SweepReport, RuntimeError> {
    sweep(root, "snap-")
}

/// 清空 worker 工作目录（`nmt-worker-<pid>-<epoch>`）里**持有者已死**的残留。
pub fn sweep_worker_dirs(root: &Path) -> Result<SweepReport, RuntimeError> {
    sweep(root, "nmt-worker-")
}

fn sweep(root: &Path, prefix: &str) -> Result<SweepReport, RuntimeError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SweepReport::default()),
        Err(e) => {
            return Err(RuntimeError::InitFailed(format!(
                "无法读取目录 {}：{e}",
                root.display()
            )))
        }
    };

    let mut report = SweepReport::default();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(prefix) else {
            continue;
        };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if !entry.path().is_dir() {
            continue;
        }
        match verdict(&entry.path(), pid) {
            Verdict::Dead => {
                if std::fs::remove_dir_all(entry.path()).is_ok() {
                    report.removed += 1;
                }
            }
            Verdict::Alive => report.kept_alive += 1,
            Verdict::Unknown => report.unverifiable += 1,
        }
    }
    Ok(report)
}

/// 把当前进程的身份写进目录（供后续回收判定）。
pub(crate) fn write_owner(dir: &Path) -> std::io::Result<()> {
    let pid = std::process::id();
    let start = start_id(pid)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    std::fs::write(dir.join(".owner"), format!("pid={pid}\nstart={start}\n"))
}

enum Verdict {
    Dead,
    Alive,
    Unknown,
}

fn verdict(dir: &Path, pid_from_name: u32) -> Verdict {
    let owner = read_owner(dir);
    let pid = owner.as_ref().map(|o| o.0).unwrap_or(pid_from_name);
    let recorded_start = owner.as_ref().and_then(|o| o.1);

    match is_alive(pid) {
        // 进程不在了 → 无论身份是否可核，都可以删
        Some(false) => Verdict::Dead,
        None => Verdict::Unknown,
        Some(true) => match (recorded_start, start_id(pid)) {
            // 有记录且能核对：启动标识不同 = PID 被复用，目录属于旧持有者
            (Some(recorded), Some(actual)) => {
                if recorded == actual {
                    Verdict::Alive
                } else {
                    Verdict::Dead
                }
            }
            // 没有启动标识（老目录 / 非 Linux）：只能保守地当成还在用
            _ => Verdict::Alive,
        },
    }
}

fn read_owner(dir: &Path) -> Option<(u32, Option<u64>)> {
    let text = std::fs::read_to_string(dir.join(".owner")).ok()?;
    let mut pid = None;
    let mut start = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("pid=") {
            pid = v.trim().parse::<u32>().ok();
        } else if let Some(v) = line.strip_prefix("start=") {
            start = v.trim().parse::<u64>().ok();
        }
    }
    Some((pid?, start))
}

/// `Some(true)` 活着、`Some(false)` 已死、`None` 判定不了（非 Linux）。
pub(crate) fn is_alive(pid: u32) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        Some(Path::new("/proc").join(pid.to_string()).exists())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Linux 进程启动标识（`/proc/<pid>/stat` 第 22 个字段，单位 jiffies）。
pub(crate) fn start_id(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text =
            std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
        // comm 字段可能含空格与右括号，从最后一个 ')' 之后切
        let tail = text.rsplit_once(')')?.1;
        tail.split_whitespace().nth(19)?.parse().ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// 默认的 worker 工作目录根（`nmt-worker` 在这里建自己的配置目录）。
pub fn worker_dir_root() -> PathBuf {
    std::env::temp_dir()
}
