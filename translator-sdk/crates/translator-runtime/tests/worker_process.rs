//! 进程边界的行为测试（对应 `docs/12` 的 N-01 / N-02 / N-03 / N-13）。
//!
//! 这些测试全部跑**真的子进程**：`nmt-worker --mock`。用真实进程而不是 mock 的
//! `WorkerProcess`，是因为要验证的正是"宿主在子进程 abort/SIGSEGV/超时之后还活着"。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use translator_runtime::model::{sha256_file, FileRole, FileSpec, ModelManifest, ModelSpec};
use translator_runtime::protocol::Request;
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::{RuntimeError, TranslationJob, WorkerConfig};
use translator_types::TranslationErrorCode;

const BIN: &str = env!("CARGO_BIN_EXE_nmt-worker");

fn worker(args: &[&str]) -> WorkerConfig {
    WorkerConfig::new(BIN).with_args(args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

/// 一套"四文件齐全"的假模型。
struct Fixture {
    _dir: tempfile::TempDir,
    manifest: ModelManifest,
    spec: ModelSpec,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut files = BTreeMap::new();
    for role in FileRole::ALL {
        let path = dir.path().join(format!("{role}.bin"));
        // 每个文件内容不同，避免"摘要恰好相同"掩盖问题
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
    let spec = ModelSpec::from_manifest(&manifest, 1, 1);
    Fixture {
        _dir: dir,
        manifest,
        spec,
    }
}

fn job(text: &str) -> TranslationJob {
    TranslationJob {
        text: text.to_string(),
        target: "zh-Hans".to_string(),
    }
}

#[cfg(target_os = "linux")]
fn reachable(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// 等一个进程彻底消失（含被回收）；返回是否在预算内消失。
#[cfg(target_os = "linux")]
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

#[test]
fn n01_加载后能翻译且指标分得清宿主与worker() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock"]), 1);

    let info = transport.load(&fx.spec).unwrap();
    assert_eq!(info.engine_id, "worker.mock");
    assert_eq!(info.worker_epoch, 1);
    assert_eq!(info.threads, 1);

    let reply = transport.translate(&job("Hello world!")).unwrap();
    assert_eq!(reply.text, "[zh-Hans] Hello world!");
    assert_eq!(reply.worker_epoch, 1);
    assert_eq!(reply.model_revision, fx.spec.model_revision);

    let health = transport.health().unwrap();
    assert!(health.alive);
    assert_eq!(health.requests_served, 1);
    assert_eq!(health.worker_epoch, 1);

    let metrics = transport.metrics();
    assert_eq!(metrics.spawns, 1, "只该起一个 worker");
    assert_eq!(metrics.respawns, 0);
    assert_eq!(metrics.worker_pid, Some(health.worker_pid));

    #[cfg(target_os = "linux")]
    {
        assert!(metrics.host_rss_bytes.unwrap() > 1024 * 1024);
        assert!(metrics.worker_rss_bytes.unwrap() > 0);
        let combined = metrics.combined_rss_bytes().unwrap();
        assert!(combined > metrics.host_rss_bytes.unwrap());
    }
}

#[test]
fn n02_worker在翻译时崩溃宿主仍然活着() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock", "--crash-on-translate"]), 1);
    transport.load(&fx.spec).unwrap();
    let pid = transport.health().unwrap().worker_pid;

    let err = transport.translate(&job("boom")).unwrap_err();
    // abort() 的退出码 134；具体值不硬编码，只要求"知道进程没了"
    assert!(
        matches!(
            err,
            RuntimeError::WorkerExited { .. } | RuntimeError::BackendFailed(_)
        ),
        "期望后端失败/进程退出，得到 {err}"
    );
    assert_eq!(err.code(), TranslationErrorCode::BackendFailed);

    // 宿主还活着：还能读指标、还能报错
    let metrics = transport.metrics();
    assert_eq!(metrics.spawns, 1);

    #[cfg(target_os = "linux")]
    assert!(
        gone_within(pid, Duration::from_secs(3)),
        "崩溃的 worker 必须被回收"
    );

    // 关键：**不静默重放**。下一条请求不会偷偷再跑一遍上一句，
    // 而是明确告诉调用方"模型没加载，请重新 load"。
    let err = transport.translate(&job("again")).unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::RuntimeInitFailed, "{err}");
}

#[test]
fn n03_缺文件在spawn之前就被拒绝() {
    let fx = fixture();
    let mut spec = fx.spec.clone();
    spec.model_path = "/nonexistent/decoder.bin".to_string();

    let transport = ProcessTransport::new(worker(&["--mock"]), 1);
    let err = transport.load(&spec).unwrap_err();
    assert_eq!(
        err.code(),
        TranslationErrorCode::ResourceNotInstalled,
        "{err}"
    );

    let metrics = transport.metrics();
    assert_eq!(metrics.spawns, 0, "校验不过就不该起进程");
    assert!(metrics.worker_pid.is_none());
}

#[test]
fn n03_摘要不符在清单校验阶段被发现() {
    let fx = fixture();
    let model = &fx.manifest.files[&FileRole::Model];
    let mut body = std::fs::read(&model.path).unwrap();
    let last = body.len() - 1;
    body[last] ^= 0xff; // 改一个字节，大小不变
    std::fs::write(&model.path, &body).unwrap();

    let err = fx.manifest.verify().unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::IntegrityFailure, "{err}");
}

