//! P3.2-C：崩溃残留回收（快照 / worker 工作目录）+ 进程身份核实。
//!
//! 三条不变量（`docs/17` §4.1.6）：
//! 1. 持有者已死 → 残留必须被清掉（否则每次崩溃都永久泄漏 50–400 MiB 磁盘）；
//! 2. 持有者还活着 → **绝不**动它的目录；
//! 3. 目录里的 pid 已被别的进程复用 → 不能让旧目录"永远活着"（靠启动标识区分）。
//!
//! 只在 Linux 运行：身份判定依赖 `/proc/<pid>/stat`。

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use translator_runtime::reclaim::{sweep_snapshots, sweep_worker_dirs};

/// 起一个立刻退出的进程，返回它已经死掉的 pid。
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("/bin/true")
        .spawn()
        .expect("无法启动 /bin/true");
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn dir(root: &Path, name: &str, owner: Option<(u32, Option<u64>)>) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).unwrap();
    if let Some((pid, start)) = owner {
        let start = match start {
            Some(v) => v.to_string(),
            None => "unknown".to_string(),
        };
        std::fs::write(path.join(".owner"), format!("pid={pid}\nstart={start}\n")).unwrap();
    }
    path
}

/// 当前进程的启动标识（与 SDK 内部算法一致：`/proc/self/stat` 第 22 字段）。
fn self_start() -> u64 {
    let text = std::fs::read_to_string("/proc/self/stat").unwrap();
    let tail = text.rsplit_once(')').unwrap().1;
    tail.split_whitespace().nth(19).unwrap().parse().unwrap()
}

#[test]
fn 持有者已死的快照目录会被清掉() {
    let root = tempfile::tempdir().unwrap();
    let gone = dead_pid();
    let path = dir(
        root.path(),
        &format!("snap-{gone}-1-deadbeef"),
        Some((gone, Some(1))),
    );

    let report = sweep_snapshots(root.path()).unwrap();
    assert_eq!(report.removed, 1, "{report:?}");
    assert!(!path.exists());
}

#[test]
fn 持有者仍存活的快照目录绝不动() {
    let root = tempfile::tempdir().unwrap();
    let me = std::process::id();
    let path = dir(
        root.path(),
        &format!("snap-{me}-1-cafe0001"),
        Some((me, Some(self_start()))),
    );

    let report = sweep_snapshots(root.path()).unwrap();
    assert_eq!(report.removed, 0, "{report:?}");
    assert_eq!(report.kept_alive, 1, "{report:?}");
    assert!(path.exists(), "活着的持有者目录不能被删");
}

#[test]
fn pid被复用不会让旧目录永远留着() {
    let root = tempfile::tempdir().unwrap();
    let me = std::process::id();
    // pid 是活的，但启动标识对不上 → 这是"同一个 pid 的新进程"，旧目录属于死掉的旧持有者
    let stale = dir(
        root.path(),
        &format!("snap-{me}-2-cafe0002"),
        Some((me, Some(self_start() + 1))),
    );
    let live = dir(
        root.path(),
        &format!("snap-{me}-3-cafe0003"),
        Some((me, Some(self_start()))),
    );

    let report = sweep_snapshots(root.path()).unwrap();
    assert_eq!(report.removed, 1, "{report:?}");
    assert_eq!(report.kept_alive, 1, "{report:?}");
    assert!(!stale.exists(), "PID 复用的旧目录必须被清掉");
    assert!(live.exists(), "真持有者的目录必须保留");
}

#[test]
fn 没有owner文件时只按pid判死() {
    let root = tempfile::tempdir().unwrap();
    let gone = dead_pid();
    let me = std::process::id();
    let dead_dir = dir(root.path(), &format!("snap-{gone}-4-ffff0001"), None);
    let live_dir = dir(root.path(), &format!("snap-{me}-4-ffff0002"), None);

    let report = sweep_snapshots(root.path()).unwrap();
    assert_eq!(report.removed, 1, "{report:?}");
    assert_eq!(report.kept_alive, 1, "{report:?}");
    assert!(!dead_dir.exists());
    assert!(live_dir.exists());
    // 老目录没有启动标识：判定不了"是不是同一个进程"，只能保守地当成还在用
}

#[test]
fn worker工作目录同样按持有者回收() {
    let root = tempfile::tempdir().unwrap();
    let gone = dead_pid();
    let me = std::process::id();
    let dead_dir = dir(
        root.path(),
        &format!("nmt-worker-{gone}-7"),
        Some((gone, Some(1))),
    );
    let live_dir = dir(
        root.path(),
        &format!("nmt-worker-{me}-7"),
        Some((me, Some(self_start()))),
    );

    let report = sweep_worker_dirs(root.path()).unwrap();
    assert_eq!(report.removed, 1, "{report:?}");
    assert_eq!(report.kept_alive, 1, "{report:?}");
    assert!(!dead_dir.exists());
    assert!(live_dir.exists());
}

#[test]
fn 目录不存在时回收是空操作() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("nope");
    let report = sweep_snapshots(&missing).unwrap();
    assert_eq!(report, Default::default());
}
