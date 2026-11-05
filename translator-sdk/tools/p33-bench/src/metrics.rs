//! `/proc` 口径的进程内存观测（Linux）。
//!
//! 三个数各说各话，**不得互相冒充**（ADR-021）：
//! - `VmRSS`：当前驻留集，含共享页（多进程相加会重复计）；
//! - `VmHWM`：历史峰值（不可回退，只能重启归零）；
//! - `Pss`（`smaps_rollup`）：按共享比例摊分，跨进程相加才接近物理增量。

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mem {
    pub rss_kib: u64,
    pub hwm_kib: u64,
    pub pss_kib: u64,
}

impl Mem {
    pub fn rss_mib(&self) -> f64 {
        self.rss_kib as f64 / 1024.0
    }
    pub fn hwm_mib(&self) -> f64 {
        self.hwm_kib as f64 / 1024.0
    }
    pub fn pss_mib(&self) -> f64 {
        self.pss_kib as f64 / 1024.0
    }
}

fn read_status(pid: u32) -> Option<(u64, u64, u32, String)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let mut rss = 0;
    let mut hwm = 0;
    let mut ppid = 0;
    let mut name = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Name:") {
            name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("VmRSS:") {
            rss = parse_kib(v);
        } else if let Some(v) = line.strip_prefix("VmHWM:") {
            hwm = parse_kib(v);
        } else if let Some(v) = line.strip_prefix("PPid:") {
            ppid = v.trim().parse().unwrap_or(0);
        }
    }
    Some((rss, hwm, ppid, name))
}

fn parse_kib(value: &str) -> u64 {
    value
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

/// 读 PSS（`smaps_rollup`）。读不到返回 0（例如权限不足），**不当成 0 MiB 使用**——
/// 调用方必须看 `pss_available`。
fn read_pss(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("Pss:"))
        .map(parse_kib)
}

pub fn self_mem() -> Mem {
    let pid = std::process::id();
    let (rss, hwm, _, _) = read_status(pid).unwrap_or((0, 0, 0, String::new()));
    Mem {
        rss_kib: rss,
        hwm_kib: hwm,
        pss_kib: read_pss(pid).unwrap_or(0),
    }
}

#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub mem: Mem,
}

/// 当前进程的**所有子孙进程**（不含自己）。
pub fn descendants() -> Vec<ProcInfo> {
    let mut parents: HashMap<u32, u32> = HashMap::new();
    let mut table: HashMap<u32, (u64, u64, String)> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_string_lossy().parse::<u32>().ok() else {
            continue;
        };
        if let Some((rss, hwm, ppid, name)) = read_status(pid) {
            parents.insert(pid, ppid);
            table.insert(pid, (rss, hwm, name));
        }
    }

    let me = std::process::id();
    let mut out = Vec::new();
    for pid in table.keys().copied() {
        // 向上追溯祖先，看是否经过自己
        let mut cursor = pid;
        let mut is_child = false;
        for _ in 0..64 {
            match parents.get(&cursor) {
                Some(&ppid) if ppid == me => {
                    is_child = true;
                    break;
                }
                Some(&ppid) if ppid == 0 || ppid == cursor => break,
                Some(&ppid) => cursor = ppid,
                None => break,
            }
        }
        if !is_child {
            continue;
        }
        let (rss, hwm, name) = table[&pid].clone();
        out.push(ProcInfo {
            pid,
            name,
            mem: Mem {
                rss_kib: rss,
                hwm_kib: hwm,
                pss_kib: read_pss(pid).unwrap_or(0),
            },
        });
    }
    out.sort_by_key(|p| p.pid);
    out
}

/// 自己 + 所有子孙的合计（RSS 相加会重复计共享页，PSS 相加才近似物理增量）。
pub fn tree_total() -> (Mem, usize) {
    let mut total = self_mem();
    let kids = descendants();
    for p in &kids {
        total.rss_kib += p.mem.rss_kib;
        total.pss_kib += p.mem.pss_kib;
    }
    (total, kids.len())
}
