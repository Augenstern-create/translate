//! S2：请求预算、全局并发、有界队列、取消/超时与错误分类。
//!
//! 这些用例全部走真实 TCP + 真实 HTTP 报文与真实 worker 池：
//! 只测纯函数无法证明"预算耗尽后不再发请求""全局并发真的共享"这类行为。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use translator_http::client::HttpRuntime;
use translator_http::errors::HttpErrorKind;
use translator_http::usage::UsageMeter;
use translator_http::{BackendsFile, HttpSettings, HttpTranslationEngine};
use translator_plugin_api::{TranslationEngine, TranslationJob};
use translator_types::{
    CancelToken, LanguageTag, RequestBudget, TextKind, TranslationErrorCode, TranslationMode,
};

#[path = "common/mock_server.rs"]
mod mock_server;

use mock_server::{MockResponse, MockServer};

// ============================================================
// 基础设施
// ============================================================

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}
fn zh() -> LanguageTag {
    LanguageTag::new("zh-CN").unwrap()
}

struct JobSpec<'a> {
    text: &'a str,
    budget: Option<Arc<RequestBudget>>,
    cancel: Option<&'a CancelToken>,
    deadline: Option<Instant>,
    /// 语言标签由 spec 自己持有，`build()` 再借用，避免悬垂引用。
    source: LanguageTag,
    target: LanguageTag,
}

impl<'a> JobSpec<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            budget: None,
            cancel: None,
            deadline: None,
            source: en(),
            target: zh(),
        }
    }

    fn budget(mut self, budget: Arc<RequestBudget>) -> Self {
        self.budget = Some(budget);
        self
    }

    fn cancel(mut self, token: &'a CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    fn deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    fn build(&self) -> TranslationJob<'_> {
        TranslationJob {
            text: self.text,
            original_text: self.text,
            source: &self.source,
            target: &self.target,
            mode: TranslationMode::Precise,
            text_kind: TextKind::Sentence,
            glossary: &[],
            placeholders: &[],
            retry_hint: None,
            deadline: self.deadline,
            cancel: self.cancel,
            max_output_tokens: None,
            budget: self.budget.clone(),
        }
    }
}

fn engine_from(toml: &str) -> HttpTranslationEngine {
    let file = BackendsFile::from_toml_str(toml).expect("配置应可解析");
    HttpTranslationEngine::from_file(&file)
        .expect("引擎应可构造")
        .into_iter()
        .next()
        .expect("应有一个后端")
}

fn simple_config(base_url: &str, max_retries: u32, timeout_ms: u64) -> String {
    format!(
        r#"
[backend.test]
base_url = "{base_url}"
model = "m"
model_revision = "r1"
api_key_env = ""
max_retries = {max_retries}
max_concurrent_requests = 1
timeout_ms = {timeout_ms}
"#
    )
}

fn run(
    engine: &HttpTranslationEngine,
    spec: &JobSpec<'_>,
) -> Result<String, translator_types::TranslationError> {
    pollster::block_on(engine.translate_job(&spec.build())).map(|out| out.text)
}

/// 记录请求数的 mock 服务端。
struct Harness {
    server: MockServer,
    hits: Arc<AtomicUsize>,
}

fn harness<F>(handler: F) -> Harness
where
    F: Fn(usize) -> MockResponse + Send + Sync + 'static,
{
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let server = MockServer::start(move |_| {
        let index = counter.fetch_add(1, Ordering::SeqCst);
        handler(index)
    });
    Harness { server, hits }
}

impl Harness {
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn url(&self) -> String {
        self.server.base_url()
    }
}

// ============================================================
// 一、预算
// ============================================================

#[test]
fn 成功请求只消耗一次调用() {
    let h = harness(|_| MockResponse::completion("你好。"));
    let engine = engine_from(&simple_config(&h.url(), 2, 3000));
    let budget = RequestBudget::shared(4, None);

    let spec = JobSpec::new("Hello.").budget(Arc::clone(&budget));
    assert_eq!(run(&engine, &spec).unwrap(), "你好。");
    assert_eq!(budget.calls_started(), 1);
    assert_eq!(h.hits(), 1);
    assert_eq!(engine.usage().http_calls_started, 1);
    assert_eq!(engine.usage().http_calls_completed, 1);
}

