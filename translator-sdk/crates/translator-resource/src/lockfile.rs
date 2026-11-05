//! 跨进程状态锁（P2.3）。
//!
//! `state.json` 的原子性只保证「不会读到半个文件」，不保证「两个进程不会互相覆盖」：
//! A 读 → B 读 → A 写 → B 写，B 那次写会把 A 的安装记录抹掉。
//!
//! # 实现选择
//!
//! 用 `OpenOptions::create_new` 做**排他创建**（`O_CREAT|O_EXCL`），零依赖、
//! 跨平台、语义明确：谁创建成功谁持锁，删除文件即释放。
//!
//! 为什么不用 `flock`/`LockFileEx`：
//!
//! - 需要 `libc`/`windows-sys` 依赖，而 `translator-resource` 刻意只依赖
//!   `serde`/`serde_json`/`sha2`（见 `error.rs` 的说明）；
//! - `flock` 在 NFS 上语义不保证；`O_EXCL` 至少会明确失败。
//!
//! # 已知限制（诚实记录）
//!
//! - **持锁进程崩溃会留下锁文件**。这里用「读 pid，判定进程已死则回收」来缓解，
//!   但 pid 复用可能让死锁文件看起来有效；超时后报 `LockTimeout` 并**打印锁文件路径**，
//!   由人决定是否删除——而不是让程序悄悄删掉别人的锁。
//! - 这不是强制锁：绕过本 SDK 直接写 `state.json` 不会被阻止。
//! - 只保护 `state.json` 的读改写；`installed/` 下的目录落位靠 `rename` 的原子性，
//!   两个进程同时安装同一版本时，后 `rename` 的一方会因为目标已存在而失败。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::error::{ResourceError, ResourceResult};
use crate::leases::{process_start_id, record_is_live};

/// 默认等待时长。安装是低频操作，5 秒足以覆盖正常竞争；再长就成了卡死。
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 已持有的状态锁。drop 即释放。
#[derive(Debug)]
pub struct StateLock {
    path: PathBuf,
    released: bool,
}

impl StateLock {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let _ = fs::remove_file(&self.path);
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        self.release_inner();
    }
}

