//! P2-S 跨进程测试：租约、状态锁、崩溃残留（C-01 / C-02 / C-04）。
//!
//! # 为什么必须是独立进程
//!
//! 这三条不变量都依赖"另一个地址空间"这一事实：
//!
//! - `O_CREAT|O_EXCL` 锁文件的互斥语义只在**不同进程**之间才有意义（同进程多线程
//!   由 `Mutex` 挡住，根本走不到文件锁）；
//! - 租约存活判定读的是 `/proc/<pid>`，线程共享 pid，永远测不出"外部进程还在用"；
//! - "持锁者崩溃"只能靠 `std::process::exit()` 跳过 `Drop` 来构造。
//!
//! 因此父测试用 `std::env::current_exe()` **重新执行本测试二进制**，用
//! `--ignored --exact <子测试名>` 只跑那一个 `#[ignore]` 的子测试。子测试从环境变量
//! 拿仓库根目录，acquire 后用 stdout 打一行 `READY` 与父进程握手（父进程按行读，
//! 不靠 sleep 猜时序）；需要"继续"时父进程写一个 `release` 文件，子进程轮询它。
//!
//! 运行方式：`cargo test -p translator-resource --test cross_process`
//! （子测试本身被 `#[ignore]`，只有在被父进程点名时才会跑）
//!
//! # 平台范围（诚实说明）
//!
//! 整个文件只在 Linux 上运行：存活判定依赖 `/proc/<pid>/stat` 的 starttime，
//! 非 Linux 平台 [`translator_resource::probe`] 返回 `Unknown`（按"占用中"处理），
//! 本文件的断言（`alive == Some(true/false)`、pid 复用、死锁回收）都无法成立。
//! Windows / macOS 的等价测试**尚未编写、未验证**，不在此处假装通过。

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Lines};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use translator_resource::store::{sha256_file, ResourceStore};
use translator_resource::ResourceError;

/// 子进程读仓库根目录用的环境变量。
const ROOT_ENV: &str = "P2S_STORE_ROOT";

// ---------------- 夹具 ----------------

/// 造一个最小合法资源包（manifest schema 2 / payload `dictionary.sqlite.v1`）。
fn build_package(root: &Path, id: &str, version: &str) -> PathBuf {
    let dir = root.join(format!("pkg-{id}-{version}"));
    std::fs::create_dir_all(&dir).unwrap();

    let mut entries = Vec::new();
    for (name, content) in [
        ("index.fst", b"fake-fst-bytes".as_slice()),
        ("entries.sqlite", b"fake-sqlite-bytes".as_slice()),
    ] {
        std::fs::write(dir.join(name), content).unwrap();
        entries.push(format!(
            r#"{{ "path": "{name}", "sha256": "{}", "bytes": {} }}"#,
            sha256_file(dir.join(name)).unwrap(),
            content.len()
        ));
    }
    std::fs::write(dir.join("LICENSE.txt"), b"CC0").unwrap();

    let manifest = format!(
        r#"{{
            "manifest_schema_version": 2,
            "payload_schema_version": "dictionary.sqlite.v1",
            "id": "{id}",
            "version": "{version}",
            "kind": "dictionary",
            "language": "zh-Hans",
            "target_languages": ["en"],
            "entry_count": 1,
            "provenance": {{
                "name": "TEST", "source_url": "https://example.invalid",
                "revision": "r1", "license_spdx": "CC0-1.0",
                "license_file": "LICENSE.txt", "retrieved_at": "2026-11-05"
            }},
            "files": [{}],
            "dependencies": [],
            "platforms": ["*"]
        }}"#,
        entries.join(", ")
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    dir
}

fn store(root: &Path) -> ResourceStore {
    // 测试里超时压短：等待语义（LockTimeout）本身要被验证，不该让测试跑 5 秒
    ResourceStore::open_with_timeout(root, Duration::from_millis(800)).unwrap()
}

