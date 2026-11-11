//! 有界 HTTP 执行队列：固定 worker + 有界等待队列 + 单一取消观察线程。
//!
//! # 为什么不再"每个请求一个线程"
//!
//! 旧实现每个请求 spawn 一个 worker 线程，取消时再 spawn 一个观察线程。
//! 并发上来后线程数随"等待中的请求"增长，且没有任何上限。
//!
//! 现在：
//!
//! ```text
//! 请求 → 有界队列（满则立刻 QueueFull）
//!          ↓
//!      固定 worker（数量 = 全局并发上限）
//!          ↓
//!      全局 + 后端配额 → HTTP
//! ```
//!
//! 取消/超时由**一个**观察线程统一处理，线程总数恒定为 `workers + 1`。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use translator_types::{CancelToken, RequestBudget};

use crate::client::{execute_once, HttpOutcome, HttpRequest};
use crate::config::HttpSettings;
use crate::errors::{HttpError, HttpErrorKind};
use crate::limiter::{acquire_dual, AcquireError, Limiter};
use crate::usage::UsageMeter;

/// 每个 worker 一次执行所需的上下文（按后端区分）。
pub struct Execute {
    pub agent: ureq::Agent,
    pub backend_id: String,
    pub backend: Arc<Limiter>,
    pub global: Arc<Limiter>,
    pub max_retries: u32,
    pub max_body: u64,
    pub default_timeout: Duration,
    pub meter: Arc<UsageMeter>,
}

/// 一次 HTTP 执行任务。
pub struct Job {
    pub exec: Arc<Execute>,
    pub request: HttpRequest,
    pub budget: Option<Arc<RequestBudget>>,
    pub cancel: Option<CancelToken>,
    pub deadline: Option<Instant>,
    pub slot: Arc<Slot>,
}

/// 结果槽：worker 写入，Future 读取。
pub struct Slot {
    result: Mutex<Option<Result<HttpOutcome, HttpError>>>,
    waker: Mutex<Option<Waker>>,
    done: AtomicBool,
}

impl Slot {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(None),
            waker: Mutex::new(None),
            done: AtomicBool::new(false),
        })
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire) || self.take_result().is_some()
    }

    fn take_result(&self) -> Option<Result<HttpOutcome, HttpError>> {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn complete(&self, value: Result<HttpOutcome, HttpError>) {
        {
            let mut slot = self.result.lock().unwrap_or_else(|e| e.into_inner());
            if slot.is_none() {
                *slot = Some(value);
            }
        }
        self.done.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            waker.wake();
        }
    }
}

/// 等待 worker 结果的 Future。
pub struct Bridge {
    slot: Arc<Slot>,
    cancel: Option<CancelToken>,
}

impl Bridge {
    pub fn new(slot: Arc<Slot>, cancel: Option<CancelToken>) -> Self {
        Self { slot, cancel }
    }
}

impl std::future::Future for Bridge {
    type Output = Result<HttpOutcome, HttpError>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(token) = &self.cancel {
            if token.is_cancelled() {
                return Poll::Ready(Err(HttpError::cancelled()));
            }
        }
        if let Some(result) = self.slot.take_result() {
            return Poll::Ready(result);
        }
        *self.slot.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(cx.waker().clone());
        if let Some(result) = self.slot.take_result() {
            return Poll::Ready(result);
        }
        Poll::Pending
    }
}

/// 取消观察：一个线程服务所有在途请求。
struct Watcher {
    entries: Mutex<Vec<(Weak<Slot>, CancelToken)>>,
    wake: Condvar,
}

impl Watcher {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(Vec::new()),
            wake: Condvar::new(),
        })
    }

    fn register(&self, slot: &Arc<Slot>, cancel: &CancelToken) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((Arc::downgrade(slot), cancel.clone()));
        self.wake.notify_one();
    }

    fn run(self: Arc<Self>) {
        loop {
            let timeout = {
                let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
                if entries.is_empty() {
                    Duration::from_millis(200)
                } else {
                    Duration::from_millis(10)
                }
            };
            {
                let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
                let (guard, _) = self
                    .wake
                    .wait_timeout(entries, timeout)
                    .unwrap_or_else(|e| e.into_inner());
                drop(guard);
            }

            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            entries.retain(|(slot, token)| {
                let Some(slot) = slot.upgrade() else {
                    return false;
                };
                if slot.is_done() {
                    return false;
                }
                if token.is_cancelled() {
                    slot.complete(Err(HttpError::cancelled()));
                    return false;
                }
                true
            });
        }
    }
}

