//! HTTP 传输层门面：把一次网络请求交给有界执行池，并桥接成 Future。
//!
//! # 分层
//!
//! ```text
//! HttpRuntimeImpl（每个 Translator 一个）
//!    ├── Pool      固定 worker + 有界队列 + 单一取消观察线程
//!    └── global    Limiter（所有后端共享的总并发上限）
//!
//! HttpTransport（每个后端一个）
//!    ├── agent            连接池 / TLS / 超时
//!    ├── backend limiter  该后端自己的并发上限
//!    └── meter            用量统计
//! ```
//!
//! 同步客户端（`ureq`）在线程里执行，宿主不需要 async runtime；
//! 执行器不会因为网络 I/O 而阻塞。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use translator_types::{CancelToken, RequestBudget};

use crate::config::HttpSettings;
use crate::errors::{HttpError, HttpErrorKind};
use crate::limiter::Limiter;
use crate::pool::{Bridge, Execute, Job, Pool, Slot};
use crate::usage::UsageMeter;

/// 一次 HTTP 请求。
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub url: String,
    /// `None` = 不发送 `Authorization` 头（本机模型服务常见）。
    pub api_key: Option<String>,
    pub body: serde_json::Value,
    /// 单次尝试的超时上限（还会与整体截止时间取小）。
    pub timeout: Duration,
}

/// 一次 HTTP 响应（状态码 + 文本体 + 重试提示）。
#[derive(Clone, Debug)]
pub struct HttpOutcome {
    pub status: u16,
    pub body: String,
    /// `Retry-After`（秒）。只在 429/503 等场景出现。
    pub retry_after: Option<u64>,
}

/// 每个 `Translator` 共享的运行时：全局并发 + 有界队列。
pub struct HttpRuntime {
    pool: Arc<Pool>,
    global: Arc<Limiter>,
}

impl std::fmt::Debug for HttpRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRuntime")
            .field("workers", &self.workers())
            .field("queue_capacity", &self.queue_capacity())
            .finish()
    }
}

impl HttpRuntime {
    pub fn new(workers: usize, queue_capacity: usize, meter: Arc<UsageMeter>) -> Arc<Self> {
        let workers = workers.max(1);
        let global = Limiter::new(workers);
        let pool = Pool::new(workers, queue_capacity, meter);
        Arc::new(Self { pool, global })
    }

    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    pub fn global(&self) -> &Arc<Limiter> {
        &self.global
    }

    pub fn workers(&self) -> usize {
        self.pool.workers()
    }

    pub fn queue_capacity(&self) -> usize {
        self.pool.capacity()
    }
}

/// 单个后端的传输器。
pub struct HttpTransport {
    runtime: Arc<HttpRuntime>,
    exec: Arc<Execute>,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("backend", &self.exec.backend_id)
            .field("max_retries", &self.exec.max_retries)
            .field("workers", &self.runtime.workers())
            .field("queue", &self.runtime.queue_capacity())
            .finish()
    }
}

impl HttpTransport {
    /// 独立传输器（测试/单后端宿主）：内部自建运行时。
    pub fn new(
        settings: &HttpSettings,
        max_retries: u32,
        max_concurrent: usize,
        connect_timeout: Duration,
        meter: Arc<UsageMeter>,
    ) -> Self {
        let runtime = HttpRuntime::new(
            settings.max_concurrent_requests.max(1),
            settings.max_queue_size,
            Arc::clone(&meter),
        );
        Self::with_runtime(
            runtime,
            settings,
            "http.default",
            max_retries,
            max_concurrent,
            connect_timeout,
            meter,
        )
    }

    /// 共享运行时（同一 `Translator` 的所有后端共用全局并发与队列）。
    pub fn with_runtime(
        runtime: Arc<HttpRuntime>,
        settings: &HttpSettings,
        backend_id: &str,
        max_retries: u32,
        max_concurrent: usize,
        connect_timeout: Duration,
        meter: Arc<UsageMeter>,
    ) -> Self {
        let exec = Arc::new(Execute {
            agent: crate::pool::build_agent(settings, connect_timeout),
            backend_id: backend_id.to_string(),
            backend: Limiter::new(max_concurrent.max(1)),
            global: Arc::clone(runtime.global()),
            max_retries,
            max_body: 1 << 20,
            default_timeout: Duration::from_secs(30),
            meter,
        });
        Self { runtime, exec }
    }

    pub fn backend_id(&self) -> &str {
        &self.exec.backend_id
    }

    pub fn workers(&self) -> usize {
        self.runtime.workers()
    }

    pub fn queue_capacity(&self) -> usize {
        self.runtime.queue_capacity()
    }

    /// 发起请求。队列满时立即返回 `QueueFull`。
    pub fn send(
        &self,
        request: HttpRequest,
        cancel: Option<CancelToken>,
        deadline: Option<Instant>,
        budget: Option<Arc<RequestBudget>>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpOutcome, HttpError>> + Send + '_>> {
        // 提交前先做最便宜的检查，避免把注定失败的请求塞进队列
        if cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            return Box::pin(async { Err(HttpError::cancelled()) });
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Box::pin(async {
                Err(HttpError::timeout("请求在提交前已超过截止时间"))
            });
        }

        let slot = Slot::new();
        let job = Job {
            exec: Arc::clone(&self.exec),
            request,
            budget,
            cancel: cancel.clone(),
            deadline,
            slot: Arc::clone(&slot),
        };
        if let Err(err) = self.runtime.pool().submit(job) {
            return Box::pin(async move { Err(err) });
        }

        Box::pin(Bridge::new(slot, cancel))
    }
}

/// 执行一次 HTTP 往返（同步，只在 worker 线程里调用）。
pub fn execute_once(
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
        timeout = timeout.min(remaining);
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
    // 先读 Retry-After：读 body 之后响应对象会被消费
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let text = response
        .body_mut()
        .with_config()
        .limit(max_body)
        .read_to_string()
        .map_err(classify_transport)?;
    Ok(HttpOutcome {
        status,
        body: text,
        retry_after,
    })
}

/// 解析 `Retry-After`：支持秒数；HTTP 日期格式解析成本高且少见，直接忽略。
fn parse_retry_after(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .map(|seconds| seconds.min(120))
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
    fn 传输层异常分类() {
        let err = classify_transport(ureq::Error::HostNotFound);
        assert_eq!(err.kind, HttpErrorKind::Unavailable);
    }

    #[test]
    fn 预取消请求不进入队列() {
        let meter = UsageMeter::new();
        let transport = HttpTransport::new(
            &HttpSettings::default(),
            0,
            1,
            Duration::from_millis(200),
            meter,
        );
        let token = CancelToken::new();
        token.cancel();
        let err = pollster::block_on(transport.send(
            HttpRequest {
                url: "http://127.0.0.1:9/v1".to_string(),
                api_key: None,
                body: serde_json::json!({}),
                timeout: Duration::from_millis(100),
            },
            Some(token),
            None,
            None,
        ))
        .unwrap_err();
        assert_eq!(err.kind, HttpErrorKind::Cancelled);
    }
}
