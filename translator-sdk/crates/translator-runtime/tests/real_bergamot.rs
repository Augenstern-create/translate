//! P3.1-B：**真实 Bergamot** 的端到端验收（`docs/14` §5 的 B-T01～B-T11）。
//!
//! 这些测试需要本机的真实权重与已构建的 native 二进制（都不入库），
//! 所以默认**干净地跳过**并在输出里写明原因；准备好环境后会真跑：
//!
//! ```bash
//! export NMT_BERGAMOT_BIN=/tmp/p3/bergamot-translator/build/app/bergamot
//! export NMT_MODEL_DIR=/tmp/p3/models
//! export LD_LIBRARY_PATH=/tmp/p3/sysroot/usr/lib/x86_64-linux-gnu:/tmp/p3/sysroot/usr/lib/x86_64-linux-gnu/openblas-pthread
//! cargo test -p translator-runtime --test real_bergamot -- --test-threads=1 --nocapture
//! ```
//!
//! 跳过不是"通过"：`docs/15` 里会分别记录"跑了什么"和"跳过了什么"。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use translator_runtime::model::{
    rss_bytes_of, sha256_file, FileRole, FileSpec, ModelManifest, ModelSpec,
};
use translator_runtime::protocol::Request;
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::{TranslationJob, WorkerConfig};
use translator_types::TranslationErrorCode;

const WORKER: &str = env!("CARGO_BIN_EXE_nmt-worker");

// ---------------------------------------------------------------------------
// 环境探测：缺任何一样就整体跳过（不是失败，也不假装通过）
// ---------------------------------------------------------------------------

struct Env {
    native: PathBuf,
    model_dir: PathBuf,
    snapshot_root: tempfile::TempDir,
}

fn env() -> Option<Env> {
    let native = PathBuf::from(std::env::var_os("NMT_BERGAMOT_BIN")?);
    let model_dir = PathBuf::from(std::env::var_os("NMT_MODEL_DIR")?);
    let model = model_dir.join("model.enzh.intgemm.alphas.bin");
    if !native.exists() || !model.exists() {
        return None;
    }
    Some(Env {
        native,
        model_dir,
        snapshot_root: tempfile::tempdir().ok()?,
    })
}

/// 拿不到环境时打印原因并跳过。
macro_rules! require_env {
    () => {
        match env() {
            Some(env) => env,
            None => {
                eprintln!(
                    "SKIP：未设置 NMT_BERGAMOT_BIN / NMT_MODEL_DIR（或模型文件不存在），\
                     本用例需要真实权重与 native 二进制（均不入库）"
                );
                return;
            }
        }
    };
}

/// 与 `fixtures/nmt/models.lock.json` 一致：四文件必需 + ssplit 前缀可选。
fn manifest(dir: &Path) -> ModelManifest {
    let files: [(FileRole, &str); 5] = [
        (FileRole::Model, "model.enzh.intgemm.alphas.bin"),
        (FileRole::SourceVocab, "srcvocab.enzh.spm"),
        (FileRole::TargetVocab, "trgvocab.enzh.spm"),
        (FileRole::LexicalShortlist, "lex.50.50.enzh.s2t.bin"),
        (FileRole::SplitPrefix, "nonbreaking_prefix.en"),
    ];
    let mut map = BTreeMap::new();
    for (role, name) in files {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let (bytes, sha256) = sha256_file(&path).unwrap();
        map.insert(
            role,
            FileSpec {
                role,
                path,
                bytes,
                sha256,
            },
        );
    }
    ModelManifest {
        package_id: "nmt.enzh.mozilla".to_string(),
        version: "3.0".to_string(),
        files: map,
    }
}

fn worker_config(env: &Env) -> WorkerConfig {
    WorkerConfig::new(WORKER).with_args([
        "--bergamot".to_string(),
        "--native".to_string(),
        env.native.display().to_string(),
        "--runtime-build".to_string(),
        "9271618ebbdc5d21ac4dc4df9e72beb7ce644774".to_string(),
    ])
}

fn job(text: &str) -> TranslationJob {
    TranslationJob {
        text: text.to_string(),
        target: "zh-Hans".to_string(),
    }
}

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// 进程是否**还在跑**（僵尸不算：`/proc/<pid>` 对僵尸也存在）。
fn pid_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // 格式：pid (comm) state ...
    match stat.rsplit(')').next() {
        Some(rest) => !rest.trim_start().starts_with('Z'),
        None => false,
    }
}

