//! P3.1-B 证据 harness：**真实 Bergamot 进程 worker** 的端到端测量。
//!
//! 跑法（权重不入库，需要本机已有 P3.0 的模型目录）：
//!
//! ```bash
//! export NMT_BERGAMOT_BIN=/tmp/p3/bergamot-translator/build/app/bergamot
//! export NMT_MODEL_DIR=/tmp/p3/models
//! export LD_LIBRARY_PATH=/tmp/p3/sysroot/usr/lib/x86_64-linux-gnu:/tmp/p3/sysroot/usr/lib/x86_64-linux-gnu/openblas-pthread
//! cargo build -p translator-runtime --release --bins
//! cargo run -p translator-runtime --release --example p3_1b_evidence
//! ```
//!
//! 口径（`docs/14` §6）：`model_preparation_ms` / `worker_spawn_ms` / `native_load_ms` /
//! `first_translation_ms` / 热 p50-p95-p99 / host_rss / worker_rss / native_rss / VmHWM /
//! 卸载前后。**相加 RSS ≠ 物理增量**，这一条在每个输出里都重复一次。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use translator_runtime::model::{
    rss_bytes_of, sha256_file, vmhwm_bytes_of, FileRole, FileSpec, ModelManifest,
};
use translator_runtime::transport::{ProcessTransport, RuntimeTransport};
use translator_runtime::{TranslationJob, WorkerConfig};

fn model_dir() -> PathBuf {
    std::env::var_os("NMT_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/p3/models"))
}

/// 与 `fixtures/nmt/models.lock.json` 一致的四文件 + 可选 ssplit 前缀。
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

fn pct(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn mib(bytes: Option<u64>) -> f64 {
    bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0)
}

