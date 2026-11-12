//! 稳定性压力测试（默认 `#[ignore]`，只在本地/发布前手工跑）。
//!
//! 目的：验证 S2 的有界队列、全局并发、预算与取消在**持续负载**下不会死锁、
//! 不会无限排队、不会泄漏线程。所有请求都打到本地 Mock，不产生真实费用。
//!
//! 运行：
//! ```text
//! STRESS_REQUESTS=2000 cargo test --release -p translator-http --test stress -- --ignored --nocapture
//! ```

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use translator_http::{live_worker_threads, BackendsFile, HttpTranslationEngine};
use translator_plugin_api::TranslationEngine;
use translator_types::{
    CancelToken, LanguageTag, RequestBudget, TextKind, TranslationErrorCode, TranslationMode,
};

#[path = "common/mock_server.rs"]
mod mock_server;

use mock_server::{MockResponse, MockServer};

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1).map(str::to_string))
        })
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn threads_now() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("Threads:"))
                .and_then(|line| line.split_whitespace().nth(1).map(str::to_string))
        })
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

#[test]
#[ignore = "压力测试：本地手工执行，不进入普通 CI"]
fn mock_压力测试() {
    let total: usize = std::env::var("STRESS_REQUESTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);
    let callers: usize = std::env::var("STRESS_CALLERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);

    // 服务端：30ms 延迟 + 5% 随机 503，模拟真实抖动
    let hits = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&hits);
    let server = MockServer::start(move |_| {
        let n = counter.fetch_add(1, Ordering::Relaxed);
        if n % 20 == 19 {
            MockResponse::json(503, "busy")
        } else {
            MockResponse::completion("你好。").with_delay(Duration::from_millis(30))
        }
    });

    let toml = format!(
        r#"
[backend.mock]
base_url = "{}"
model = "m"
api_key_env = ""
max_retries = 2
max_concurrent_requests = 4
timeout_ms = 5000

[http]
max_concurrent_requests = 4
max_queue_size = 8
"#,
        server.base_url()
    );
    let file = BackendsFile::from_toml_str(&toml).expect("配置应可解析");
    let engine = Arc::new(
        HttpTranslationEngine::from_file(&file)
            .expect("引擎应可构造")
            .into_iter()
            .next()
            .expect("应有一个后端"),
    );

    let ok = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let queue_full = Arc::new(AtomicU64::new(0));
    let cancelled = Arc::new(AtomicU64::new(0));
    let timed_out = Arc::new(AtomicU64::new(0));
    let other = Arc::new(AtomicU64::new(0));

    let rss_before = rss_kb();
    let threads_before = threads_now();
    let peak_threads = Arc::new(AtomicUsize::new(threads_before));

    // 采样线程数（每 10ms 一次），直到测试结束
    let sampling = Arc::new(AtomicUsize::new(1));
    {
        let peak = Arc::clone(&peak_threads);
        let flag = Arc::clone(&sampling);
        std::thread::spawn(move || {
            while flag.load(Ordering::Relaxed) == 1 {
                peak.fetch_max(threads_now(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    }

    let started = Instant::now();
    let mut handles = Vec::new();
    for caller in 0..callers {
        let engine = Arc::clone(&engine);
        let (ok, failed, queue_full, cancelled, timed_out, other) = (
            Arc::clone(&ok),
            Arc::clone(&failed),
            Arc::clone(&queue_full),
            Arc::clone(&cancelled),
            Arc::clone(&timed_out),
            Arc::clone(&other),
        );
        handles.push(std::thread::spawn(move || {
            let quota = total / callers;
            for index in 0..quota {
                // 每 100 个请求里取消一个，覆盖取消路径
                let cancel = if index % 100 == 7 {
                    let token = CancelToken::new();
                    token.cancel();
                    Some(token)
                } else {
                    None
                };
                let budget = RequestBudget::shared(4, None);
                let job = translator_plugin_api::TranslationJob {
                    text: "Hello.",
                    original_text: "Hello.",
                    source: &LanguageTag::new("en").unwrap(),
                    target: &LanguageTag::new("zh-CN").unwrap(),
                    mode: TranslationMode::Precise,
                    text_kind: TextKind::Sentence,
                    glossary: &[],
                    placeholders: &[],
                    retry_hint: None,
                    deadline: None,
                    cancel: cancel.as_ref(),
                    max_output_tokens: None,
                    budget: Some(budget),
                };
                match pollster::block_on(engine.translate_job(&job)) {
                    Ok(_) => {
                        ok.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(err) => {
                        match err.code {
                            TranslationErrorCode::QueueFull => {
                                queue_full.fetch_add(1, Ordering::Relaxed);
                            }
                            TranslationErrorCode::Cancelled => {
                                cancelled.fetch_add(1, Ordering::Relaxed);
                            }
                            TranslationErrorCode::BackendTimeout
                            | TranslationErrorCode::Timeout => {
                                timed_out.fetch_add(1, Ordering::Relaxed);
                            }
                            _ => {
                                other.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let _ = caller;
            }
        }));
    }
    for handle in handles {
        handle.join().expect("调用线程不应 panic");
    }
    let elapsed = started.elapsed();
    sampling.store(0, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(50));
    let rss_after = rss_kb();

    let usage = engine.usage();
    let report = format!(
        "requests={total} ok={} failed={} cancelled={} queue_full={} timeout={} other={} http_calls={} tokens={}/{} elapsed={:.1}s avg={:.1}ms peak_threads={} worker_threads_now={} threads_before={} rss_before={}KB rss_after={}KB delta={}KB",
        ok.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        cancelled.load(Ordering::Relaxed),
        queue_full.load(Ordering::Relaxed),
        timed_out.load(Ordering::Relaxed),
        other.load(Ordering::Relaxed),
        usage.http_calls_started,
        usage.tokens.input_tokens,
        usage.tokens.output_tokens,
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / total.max(1) as f64,
        peak_threads.load(Ordering::Relaxed),
        live_worker_threads(),
        threads_before,
        rss_before,
        rss_after,
        rss_after as i64 - rss_before as i64,
    );
    println!("STRESS {report}");
    std::fs::write("/tmp/stress-report.txt", &report).ok();

    assert!(ok.load(Ordering::Relaxed) > 0, "至少要有一个成功");
    assert_eq!(
        live_worker_threads(),
        4,
        "worker 数量必须等于配置的全局并发"
    );
    assert!(
        hits.load(Ordering::Relaxed) >= ok.load(Ordering::Relaxed),
        "服务端命中数应不少于成功数"
    );
}