/// 扫描 `/proc/*/cmdline` 里包含 `needle` 的进程（用来抓"native 变孤儿"）。
fn procs_matching(needle: &str) -> Vec<u32> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let cmdline = String::from_utf8_lossy(&cmdline);
        if cmdline.contains(needle) {
            found.push(pid);
        }
    }
    found
}

// ---------------------------------------------------------------------------
// B-T01 / B-T02 / B-T04：真实加载、真译文、连续跑
// ---------------------------------------------------------------------------

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt01_真实bergamot在worker里加载并返回真实运行时元数据() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 1);

    let info = transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .expect("真实模型应当能加载");

    assert_eq!(info.engine_id, "nmt.bergamot");
    assert_eq!(info.runtime.as_deref(), Some("bergamot-cli"));
    assert_eq!(
        info.runtime_build.as_deref(),
        Some("9271618ebbdc5d21ac4dc4df9e72beb7ce644774")
    );
    assert_eq!(info.model_revision, manifest.content_digest());
    assert_eq!(info.threads, 1);
    assert!(
        info.native_pid.map(pid_alive).unwrap_or(false),
        "native 进程应当活着：{:?}",
        info.native_pid
    );

    let metrics = transport.metrics();
    assert_eq!(metrics.spawns, 1);
    assert!(metrics.model_preparation_ms.is_some(), "快照耗时要被记录");
    assert!(metrics.snapshot_bytes.unwrap() > 40 * 1024 * 1024);

    transport.shutdown().unwrap();
}

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt02_hello_world_得到真译文而不是mock() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 1);
    transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .unwrap();

    let reply = transport.translate(&job("Hello world!")).unwrap();
    println!("Hello world! → {:?}", reply.text);
    assert!(
        !reply.text.contains("[zh-Hans]"),
        "不能是 mock 译文：{}",
        reply.text
    );
    assert!(
        reply.text.contains("你好") && reply.text.contains("世界"),
        "{}",
        reply.text
    );
    assert_eq!(reply.model_revision, manifest.content_digest());
    assert_eq!(reply.worker_epoch, 1);

    transport.shutdown().unwrap();
}

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt03_黄金样本的已知失败必须如实暴露() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 1);
    transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .unwrap();

    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/nmt/golden-v1.jsonl");
    let text = std::fs::read_to_string(&golden).unwrap();
    let mut passed = 0;
    let mut failed = Vec::new();
    let mut total = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        let id = row["id"].as_str().unwrap().to_string();
        let source = row["text"].as_str().unwrap().to_string();
        let expect_error = row["constraints"]["expect_error"]
            .as_bool()
            .unwrap_or(false);
        total += 1;

        if expect_error {
            // 空/纯空白：当前 CLI 行为是返回空串而不是报错，这里只记录不判定
            let reply = transport.translate(&job(&source));
            println!("{id}: expect_error=true → {reply:?}");
            continue;
        }

        let reply = match transport.translate(&job(&source)) {
            Ok(reply) => reply,
            Err(err) => {
                failed.push(format!("{id}: 翻译失败 {err}"));
                continue;
            }
        };
        let mut ok = true;
        if let Some(list) = row["constraints"]["must_contain"].as_array() {
            for needle in list {
                let needle = needle.as_str().unwrap();
                if !reply.text.contains(needle) {
                    ok = false;
                    failed.push(format!("{id}: 缺少 {needle:?} in {:?}", reply.text));
                }
            }
        }
        if let Some(list) = row["constraints"]["must_contain_any"].as_array() {
            let any = list
                .iter()
                .filter_map(|n| n.as_str())
                .any(|n| reply.text.contains(n));
            if !any {
                ok = false;
                failed.push(format!("{id}: 一个都不含 {list:?} in {:?}", reply.text));
            }
        }
        if ok {
            passed += 1;
        }
    }

    println!("黄金样本：{passed}/{total} 通过；失败 {} 条", failed.len());
    for line in &failed {
        println!("  - {line}");
    }
    // 已知且必须继续暴露的失败：模板变量 / 反引号（P3.1-C 的阻断项）
    let known: Vec<&String> = failed
        .iter()
        .filter(|f| f.contains("var-01") || f.contains("code-01") || f.contains("multi-01"))
        .collect();
    assert!(
        failed.len() <= known.len(),
        "除已知的占位符/代码类失败外不应有新的失败：{failed:?}"
    );
    assert!(passed >= 18, "至少 18/24（P3.0 记录为 23/24）：{passed}");

    transport.shutdown().unwrap();
}

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt04_连续一百次真实翻译不丢帧() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 1);
    let info = transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .unwrap();

    let inputs = [
        "Hello world!",
        "The library opens at nine in the morning.",
        "Please download the offline dictionary package first.",
    ];
    let mut samples = Vec::new();
    for i in 0..100 {
        let started = Instant::now();
        let reply = transport
            .translate(&job(inputs[i % inputs.len()]))
            .unwrap_or_else(|e| panic!("第 {i} 次翻译失败：{e}"));
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert!(!reply.text.is_empty(), "第 {i} 次返回空译文");
        assert_eq!(reply.model_revision, info.model_revision);
        assert_eq!(reply.worker_epoch, info.worker_epoch);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "100 次真实翻译：p50 {:.2} ms / p95 {:.2} ms / max {:.2} ms",
        samples[49], samples[94], samples[99]
    );
    assert_eq!(transport.metrics().spawns, 1, "100 次翻译不该重启 worker");
    transport.shutdown().unwrap();
}