fn main() {
    // 最后可选地写一份 JSON（见文末 NMT_BASELINE_OUT）
    let mut out = serde_json::Map::new();

    let bin = std::env::var("NMT_BERGAMOT_BIN")
        .unwrap_or_else(|_| "/tmp/p3/bergamot-translator/build/app/bergamot".to_string());
    let dir = model_dir();
    let snapshot_root = std::env::temp_dir().join("p3_1b_snapshots");
    std::fs::create_dir_all(&snapshot_root).unwrap();

    let manifest = manifest(&dir);
    manifest.verify().unwrap();
    println!(
        "模型：{} 个文件 / {:.1} MiB（{}）",
        manifest.files.len(),
        manifest.files.values().map(|f| f.bytes).sum::<u64>() as f64 / (1024.0 * 1024.0),
        manifest.content_digest()
    );
    println!("native：{bin}");

    let transport = ProcessTransport::new(
        WorkerConfig::new(native_worker())
            .with_args([
                "--bergamot".to_string(),
                "--native".to_string(),
                bin.clone(),
                "--runtime-build".to_string(),
                std::env::var("NMT_BERGAMOT_BUILD")
                    .unwrap_or_else(|_| "9271618ebbdc5d21ac4dc4df9e72beb7ce644774".to_string()),
            ])
            .with_load_timeout(std::time::Duration::from_secs(60))
            .with_translate_timeout(std::time::Duration::from_secs(30)),
        0,
    );

    // ---- 加载（含快照复制 + 校验）----
    let started = Instant::now();
    let info = transport
        .load_from_manifest(&manifest, &snapshot_root, 1, 1)
        .unwrap();
    let load_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    let metrics = transport.metrics();
    println!(
        "\n[加载] 总墙钟 {load_wall_ms:.1} ms = 快照 {:.1} ms + worker/native 其余\n  engine={} runtime={:?} build={:?}",
        metrics.model_preparation_ms.unwrap_or(0) as f64,
        info.engine_id,
        info.runtime,
        info.runtime_build.as_deref().map(|s| &s[..s.len().min(12)])
    );
    println!(
        "  revision={} epoch={} snapshot={:.1} MiB native_pid={:?}",
        info.model_revision,
        info.worker_epoch,
        mib(metrics.snapshot_bytes),
        info.native_pid
    );
    // worker 的 stderr 里有 native 自己报的 load_ms（比推算更可信）
    for line in transport.worker_stderr() {
        if line.contains("load_ms") || line.contains("native pid") {
            println!("  worker: {line}");
        }
    }

    let job = |text: &str| TranslationJob {
        text: text.to_string(),
        target: "zh-Hans".to_string(),
    };

    // ---- 冷首句 ----
    let started = Instant::now();
    let first = transport.translate(&job("Hello world!")).unwrap();
    let first_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "\n[首句] {:.1} ms → {:?}（worker 侧 {} ms）",
        first_ms, first.text, first.worker_elapsed_ms
    );

    // ---- 热翻译 ----
    let samples_text = ["Hello world!", "This is a test of the translation system."];
    let mut samples = Vec::new();
    for i in 0..100 {
        let text = samples_text[i % 2];
        let started = Instant::now();
        let reply = transport.translate(&job(text)).unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        if i < 2 {
            println!("  示例：{text:?} → {:?}", reply.text);
        }
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let warm = samples.clone();
    println!(
        "\n[热翻译] n={} p50 {:.2} ms / p95 {:.2} ms / p99 {:.2} ms / max {:.2} ms",
        samples.len(),
        pct(&samples, 0.50),
        pct(&samples, 0.95),
        pct(&samples, 0.99),
        samples[samples.len() - 1]
    );

    // ---- 内存（三进程分开）----
    let metrics = transport.metrics();
    let worker_pid = metrics.worker_pid;
    let native_pid = info.native_pid;
    println!(
        "\n[内存] host {:.1} MiB / worker(Rust 监督者) {:.1} MiB / native {:.1} MiB",
        mib(metrics.host_rss_bytes),
        mib(metrics.worker_rss_bytes),
        mib(native_pid.and_then(rss_bytes_of))
    );
    println!(
        "  峰值 VmHWM：worker {:.1} MiB / native {:.1} MiB",
        mib(worker_pid.and_then(vmhwm_bytes_of)),
        mib(native_pid.and_then(vmhwm_bytes_of))
    );
    println!(
        "  三者 RSS 相加 {:.1} MiB（相加≠物理增量：共享页被重复计入）",
        mib(metrics.host_rss_bytes)
            + mib(metrics.worker_rss_bytes)
            + mib(native_pid.and_then(rss_bytes_of))
    );
    // 立刻快照这些数：native 卸载后 pid 就没了，事后取只会得到 0
    let mem = (
        metrics.host_rss_bytes.unwrap_or(0),
        metrics.worker_rss_bytes.unwrap_or(0),
        native_pid.and_then(rss_bytes_of).unwrap_or(0),
        native_pid.and_then(vmhwm_bytes_of).unwrap_or(0),
        worker_pid.and_then(vmhwm_bytes_of).unwrap_or(0),
    );

    // ---- 卸载：native 进程结束 ----
    let started = Instant::now();
    transport.unload().unwrap();
    println!(
        "\n[卸载] {:.1} ms；native 存活={}；worker 存活={}；快照已删={}",
        started.elapsed().as_secs_f64() * 1000.0,
        native_pid
            .map(|p| Path::new(&format!("/proc/{p}")).exists())
            .unwrap_or(false),
        transport.metrics().worker_pid.is_some(),
        !transport.has_snapshot()
    );
    println!(
        "  卸载后 worker RSS {:.1} MiB（宿主 {:.1} MiB）",
        mib(transport.metrics().worker_rss_bytes),
        mib(transport.metrics().host_rss_bytes)
    );

    // ---- 重新加载（第二次冷启动）----
    let started = Instant::now();
    let info2 = transport
        .load_from_manifest(&manifest, &snapshot_root, 1, 1)
        .unwrap();
    println!(
        "[重载] {:.1} ms（快照 {:.1} ms），epoch={} native_pid={:?}",
        started.elapsed().as_secs_f64() * 1000.0,
        transport.metrics().model_preparation_ms.unwrap_or(0) as f64,
        info2.worker_epoch,
        info2.native_pid
    );
    let reply = transport.translate(&job("Hello world!")).unwrap();
    println!("  重载后译文 {:?}", reply.text);

    transport.shutdown().unwrap();
    println!(
        "\n[shutdown] 完成；pid={:?}",
        transport.metrics().worker_pid
    );

    // ---- 可选：写基线 JSON（P3.0 的 fixtures 里也有同类文件）----
    if let Some(path) = std::env::var_os("NMT_BASELINE_OUT") {
        out.insert("schema".into(), "p3.1b-baseline/v1".into());
        out.insert("generated_at".into(), "2026-11-05".into());
        out.insert("native".into(), serde_json::Value::String(bin.clone()));
        out.insert(
            "model_files".into(),
            serde_json::Value::from(manifest.files.len() as u64),
        );
        out.insert(
            "model_bytes".into(),
            serde_json::Value::from(manifest.files.values().map(|f| f.bytes).sum::<u64>()),
        );
        out.insert(
            "model_revision".into(),
            serde_json::Value::String(manifest.content_digest()),
        );
        out.insert("load_wall_ms".into(), serde_json::Value::from(load_wall_ms));
        out.insert(
            "model_preparation_ms".into(),
            serde_json::Value::from(metrics.model_preparation_ms.unwrap_or(0)),
        );
        out.insert(
            "snapshot_bytes".into(),
            serde_json::Value::from(metrics.snapshot_bytes.unwrap_or(0)),
        );
        out.insert(
            "first_translation_ms".into(),
            serde_json::Value::from(first_ms),
        );
        out.insert("warm_n".into(), serde_json::Value::from(warm.len() as u64));
        out.insert(
            "warm_p50_ms".into(),
            serde_json::Value::from(pct(&warm, 0.50)),
        );
        out.insert(
            "warm_p95_ms".into(),
            serde_json::Value::from(pct(&warm, 0.95)),
        );
        out.insert(
            "warm_p99_ms".into(),
            serde_json::Value::from(pct(&warm, 0.99)),
        );
        out.insert(
            "warm_max_ms".into(),
            serde_json::Value::from(*warm.last().unwrap_or(&0.0)),
        );
        out.insert("host_rss_bytes".into(), serde_json::Value::from(mem.0));
        out.insert("worker_rss_bytes".into(), serde_json::Value::from(mem.1));
        out.insert("native_rss_bytes".into(), serde_json::Value::from(mem.2));
        out.insert("native_vmhwm_bytes".into(), serde_json::Value::from(mem.3));
        out.insert("worker_vmhwm_bytes".into(), serde_json::Value::from(mem.4));
        out.insert("threads".into(), serde_json::Value::from(1));
        out.insert("beam_size".into(), serde_json::Value::from(1));
        out.insert(
            "note".into(),
            serde_json::Value::String(
                "host/worker/native 的 RSS 相加不是物理内存增量；峰值看 vmhwm".to_string(),
            ),
        );
        std::fs::write(
            std::path::PathBuf::from(path),
            serde_json::to_string_pretty(&serde_json::Value::Object(out)).unwrap(),
        )
        .unwrap();
        println!("基线已写入 NMT_BASELINE_OUT");
    }
}

/// `target/<profile>/nmt-worker`
fn native_worker() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().join("nmt-worker")
}