/// 获取状态锁。`holder` 会写进锁文件，方便排查"是谁卡住了我"。
pub(crate) fn acquire(root: &Path, timeout: Duration, holder: &str) -> ResourceResult<StateLock> {
    let path = root.join("state.lock");
    let deadline = Instant::now() + timeout;

    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                // pid + 启动标识一起写：只有 pid 的话，持有者崩溃后 pid 被复用，
                // 这个锁会被误判成"还有人在用"（永远等不到），或被误判成"死锁"（误删活锁）。
                let pid = std::process::id();
                let body = format!(
                    "pid={pid}\nstart={}\nunix={}\nholder={holder}\n",
                    process_start_id(pid)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );
                file.write_all(body.as_bytes())?;
                return Ok(StateLock {
                    path,
                    released: false,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // 持有者已死则回收，然后立刻重试（不消耗等待预算）
                if lock_is_stale(&path) {
                    let _ = fs::remove_file(&path);
                    continue;
                }
                if Instant::now() >= deadline {
                    return Err(ResourceError::LockTimeout {
                        path: path.display().to_string(),
                        holder: read_holder(&path),
                    });
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// 锁文件是否属于一个已经死掉的进程。
///
/// 判定必须保守：读不出 pid、或平台无法判定存活，都返回 `false`（当作还活着）。
/// 能读到 `start=` 时优先比较启动标识——这样 pid 被复用的死锁文件仍能被回收。
fn lock_is_stale(path: &Path) -> bool {
    let Some((pid, start)) = read_lock_identity(path) else {
        return false;
    };
    if pid == std::process::id() {
        // 自己的进程还活着，锁只可能是"自己上一次忘了释放"——
        // 但本 SDK 不会嵌套取锁，出现这种情况说明有别的 bug，不在这里静默回收。
        return false;
    }
    !record_is_live(pid, start)
}

/// 从锁文件里读 `(pid, start_id)`。`start` 缺失或写成 `-` 时为 `None`。
fn read_lock_identity(path: &Path) -> Option<(u32, Option<u64>)> {
    let text = fs::read_to_string(path).ok()?;
    let mut pid = None;
    let mut start = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("pid=") {
            pid = rest.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix("start=") {
            start = rest.trim().parse().ok();
        }
    }
    pid.map(|pid| (pid, start))
}

fn read_holder(path: &Path) -> String {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("holder=").map(str::to_string))
        })
        .unwrap_or_else(|| "未知".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 取锁与释放() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = acquire(tmp.path(), DEFAULT_LOCK_TIMEOUT, "test").unwrap();
        assert!(lock.path().exists());
        let path = lock.path().to_path_buf();
        lock.release();
        assert!(!path.exists(), "释放后锁文件必须消失");
    }

    #[test]
    fn drop即释放() {
        let tmp = tempfile::tempdir().unwrap();
        let path = {
            let lock = acquire(tmp.path(), DEFAULT_LOCK_TIMEOUT, "test").unwrap();
            lock.path().to_path_buf()
        };
        assert!(!path.exists());
    }

    #[test]
    fn 同一进程重入会超时() {
        let tmp = tempfile::tempdir().unwrap();
        let _held = acquire(tmp.path(), DEFAULT_LOCK_TIMEOUT, "first").unwrap();
        let err = acquire(tmp.path(), Duration::from_millis(60), "second").unwrap_err();
        match err {
            ResourceError::LockTimeout { path, holder } => {
                assert!(path.ends_with("state.lock"));
                assert_eq!(holder, "first", "错误里要指出是谁在持锁");
            }
            other => panic!("期望 LockTimeout，实际 {other:?}"),
        }
    }

    #[test]
    fn 死进程的锁会被回收() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.lock");
        fs::write(&path, "pid=4194303\nunix=0\nholder=ghost\n").unwrap();
        #[cfg(target_os = "linux")]
        {
            let lock = acquire(tmp.path(), Duration::from_millis(200), "me").unwrap();
            assert!(lock.path().exists());
        }
        #[cfg(not(target_os = "linux"))]
        {
            // 非 Linux 无法判定存活 → 必须超时而不是猜
            assert!(acquire(tmp.path(), Duration::from_millis(60), "me").is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pid被复用的锁文件会被回收() {
        // 需要一个"活着但不是自己"的 pid：起一个 sleep 子进程
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("启动 sleep");
        let pid = child.id();
        let real = crate::leases::process_start_id(pid).expect("应能读到子进程 starttime");

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.lock");

        // 情形 1：pid 活着但启动标识对不上 → pid 被复用，旧锁必须被回收
        fs::write(
            &path,
            format!("pid={pid}\nstart={}\nunix=0\nholder=ghost\n", real + 1),
        )
        .unwrap();
        let lock = acquire(tmp.path(), Duration::from_millis(500), "me").expect("应回收后取锁");
        drop(lock);

        // 情形 2：pid 活着且启动标识一致 → 锁确实被活进程持有，不得回收
        fs::write(
            &path,
            format!("pid={pid}\nstart={real}\nunix=0\nholder=live\n"),
        )
        .unwrap();
        let err = acquire(tmp.path(), Duration::from_millis(60), "me").unwrap_err();
        assert!(
            matches!(err, ResourceError::LockTimeout { .. }),
            "活锁不得被回收，实际：{err}"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn 启动标识一致时不回收活锁() {
        let tmp = tempfile::tempdir().unwrap();
        let held = acquire(tmp.path(), DEFAULT_LOCK_TIMEOUT, "first").unwrap();
        // 再取一次必然超时（同一进程 → 视为活锁）
        assert!(acquire(tmp.path(), Duration::from_millis(60), "second").is_err());
        drop(held);
    }

    #[test]
    fn 无pid字段的锁文件不被回收() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.lock");
        fs::write(&path, "这不是锁文件").unwrap();
        assert!(acquire(tmp.path(), Duration::from_millis(60), "me").is_err());
        assert!(path.exists(), "读不懂的锁不许删：可能是别人的格式");
    }
}
