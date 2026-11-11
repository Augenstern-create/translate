//! 传输层：把**阻塞**的 HTTP 调用桥接成 Future，并负责超时、重试、限流与取消。
//!
//! # 为什么不用 async HTTP 客户端
//!
//! SDK 至今**没有** async runtime 依赖（`BoxFuture` 由宿主/`pollster` 驱动）。
//! 引入 tokio 只为了发一个 HTTP 请求，会把运行时选择强加给宿主。
//! 因此这里用同步客户端 `ureq`，在**独立 worker 线程**里执行，通过
//! `Waker` 通知 Future：执行器永远不阻塞，宿主也不被迫选运行时。
//!
//! # 不变量
//!
//! 1. **执行器不阻塞**：所有网络 I/O 都在 worker 线程里。
//! 2. **重试有界**：最多 `max_retries` 次额外尝试，且全部共享同一个截止时间。
//! 3. **取消即停止**：Future 立刻返回 `Cancelled`；worker 在两次重试之间检查令牌，
//!    不会继续烧钱。已在飞行中的那一次请求无法中断（HTTP 客户端拿不到中断句柄），
//!    这一点在 `docs/known-issues.md` 里明确记录。
//! 4. **不重试不可重试的错误**：鉴权/余额/配置错误原样重发只会重复失败。

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use translator_types::CancelToken;

use crate::config::HttpSettings;
use crate::errors::{HttpError, HttpErrorKind};
use crate::usage::UsageMeter;

/// 一次 HTTP 请求。
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub url: String,
    /// `None` = 不发送 `Authorization` 头（本机模型服务常见）。
    pub api_key: Option<String>,
    pub body: serde_json::Value,
    /// 单次尝试的总超时（由 worker 再与全局截止时间取小）。
    pub timeout: Duration,
}

/// 一次 HTTP 响应（状态码 + 文本体）。
#[derive(Clone, Debug)]
pub struct HttpOutcome {
    pub status: u16,
    pub body: String,
}

/// 并发闸门：限制同时在飞行中的请求数量。
///
/// 等待发生在 **worker 线程**里（阻塞是允许的），不会占用执行器。
#[derive(Debug)]
struct Limiter {
    limit: usize,
    active: Mutex<usize>,
    cv: Condvar,
}

impl Limiter {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            active: Mutex::new(0),
            cv: Condvar::new(),
        }
    }

    fn acquire(&self) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        while *active >= self.limit {
            active = self.cv.wait(active).unwrap_or_else(|e| e.into_inner());
        }
        *active += 1;
    }

    fn release(&self) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        *active = active.saturating_sub(1);
        self.cv.notify_one();
    }
}

/// 线程桥接槽：worker 写结果，Future 读结果。
struct Slot {
    result: Mutex<Option<Result<HttpOutcome, HttpError>>>,
    waker: Mutex<Option<Waker>>,
    finished: AtomicUsize,
    /// 是否已经启动过"取消观察线程"（每个请求最多一个）。
    watcher_started: AtomicUsize,
}

impl Slot {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(None),
            waker: Mutex::new(None),
            finished: AtomicUsize::new(0),
            watcher_started: AtomicUsize::new(0),
        })
    }

    fn complete(&self, value: Result<HttpOutcome, HttpError>) {
        {
            let mut slot = self.result.lock().unwrap_or_else(|e| e.into_inner());
            // 只保留第一个结果（取消之后到达的结果被丢弃）
            if slot.is_none() {
                *slot = Some(value);
            }
        }
        self.finished.store(1, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            waker.wake();
        }
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire) == 1
    }

    fn take(&self) -> Option<Result<HttpOutcome, HttpError>> {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// 等待 worker 结果的 Future。
struct Bridge {
    slot: Arc<Slot>,
    cancel: Option<CancelToken>,
}

impl Future for Bridge {
    type Output = Result<HttpOutcome, HttpError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // 取消优先：即使结果已经到了也不返回它（调用方已经不要了）
        if let Some(token) = &self.cancel {
            if token.is_cancelled() {
                return Poll::Ready(Err(HttpError::cancelled()));
            }
        }

        if let Some(result) = self.slot.take() {
            return Poll::Ready(result);
        }

        // 先登记 waker 再复查一次，避免 worker 在两者之间完成导致丢失唤醒。
        *self.slot.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
        if let Some(result) = self.slot.take() {
            return Poll::Ready(result);
        }

        // `CancelToken` 没有回调机制：它只是一块原子标志。为了让"取消"能真正
        // 唤醒执行器（而不是等后端自己返回），这里起一个极轻的观察线程，
        // 每 10ms 看一次标志位。每个带取消令牌的请求最多一个。
        if let Some(token) = self.cancel.clone() {
            if self
                .slot
                .watcher_started
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let slot = Arc::clone(&self.slot);
                std::thread::Builder::new()
                    .name("translate-cancel-watch".to_string())
                    .spawn(move || {
                        while !slot.is_finished() {
                            if token.is_cancelled() {
                                slot.complete(Err(HttpError::cancelled()));
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    })
                    .ok();
            }
        }

        Poll::Pending
    }
}

/// HTTP 传输器。
pub struct HttpTransport {
    agent: ureq::Agent,
    limiter: Arc<Limiter>,
    max_retries: u32,
    /// 响应体上限：防止对端返回超大 HTML 时把内存吃光。
    max_body_bytes: u64,
    meter: Arc<UsageMeter>,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("max_retries", &self.max_retries)
            .field("max_body_bytes", &self.max_body_bytes)
            .finish()
    }
}

