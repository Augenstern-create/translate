//! S1-03 / S1-04：本地 Web 界面的接入控制与请求限制（真实 socket 测试）。
//!
//! 这些用例刻意走**真实 TCP + 真实 HTTP 报文**：
//! 只测纯函数无法证明"超大 Content-Length 会在读取前被拒绝"这类行为。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use translator_api::Translator;

// ============================================================
// 测试基础设施
// ============================================================

/// 极小的 OpenAI 兼容 mock 服务端：固定返回一句中文。
struct MockModel {
    port: u16,
}

impl MockModel {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 mock 端口失败");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(stream) = incoming else { continue };
                std::thread::spawn(move || {
                    let _ = serve_completion(stream);
                });
            }
        });
        Self { port }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

fn serve_completion(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    // 必须把请求**读完整**再响应：只读一次缓冲区时，Windows 上可能出现
    // "客户端还在写 body，服务端已经关连接"的竞态，表现为客户端写失败。
    read_full_request(&mut stream)?;
    let body = serde_json::json!({
        "id": "chatcmpl-test",
        "model": "mock-model-2026-11-11",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "你好。" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13 }
    })
    .to_string();
    let payload = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

/// 读到 `\r\n\r\n`，再按 Content-Length 读满请求体。
fn read_full_request(stream: &mut TcpStream) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader};
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = String::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        head.push_str(&line);
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
    }
    Ok(())
}

/// 启动待测的 UI 服务端，返回端口。
fn start_ui() -> u16 {
    let model = MockModel::start();
    let toml = format!(
        r#"
[backend.local]
base_url = "{}"
model = "mock-model"
model_revision = "test-1"
api_key_env = ""
max_retries = 0
timeout_ms = 5000
"#,
        model.base_url()
    );
    let translator = Translator::builder()
        .http_config_toml(&toml)
        .expect("配置应可解析")
        .build()
        .expect("应可构造 Translator");

    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 UI 端口失败");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let _ = translator_cli::serve::serve_listener(listener, Arc::new(translator));
    });
    // 等服务端进入 accept 循环
    std::thread::sleep(Duration::from_millis(80));
    port
}

/// 发送原始请求，返回完整响应文本。
fn raw_request(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接 UI 失败");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("设置超时失败");
    stream.write_all(request.as_bytes()).expect("写请求失败");
    stream.flush().ok();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

fn status_of(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

fn body_of(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or("")
}

/// 从首页 HTML 里取出 CSRF 令牌。
fn csrf_token(port: u16) -> String {
    let response = raw_request(
        port,
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    );
    let marker = "name=\"csrf-token\" content=\"";
    let start = response.find(marker).expect("首页应包含 CSRF 令牌") + marker.len();
    let rest = &response[start..];
    rest[..rest.find('"').expect("令牌应以引号结束")].to_string()
}

fn post(port: u16, path: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut head = format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    ));
    raw_request(port, &head)
}

// ============================================================
// 用例
// ============================================================