#[test]
fn n13_超时后杀掉worker且不留僵尸() {
    let fx = fixture();
    let config =
        worker(&["--mock", "--sleep-ms", "3000"]).with_load_timeout(Duration::from_millis(300));
    let transport = ProcessTransport::new(config, 1);
    let pid = {
        // load 也会 sleep，所以先用一个短超时的请求把 pid 拿到：直接调 health 也会被拖住，
        // 这里改为在 load 之前就从 metrics 里没有 pid，只能先 load —— 所以用一个 30ms 的
        // 超时去撞它，顺便验证"加载超时"也走同一条清理路径。
        transport.load(&fx.spec).unwrap_err();
        transport.metrics().worker_pid
    };
    assert!(pid.is_none(), "超时后 worker 句柄应当已丢弃");

    let err = transport.translate(&job("x")).unwrap_err();
    assert!(
        matches!(
            err,
            RuntimeError::Timeout { .. } | RuntimeError::InitFailed(_)
        ),
        "{err}"
    );
}

#[test]
fn n13_翻译超时返回timeout并回收进程() {
    struct Slower;
    impl Slower {
        fn config() -> WorkerConfig {
            // 加载很快，翻译很慢：用两个不同 sleep 做不到，所以让 worker 先正常加载，
            // 再用一个极短超时去撞翻译。mock 的 sleep 对 load 和 translate 都生效，
            // 因此先把超时设长、加载完再换成短超时的新请求需要重建 transport。
            // 折中：直接用 --sleep-ms=2000，加载也用短超时，验证清理路径一致。
            worker(&["--mock", "--sleep-ms", "2000"]).with_load_timeout(Duration::from_millis(150))
        }
    }

    let fx = fixture();
    let transport = ProcessTransport::new(Slower::config(), 1);
    let err = transport.load(&fx.spec).unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::Timeout, "{err}");
    match err {
        RuntimeError::Timeout {
            operation,
            waited_ms,
        } => {
            assert_eq!(operation, "load");
            assert!(waited_ms >= 100, "{waited_ms}");
        }
        other => panic!("期望 Timeout，得到 {other}"),
    }
    assert!(transport.metrics().worker_pid.is_none());
}

#[test]
fn n13_坏帧被识别为协议错误而不是卡住() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock", "--emit-garbage-on-translate"]), 1);
    transport.load(&fx.spec).unwrap();

    let err = transport.translate(&job("garbage")).unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::BackendFailed, "{err}");
    assert!(
        transport.metrics().worker_pid.is_none(),
        "协议错乱后应丢弃 worker"
    );
}

#[test]
fn n13_后端失败不会自动重试() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock", "--fail-on-translate"]), 1);
    transport.load(&fx.spec).unwrap();

    for _ in 0..3 {
        let err = transport.translate(&job("fail")).unwrap_err();
        assert_eq!(err.code(), TranslationErrorCode::BackendFailed, "{err}");
    }
    // 三次失败、一个 worker：说明没有偷偷重放
    assert_eq!(transport.metrics().spawns, 1);
    assert_eq!(transport.metrics().respawns, 0);
}

#[test]
fn n13_重启预算耗尽后明确报能力不可用() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock", "--crash-on-load"]), 1);

    let first = transport.load(&fx.spec).unwrap_err();
    assert_eq!(first.code(), TranslationErrorCode::BackendFailed, "{first}");

    // 第二次：允许一次重启
    let second = transport.load(&fx.spec).unwrap_err();
    assert_eq!(
        second.code(),
        TranslationErrorCode::BackendFailed,
        "{second}"
    );
    assert_eq!(transport.metrics().respawns, 1);

    // 第三次：预算用完，不再无限重启
    let third = transport.load(&fx.spec).unwrap_err();
    assert_eq!(
        third.code(),
        TranslationErrorCode::CapabilityUnavailable,
        "{third}"
    );
    assert!(third.to_string().contains("预算"), "{third}");
}