impl HttpTransport {
    pub fn new(
        settings: &HttpSettings,
        max_retries: u32,
        max_concurrent: usize,
        connect_timeout: Duration,
        meter: Arc<UsageMeter>,
    ) -> Self {
        let config = ureq::Agent::config_builder()
            // 状态码不当作错误：4xx/5xx 的**响应体**里才有对端的错误说明，
            // 拿不到就只剩一个数字，无法区分"余额不足"和"密钥错误"。
            .http_status_as_error(false)
            .timeout_connect(Some(connect_timeout))
            .user_agent(settings.user_agent.clone())
            .build();
        Self {
            agent: config.into(),
            limiter: Arc::new(Limiter::new(max_concurrent)),
            max_retries,
            max_body_bytes: 1 << 20,
            meter,
        }
    }

    /// 发起请求（带重试）。Future 在 worker 线程完成后就绪。
    pub fn send<'a>(
        &'a self,
        request: HttpRequest,
        cancel: Option<CancelToken>,
        deadline: Option<Instant>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpOutcome, HttpError>> + Send + 'a>> {
        let slot = Slot::new();
        let bridge = Bridge {
            slot: Arc::clone(&slot),
            cancel: cancel.clone(),
        };

        let slot_for_thread = Arc::clone(&slot);
        let agent = self.agent.clone();
        let limiter = Arc::clone(&self.limiter);
        let meter = Arc::clone(&self.meter);
        let max_retries = self.max_retries;
        let max_body = self.max_body_bytes;

        let spawned = std::thread::Builder::new()
            .name("translate-http".to_string())
            .spawn(move || {
                let outcome = run_with_retries(
                    agent,
                    limiter,
                    meter,
                    request,
                    cancel,
                    deadline,
                    max_retries,
                    max_body,
                );
                slot_for_thread.complete(outcome);
            });

        if let Err(err) = spawned {
            // 线程起不来（极端资源耗尽）：直接给结构化错误，不要 panic。
            slot.complete(Err(HttpError::new(
                HttpErrorKind::Transport,
                format!("无法启动 HTTP worker 线程：{err}"),
            )));
        }

        Box::pin(bridge)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_with_retries(
    agent: ureq::Agent,
    limiter: Arc<Limiter>,
    meter: Arc<UsageMeter>,
    request: HttpRequest,
    cancel: Option<CancelToken>,
    deadline: Option<Instant>,
    max_retries: u32,
    max_body: u64,
) -> Result<HttpOutcome, HttpError> {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;

        if is_cancelled(&cancel) {
            meter.record_cancelled();
            return Err(HttpError::cancelled().with_attempts(attempt));
        }
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                return Err(HttpError::timeout("请求在发出前已超过截止时间").with_attempts(attempt));
            }
        }

        limiter.acquire();
        let result = execute_once(&agent, &request, deadline, max_body);
        limiter.release();

        match result {
            Ok(outcome) => {
                let retryable_status = matches!(outcome.status, 408 | 425 | 429 | 500..=599);
                if retryable_status && attempt <= max_retries && !is_cancelled(&cancel) {
                    meter.record_retry();
                    if !backoff(attempt, deadline, &cancel) {
                        // 退避期间被取消/超时：不再重试
                        if is_cancelled(&cancel) {
                            meter.record_cancelled();
                            return Err(HttpError::cancelled().with_attempts(attempt));
                        }
                        return Err(
                            HttpError::timeout("重试等待期间超过截止时间").with_attempts(attempt)
                        );
                    }
                    continue;
                }
                meter.record_request();
                return Ok(outcome);
            }
            Err(err) => {
                let retryable = err.kind.is_retryable();
                if retryable && attempt <= max_retries && !is_cancelled(&cancel) {
                    meter.record_retry();
                    if !backoff(attempt, deadline, &cancel) {
                        if is_cancelled(&cancel) {
                            meter.record_cancelled();
                            return Err(HttpError::cancelled().with_attempts(attempt));
                        }
                        return Err(
                            HttpError::timeout("重试等待期间超过截止时间").with_attempts(attempt)
                        );
                    }
                    continue;
                }
                meter.record_failure();
                return Err(err.with_attempts(attempt));
            }
        }
    }
}

