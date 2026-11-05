//! P3.1-A 的可复现证据harness：跑 `cargo run -p translator-runtime --release --example p3_1a_evidence`。
//!
//! 打印三组数字：
//! 1. **进程隔离**：worker `abort()` 之后宿主还活着，且宿主能读到退出码；
//! 2. **IPC 开销**：200 次往返的宿主侧墙钟分位数（mock worker 不做推理，所以这是纯 IPC 成本）；
//! 3. **内存口径**：宿主 RSS / worker RSS / 两者相加——**相加不是物理增量**。
//!
//! 注意：这里的 worker 是 mock。真实 Bergamot 的 413.8 MiB（1 线程）来自 P3.0 实测，
//! 本 harness 的数字**不能**用来支撑预算。

use std::collections::BTreeMap;
use std::time::Instant;

use translator_runtime::model::{sha256_file, FileRole, FileSpec, ModelManifest, ModelSpec};
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::{TranslationJob, WorkerConfig};

fn fixture() -> (tempfile::TempDir, ModelSpec) {
    let dir = tempfile::tempdir().unwrap();
    let mut files = BTreeMap::new();
    for role in FileRole::ALL {
        let path = dir.path().join(format!("{role}.bin"));
        std::fs::write(&path, format!("evidence-{role}").as_bytes()).unwrap();
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
        package_id: "nmt.evidence.enzh".to_string(),
        version: "1.0.0".to_string(),
        files,
    };
    manifest.verify().unwrap();
    let spec = ModelSpec::from_manifest(&manifest, 1, 1);
    (dir, spec)
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn main() {
    let bin = std::env::var("NMT_WORKER_BIN").unwrap_or_else(|_| {
        // target/<profile>/examples/../nmt-worker
        let exe = std::env::current_exe().unwrap();
        exe.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("nmt-worker")
            .display()
            .to_string()
    });
    println!("worker 可执行文件：{bin}");

    let (_dir, spec) = fixture();
    let job = TranslationJob {
        text: "Hello world!".to_string(),
        target: "zh-Hans".to_string(),
    };

    // ---------- 1. 正常路径 + IPC 开销 ----------
    let transport =
        ProcessTransport::new(WorkerConfig::new(&bin).with_args(["--mock".to_string()]), 1);
    let cold = Instant::now();
    let info = transport.load(&spec).unwrap();
    let cold_load = cold.elapsed();
    let cold2 = Instant::now();
    let first = transport.translate(&job).unwrap();
    let cold_translate = cold2.elapsed();
    println!(
        "引擎 {} / revision {} / epoch {}",
        info.engine_id, info.model_revision, info.worker_epoch
    );
    println!(
        "首次加载 {:.1} ms，首句翻译 {:.1} ms，输出 {:?}",
        cold_load.as_secs_f64() * 1000.0,
        cold_translate.as_secs_f64() * 1000.0,
        first.text
    );

    let mut samples = Vec::new();
    for _ in 0..200 {
        let started = Instant::now();
        transport.translate(&job).unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "IPC 往返（宿主侧墙钟，n={}）：p50 {:.3} ms / p95 {:.3} ms / p99 {:.3} ms / max {:.3} ms",
        samples.len(),
        pct(&samples, 0.50),
        pct(&samples, 0.95),
        pct(&samples, 0.99),
        samples[samples.len() - 1]
    );

    let metrics = transport.metrics();
    let host = metrics.host_rss_bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0);
    let worker = metrics
        .worker_rss_bytes
        .map(|b| b as f64 / (1024.0 * 1024.0))
        .unwrap_or(f64::NAN);
    let combined = metrics.combined_rss_bytes().unwrap_or(0) as f64 / (1024.0 * 1024.0);
    println!(
        "内存：宿主 {host:.1} MiB + worker {worker:.1} MiB = 相加 {combined:.1} MiB（相加≠物理增量，共享页会重复计）"
    );
    println!(
        "进程：spawns={} respawns={} pid={:?}",
        metrics.spawns, metrics.respawns, metrics.worker_pid
    );
    transport.shutdown().unwrap();

    // ---------- 2. 崩溃隔离 ----------
    let transport = ProcessTransport::new(
        WorkerConfig::new(&bin)
            .with_args(["--mock".to_string(), "--crash-on-translate".to_string()]),
        1,
    );
    transport.load(&spec).unwrap();
    let pid = transport.health().unwrap().worker_pid;
    let err = transport.translate(&job).unwrap_err();
    println!("\n[崩溃隔离] worker pid={pid} 在翻译中 abort()");
    println!("宿主收到的错误：{} → 错误码 {:?}", err, err.code());
    if let translator_runtime::RuntimeError::WorkerExited { code, signal, .. } = &err {
        println!("宿主读到的退出码 {code:?} / 信号 {signal:?}（abort → SIGABRT 6）");
    }
    println!(
        "宿主存活：能继续读指标 pid={:?}",
        transport.metrics().worker_pid
    );

    // 故意不做 restart，验证"不静默重放"
    let err = transport.translate(&job).unwrap_err();
    println!("崩溃后的下一次请求：{} → {:?}", err, err.code());

    // ---------- 3. 受控重启 ----------
    let transport = ProcessTransport::new(
        WorkerConfig::new(&bin)
            .with_args(["--mock".to_string(), "--crash-on-translate".to_string()]),
        1,
    );
    let first = transport.load(&spec).unwrap();
    let pid1 = transport.health().unwrap().worker_pid;
    let _ = transport.translate(&job).unwrap_err();
    let second = transport.load(&spec).unwrap();
    let pid2 = transport.health().unwrap().worker_pid;
    println!(
        "\n[受控重启] epoch {} → {}，pid {pid1} → {pid2}，respawns={}",
        first.worker_epoch,
        second.worker_epoch,
        transport.metrics().respawns
    );

    // ---------- 4. shutdown 延迟 ----------
    let started = Instant::now();
    transport.shutdown().unwrap();
    println!(
        "[shutdown] {:.1} ms（含礼貌帧→SIGTERM→SIGKILL 的升级）",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