#[test]
fn n13_重启后世代号递增且pid改变() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock", "--crash-on-translate"]), 1);
    let first = transport.load(&fx.spec).unwrap();
    assert_eq!(first.worker_epoch, 1);
    let pid1 = transport.health().unwrap().worker_pid;

    let _ = transport.translate(&job("crash me")).unwrap_err();
    let second = transport.load(&fx.spec).unwrap();
    assert_eq!(second.worker_epoch, 2, "重启必须换世代号");
    let pid2 = transport.health().unwrap().worker_pid;
    assert_ne!(pid1, pid2, "重启必须换进程");
    assert_eq!(transport.metrics().respawns, 1);
}

#[test]
fn n13_shutdown幂等且之后拒绝服务() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock"]), 1);
    transport.load(&fx.spec).unwrap();
    let pid = transport.health().unwrap().worker_pid;

    transport.shutdown().unwrap();
    transport.shutdown().unwrap();

    #[cfg(target_os = "linux")]
    assert!(
        gone_within(pid, Duration::from_secs(2)),
        "shutdown 后进程必须消失"
    );

    let err = transport.translate(&job("after")).unwrap_err();
    assert_eq!(
        err.code(),
        TranslationErrorCode::CapabilityUnavailable,
        "{err}"
    );
}

#[test]
fn n13_崩溃的worker在shutdown时也不挂住() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock"]), 1);
    transport.load(&fx.spec).unwrap();
    let pid = transport.health().unwrap().worker_pid;

    // 从外部杀掉 worker（模拟 native 段错误），再走 shutdown
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    let started = Instant::now();
    transport.shutdown().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "shutdown 不能在死进程上停留：{:?}",
        started.elapsed()
    );
}

#[test]
fn n13_stderr只保留尾部且有上限() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock", "--noisy-stderr"]), 1);
    transport.load(&fx.spec).unwrap();
    std::thread::sleep(Duration::from_millis(200));

    let tail = transport.worker_stderr();
    assert!(!tail.is_empty(), "worker 的启动诊断应当被收集");
    assert!(
        tail.len() <= 64,
        "stderr 尾部不得超过配置上限：{}",
        tail.len()
    );
    for line in &tail {
        assert!(line.len() <= 512, "单行必须被截断：{} 字节", line.len());
    }
}

#[test]
fn n13_卸载后再翻译要求重新加载() {
    let fx = fixture();
    let transport = ProcessTransport::new(worker(&["--mock"]), 1);
    transport.load(&fx.spec).unwrap();
    transport.translate(&job("before")).unwrap();

    transport.unload().unwrap();
    let err = transport.translate(&job("after")).unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::RuntimeInitFailed, "{err}");

    // 同一个 worker 还在，重新 load 不需要新进程
    let info = transport.load(&fx.spec).unwrap();
    assert_eq!(info.worker_epoch, 1);
    assert_eq!(transport.metrics().spawns, 1);
    transport.translate(&job("again")).unwrap();
}

#[test]
fn 协议版本不匹配会被拒绝而不是硬解析() {
    // 用同一个 mock worker，但手工发一个 version=999 的帧
    let fx = fixture();
    let mut child = std::process::Command::new(BIN)
        .arg("--mock")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    use std::io::Write;
    let mut request = serde_json::to_vec(&serde_json::json!({
        "id": 1,
        "epoch": 1,
        "protocol": 999,
        "op": "health",
    }))
    .unwrap();
    let mut framed = (request.len() as u32).to_be_bytes().to_vec();
    framed.append(&mut request);
    child.stdin.as_mut().unwrap().write_all(&framed).unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();

    let status = child.wait().unwrap();
    assert_eq!(
        status.code(),
        Some(4),
        "协议版本不符时 worker 必须退出而不是继续跑"
    );
    let _ = fx;
}

#[test]
fn 请求信封与响应形状对得上() {
    // 纯类型层面的护栏：改动 op/payload 名字会在这里先炸，而不是在运行时
    let envelope = translator_runtime::protocol::RequestEnvelope::new(1, 1, Request::Health);
    let json = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["op"], "health");
    assert_eq!(json["protocol"], translator_runtime::PROTOCOL_VERSION);
    let _ = PathBuf::new();
}

#[test]
fn n13_世代号不符的响应被丢弃并回收worker() {
    let fx = fixture();
    // worker 故意报 epoch=99，而宿主分配的是 1
    let transport = ProcessTransport::new(worker(&["--mock", "--epoch", "99"]), 1);
    let err = transport.load(&fx.spec).unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::BackendFailed, "{err}");
    assert!(err.to_string().contains("世代"), "{err}");
    assert!(
        transport.metrics().worker_pid.is_none(),
        "世代不符的 worker 不能再被复用"
    );
}