#[test]
fn 传输重试也消耗预算() {
    // 前两次 503，第三次成功：max_retries=2 → 共 3 次真实调用
    let h = harness(|index| {
        if index < 2 {
            MockResponse::json(503, "busy")
        } else {
            MockResponse::completion("好了。")
        }
    });
    let engine = engine_from(&simple_config(&h.url(), 2, 5000));
    let budget = RequestBudget::shared(10, None);

    let spec = JobSpec::new("Hello.").budget(Arc::clone(&budget));
    assert_eq!(run(&engine, &spec).unwrap(), "好了。");
    assert_eq!(h.hits(), 3);
    assert_eq!(budget.calls_started(), 3);
    assert_eq!(engine.usage().transport_retries, 2);
}

#[test]
fn 预算耗尽后不再发请求() {
    // 一直 503，max_retries=5 但预算只有 2
    let h = harness(|_| MockResponse::json(503, "busy"));
    let engine = engine_from(&simple_config(&h.url(), 5, 5000));
    let budget = RequestBudget::shared(2, None);

    let err = run(&engine, &JobSpec::new("Hello.").budget(Arc::clone(&budget))).unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::BudgetExceeded,
        "{}",
        err.message
    );
    assert_eq!(h.hits(), 2, "预算耗尽后不得继续发请求");
    assert_eq!(budget.calls_started(), 2);
}

#[test]
fn 预算在同一次逻辑请求的多次调用间共享() {
    let h = harness(|_| MockResponse::completion("好。"));
    let engine = engine_from(&simple_config(&h.url(), 0, 3000));
    let budget = RequestBudget::shared(3, None);

    // 用同一个 budget 连续发三次：第三次刚好用完，第四次必须被拒
    for _ in 0..3 {
        let spec = JobSpec::new("Hello.").budget(Arc::clone(&budget));
        run(&engine, &spec).unwrap();
    }
    assert_eq!(h.hits(), 3);

    let spec = JobSpec::new("Hello.").budget(Arc::clone(&budget));
    let err = run(&engine, &spec).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::BudgetExceeded);
    assert_eq!(h.hits(), 3, "预算用尽后不得再发请求");
}

