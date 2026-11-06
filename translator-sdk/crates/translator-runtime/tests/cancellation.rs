//! G06 取消合同测试（Linux；依赖真实子进程与 `/proc`）。
//!
//! 覆盖台账要求的两态：
//! - **排队中**：请求还没拿到传输层状态锁，取消应立即返回，不 spawn 新 worker；
//! - **执行中**：worker 正在推理，取消应终止独占 worker（含其进程组），
//!   不留 zombie / native 孤儿，且下一条请求不静默重放。
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use translator_runtime::model::{sha256_file, FileRole, FileSpec, ModelManifest, ModelSpec};
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::{RuntimeError, TranslationJob, WorkerConfig, WorkerProcess};
use translator_types::{CancelToken, TranslationErrorCode};

const BIN: &str = env!("CARGO_BIN_EXE_nmt-worker");

fn worker(args: &[&str]) -> WorkerConfig {
    WorkerConfig::new(BIN).with_args(args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

struct Fixture {
    _dir: tempfile::TempDir,
    spec: ModelSpec,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut files = std::collections::BTreeMap::new();
    for role in FileRole::ALL {
        let path = dir.path().join(format!("{role}.bin"));
        std::fs::write(&path, format!("fake-{role}-payload").as_bytes()).unwrap();
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
    let manifest = ModelManifest {
        package_id: "nmt.test.enzh".to_string(),
        version: "1.0.0".to_string(),
        files,
    };
    manifest.verify().unwrap();
    Fixture {
        _dir: dir,
        spec: ModelSpec::from_manifest(&manifest, 1, 1),
    }
}

fn job(text: &str) -> TranslationJob {
    TranslationJob {
        text: text.to_string(),
        target: "zh-Hans".to_string(),
    }
}

fn reachable(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn gone_within(pid: u32, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if !reachable(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    !reachable(pid)
}

fn wait_until(mut cond: impl FnMut() -> bool, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    cond()
}

#[test]
fn g06_排队中取消立即返回且不spawn() {
    let fx = fixture();
    let transport = Arc::new(ProcessTransport::new(
        worker(&["--mock", "--sleep-ms", "600"]),
        1,
    ));
    transport.load(&fx.spec).unwrap();
    assert_eq!(transport.metrics().spawns, 1);

    // 线程 A 占住状态锁（worker 正在"推理"600 ms）。
    let started = Arc::new(AtomicBool::new(false));
    let a_transport = Arc::clone(&transport);
    let a_started = Arc::clone(&started);
    let a = std::thread::spawn(move || {
        a_started.store(true, Ordering::SeqCst);
        a_transport.translate(&job("hold"))
    });
    assert!(wait_until(
        || started.load(Ordering::SeqCst),
        Duration::from_secs(1)
    ));
    // 给 A 一点时间真正拿到锁并发出请求
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(transport.active_requests(), 1, "A 应已在执行");

    // 线程 B 在排队；取消后应立即返回 Cancelled，且不产生新 worker。
    let token = CancelToken::new();
    let b_token = token.clone();
    let b_transport = Arc::clone(&transport);
    let started_at = Instant::now();
    let b = std::thread::spawn(move || {
        b_transport.translate_with_deadline_and_cancel(
            &job("queued"),
            Instant::now() + Duration::from_secs(5),
            Some(&b_token),
        )
    });
    std::thread::sleep(Duration::from_millis(80));
    token.cancel();
    let err = b.join().unwrap().unwrap_err();
    let elapsed = started_at.elapsed();
    assert!(matches!(err, RuntimeError::Cancelled), "{err}");
    assert!(
        elapsed < Duration::from_secs(1),
        "排队取消太慢：{elapsed:?}"
    );
    assert_eq!(
        transport.metrics().spawns,
        1,
        "排队取消不得 spawn 新 worker"
    );

    // A 不受影响，正常跑完。
    assert_eq!(a.join().unwrap().unwrap().text, "[zh-Hans] hold");
}

#[test]
fn g06_执行中取消终止worker并不留孤儿() {
    let fx = fixture();
    let transport = Arc::new(ProcessTransport::new(
        worker(&["--mock", "--sleep-ms", "2000"]),
        1,
    ));
    // load 也会 sleep 2 s；这是配置的固定行为，接受它换取"执行中取消"的真实窗口。
    transport.load(&fx.spec).unwrap();
    let pid = transport.worker_pid().expect("worker 应活着");

    let token = CancelToken::new();
    let cancel = token.clone();
    let t = Arc::clone(&transport);
    let handle = std::thread::spawn(move || {
        t.translate_with_deadline_and_cancel(
            &job("inflight"),
            Instant::now() + Duration::from_secs(30),
            Some(&cancel),
        )
    });
    assert!(
        wait_until(|| transport.active_requests() == 1, Duration::from_secs(2)),
        "请求应进入执行态"
    );
    let cancelled_at = Instant::now();
    token.cancel();
    let err = handle.join().unwrap().unwrap_err();
    assert!(matches!(err, RuntimeError::Cancelled), "{err}");
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "取消应在 ~20 ms 量级生效，实际 {:?}",
        cancelled_at.elapsed()
    );

    assert!(
        gone_within(pid, Duration::from_secs(3)),
        "worker {pid} 必须被回收"
    );
    assert_eq!(transport.metrics().spawns, 1, "取消不 spawn 新 worker");
    assert!(transport.worker_pid().is_none());

    // 不静默重放：下一条请求明确要求重新 load。
    let err = transport.translate(&job("again")).unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::RuntimeInitFailed, "{err}");

    // 重新 load 后可以继续工作（新 epoch）。
    let info = transport.load(&fx.spec).unwrap();
    assert!(info.worker_epoch >= 2, "应启动新 worker：{info:?}");
    assert_eq!(
        transport.translate(&job("ok")).unwrap().text,
        "[zh-Hans] ok"
    );
}

#[test]
fn g06_已取消的令牌不产生任何进程() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock"]), 1);
    let token = CancelToken::new();
    token.cancel();
    let err = transport
        .translate_with_deadline_and_cancel(
            &job("never"),
            Instant::now() + Duration::from_secs(5),
            Some(&token),
        )
        .unwrap_err();
    assert!(matches!(err, RuntimeError::Cancelled), "{err}");
    assert_eq!(transport.metrics().spawns, 0, "预取消不得 spawn");
    let _ = fx;
}

#[test]
fn g06_终止会收掉worker的整个进程组() {
    // 用一个会派生后台子进程的假 worker 验证进程组语义：
    // 只杀 worker 而留下 `sleep` 就是 G06 说的 native 孤儿。
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("grandchild.pid");
    let script = format!("sleep 300 & echo $! > {}; wait", marker.display());
    let mut worker =
        WorkerProcess::spawn(WorkerConfig::new("/bin/sh").with_args(["-c".to_string(), script]))
            .unwrap();
    let worker_pid = worker.pid();
    assert!(
        wait_until(|| marker.exists(), Duration::from_secs(3)),
        "假 worker 应写出孙进程 pid"
    );
    let grandchild: u32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(reachable(grandchild));

    worker.terminate();
    assert!(
        gone_within(worker_pid, Duration::from_secs(3)),
        "worker 未被回收"
    );
    assert!(
        gone_within(grandchild, Duration::from_secs(3)),
        "孙进程 {grandchild} 成了孤儿"
    );
}

/// 编译期保证 `PathBuf` 未使用时不会告警（不同测试用到不同导入）。
#[allow(dead_code)]
fn _unused(_: PathBuf) {}
