//! 极小的本地 HTTP 服务端，仅用于测试。
//!
//! 目的不是"再写一个 HTTP 库"，而是让**真实 TCP + 真实 HTTP 报文**进入测试：
//! 只测 `build_request_body` / `parse_response` 会漏掉超时、重试、状态码分类、
//! 取消等真正容易出错的部分。
//!
//! 仅在 `cfg(test)` 或 `feature = "mock-server"` 下编译。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// 收到的请求。
#[derive(Clone, Debug)]
pub struct MockRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl MockRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 解析请求体 JSON（失败返回 `None`，测试里当成断言失败更直观）。
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.body).ok()
    }
}

/// 要回给客户端的响应。
#[derive(Clone, Debug)]
pub struct MockResponse {
    pub status: u16,
    pub body: String,
    pub delay: Option<Duration>,
}

impl MockResponse {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            delay: None,
        }
    }

    /// 标准成功的 Chat Completions 响应。
    pub fn completion(text: &str) -> Self {
        Self::json(200, chat_completion_body(text))
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}

/// 构造合法的 Chat Completions 响应体。
pub fn chat_completion_body(text: &str) -> String {
    serde_json::json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "model": "configured-model-id-2026-11-01",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 32, "completion_tokens": 12, "total_tokens": 44}
    })
    .to_string()
}

/// 本地 mock 服务端。`Drop` 时自动停止。
pub struct MockServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    /// 启动服务端；`handler` 在**服务线程**里执行（可阻塞，用来模拟慢后端）。
    pub fn start<F>(handler: F) -> Self
    where
        F: Fn(MockRequest) -> MockResponse + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let addr = listener.local_addr().expect("读取本地地址失败");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handler = Arc::new(handler);

        let handle = std::thread::Builder::new()
            .name("mock-http".to_string())
            .spawn(move || {
                for incoming in listener.incoming() {
                    if stop_thread.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = incoming else { continue };
                    let handler = Arc::clone(&handler);
                    // 每个连接独立线程：允许"慢请求 + 并发请求"同时存在
                    std::thread::spawn(move || {
                        let _ = serve(stream, handler);
                    });
                }
            })
            .expect("启动 mock 服务线程失败");

        Self {
            addr,
            stop,
            handle: Some(handle),
        }
    }

    /// `http://127.0.0.1:<port>/v1`
    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 唤醒 accept()
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve<F>(stream: TcpStream, handler: Arc<F>) -> std::io::Result<()>
where
    F: Fn(MockRequest) -> MockResponse + Send + Sync + 'static,
{
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let request = MockRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    };

    let response = handler(request);
    if let Some(delay) = response.delay {
        std::thread::sleep(delay);
    }

    let mut stream = stream;
    let payload = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        reason(response.status),
        response.body.len(),
        response.body
    );
    stream.write_all(payload.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}