#[test]
fn 多线程共享预算不会超额() {
    let budget = RequestBudget::shared(10, None);
    let ok = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..32 {
        let budget = Arc::clone(&budget);
        let ok = Arc::clone(&ok);
        handles.push(std::thread::spawn(move || {
            if budget.try_start_call() {
                ok.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(ok.load(Ordering::SeqCst), 10, "只能成功 10 次");
    assert_eq!(budget.calls_started(), 10);
}

// ============================================================
// 二、并发（全局 + 后端）
// ============================================================

/// 记录峰值并发的 mock。
fn concurrency_harness(delay_ms: u64) -> (Harness, Arc<AtomicUsize>) {
    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (cur, hi) = (Arc::clone(&current), Arc::clone(&peak));
    let h = harness(move |_| {
        let now = cur.fetch_add(1, Ordering::SeqCst) + 1;
        hi.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(delay_ms));
        cur.fetch_sub(1, Ordering::SeqCst);
        MockResponse::completion("好。")
    });
    (h, peak)
}

#[test]
fn 全局并发在所有后端之间共享() {
    let (h, peak) = concurrency_harness(120);
    let settings = HttpSettings {
        max_concurrent_requests: 2,
        max_queue_size: 32,
        ..HttpSettings::default()
    };
    let runtime = HttpRuntime::new(2, 32, UsageMeter::new());

    let backend = |name: &str| {
        format!(
            r#"
[backend.{name}]
base_url = "{}"
model = "m"
api_key_env = ""
max_retries = 0
max_concurrent_requests = 2
timeout_ms = 8000
"#,
            h.url()
        )
    };
    let file_a = BackendsFile::from_toml_str(&backend("a")).unwrap();
    let file_b = BackendsFile::from_toml_str(&backend("b")).unwrap();
    let cfg_a = file_a.enabled_backends().unwrap()[0].1.clone();
    let cfg_b = file_b.enabled_backends().unwrap()[0].1.clone();
    let engine_a = Arc::new(
        HttpTranslationEngine::with_runtime(cfg_a, &settings, Arc::clone(&runtime)).unwrap(),
    );
    let engine_b = Arc::new(
        HttpTranslationEngine::with_runtime(cfg_b, &settings, Arc::clone(&runtime)).unwrap(),
    );

    let mut handles = Vec::new();
    for index in 0..8 {
        let engine = if index % 2 == 0 {
            Arc::clone(&engine_a)
        } else {
            Arc::clone(&engine_b)
        };
        handles.push(std::thread::spawn(move || {
            let spec = JobSpec::new("Hello.");
            pollster::block_on(engine.translate_job(&spec.build())).expect("应成功");
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }

    let observed = peak.load(Ordering::SeqCst);
    assert!(observed <= 2, "全局并发必须 ≤ 2，实际 {observed}");
    assert_eq!(runtime.global().active(), 0, "配额必须全部归还");
}

#[test]
fn 后端并发限制独立生效() {
    let (h, peak) = concurrency_harness(100);
    let settings = HttpSettings {
        max_concurrent_requests: 4,
        max_queue_size: 32,
        ..HttpSettings::default()
    };
    let runtime = HttpRuntime::new(4, 32, UsageMeter::new());
    let toml = format!(
        r#"
[backend.one]
base_url = "{}"
model = "m"
api_key_env = ""
max_retries = 0
max_concurrent_requests = 1
timeout_ms = 8000
"#,
        h.url()
    );
    let file = BackendsFile::from_toml_str(&toml).unwrap();
    let cfg = file.enabled_backends().unwrap()[0].1.clone();
    let engine = Arc::new(HttpTranslationEngine::with_runtime(cfg, &settings, runtime).unwrap());

    let mut handles = Vec::new();
    for _ in 0..4 {
        let engine = Arc::clone(&engine);
        handles.push(std::thread::spawn(move || {
            let spec = JobSpec::new("Hello.");
            pollster::block_on(engine.translate_job(&spec.build())).expect("应成功");
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), 1, "单后端并发必须被压到 1");
}

#[test]
fn 失败路径不会泄漏配额() {
    let h = harness(|_| MockResponse::json(500, "boom"));
    let settings = HttpSettings::default();
    let runtime = HttpRuntime::new(2, 8, UsageMeter::new());
    let file = BackendsFile::from_toml_str(&simple_config(&h.url(), 0, 3000)).unwrap();
    let cfg = file.enabled_backends().unwrap()[0].1.clone();
    let engine = HttpTranslationEngine::with_runtime(cfg, &settings, Arc::clone(&runtime)).unwrap();

    for _ in 0..5 {
        let _ = run(&engine, &JobSpec::new("Hello."));
    }
    assert_eq!(runtime.global().active(), 0, "失败后全局配额必须归还");
}

// ============================================================
// 三、有界队列
// ============================================================

#[test]
fn 队列满时立即返回_queuefull() {
    // 1 个 worker、队列容量 1、每次 400ms：第 4 个请求必然被拒绝
    let (h, _peak) = concurrency_harness(400);
    let settings = HttpSettings {
        max_concurrent_requests: 1,
        max_queue_size: 1,
        ..HttpSettings::default()
    };
    let runtime = HttpRuntime::new(1, 1, UsageMeter::new());
    let file = BackendsFile::from_toml_str(&simple_config(&h.url(), 0, 8000)).unwrap();
    let cfg = file.enabled_backends().unwrap()[0].1.clone();
    let engine = Arc::new(HttpTranslationEngine::with_runtime(cfg, &settings, runtime).unwrap());

    let started = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let engine = Arc::clone(&engine);
        handles.push(std::thread::spawn(move || {
            let spec = JobSpec::new("Hello.");
            pollster::block_on(engine.translate_job(&spec.build()))
                .map(|_| ())
                .map_err(|e| e.code)
        }));
    }
    let mut rejected = 0;
    for handle in handles {
        match handle.join().unwrap() {
            Ok(()) => {}
            Err(code) => {
                assert_eq!(code, TranslationErrorCode::QueueFull, "只允许队列满错误");
                rejected += 1;
            }
        }
    }
    assert!(rejected >= 1, "至少应有一个请求被队列拒绝");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "拒绝必须是立即的"
    );
}

#[test]
fn 排队中的请求取消后不会执行() {
    let (h, _peak) = concurrency_harness(350);
    let settings = HttpSettings {
        max_concurrent_requests: 1,
        max_queue_size: 8,
        ..HttpSettings::default()
    };
    let runtime = HttpRuntime::new(1, 8, UsageMeter::new());
    let file = BackendsFile::from_toml_str(&simple_config(&h.url(), 0, 8000)).unwrap();
    let cfg = file.enabled_backends().unwrap()[0].1.clone();
    let engine = Arc::new(HttpTranslationEngine::with_runtime(cfg, &settings, runtime).unwrap());

    // 先占住唯一的 worker
    let engine_first = Arc::clone(&engine);
    let first = std::thread::spawn(move || {
        let spec = JobSpec::new("Hello.");
        pollster::block_on(engine_first.translate_job(&spec.build())).map(|_| ())
    });
    std::thread::sleep(Duration::from_millis(60));

    // 第二个请求进入队列后被取消
    let token = CancelToken::new();
    let token_for_cancel = token.clone();
    let budget = RequestBudget::shared(4, None);
    let budget_for_second = Arc::clone(&budget);
    let engine_second = Arc::clone(&engine);
    let second = std::thread::spawn(move || {
        let spec = JobSpec::new("Hello.")
            .cancel(&token)
            .budget(budget_for_second);
        pollster::block_on(engine_second.translate_job(&spec.build()))
            .map(|_| ())
            .map_err(|e| e.code)
    });
    std::thread::sleep(Duration::from_millis(60));
    token_for_cancel.cancel();

    assert_eq!(
        second.join().unwrap().unwrap_err(),
        TranslationErrorCode::Cancelled
    );
    assert!(first.join().unwrap().is_ok());
    assert_eq!(h.hits(), 1, "被取消的排队请求不得发到服务端");
}

#[test]
fn 排队中的请求超时后不会执行() {
    let (h, _peak) = concurrency_harness(350);
    let settings = HttpSettings {
        max_concurrent_requests: 1,
        max_queue_size: 8,
        ..HttpSettings::default()
    };
    let runtime = HttpRuntime::new(1, 8, UsageMeter::new());
    let file = BackendsFile::from_toml_str(&simple_config(&h.url(), 0, 8000)).unwrap();
    let cfg = file.enabled_backends().unwrap()[0].1.clone();
    let engine = Arc::new(HttpTranslationEngine::with_runtime(cfg, &settings, runtime).unwrap());

    let engine_first = Arc::clone(&engine);
    let first = std::thread::spawn(move || {
        let spec = JobSpec::new("Hello.");
        pollster::block_on(engine_first.translate_job(&spec.build())).map(|_| ())
    });
    std::thread::sleep(Duration::from_millis(60));

    let budget = RequestBudget::shared(4, None);
    let budget_for_second = Arc::clone(&budget);
    let engine_second = Arc::clone(&engine);
    let second = std::thread::spawn(move || {
        let spec = JobSpec::new("Hello.")
            .deadline(Instant::now() + Duration::from_millis(80))
            .budget(budget_for_second);
        pollster::block_on(engine_second.translate_job(&spec.build()))
            .map(|_| ())
            .map_err(|e| e.code)
    });

    let code = second.join().unwrap().unwrap_err();
    assert!(
        matches!(
            code,
            TranslationErrorCode::Timeout | TranslationErrorCode::BackendTimeout
        ),
        "排队超时应返回超时，实际 {code:?}"
    );
    assert!(first.join().unwrap().is_ok());
    assert_eq!(h.hits(), 1, "超时的排队请求不得发到服务端");
    assert_eq!(
        budget.calls_started(),
        0,
        "排队超时的请求不得扣除物理调用预算"
    );
}

#[test]
fn worker_数量固定且队列容量可查询() {
    let runtime = HttpRuntime::new(3, 7, UsageMeter::new());
    assert_eq!(runtime.workers(), 3);
    assert_eq!(runtime.queue_capacity(), 7);
    assert_eq!(runtime.global().limit(), 3);
}

// ============================================================
// 四、错误分类与重试
// ============================================================

fn status_case(status: u16, expected: HttpErrorKind, expected_hits: usize, max_retries: u32) {
    let h = harness(move |_| MockResponse::json(status, "error"));
    let engine = engine_from(&simple_config(&h.url(), max_retries, 3000));
    let err = run(&engine, &JobSpec::new("Hello.")).unwrap_err();
    let expected_code = expected.code();
    assert_eq!(
        err.code, expected_code,
        "{status} 的错误码不符：{}",
        err.message
    );
    assert_eq!(h.hits(), expected_hits, "{status} 的实际请求次数不符");
}

#[test]
fn 鉴权失败不重试() {
    status_case(401, HttpErrorKind::Auth, 1, 3);
    status_case(403, HttpErrorKind::Auth, 1, 3);
}

#[test]
fn 服务端故障在预算内有限重试() {
    status_case(500, HttpErrorKind::Unavailable, 3, 2);
    status_case(502, HttpErrorKind::Unavailable, 3, 2);
    status_case(503, HttpErrorKind::Unavailable, 3, 2);
    // 504 是"网关超时"，按超时分类（仍会重试，但不重试到无上限）
    status_case(504, HttpErrorKind::Timeout, 3, 2);
}

#[test]
fn 请求超时状态码会重试并最终报超时() {
    status_case(408, HttpErrorKind::Timeout, 3, 2);
}

#[test]
fn 限流遵守_retry_after() {
    // 首次 429 带 Retry-After: 1，第二次成功
    let h = harness(|index| {
        if index == 0 {
            MockResponse::json(429, r#"{"error":"slow down"}"#).with_header("Retry-After", "1")
        } else {
            MockResponse::completion("好了。")
        }
    });
    let engine = engine_from(&simple_config(&h.url(), 2, 8000));

    let started = Instant::now();
    let text = run(&engine, &JobSpec::new("Hello.")).unwrap();
    let elapsed = started.elapsed();

    assert_eq!(text, "好了。");
    assert_eq!(h.hits(), 2);
    assert!(
        elapsed >= Duration::from_millis(900),
        "应遵守 Retry-After=1s，实际 {elapsed:?}"
    );
}

#[test]
fn 非法响应体报_invalidresponse() {
    let h = harness(|_| MockResponse::json(200, "<html>not json</html>"));
    let engine = engine_from(&simple_config(&h.url(), 2, 3000));
    let err = run(&engine, &JobSpec::new("Hello.")).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::InvalidBackendResponse);
    assert_eq!(h.hits(), 1, "响应非法不触发传输层重试");
}

#[test]
fn 长响应超时返回_backendtimeout() {
    let h = harness(|_| MockResponse::completion("太慢").with_delay(Duration::from_millis(700)));
    let engine = engine_from(&simple_config(&h.url(), 0, 150));
    let err = run(&engine, &JobSpec::new("Hello.")).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::BackendTimeout);
}

#[test]
fn 取消后不再重试也不切换() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let server = MockServer::start(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        MockResponse::json(503, "busy").with_delay(Duration::from_millis(120))
    });
    let engine = engine_from(&simple_config(&server.base_url(), 3, 5000));

    let token = CancelToken::new();
    let token_for_thread = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        token_for_thread.cancel();
    });

    let spec = JobSpec::new("Hello.").cancel(&token);
    let err = run(&engine, &spec).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::Cancelled);
    // 取消后不再发起新的重试
    let observed = hits.load(Ordering::SeqCst);
    assert!(observed <= 2, "取消后不应继续重试，实际 {observed} 次");
}

#[test]
fn 用量统计区分完成与未知计费() {
    // 第一次成功、第二次连接被重置（服务端直接关闭）→ 未知计费
    let ok_once = harness(|_| MockResponse::completion("好。"));
    let engine = engine_from(&simple_config(&ok_once.url(), 0, 3000));
    run(&engine, &JobSpec::new("Hello.")).unwrap();

    let usage = engine.usage();
    assert_eq!(usage.logical_requests, 1);
    assert_eq!(usage.http_calls_started, 1);
    assert_eq!(usage.http_calls_completed, 1);
    assert_eq!(usage.potentially_billable_unknown_calls, 0);
}

// ============================================================
// 五、结果槽与线程生命周期（审查修复回归）
// ============================================================

#[test]
fn 状态检查不得消费待返回的结果() {
    use translator_http::Slot;

    let slot = Slot::new();
    slot.complete(Ok(translator_http::HttpOutcome {
        status: 200,
        body: "ok".to_string(),
        retry_after: None,
    }));
    // 观察线程会反复调用 is_done()；它绝不能把结果取走
    for _ in 0..5 {
        assert!(slot.is_done(), "结果应已就绪");
    }
    let received = pollster::block_on(translator_http::pool::Bridge::new(Arc::clone(&slot), None));
    assert!(received.is_ok(), "结果被状态检查消费掉了");
}

#[test]
fn worker_完成与观察线程并发时响应不丢失() {
    use translator_http::Slot;

    let slot = Slot::new();
    let completer = {
        let slot = Arc::clone(&slot);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2));
            slot.complete(Ok(translator_http::HttpOutcome {
                status: 200,
                body: "ok".to_string(),
                retry_after: None,
            }));
        })
    };
    // 一边轮询 Future，一边让另一个线程检查 is_done（模拟观察线程）
    let probe = {
        let slot = Arc::clone(&slot);
        std::thread::spawn(move || {
            for _ in 0..50 {
                let _ = slot.is_done();
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let received = pollster::block_on(translator_http::pool::Bridge::new(slot, None));
    completer.join().unwrap();
    probe.join().unwrap();
    assert!(received.is_ok(), "并发检查把响应吃掉了");
}

#[test]
fn 直接调用引擎入口也有预算上限() {
    // 不走协调器、直接 translate()：仍必须受"1 + max_retries"约束
    let h = harness(|_| MockResponse::json(503, "busy"));
    let engine = engine_from(&simple_config(&h.url(), 1, 3000));
    let request = translator_types::TranslateRequest::new("Hello.", zh());

    let err = pollster::block_on(engine.translate(&request)).unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::BackendUnavailable,
        "{}",
        err.message
    );
    assert_eq!(h.hits(), 2, "直连入口必须被 max_retries 限住");
}