/// 有界执行池。同一 `Translator` 内所有后端共享一个。
pub struct Pool {
    tx: SyncSender<Job>,
    workers: usize,
    capacity: usize,
    #[allow(dead_code)]
    watcher: Arc<Watcher>,
    meter: Arc<UsageMeter>,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("workers", &self.workers)
            .field("capacity", &self.capacity)
            .finish()
    }
}

impl Pool {
    pub fn new(workers: usize, capacity: usize, meter: Arc<UsageMeter>) -> Arc<Self> {
        let workers = workers.max(1);
        let capacity = capacity.max(1);
        let (tx, rx) = sync_channel::<Job>(capacity);
        let rx = Arc::new(Mutex::new(rx));
        let watcher = Watcher::new();
        {
            let watcher = Arc::clone(&watcher);
            std::thread::Builder::new()
                .name("translate-cancel-watch".to_string())
                .spawn(move || watcher.run())
                .ok();
        }
        let pool = Arc::new(Self {
            tx,
            workers,
            capacity,
            watcher,
            meter,
        });
        for index in 0..workers {
            let rx = Arc::clone(&rx);
            std::thread::Builder::new()
                .name(format!("translate-http-{index}"))
                .spawn(move || worker_loop(rx))
                .expect("无法启动 HTTP worker 线程");
        }
        pool
    }

    pub fn workers(&self) -> usize {
        self.workers
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn meter(&self) -> &Arc<UsageMeter> {
        &self.meter
    }

    /// 提交任务。队列满时**立即**返回错误，不阻塞调用方。
    pub fn submit(&self, job: Job) -> Result<(), HttpError> {
        if let Some(cancel) = &job.cancel {
            self.watcher.register(&job.slot, cancel);
        }
        match self.tx.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(HttpError::new(
                HttpErrorKind::QueueFull,
                format!("HTTP 执行队列已满（容量 {}）", self.capacity),
            )),
            Err(TrySendError::Disconnected(_)) => Err(HttpError::new(
                HttpErrorKind::Transport,
                "HTTP 执行队列已关闭",
            )),
        }
    }
}

fn worker_loop(rx: Arc<Mutex<Receiver<Job>>>) {
    loop {
        let job = {
            // 只在取任务时持锁，执行期间不占用
            let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
            guard.recv()
        };
        match job {
            Ok(job) => run_job(job),
            Err(_) => return,
        }
    }
}