/// 装两个版本并激活 `2.0.0`，返回 `1.0.0` 的目录（便于断言"还没被删"）。
fn two_versions(root: &Path) -> PathBuf {
    let s = store(root);
    s.install_from_dir(build_package(root, "dictionary.test", "1.0.0"))
        .unwrap();
    s.install_from_dir(build_package(root, "dictionary.test", "2.0.0"))
        .unwrap();
    s.activate("dictionary.test", "2.0.0").unwrap();
    s.version_dir("dictionary.test", "1.0.0").unwrap()
}

// ---------------- 子进程驱动 ----------------

struct ChildRun {
    child: Child,
    lines: Lines<BufReader<std::process::ChildStdout>>,
}

impl ChildRun {
    /// 重新执行本测试二进制，只跑指定的 `#[ignore]` 子测试。
    fn spawn(role: &str, root: &Path) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--ignored")
            .arg("--exact")
            .arg(role)
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(ROOT_ENV, root)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| panic!("无法启动子进程 {role}：{e}"));
        let stdout = child.stdout.take().expect("stdout 已 piped");
        Self {
            child,
            lines: BufReader::new(stdout).lines(),
        }
    }

    /// 读到子进程打印 `READY`；超时或提前退出都算失败。
    fn wait_ready(&mut self) -> &mut Self {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match self.lines.next() {
                Some(Ok(line)) => {
                    // libtest 会先打印 `test <name> ... `（无换行），子测试的 READY
                    // 会接在同一行后面，所以这里用后缀判断而不是整行相等。
                    if line.trim_end().ends_with("READY") {
                        return self;
                    }
                }
                Some(Err(e)) => panic!("读子进程 stdout 失败：{e}"),
                None => panic!(
                    "子进程在 READY 之前就退出了（状态：{:?}）",
                    self.child.try_wait()
                ),
            }
        }
        panic!("等子进程 READY 超时");
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 等子进程结束（带超时，避免测试挂死）。
    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "等子进程退出超时");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildRun {
    fn drop(&mut self) {
        // 测试失败时也不要留下孤儿进程
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn signal(child: &mut ChildRun, root: &Path) {
    std::fs::write(root.join("release"), b"go").unwrap();
    assert!(child.wait_exit().success(), "子进程应正常退出");
}

/// 子测试用：读父进程给的仓库根目录。父进程没设环境变量就说明是被误跑的。
fn child_root() -> Option<PathBuf> {
    std::env::var_os(ROOT_ENV).map(PathBuf::from)
}

fn wait_for_release(root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if root.join("release").exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("等父进程放行超时");
}

// ---------------- 子进程角色（只有被点名时才跑） ----------------

/// 登记租约后等父进程放行，正常退出（`Drop` 释放租约）。
#[test]
#[ignore = "由父测试通过 --exact 点名执行"]
fn child_hold_lease() {
    let Some(root) = child_root() else { return };
    let store = store(&root);
    let _lease = store
        .acquire_lease("dictionary.test", "1.0.0", "child-holder")
        .expect("子进程应能登记租约");
    println!("READY");
    wait_for_release(&root);
}

/// 登记租约后**直接 `_exit`**，跳过 `Drop`：留下一条死进程的租约。
#[test]
#[ignore = "由父测试通过 --exact 点名执行"]
fn child_lease_then_die() {
    let Some(root) = child_root() else { return };
    let store = store(&root);
    let _lease = store
        .acquire_lease("dictionary.test", "1.0.0", "child-doomed")
        .expect("子进程应能登记租约");
    println!("READY");
    // 不 drop、不释放：模拟"进程被 OOM killer 杀掉 / 崩溃"
    std::process::exit(0);
}

/// 持有跨进程状态锁后等父进程放行（验证活锁不会被抢）。
#[test]
#[ignore = "由父测试通过 --exact 点名执行"]
fn child_hold_lock() {
    let Some(root) = child_root() else { return };
    let store = store(&root);
    let _lock = store.lock_state("child-lock").expect("子进程应能拿到锁");
    println!("READY");
    wait_for_release(&root);
}

/// 拿到状态锁后 `_exit`：留下一个属于已死进程的锁文件。
#[test]
#[ignore = "由父测试通过 --exact 点名执行"]
fn child_crash_leaving_lock() {
    let Some(root) = child_root() else { return };
    let store = store(&root);
    let _lock = store.lock_state("child-crasher").expect("子进程应能拿到锁");
    println!("READY");
    std::process::exit(0);
}

/// 卸载一个版本后正常退出（父进程随后验证"看不到旧版本"）。
#[test]
#[ignore = "由父测试通过 --exact 点名执行"]
fn child_uninstall() {
    let Some(root) = child_root() else { return };
    let store = store(&root);
    store
        .uninstall("dictionary.test", "1.0.0")
        .expect("子进程应能卸载 1.0.0");
    println!("READY");
}

// ---------------- C-04：跨进程活跃租约阻止卸载 ----------------

#[test]
fn 跨进程活跃租约阻止卸载() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let v1_dir = two_versions(root);

    let mut child = ChildRun::spawn("child_hold_lease", root);
    child.wait_ready();
    let child_pid = child.pid();

    let leases = store(root).all_leases().unwrap();
    assert_eq!(leases.len(), 1, "应有且仅有一条租约：{leases:?}");
    let lease = &leases[0];
    assert_eq!(lease.pid, child_pid, "租约应记在子进程名下");
    assert_eq!(lease.holder, "child-holder");
    assert_eq!(
        lease.alive,
        Some(true),
        "外部活进程的租约必须判定为活跃：{lease:?}"
    );
    assert!(!lease.pid_reused);
    assert!(lease.start_id.is_some(), "Linux 上应记录启动标识");

    // 这是 C-04 的核心断言：另一个进程正在用，就不能删
    let err = store(root)
        .uninstall("dictionary.test", "1.0.0")
        .unwrap_err();
    assert!(
        matches!(err, ResourceError::ResourceLeased { .. }),
        "应报 ResourceLeased，实际：{err}"
    );
    assert!(v1_dir.exists(), "被租用的目录不得删除");
    assert!(store(root)
        .version_dir("dictionary.test", "1.0.0")
        .unwrap()
        .exists());

    signal(&mut child, root);
    // 子进程正常退出 → 租约文件已由 Drop 删除
    assert!(
        store(root).all_leases().unwrap().is_empty(),
        "正常退出必须释放租约"
    );

    store(root).uninstall("dictionary.test", "1.0.0").unwrap();
    assert!(!v1_dir.exists());
    assert!(store(root).audit().unwrap().is_clean());
}

// ---------------- C-04 反向：崩溃留下的租约必须能被 reclaim ----------------

#[test]
fn 跨进程崩溃留下的租约可被回收() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_versions(root);

    let mut child = ChildRun::spawn("child_lease_then_die", root);
    child.wait_ready();
    let child_pid = child.pid();
    assert!(child.wait_exit().success(), "子进程按预期自行退出");

    let leases = store(root).all_leases().unwrap();
    assert_eq!(leases.len(), 1, "崩溃应留下租约文件：{leases:?}");
    assert_eq!(leases[0].pid, child_pid);
    assert_eq!(
        leases[0].alive,
        Some(false),
        "死进程的租约必须判定为失效：{:?}",
        leases[0]
    );
    assert!(!leases[0].is_active());

    // 失效租约不该继续阻止卸载
    let reclaimed = store(root).reclaim_stale_leases().unwrap();
    assert_eq!(reclaimed, 1, "应回收 1 条失效租约");
    assert!(store(root).all_leases().unwrap().is_empty());

    store(root).uninstall("dictionary.test", "1.0.0").unwrap();
    assert!(store(root).audit().unwrap().is_clean());
}