// ---------------------------------------------------------------------------
// B-T05：加载/卸载三轮
// ---------------------------------------------------------------------------

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt05_三轮加载卸载后内存不持续增长() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 1);

    let mut worker_rss = Vec::new();
    let mut native_rss = Vec::new();
    for round in 1..=3 {
        let info = transport
            .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
            .unwrap_or_else(|e| panic!("第 {round} 轮加载失败：{e}"));
        transport.translate(&job("Hello world!")).unwrap();
        worker_rss.push(transport.metrics().worker_rss_bytes.unwrap_or(0));
        native_rss.push(info.native_pid.and_then(rss_bytes_of).unwrap_or(0));
        transport.unload().unwrap();
        assert!(!transport.has_snapshot(), "卸载后快照必须删除");
        assert!(
            info.native_pid.map(|p| !pid_alive(p)).unwrap_or(true),
            "第 {round} 轮卸载后 native 应当退出"
        );
    }
    println!("worker RSS 三轮：{worker_rss:?}；native RSS 三轮：{native_rss:?}");
    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    println!(
        "  → worker {:.1}/{:.1}/{:.1} MiB，native {:.1}/{:.1}/{:.1} MiB",
        mib(worker_rss[0]),
        mib(worker_rss[1]),
        mib(worker_rss[2]),
        mib(native_rss[0]),
        mib(native_rss[1]),
        mib(native_rss[2])
    );

    let first = worker_rss[0] as f64;
    let last = worker_rss[2] as f64;
    assert!(
        last < first * 1.5 + 32.0 * 1024.0 * 1024.0,
        "worker RSS 不应持续增长：{worker_rss:?}"
    );
    assert_eq!(
        transport.metrics().spawns,
        1,
        "卸载不重启 worker（native 重建）"
    );
    transport.shutdown().unwrap();
}

