//! 生命周期与一致性（`docs/17` §4.2 的 LC-01/02/04/06/09）。
//!
//! 这些用例跑 mock worker（不需要真实权重），验证的是**调度语义**：
//! single-flight、排队 deadline、活跃请求与卸载的关系、快照不堆积。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use translator_runtime::model::{sha256_file, FileRole, FileSpec, ModelManifest};
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::{TranslationJob, WorkerConfig};
use translator_types::TranslationErrorCode;

const WORKER: &str = env!("CARGO_BIN_EXE_nmt-worker");

struct Fixture {
    _dir: tempfile::TempDir,
    root: tempfile::TempDir,
    manifest: ModelManifest,
}

fn fixture(version: &str, model_body: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut files = BTreeMap::new();
    for role in [
        FileRole::Model,
        FileRole::SourceVocab,
        FileRole::TargetVocab,
        FileRole::LexicalShortlist,
    ] {
        let path = dir.path().join(format!("{}.bin", role.as_str()));
        let body = if role == FileRole::Model {
            model_body.to_string()
        } else {
            format!("{role}-body")
        };
        std::fs::write(&path, body).unwrap();
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
    Fixture {
        _dir: dir,
        root: tempfile::tempdir().unwrap(),
        manifest: ModelManifest {
            package_id: "nmt.lifecycle.test".into(),
            version: version.into(),
            files,
        },
    }
}

fn transport(args: &[&str], budget: u32) -> Arc<ProcessTransport> {
    Arc::new(ProcessTransport::new(
        WorkerConfig::new(WORKER).with_args(args.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        budget,
    ))
}

fn job(text: &str) -> TranslationJob {
    TranslationJob {
        text: text.to_string(),
        target: "zh-Hans".to_string(),
    }
}

/// LC-01：并发 10 次加载同一模型 → 只真正加载一次。
#[test]
fn lc01_并发加载同一模型只spawn一次() {
    let fx = fixture("1.0.0", "model-body");
    let transport = transport(&["--mock"], 1);

    let mut handles = Vec::new();
    for _ in 0..10 {
        let transport = Arc::clone(&transport);
        let manifest = fx.manifest.clone();
        let root = fx.root.path().to_path_buf();
        handles.push(std::thread::spawn(move || {
            transport
                .load_from_manifest(&manifest, &root, 1, 1)
                .map(|info| info.model_revision)
        }));
    }
    let mut revisions = Vec::new();
    for handle in handles {
        revisions.push(handle.join().unwrap().expect("并发加载都应当成功"));
    }
    assert!(
        revisions.windows(2).all(|w| w[0] == w[1]),
        "revision 必须一致"
    );
    assert_eq!(
        transport.metrics().spawns,
        1,
        "10 个并发加载只该起 1 个 worker：{:?}",
        transport.metrics()
    );
    // 快照也只该有一份（其余调用复用结果后自行删除）
    let snaps: Vec<_> = std::fs::read_dir(fx.root.path())
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(snaps.len(), 1, "只该留一份快照：{snaps:?}");
}

/// LC-02：翻译进行中调用 unload → 不死锁、不打断、翻译照常成功。
#[test]
fn lc02_翻译进行中卸载不打断也不死锁() {
    let fx = fixture("1.0.0", "model-body");
    let transport = transport(&["--mock", "--sleep-ms", "300"], 1);
    transport
        .load_from_manifest(&fx.manifest, fx.root.path(), 1, 1)
        .unwrap();

    let worker = Arc::clone(&transport);
    let translate = std::thread::spawn(move || worker.translate(&job("slow")));

    // 等请求真的进到 worker 里
    let deadline = Instant::now() + Duration::from_secs(2);
    while transport.active_requests() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    let started = Instant::now();
    transport.unload().unwrap();
    let waited = started.elapsed();

    let reply = translate
        .join()
        .expect("翻译线程不该崩")
        .expect("翻译应当成功");
    assert!(reply.text.contains("[zh-Hans]"), "{reply:?}");
    assert!(
        waited >= Duration::from_millis(100),
        "unload 应当等翻译做完（实际只等了 {waited:?}）"
    );
    assert_eq!(transport.active_requests(), 0);
    assert!(!transport.has_snapshot(), "卸载后快照必须消失");
}

/// LC-04：第二个请求的 deadline 到期 → 在真正调用 native 之前返回 Timeout。
#[test]
fn lc04_排队deadline到期返回timeout而不是无限等() {
    let fx = fixture("1.0.0", "model-body");
    let transport = transport(&["--mock", "--sleep-ms", "400"], 1);
    transport
        .load_from_manifest(&fx.manifest, fx.root.path(), 1, 1)
        .unwrap();

    let worker = Arc::clone(&transport);
    let slow = std::thread::spawn(move || worker.translate(&job("slow")));
    let deadline = Instant::now() + Duration::from_secs(2);
    while transport.active_requests() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    let started = Instant::now();
    let err = transport
        .translate_with_deadline(&job("queued"), Instant::now() + Duration::from_millis(50))
        .unwrap_err();
    let elapsed = started.elapsed();

    assert_eq!(err.code(), TranslationErrorCode::Timeout, "{err}");
    match err {
        translator_runtime::RuntimeError::Timeout { operation, .. } => {
            assert_eq!(operation, "queue", "必须是排队超时而不是执行超时");
        }
        other => panic!("期望 Timeout，得到 {other}"),
    }
    assert!(
        elapsed < Duration::from_millis(300),
        "排队超时必须早于慢请求结束（实际 {elapsed:?}）"
    );
    // 慢请求不受影响
    slow.join().unwrap().unwrap();
}

/// LC-06：反复加载/卸载 10 轮 → 快照不堆积、没有残留进程。
#[test]
fn lc06_十轮加载卸载不堆积快照与进程() {
    let fx = fixture("1.0.0", "model-body");
    let transport = transport(&["--mock"], 1);

    let mut worker_pids = Vec::new();
    for round in 1..=10 {
        let info = transport
            .load_from_manifest(&fx.manifest, fx.root.path(), 1, 1)
            .unwrap_or_else(|e| panic!("第 {round} 轮加载失败：{e}"));
        transport.translate(&job("Hello world!")).unwrap();
        transport.unload().unwrap();
        assert!(!transport.has_snapshot(), "第 {round} 轮卸载后仍有快照");
        let left: Vec<_> = std::fs::read_dir(fx.root.path())
            .unwrap()
            .flatten()
            .collect();
        assert!(left.is_empty(), "第 {round} 轮留下了快照：{left:?}");
        if let Some(pid) = info.native_pid {
            worker_pids.push(pid);
        }
    }
    assert_eq!(transport.metrics().spawns, 1, "卸载不重启 worker");
    assert_eq!(transport.metrics().respawns, 0);
    // mock 没有 native 进程；这里只保证 worker 仍在（卸载不该杀 worker）
    assert!(transport.metrics().worker_pid.is_some());
}

/// LC-03/LC-09：同 id@version、不同内容 → 必须重新加载，不能复用旧状态。
#[test]
fn lc03_同版本不同内容不复用旧加载() {
    let fx_a = fixture("1.0.0", "model-body-A");
    let fx_b = fixture("1.0.0", "model-body-B");
    assert_ne!(
        fx_a.manifest.content_digest(),
        fx_b.manifest.content_digest(),
        "两份模型的摘要必须不同"
    );

    let transport = transport(&["--mock"], 1);
    let a = transport
        .load_from_manifest(&fx_a.manifest, fx_a.root.path(), 1, 1)
        .unwrap();
    assert_eq!(a.model_revision, fx_a.manifest.content_digest());

    let b = transport
        .load_from_manifest(&fx_b.manifest, fx_b.root.path(), 1, 1)
        .unwrap();
    assert_eq!(
        b.model_revision,
        fx_b.manifest.content_digest(),
        "内容变了必须换 revision"
    );
    assert_ne!(a.model_revision, b.model_revision);
    // 同一个 worker 进程可以换模型，但 revision 不能沿用旧的
    let reply = transport.translate(&job("x")).unwrap();
    assert_eq!(reply.model_revision, fx_b.manifest.content_digest());
}

/// 卸载后仍在用的旧快照必须已经删掉：不留"已打开文件"之外的残留。
#[test]
fn 卸载释放快照目录() {
    let fx = fixture("1.0.0", "model-body");
    let transport = transport(&["--mock"], 1);
    transport
        .load_from_manifest(&fx.manifest, fx.root.path(), 1, 1)
        .unwrap();
    let dir = transport.snapshot_model_path().unwrap();
    assert!(dir.exists());
    transport.unload().unwrap();
    assert!(!Path::new(&dir).exists(), "快照文件应当随卸载删除");
}