#[test]
fn 首页可访问且带随机令牌与安全头() {
    let port = start_ui();
    let response = raw_request(
        port,
        &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(status_of(&response), 200, "{response}");
    assert!(response.contains("X-Content-Type-Options: nosniff"));
    assert!(
        !body_of(&response).contains("innerHTML"),
        "页面不得使用 innerHTML"
    );

    let token = csrf_token(port);
    assert_eq!(
        token.len(),
        64,
        "令牌应为 256 bit（64 个十六进制字符）：{token}"
    );
}

#[test]
fn 合法请求可以完成翻译() {
    let port = start_ui();
    let token = csrf_token(port);
    let response = post(
        port,
        "/api/translate",
        &[
            ("Origin", &format!("http://127.0.0.1:{port}")),
            ("Content-Type", "application/json"),
            ("x-csrf-token", &token),
        ],
        r#"{"text":"Hello.","mode":"precise"}"#,
    );
    assert_eq!(status_of(&response), 200, "{response}");
    assert!(
        body_of(&response).contains("你好。"),
        "{}",
        body_of(&response)
    );
}

#[test]
fn 健康检查不需要令牌() {
    let port = start_ui();
    let response = raw_request(
        port,
        &format!("GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(status_of(&response), 200, "{response}");
    assert!(body_of(&response).contains("\"status\":\"ok\""));
}

#[test]
fn 缺少_csrf_令牌被拒绝() {
    let port = start_ui();
    let response = post(
        port,
        "/api/translate",
        &[("Content-Type", "application/json")],
        r#"{"text":"Hello."}"#,
    );
    assert_eq!(status_of(&response), 403, "{response}");
    assert!(body_of(&response).contains("CSRF"));
}

#[test]
fn 错误_csrf_令牌被拒绝() {
    let port = start_ui();
    let response = post(
        port,
        "/api/translate",
        &[
            ("Content-Type", "application/json"),
            ("x-csrf-token", "00000000000000000000000000000000"),
        ],
        r#"{"text":"Hello."}"#,
    );
    assert_eq!(status_of(&response), 403, "{response}");
}

#[test]
fn 跨站_origin_被拒绝() {
    let port = start_ui();
    let token = csrf_token(port);
    let response = post(
        port,
        "/api/translate",
        &[
            ("Origin", "http://evil.example"),
            ("Content-Type", "application/json"),
            ("x-csrf-token", &token),
        ],
        r#"{"text":"Hello."}"#,
    );
    assert_eq!(status_of(&response), 403, "{response}");
    assert!(body_of(&response).contains("Origin"));
}

#[test]
fn 非法_host_被拒绝() {
    let port = start_ui();
    // 即便令牌正确，Host 不是本机也要拒绝（防 DNS rebinding）
    let token = csrf_token(port);
    let request = format!(
        "POST /api/translate HTTP/1.1\r\nHost: evil.example:{port}\r\nContent-Type: application/json\r\nx-csrf-token: {token}\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{{\"text\":\"Hello.\"}}"
    );
    let response = raw_request(port, &request);
    assert_eq!(status_of(&response), 403, "{response}");
    assert!(body_of(&response).contains("Host"));
}

#[test]
fn 超大请求体在读取前被拒绝() {
    let port = start_ui();
    let token = csrf_token(port);
    // 只发头部，声明一个巨大的 Content-Length，不发请求体
    let request = format!(
        "POST /api/translate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nx-csrf-token: {token}\r\nContent-Length: 10485760\r\nConnection: close\r\n\r\n"
    );
    let started = std::time::Instant::now();
    let response = raw_request(port, &request);
    assert_eq!(status_of(&response), 413, "{response}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "超限请求应在读取前立即拒绝"
    );
}

#[test]
fn 非法_content_type_被拒绝() {
    let port = start_ui();
    let token = csrf_token(port);
    let response = post(
        port,
        "/api/translate",
        &[("Content-Type", "text/html"), ("x-csrf-token", &token)],
        r#"{"text":"Hello."}"#,
    );
    assert_eq!(status_of(&response), 415, "{response}");
}

#[test]
fn 非法_content_length_被拒绝() {
    let port = start_ui();
    let token = csrf_token(port);
    let request = format!(
        "POST /api/translate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nx-csrf-token: {token}\r\nContent-Length: abc\r\nConnection: close\r\n\r\n"
    );
    let response = raw_request(port, &request);
    assert_eq!(status_of(&response), 400, "{response}");
}

#[test]
fn chunked_传输被拒绝() {
    let port = start_ui();
    let token = csrf_token(port);
    let request = format!(
        "POST /api/translate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nx-csrf-token: {token}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    let response = raw_request(port, &request);
    assert_eq!(status_of(&response), 400, "{response}");
}

#[test]
fn 非法_json_被拒绝() {
    let port = start_ui();
    let token = csrf_token(port);
    let response = post(
        port,
        "/api/translate",
        &[
            ("Content-Type", "application/json"),
            ("x-csrf-token", &token),
        ],
        "{ not json }",
    );
    assert_eq!(status_of(&response), 400, "{response}");
    assert!(body_of(&response).contains("JSON"));
}

#[test]
fn 不支持的方法返回_405() {
    let port = start_ui();
    let response = raw_request(
        port,
        &format!("PUT /api/translate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(status_of(&response), 405, "{response}");
    assert!(response.contains("Allow: GET, HEAD, POST"));
}

#[test]
fn 超大请求头被拒绝() {
    let port = start_ui();
    let filler = "x".repeat(40 * 1024);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: {filler}\r\nConnection: close\r\n\r\n"
    );
    let response = raw_request(port, &request);
    assert_eq!(status_of(&response), 431, "{response}");
}

#[test]
fn 重复的安全请求头被拒绝() {
    let port = start_ui();
    let token = csrf_token(port);

    let cases = [
        (
            "Host",
            format!(
                "POST /api/translate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nx-csrf-token: {token}\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{{\"text\":\"Hello.\"}}"
            ),
        ),
        (
            "Origin",
            format!(
                "POST /api/translate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nx-csrf-token: {token}\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{{\"text\":\"Hello.\"}}"
            ),
        ),
        (
            "x-csrf-token",
            format!(
                "POST /api/translate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nx-csrf-token: {token}\r\nx-csrf-token: {token}\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{{\"text\":\"Hello.\"}}"
            ),
        ),
        (
            "Content-Type",
            format!(
                "POST /api/translate HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Type: application/json\r\nx-csrf-token: {token}\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{{\"text\":\"Hello.\"}}"
            ),
        ),
    ];

    for (name, request) in cases {
        let response = raw_request(port, &request);
        assert_eq!(
            status_of(&response),
            400,
            "重复 {name} 应返回 400：{response}"
        );
        assert!(
            body_of(&response).contains(name) || body_of(&response).contains("重复"),
            "错误信息应说明重复的头：{response}"
        );
    }
}

#[test]
fn 重启后令牌会变化() {
    let first = csrf_token(start_ui());
    let second = csrf_token(start_ui());
    assert_ne!(first, second, "每次启动都应生成新的 CSRF 令牌");
}

#[test]
fn csp_不影响合法翻译与查词请求() {
    let port = start_ui();
    let token = csrf_token(port);

    // 翻译
    let response = post(
        port,
        "/api/translate",
        &[
            ("Origin", &format!("http://127.0.0.1:{port}")),
            ("Content-Type", "application/json"),
            ("x-csrf-token", &token),
        ],
        r#"{"text":"Hello."}"#,
    );
    assert_eq!(status_of(&response), 200, "翻译请求失败：{response}");

    // 查词（未安装词典 → 明确的业务错误，而不是被 CSP/CSRF 拦掉）
    let lookup = post(
        port,
        "/api/lookup",
        &[
            ("Origin", &format!("http://127.0.0.1:{port}")),
            ("Content-Type", "application/json"),
            ("x-csrf-token", &token),
        ],
        r#"{"term":"hello"}"#,
    );
    assert_eq!(status_of(&lookup), 400, "{lookup}");
    assert!(
        body_of(&lookup).contains("词典"),
        "应返回词典未安装的业务错误：{lookup}"
    );
}