// ---------------------------------------------------------------------------
// B-T06：真实 native 崩溃
// ---------------------------------------------------------------------------

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt06_真实native崩溃不影响宿主且错误分类正确() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let mut spec: ModelSpec = {
        let snap = translator_runtime::ModelSnapshot::materialize(
            &manifest,
            env.snapshot_root.path(),
            1,
            1,
        )
        .unwrap();
        snap.spec().clone()
    };
    // 指向不存在的权重：P3.0 记录此时 Marian 直接 abort()（SIGABRT）
    spec.model_path = env
        .model_dir
        .join("model.does-not-exist.bin")
        .display()
        .to_string();

    // 直接用 WorkerProcess 绕过宿主侧路径检查，让坏路径真的进 native
    let mut worker = translator_runtime::worker::WorkerProcess::spawn(
        WorkerConfig::new(WORKER)
            .with_args([
                "--bergamot".to_string(),
                "--native".to_string(),
                env.native.display().to_string(),
                "--dev-skip-recheck".to_string(),
            ])
            .with_load_timeout(Duration::from_secs(60)),
    )
    .unwrap();

    let err = worker.request(Request::Load { spec }).unwrap_err();
    println!("native 崩溃后宿主收到：{err}");
    assert_eq!(
        err.code(),
        TranslationErrorCode::RuntimeInitFailed,
        "加载期 native 崩溃应分类为初始化失败：{err}"
    );
    // 宿主还活着：还能继续问它问题
    assert!(pid_alive(std::process::id()));
    drop(worker);

    // 同一个 worker 二进制再起一次，用**正确**的模型仍能工作：坏模型不会毒化进程
    let transport = ProcessTransport::new(worker_config(&env), 1);
    transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .expect("崩溃之后仍应能重新加载");
    transport.translate(&job("Hello world!")).unwrap();
    transport.shutdown().unwrap();
}

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt06b_native被外部杀掉时worker退出且宿主不崩() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 2);
    let info = transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .unwrap();
    let native_pid = info.native_pid.unwrap();
    let worker_pid = transport.metrics().worker_pid.unwrap();

    #[cfg(unix)]
    {
        // SAFETY: 只读 pid + 常量信号；目标是**另一个**进程
        unsafe {
            libc::kill(native_pid as libc::pid_t, libc::SIGKILL);
        }
    }
    // 等 native 停下（被杀后可能短暂是僵尸，等 worker 收尸）
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_running(native_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!pid_running(native_pid), "native 应已被杀");

    let err = transport.translate(&job("Hello again")).unwrap_err();
    println!("native 被杀后：{err}");
    assert!(
        matches!(
            err.code(),
            TranslationErrorCode::BackendFailed | TranslationErrorCode::RuntimeInitFailed
        ),
        "{err}"
    );

    // worker 自己也退出（native 死了模型就没了），下一次请求才重启。
    // 注意：宿主还没 wait 它，所以它会短暂是僵尸——判"还活着"要看状态位。
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_running(worker_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!pid_running(worker_pid), "worker 应随 native 一起退出");
    assert!(pid_alive(std::process::id()), "宿主必须活着");

    // shutdown 会收尸
    transport.shutdown().unwrap();
    assert!(!pid_alive(worker_pid), "shutdown 之后不该留下僵尸");
}

// ---------------------------------------------------------------------------
// B-T07 / B-T08：坏模型 / 源文件被改
// ---------------------------------------------------------------------------

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt07_坏模型在native之前就被拒绝() {
    let env = require_env!();
    let transport = ProcessTransport::new(worker_config(&env), 1);

    // 缺文件
    let mut missing = manifest(&env.model_dir);
    missing
        .files
        .get_mut(&FileRole::Model)
        .unwrap()
        .path
        .push(".missing");
    let err = transport
        .load_from_manifest(&missing, env.snapshot_root.path(), 1, 1)
        .unwrap_err();
    assert_eq!(
        err.code(),
        TranslationErrorCode::ResourceNotInstalled,
        "{err}"
    );
    assert_eq!(transport.metrics().spawns, 0, "校验不过不该起进程");

    // 同长度改一个字节（copy 出来再改，不动原始权重）
    let dir = tempfile::tempdir().unwrap();
    let mut tampered = manifest(&env.model_dir);
    let model = tampered.files[&FileRole::Model].clone();
    let copy = dir.path().join("model.tampered.bin");
    std::fs::copy(&model.path, &copy).unwrap();
    let mut body = std::fs::read(&copy).unwrap();
    let mid = body.len() / 2;
    body[mid] ^= 0xff;
    std::fs::write(&copy, &body).unwrap();
    tampered.files.get_mut(&FileRole::Model).unwrap().path = copy;

    let err = transport
        .load_from_manifest(&tampered, env.snapshot_root.path(), 1, 1)
        .unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::IntegrityFailure, "{err}");
    assert_eq!(transport.metrics().spawns, 0);

    // 截断
    let mut truncated = manifest(&env.model_dir);
    let short = dir.path().join("model.truncated.bin");
    std::fs::write(&short, &body[..1024]).unwrap();
    truncated.files.get_mut(&FileRole::Model).unwrap().path = short;
    let err = transport
        .load_from_manifest(&truncated, env.snapshot_root.path(), 1, 1)
        .unwrap_err();
    assert!(
        matches!(
            err.code(),
            TranslationErrorCode::IntegrityFailure | TranslationErrorCode::ResourceNotInstalled
        ),
        "{err}"
    );
}

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt08_快照隔离与worker侧复核() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 1);
    transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .unwrap();

    // (a) 快照与源目录分离：源文件被改，已加载的 worker 不受影响
    let before = transport.translate(&job("Hello world!")).unwrap().text;
    let spec = transport.snapshot_spec().expect("应当能拿到快照 spec");
    let snapshot_model = transport.snapshot_model_path().unwrap();
    assert!(
        !snapshot_model.starts_with(&env.model_dir),
        "快照必须不在源目录里：{}",
        snapshot_model.display()
    );

    // (b) 改**快照**里的权重：已打开 fd 的 native 仍按原内容翻（证明"事后改文件"改不动已加载的模型）
    let mut body = std::fs::read(&snapshot_model).unwrap();
    let mid = body.len() / 2;
    body[mid] ^= 0xff;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&snapshot_model, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(&snapshot_model, &body).unwrap();

    let after = transport.translate(&job("Hello world!")).unwrap().text;
    assert_eq!(before, after, "已加载的模型不该被文件改动影响");

    // (c) 但 worker 侧的**打开前复核**必须拒绝这份被改过的快照
    let fresh = ProcessTransport::new(worker_config(&env), 1);
    let err = fresh.load(&spec).unwrap_err();
    println!("快照被改后 worker 侧复核：{err}");
    assert_eq!(
        err.code(),
        TranslationErrorCode::IntegrityFailure,
        "worker 侧复核必须拒绝被改过的快照：{err}"
    );
    fresh.shutdown().unwrap();
    transport.shutdown().unwrap();
}