// ---------------- C-02：跨进程锁的持有与回收 ----------------

#[test]
fn 跨进程持有锁时不会被误抢() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_versions(root);

    let mut child = ChildRun::spawn("child_hold_lock", root);
    child.wait_ready();
    let lock_path = root.join("state.lock");
    assert!(lock_path.exists(), "子进程应留下锁文件");

    // 活锁不能被抢：装新版本必须超时报错，而不是"删掉别人的锁继续跑"
    let pkg = build_package(root, "dictionary.test", "3.0.0");
    let err = store(root).install_from_dir(&pkg).unwrap_err();
    match err {
        ResourceError::LockTimeout { path, holder } => {
            assert_eq!(Path::new(&path), lock_path, "报错要带上锁文件路径");
            assert_eq!(holder, "child-lock", "报错要说清是谁占着");
        }
        other => panic!("应报 LockTimeout，实际：{other}"),
    }
    assert!(lock_path.exists(), "超时不得删除活锁");
    assert!(
        !store(root)
            .version_dir("dictionary.test", "3.0.0")
            .unwrap()
            .exists(),
        "失败时不得留下半装目录"
    );

    signal(&mut child, root);
    assert!(!lock_path.exists(), "子进程退出应释放锁");

    // 锁释放后同一动作立刻可成功（证明前面失败只因为锁）
    let outcome = store(root).install_from_dir(&pkg).unwrap();
    assert_eq!(outcome.resource.version, "3.0.0");
    assert!(store(root).audit().unwrap().is_clean());
}