fn is_cancelled(token: &Option<CancelToken>) -> bool {
    token.as_ref().is_some_and(CancelToken::is_cancelled)
}

/// 指数退避（100ms、200ms、400ms…上限 2s），且不超过截止时间。
/// 返回 `false` 表示等待期间被取消或已超过截止时间。
fn backoff(attempt: u32, deadline: Option<Instant>, cancel: &Option<CancelToken>) -> bool {
    let base = 100u64.saturating_mul(1u64 << attempt.min(4));
    let mut wait = Duration::from_millis(base.min(2_000));
    if let Some(deadline) = deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        if wait > remaining {
            wait = remaining;
        }
    }
    // 分片睡眠，保证取消能较快生效
    let step = Duration::from_millis(25);
    let mut slept = Duration::ZERO;
    while slept < wait {
        if is_cancelled(cancel) {
            return false;
        }
        let slice = step.min(wait - slept);
        std::thread::sleep(slice);
        slept += slice;
    }
    !is_cancelled(cancel)
}

fn execute_once(
    agent: &ureq::Agent,
    request: &HttpRequest,
    deadline: Option<Instant>,
    max_body: u64,
) -> Result<HttpOutcome, HttpError> {
    let mut timeout = request.timeout;
    if let Some(deadline) = deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HttpError::timeout("请求超过截止时间"));
        }
        if remaining < timeout {
            timeout = remaining;
        }
    }

    let body = serde_json::to_string(&request.body)
        .map_err(|e| HttpError::config(format!("请求体序列化失败：{e}")))?;

    let mut builder = agent
        .post(&request.url)
        .config()
        .timeout_global(Some(timeout))
        .build()
        .header("content-type", "application/json")
        .header("accept", "application/json");

    if let Some(key) = &request.api_key {
        builder = builder.header("authorization", format!("Bearer {key}"));
    }

    let mut response = builder.send(body.as_str()).map_err(classify_transport)?;

    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(max_body)
        .read_to_string()
        .map_err(classify_transport)?;

    Ok(HttpOutcome { status, body: text })
}

fn classify_transport(err: ureq::Error) -> HttpError {
    match err {
        ureq::Error::Timeout(_) => HttpError::timeout("后端请求超时"),
        ureq::Error::HostNotFound => HttpError::new(HttpErrorKind::Unavailable, "无法解析后端域名"),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            HttpError::timeout(format!("后端请求超时：{io}"))
        }
        ureq::Error::Io(io) => {
            HttpError::new(HttpErrorKind::Unavailable, format!("连接后端失败：{io}"))
        }
        ureq::Error::StatusCode(code) => HttpError::from_status(code, ""),
        other => HttpError::new(HttpErrorKind::Transport, format!("HTTP 传输错误：{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 延迟退避且在取消时立即停止() {
        let cancel = Some(CancelToken::new());
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            c.as_ref().unwrap().cancel();
        });
        let start = Instant::now();
        assert!(!backoff(3, None, &cancel));
        assert!(
            start.elapsed() < Duration::from_millis(800),
            "取消后应尽快返回"
        );
    }

    #[test]
    fn 截止时间已过时退避立即失败() {
        let deadline = Instant::now() - Duration::from_millis(1);
        assert!(!backoff(1, Some(deadline), &None));
    }

    #[test]
    fn 传输层异常分类() {
        let err = classify_transport(ureq::Error::HostNotFound);
        assert_eq!(err.kind, HttpErrorKind::Unavailable);
    }
}