// ---------------------------------------------------------------------------
// B-T09 / B-T10 / B-T11
// ---------------------------------------------------------------------------

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt09_unicode与长文本走帧协议不坏() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 1);
    transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .unwrap();

    for text in [
        "Hello world!",
        "API 响应时间 latency 必须低于 100 ms。",
        "Wait... really?! I can't believe it.",
        "First line.\nSecond line.", // 换行会被清洗成空格，不能把行协议劈开
        "长文本：Translation quality depends on the training data, the tokenizer, and the decoding parameters.",
    ] {
        let reply = transport
            .translate(&job(text))
            .unwrap_or_else(|e| panic!("{text:?} 翻译失败：{e}"));
        println!("{text:?} → {:?}", reply.text);
        assert!(!reply.text.is_empty(), "空译文：{text:?}");
        assert!(reply.text.is_char_boundary(reply.text.len()), "UTF-8 边界");
        assert!(!reply.text.contains('\u{0}'));
    }
    transport.shutdown().unwrap();
}

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt10_加载超时不留孤儿native() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    // 极短的加载超时：宿主会在 native 还在加载时杀掉 worker
    let transport = ProcessTransport::new(
        worker_config(&env).with_load_timeout(Duration::from_millis(150)),
        1,
    );
    let err = transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .unwrap_err();
    assert_eq!(err.code(), TranslationErrorCode::Timeout, "{err}");
    assert!(
        transport.metrics().worker_pid.is_none(),
        "超时后 worker 应被丢弃"
    );

    // worker 被 SIGKILL 时，native 通过 PDEATHSIG 跟着死 → 不留 400 MiB 的孤儿
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut leftovers = procs_matching("nmt-worker-");
    while !leftovers.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        leftovers = procs_matching("nmt-worker-");
    }
    assert!(leftovers.is_empty(), "残留 native 进程：{leftovers:?}");
    transport.shutdown().unwrap();
}

#[test]
#[ignore = "需要真实权重 + native 二进制（均不入库）；设好 NMT_BERGAMOT_BIN/NMT_MODEL_DIR 后加 --ignored 运行"]
fn bt11_三个进程的内存分开可测() {
    let env = require_env!();
    let manifest = manifest(&env.model_dir);
    let transport = ProcessTransport::new(worker_config(&env), 1);
    let info = transport
        .load_from_manifest(&manifest, env.snapshot_root.path(), 1, 1)
        .unwrap();
    transport.translate(&job("Hello world!")).unwrap();

    let metrics = transport.metrics();
    let native = info.native_pid.and_then(rss_bytes_of).unwrap_or(0);
    let worker = metrics.worker_rss_bytes.unwrap_or(0);
    let host = metrics.host_rss_bytes.unwrap_or(0);
    println!(
        "host {:.1} MiB / worker {:.1} MiB / native {:.1} MiB",
        host as f64 / 1048576.0,
        worker as f64 / 1048576.0,
        native as f64 / 1048576.0
    );
    assert!(host > 1024 * 1024);
    assert!(worker > 0);
    assert!(
        native > 200 * 1024 * 1024,
        "真实模型必须占几百 MiB（P3.0 记录 341–414 MiB），实际 {native}"
    );
    transport.shutdown().unwrap();
}