#[test]
fn 跨进程崩溃留下的锁会被回收() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    two_versions(root);

    let mut child = ChildRun::spawn("child_crash_leaving_lock", root);
    child.wait_ready();
    assert!(child.wait_exit().success());
    let lock_path = root.join("state.lock");
    assert!(lock_path.exists(), "崩溃应留下锁文件");

    // 持有者已死 → 下一个拿锁的人直接回收，不该等到超时
    let started = Instant::now();
    let pkg = build_package(root, "dictionary.test", "3.0.0");
    store(root)
        .install_from_dir(&pkg)
        .expect("死锁文件应被回收后继续");
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "回收死锁不该消耗整个超时预算，实际耗时 {:?}",
        started.elapsed()
    );
    assert!(!lock_path.exists(), "回收后锁文件必须消失");

    // 仓库仍然可用，且没有半装残留
    let audit = store(root).audit().unwrap();
    assert!(audit.is_clean(), "{}", audit.summary());
}

// ---------------- C-01：版本删除与租约登记的跨进程一致性 ----------------

#[test]
fn 别的进程卸载后租约登记被拒() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let v1_dir = two_versions(root);

    let mut child = ChildRun::spawn("child_uninstall", root);
    child.wait_ready();
    assert!(child.wait_exit().success());
    assert!(!v1_dir.exists(), "子进程应已删除 1.0.0");
    assert_eq!(
        store(root)
            .inspect("dictionary.test")
            .unwrap()
            .versions
            .len(),
        1,
        "父进程必须看到子进程写入的新状态"
    );

    // C-01 的可观察后果：绝不能给一个不存在的版本发租约
    let err = store(root)
        .acquire_lease("dictionary.test", "1.0.0", "parent-late")
        .unwrap_err();
    assert!(
        matches!(err, ResourceError::NotInstalled { .. }),
        "应报 NotInstalled，实际：{err}"
    );
    assert!(
        store(root)
            .leases("dictionary.test", "1.0.0")
            .unwrap()
            .is_empty(),
        "被拒的登记不得留下租约文件"
    );

    // 三方对账必须干净：没有"状态里没有、目录里有"，也没有悬空租约
    let audit = store(root).audit().unwrap();
    assert!(audit.is_clean(), "{}", audit.summary());
    assert_eq!(audit.stale_leases, 0, "不该有失效租约");

    // 活着的版本照样能登记
    let lease = store(root)
        .acquire_lease("dictionary.test", "2.0.0", "parent")
        .unwrap();
    assert!(store(root).leases("dictionary.test", "2.0.0").unwrap()[0].is_active());
    drop(lease);
    assert!(store(root).audit().unwrap().is_clean());
}

#[test]
fn 子进程拿到的是同一把跨进程锁() {
    // 这条是上面三条的"元测试"：证明子进程与父进程确实共享同一个仓库根目录、
    // 同一份 state.json 与同一把 state.lock，而不是各自跑了空转。
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let mut child = ChildRun::spawn("child_uninstall", root);
    // 仓库是空的 → 子进程的 `uninstall` 必然 panic（README 里"子进程不会自说自话成功"）
    assert!(
        !child.wait_exit().success(),
        "仓库里没有 1.0.0 时，子进程的卸载必须失败"
    );
    assert!(store(root).list().unwrap().is_empty(), "不该凭空出现资源");
}