fn run_job(job: Job) {
    let Job {
        exec,
        request,
        budget,
        cancel,
        deadline,
        slot,
    } = job;

    let cancelled = || cancel.as_ref().is_some_and(CancelToken::is_cancelled);
    let expired = || deadline.is_some_and(|d| Instant::now() >= d);

    if cancelled() {
        exec.meter.record_cancelled();
        slot.complete(Err(HttpError::cancelled()));
        return;
    }
    if expired() {
        slot.complete(Err(HttpError::timeout("请求在排队期间超过截止时间")));
        return;
    }

    let mut attempt: u32 = 0;
    loop {
        attempt += 1;

        // 1) 取消 / 截止时间
        if cancelled() {
            exec.meter.record_cancelled();
            slot.complete(Err(HttpError::cancelled()));
            return;
        }
        if expired() {
            slot.complete(Err(HttpError::timeout("请求超过截止时间")));
            return;
        }

        // 2) 物理调用预算（只有真的要发请求时才扣）
        if let Some(budget) = &budget {
            if !budget.try_start_call() {
                exec.meter.record_failure();
                slot.complete(Err(HttpError::new(
                    HttpErrorKind::BudgetExceededLocal,
                    format!(
                        "已达到本次请求的 HTTP 调用上限（{} 次）",
                        budget.max_calls()
                    ),
                )));
                return;
            }
        }

        // 3) 并发配额：全局 + 后端，共享同一截止时间
        let permits = match acquire_dual(&exec.global, &exec.backend, deadline, cancel.as_ref()) {
            Ok(permits) => permits,
            Err(AcquireError::Cancelled) => {
                exec.meter.record_cancelled();
                slot.complete(Err(HttpError::cancelled()));
                return;
            }
            Err(AcquireError::Timeout) => {
                slot.complete(Err(HttpError::timeout("等待并发配额时超过截止时间")));
                return;
            }
        };

        // 4) 拿到配额后再确认一次：排队期间可能被取消/超时
        if cancelled() {
            drop(permits);
            exec.meter.record_cancelled();
            slot.complete(Err(HttpError::cancelled()));
            return;
        }
        if expired() {
            drop(permits);
            slot.complete(Err(HttpError::timeout("取得配额后已超过截止时间")));
            return;
        }

        exec.meter.record_call_started();
        let outcome = execute_once(&exec.agent, &request, deadline, exec.max_body);
        drop(permits);

        match outcome {
            Ok(outcome) => {
                exec.meter.record_call_completed();
                let status = outcome.status;
                let retryable = matches!(status, 408 | 425 | 429 | 500..=599);
                if retryable && attempt <= exec.max_retries && !cancelled() && !expired() {
                    // 429/503 若带 Retry-After 就遵守它（但仍受截止时间约束）
                    let wait = outcome
                        .retry_after
                        .map(Duration::from_secs)
                        .unwrap_or_else(|| backoff_delay(attempt));
                    if !sleep_within(wait, deadline, cancel.as_ref()) {
                        if cancelled() {
                            exec.meter.record_cancelled();
                            slot.complete(Err(HttpError::cancelled()));
                        } else {
                            slot.complete(Err(HttpError::timeout("重试等待超过截止时间")));
                        }
                        return;
                    }
                    exec.meter.record_transport_retry();
                    continue;
                }
                slot.complete(Ok(outcome));
                return;
            }
            Err(err) => {
                // 请求已经发出但没有响应：可能仍会产生费用
                exec.meter.record_unknown_billable();
                exec.meter.record_failure();
                let retryable = err.kind.is_retryable();
                if retryable && attempt <= exec.max_retries && !cancelled() && !expired() {
                    if !sleep_within(backoff_delay(attempt), deadline, cancel.as_ref()) {
                        if cancelled() {
                            exec.meter.record_cancelled();
                            slot.complete(Err(HttpError::cancelled()));
                        } else {
                            slot.complete(Err(HttpError::timeout("重试等待超过截止时间")));
                        }
                        return;
                    }
                    exec.meter.record_transport_retry();
                    continue;
                }
                slot.complete(Err(err.with_attempts(attempt)));
                return;
            }
        }
    }
}

fn backoff_delay(attempt: u32) -> Duration {
    let base = 100u64.saturating_mul(1u64 << attempt.min(4));
    Duration::from_millis(base.min(2_000))
}

/// 睡眠 `wait`，期间检查取消与截止时间；返回 `false` 表示被打断/超时。
fn sleep_within(wait: Duration, deadline: Option<Instant>, cancel: Option<&CancelToken>) -> bool {
    let step = Duration::from_millis(25);
    let mut slept = Duration::ZERO;
    let mut total = wait;
    if let Some(deadline) = deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        total = total.min(remaining);
    }
    while slept < total {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return false;
        }
        let slice = step.min(total - slept);
        std::thread::sleep(slice);
        slept += slice;
    }
    !cancel.is_some_and(CancelToken::is_cancelled)
}

/// 让引擎复用同一份 HTTP 配置（agent、超时、上限）。
pub fn build_agent(settings: &HttpSettings, connect_timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        // 状态码不当作错误：错误体里有对端说明，拿不到就无法区分
        // "余额不足"与"密钥错误"。
        .http_status_as_error(false)
        .timeout_connect(Some(connect_timeout))
        .user_agent(settings.user_agent.clone())
        .build();
    config.into()
}
