//! 最小可用翻译界面：本地回环上的单页 Web UI。
//!
//! # 为什么是"内置在 CLI 里的一页网页"
//!
//! 目标是**最小可用**：用户不需要写代码、不需要理解 crate、打开浏览器就能
//! 翻译和查词。为此不需要引入 GUI 框架，也不需要前端构建链。
//!
//! # 安全模型（S1-03 / S1-04）
//!
//! 这是一个**本机、单用户、无身份体系**的界面。它仍然必须防住两类真实威胁：
//!
//! 1. **任意网页向本机服务发写请求**（CSRF / DNS rebinding）；
//! 2. **超限或畸形请求**把进程拖死。
//!
//! 因此：
//!
//! - 只绑定 `127.0.0.1`，没有"对外开放"选项；
//! - 校验 `Host`：只接受 `127.0.0.1:<port>` / `localhost:<port>`（防 DNS rebinding）；
//! - 校验 `Origin`：浏览器提交的跨站请求一律 403；
//! - CSRF：启动时生成随机会话令牌，页面通过 `<meta>` 取得，POST 必须带回
//!   `x-csrf-token`；令牌不是硬编码常量；
//! - 请求头 / 请求行 / 请求体都有上限，超限**在读取前**直接拒绝（431/413）；
//! - 只允许 GET/HEAD 与 POST；POST 必须是 `application/json`；
//! - 不支持的 `Transfer-Encoding`、非法 `Content-Length` 直接 400；
//! - 每个连接有读写超时。
//!
//! # XSS（S1-02）
//!
//! 页面渲染译文与词典释义一律使用 `textContent` / `createElement`，
//! **不**使用 `innerHTML`，也不引入任何外部脚本或样式。

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use translator_api::Translator;
use translator_types::{LanguageTag, LookupRequest, TextKind, TranslateRequest, TranslationMode};

/// 只监听回环地址：这是安全边界，不是可配置项。
const BIND_HOST: &str = "127.0.0.1";

/// 请求行上限（字节）。
const MAX_REQUEST_LINE: usize = 8 * 1024;
/// 请求头上限（字节，含请求行）。
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// 请求体上限（字节）。
const MAX_BODY_BYTES: usize = 256 * 1024;
/// 单次连接读超时。
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// 单次连接写超时（翻译可能较慢，但不能无限等）。
const WRITE_TIMEOUT: Duration = Duration::from_secs(180);

/// 启动本地 UI，阻塞直到进程退出。
pub fn serve(translator: Arc<Translator>, port: u16) -> Result<(), String> {
    let listener = TcpListener::bind((BIND_HOST, port))
        .map_err(|e| format!("无法绑定 {BIND_HOST}:{port}：{e}"))?;
    serve_listener(listener, translator)
}

/// 默认最大并发连接数（本地单用户界面足够，同时防止资源被打满）。
pub const MAX_CONNECTIONS: usize = 64;

/// 在已绑定的监听器上提供服务（测试用，端口可以是 0）。
pub fn serve_listener(listener: TcpListener, translator: Arc<Translator>) -> Result<(), String> {
    serve_listener_with(listener, translator, MAX_CONNECTIONS)
}

/// 带连接上限的版本（测试用小上限验证拒绝行为）。
pub fn serve_listener_with(
    listener: TcpListener,
    translator: Arc<Translator>,
    max_connections: usize,
) -> Result<(), String> {
    let addr = listener
        .local_addr()
        .map_err(|e| format!("读取监听地址失败：{e}"))?;
    let port = addr.port();
    // 令牌必须是密码学随机；拿不到就直接拒绝启动（不回退弱随机）
    let csrf = generate_token()?;
    let state = Arc::new(ServerState { translator, csrf });

    println!("翻译界面已启动：http://{addr}/");
    println!("（仅监听回环地址；按 Ctrl+C 结束。文本会发送到已配置的后端）");
    eprintln!(
        "提示：后端 = {:?}；如需改配置请编辑 translate.toml 后重启",
        state.translator.backend_ids()
    );

    // 连接上限：每连接一个线程，但线程数必须**有界**，
    // 否则本地界面也能被大量连接打满内存/句柄。
    let active = Arc::new(AtomicUsize::new(0));
    let max_connections = max_connections.max(1);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if active.fetch_add(1, Ordering::SeqCst) >= max_connections {
            active.fetch_sub(1, Ordering::SeqCst);
            let _ = reject_busy(stream);
            continue;
        }
        let state = Arc::clone(&state);
        let active = Arc::clone(&active);
        std::thread::spawn(move || {
            // 守卫保证任何返回路径都会归还配额
            struct Guard(Arc<AtomicUsize>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _guard = Guard(active);
            if let Err(err) = handle(stream, &state, port) {
                eprintln!("请求处理失败：{err}");
            }
        });
    }
    Ok(())
}

/// 连接数超限时立即回复 503 并关闭，不排队、不创建新线程。
fn reject_busy(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let body = r#"{"error":"服务繁忙，请稍后重试"}"#;
    let response = format!(
        "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\ncache-control: no-store\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

struct ServerState {
    translator: Arc<Translator>,
    /// 每次启动随机生成的 CSRF 令牌。
    csrf: String,
}

// ============================================================
// 请求解析（带硬上限）
// ============================================================

/// 一次已经通过头部解析的请求。
struct Request {
    method: String,
    path: String,
    host: Option<String>,
    origin: Option<String>,
    csrf: Option<String>,
    content_type: Option<String>,
    body: String,
}

/// 头部读取失败的原因（直接映射到 HTTP 状态码）。
enum HeadError {
    /// 请求头或请求行过大 → 431。
    HeadersTooLarge,
    /// 请求体超过上限 → 413（在读取之前拒绝）。
    BodyTooLarge,
    /// 报文格式错误（请求行、Content-Length 等）。
    Malformed(String),
    /// 不支持的传输方式（chunked）。
    UnsupportedFraming,
}

impl HeadError {
    fn status(&self) -> u16 {
        match self {
            Self::HeadersTooLarge => 431,
            Self::BodyTooLarge => 413,
            Self::Malformed(_) => 400,
            Self::UnsupportedFraming => 400,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::HeadersTooLarge => "请求头超过大小限制".to_string(),
            Self::BodyTooLarge => "请求体超过大小限制".to_string(),
            Self::Malformed(msg) => msg.clone(),
            Self::UnsupportedFraming => {
                "不支持 Transfer-Encoding（请使用 Content-Length）".to_string()
            }
        }
    }
}

fn read_request(stream: &TcpStream) -> Result<Request, HeadError> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| HeadError::Malformed(format!("无法读取连接：{e}")))?,
    );

    let mut total = 0usize;
    let mut line = String::new();
    read_line(&mut reader, &mut total, &mut line)?;
    if line.is_empty() {
        return Err(HeadError::Malformed("空请求行".to_string()));
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let version = parts.next().unwrap_or("");
    if method.is_empty() || !version.starts_with("HTTP/1.") || parts.next().is_some() {
        return Err(HeadError::Malformed("请求行格式非法".to_string()));
    }

    let mut host = None;
    let mut origin = None;
    let mut csrf = None;
    let mut content_type = None;
    let mut content_length: Option<usize> = None;
    let mut header_count = 0usize;
    // 安全相关的头只允许出现一次：重复值会让"校验哪一个"变成实现细节。
    let mut seen_security_headers: Vec<&'static str> = Vec::new();

    loop {
        read_line(&mut reader, &mut total, &mut line)?;
        if line.is_empty() {
            break;
        }
        header_count += 1;
        if header_count > 64 {
            return Err(HeadError::HeadersTooLarge);
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(HeadError::Malformed(format!(
                "非法请求头：{}",
                truncate(&line, 40)
            )));
        };
        let name_lower = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();

        // 安全头重复 → 400（Host / Origin / x-csrf-token / Content-Type）
        let security_header = match name_lower.as_str() {
            "host" => Some("Host"),
            "origin" => Some("Origin"),
            "x-csrf-token" => Some("x-csrf-token"),
            "content-type" => Some("Content-Type"),
            _ => None,
        };
        if let Some(label) = security_header {
            if seen_security_headers.contains(&label) {
                return Err(HeadError::Malformed(format!("重复的 {label} 请求头")));
            }
            seen_security_headers.push(label);
        }

        match name_lower.as_str() {
            "host" => host = Some(value),
            "origin" => origin = Some(value),
            "x-csrf-token" => csrf = Some(value),
            "content-type" => content_type = Some(value),
            "content-length" => {
                if content_length.is_some() {
                    return Err(HeadError::Malformed("重复的 Content-Length".to_string()));
                }
                let parsed: usize = value.parse().map_err(|_| {
                    HeadError::Malformed(format!("非法 Content-Length：{}", truncate(&value, 20)))
                })?;
                if parsed > MAX_BODY_BYTES {
                    // 超限必须在读取请求体**之前**拒绝，不能截断后继续处理
                    return Err(HeadError::BodyTooLarge);
                }
                content_length = Some(parsed);
            }
            "transfer-encoding" => return Err(HeadError::UnsupportedFraming),
            _ => {}
        }
    }

    // 请求体：严格按 Content-Length 读满
    let mut body = Vec::new();
    if let Some(len) = content_length {
        body.resize(len, 0);
        reader
            .read_exact(&mut body)
            .map_err(|e| HeadError::Malformed(format!("读取请求体失败：{e}")))?;
    }

    Ok(Request {
        method,
        path,
        host,
        origin,
        csrf,
        content_type,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

/// 逐行读取（只认 `\n`，容忍 `\r\n`），并累计总字节数。
fn read_line(
    reader: &mut BufReader<TcpStream>,
    total: &mut usize,
    line: &mut String,
) -> Result<(), HeadError> {
    line.clear();
    let mut raw = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = reader
            .read(&mut byte)
            .map_err(|e| HeadError::Malformed(format!("读取请求头失败：{e}")))?;
        if n == 0 {
            break;
        }
        *total += 1;
        if *total > MAX_HEAD_BYTES {
            return Err(HeadError::HeadersTooLarge);
        }
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            raw.push(byte[0]);
        }
        if raw.len() > MAX_REQUEST_LINE {
            return Err(HeadError::HeadersTooLarge);
        }
    }
    *line = String::from_utf8_lossy(&raw).to_string();
    Ok(())
}

// ============================================================
// 校验规则（纯函数，便于测试）
// ============================================================

/// `Host` 必须是本机回环地址与实际监听端口（防 DNS rebinding）。
fn host_allowed(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host else {
        // HTTP/1.1 必须带 Host；缺失按不合法处理
        return false;
    };
    let host = host.trim().to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

/// 浏览器发起的跨站请求必须被拒绝；没有 Origin 的请求（curl / 本机页面）放行。
fn origin_allowed(origin: Option<&str>, port: u16) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let origin = origin.trim().to_ascii_lowercase();
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}

/// CSRF 令牌比较（定长比较，避免明显的时序差异）。
fn csrf_matches(provided: Option<&str>, expected: &str) -> bool {
    let Some(provided) = provided else {
        return false;
    };
    if provided.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in provided.as_bytes().iter().zip(expected.as_bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// POST 必须是 JSON。
fn content_type_allowed(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let value = value.trim().to_ascii_lowercase();
    value == "application/json" || value.starts_with("application/json;")
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

// ============================================================
// 路由与响应
// ============================================================

fn handle(mut stream: TcpStream, state: &ServerState, port: u16) -> std::io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;

    let request = match read_request(&stream) {
        Ok(request) => request,
        Err(err) => {
            return respond_json(
                &mut stream,
                err.status(),
                &serde_json::json!({ "error": err.message() }).to_string(),
            );
        }
    };

    // Host 校验对所有请求生效
    if !host_allowed(request.host.as_deref(), port) {
        return respond_json(
            &mut stream,
            403,
            &serde_json::json!({ "error": "非法的 Host" }).to_string(),
        );
    }

    let path = request.path.split('?').next().unwrap_or("").to_string();
    match (request.method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => {
            // CSP nonce 每次响应重新生成；脚本与样式都只认这个 nonce，
            // 不使用 'unsafe-inline'。
            let nonce = match generate_nonce() {
                Ok(nonce) => nonce,
                Err(err) => {
                    return respond_json(
                        &mut stream,
                        500,
                        &serde_json::json!({ "error": err }).to_string(),
                    );
                }
            };
            let html = INDEX_HTML
                .replace("{{CSRF_TOKEN}}", &state.csrf)
                .replace("{{SCRIPT_NONCE}}", &nonce);
            let csp = content_security_policy(&nonce);
            respond_html(&mut stream, &html, &csp)
        }
        ("HEAD", "/") => respond_head(&mut stream, "text/html; charset=utf-8"),
        ("GET", "/api/health") => {
            let body = serde_json::json!({
                "status": "ok",
                "backends": state.translator.backend_ids(),
                "mode": format!("{:?}", state.translator.default_options().mode),
            })
            .to_string();
            respond_json(&mut stream, 200, &body)
        }
        ("POST", "/api/translate") | ("POST", "/api/lookup") => {
            if !origin_allowed(request.origin.as_deref(), port) {
                return respond_json(
                    &mut stream,
                    403,
                    &serde_json::json!({ "error": "非法的 Origin" }).to_string(),
                );
            }
            if !csrf_matches(request.csrf.as_deref(), &state.csrf) {
                return respond_json(
                    &mut stream,
                    403,
                    &serde_json::json!({ "error": "缺少或错误的 CSRF 令牌" }).to_string(),
                );
            }
            if !content_type_allowed(request.content_type.as_deref()) {
                return respond_json(
                    &mut stream,
                    415,
                    &serde_json::json!({ "error": "Content-Type 必须是 application/json" })
                        .to_string(),
                );
            }
            let outcome = if path == "/api/translate" {
                translate(&state.translator, &request.body)
            } else {
                lookup(&state.translator, &request.body)
            };
            match outcome {
                Ok(body) => respond_json(&mut stream, 200, &body),
                Err(message) => respond_json(
                    &mut stream,
                    400,
                    &serde_json::json!({ "error": message }).to_string(),
                ),
            }
        }
        ("GET", _) | ("HEAD", _) => respond_json(
            &mut stream,
            404,
            &serde_json::json!({ "error": "not found" }).to_string(),
        ),
        _ => respond_json_with_allow(
            &mut stream,
            405,
            &serde_json::json!({ "error": "method not allowed" }).to_string(),
        ),
    }
}

fn translate(translator: &Translator, body: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("请求体不是合法 JSON：{e}"))?;
    let text = value
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("缺少 text 字段")?
        .to_string();
    if text.trim().is_empty() {
        return Err("text 不能为空".to_string());
    }
    let mode = match value.get("mode").and_then(|v| v.as_str()) {
        Some("fast") => TranslationMode::Fast,
        _ => translator.default_options().mode,
    };

    let request = TranslateRequest::new(text, LanguageTag::new("zh-CN").map_err(|e| e.message)?)
        .with_source(LanguageTag::new("en").map_err(|e| e.message)?);
    let mut options = translator.default_options().clone();
    options.mode = mode;
    options.text_kind = TextKind::Sentence;

    let started = std::time::Instant::now();
    let output =
        pollster::block_on(translator.translate_with_diagnostics_options(&request, &options))
            .map_err(|e| format!("{}（{}）", e.message, e.code))?;
    let elapsed = started.elapsed().as_millis() as u64;

    Ok(serde_json::json!({
        "text": output.result.text,
        "engine_id": output.result.engine_id,
        "elapsed_ms": elapsed,
        "attempts": output.diagnostics.attempts,
        "http_calls": output.diagnostics.http_calls,
        "cached": output.diagnostics.cache_hit,
        "protection": output.diagnostics.protection_strategy,
        "repair": output.diagnostics.repair,
        "warnings": output.result.warnings.iter().map(|w| format!("{w:?}")).collect::<Vec<_>>(),
        "quality": output.diagnostics.quality.as_ref().map(|q| serde_json::json!({
            "fatal": q.has_fatal(),
            "issues": q.issues.len(),
            "summary": q.summary(),
        })),
    })
    .to_string())
}

fn lookup(translator: &Translator, body: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("请求体不是合法 JSON：{e}"))?;
    let term = value
        .get("term")
        .and_then(|v| v.as_str())
        .ok_or("缺少 term 字段")?
        .to_string();
    let mut request = LookupRequest::new(term);
    if let Ok(en) = LanguageTag::new("en") {
        request = request.with_language(en);
    }
    let entries = pollster::block_on(translator.lookup(&request))
        .map_err(|e| format!("{}（{}）", e.message, e.code))?;
    let list: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "headword": entry.headword,
                "source_id": entry.source_id,
                "senses": entry.senses.iter().map(|s| s.definition.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(serde_json::json!({ "entries": list }).to_string())
}

fn security_headers() -> &'static str {
    "X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\n"
}

fn respond_json(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    respond_json_inner(stream, status, body, "")
}

fn respond_json_with_allow(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    respond_json_inner(stream, status, body, "Allow: GET, HEAD, POST\r\n")
}

fn respond_json_inner(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
    extra: &str,
) -> std::io::Result<()> {
    let payload = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{extra}{security}Connection: close\r\n\r\n{body}",
        body.len(),
        reason = reason(status),
        extra = extra,
        security = security_headers(),
        body = body
    );
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

/// 页面 CSP：默认全禁，只放行带 nonce 的脚本/样式与同源 fetch。
fn content_security_policy(nonce: &str) -> String {
    format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; \
         connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    )
}

fn respond_html(stream: &mut TcpStream, body: &str, csp: &str) -> std::io::Result<()> {
    let payload = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: {}\r\n{}Connection: close\r\n\r\n{}",
        body.len(),
        csp,
        security_headers(),
        body
    );
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

fn respond_head(stream: &mut TcpStream, content_type: &str) -> std::io::Result<()> {
    let payload = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: 0\r\nCache-Control: no-store\r\n{}Connection: close\r\n\r\n",
        security_headers()
    );
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        431 => "Request Header Fields Too Large",
        _ => "Status",
    }
}

/// 从操作系统 CSPRNG 取 `n` 字节并编码为十六进制。
///
/// **失败即失败**：拿不到密码学随机数时不允许退化成时间/进程号之类的弱随机，
/// 调用方必须把它当成启动错误处理（`serve` 返回 `Err`）。
fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).map_err(|e| format!("无法从操作系统获取安全随机数：{e}"))?;
    Ok(buffer.iter().map(|b| format!("{b:02x}")).collect())
}

/// CSRF 会话令牌：256 bit（32 字节）随机。
fn generate_token() -> Result<String, String> {
    random_hex(32)
}

/// CSP nonce：128 bit（16 字节）随机，每次响应重新生成。
fn generate_nonce() -> Result<String, String> {
    random_hex(16)
}

/// 单页界面。刻意不引用任何外部资源：离线也能用；渲染一律用 textContent。
const INDEX_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="csrf-token" content="{{CSRF_TOKEN}}">
<title>英译中翻译</title>
<style nonce="{{SCRIPT_NONCE}}">
  :root { color-scheme: light dark; }
  body { font-family: system-ui, -apple-system, "Segoe UI", "Noto Sans CJK SC", sans-serif;
         max-width: 780px; margin: 2rem auto; padding: 0 1rem; line-height: 1.6; }
  h1 { font-size: 1.35rem; margin-bottom: .25rem; }
  .hint { opacity: .7; font-size: .85rem; margin-bottom: 1.5rem; }
  textarea { width: 100%; min-height: 9rem; padding: .6rem; font: inherit;
             border: 1px solid #8884; border-radius: 8px; resize: vertical; }
  .row { display: flex; gap: .6rem; align-items: center; margin: .6rem 0; flex-wrap: wrap; }
  button { padding: .45rem 1rem; font: inherit; border-radius: 8px; border: 1px solid #8886;
           background: #3b82f6; color: #fff; cursor: pointer; }
  button.secondary { background: transparent; color: inherit; }
  button:disabled { opacity: .5; cursor: progress; }
  .out { margin-top: 1rem; padding: .8rem; border: 1px solid #8884; border-radius: 8px;
         min-height: 3rem; white-space: pre-wrap; }
  .meta { font-size: .8rem; opacity: .65; margin-top: .4rem; }
  .warn { color: #b45309; font-size: .85rem; }
  .err { color: #dc2626; }
  input[type=text] { padding: .45rem; font: inherit; border-radius: 8px; border: 1px solid #8884; }
  ul { padding-left: 1.2rem; }
  .entry-head { font-weight: 600; }
  /* 这些原本是内联 style 属性；CSP 不允许 style 属性内联，因此改成类 */
  hr.sep { margin: 2rem 0; border: none; border-top: 1px solid #8883; }
  h1.sub { font-size: 1.1rem; }
  input.grow { flex: 1 1 12rem; }
</style>
</head>
<body>
<h1>英译中翻译</h1>
<div class="hint">文本会发送到本机配置的翻译后端。保护内容（URL、变量、代码、编号）会被原样保留。</div>

<textarea id="input" placeholder="输入英文，例如：The file at {path} was not found."></textarea>
<div class="row">
  <label><input type="radio" name="mode" value="precise" checked> 精准</label>
  <label><input type="radio" name="mode" value="fast"> 快速</label>
  <button id="go">翻译</button>
  <button id="clear" class="secondary">清空</button>
</div>
<div class="out" id="out"></div>
<div class="meta" id="meta"></div>
<div class="warn" id="warn"></div>

<hr class="sep">

<h1 class="sub">查词</h1>
<div class="row">
  <input type="text" id="term" placeholder="hello" class="grow">
  <button id="lookup">查询</button>
</div>
<div id="dict"></div>

<script nonce="{{SCRIPT_NONCE}}">
(function () {
  "use strict";
  var $ = function (id) { return document.getElementById(id); };
  var CSRF = document.querySelector('meta[name="csrf-token"]').getAttribute("content");

  function setText(el, value) { el.textContent = value; }

  async function postJSON(url, payload) {
    var res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", "x-csrf-token": CSRF },
      body: JSON.stringify(payload),
    });
    var data = await res.json().catch(function () { return { error: "响应不是 JSON" }; });
    if (!res.ok) { throw new Error(data.error || ("HTTP " + res.status)); }
    return data;
  }

  $("go").onclick = async function () {
    var text = $("input").value.trim();
    if (!text) { return; }
    var mode = document.querySelector("input[name=mode]:checked").value;
    $("go").disabled = true;
    setText($("out"), "翻译中…");
    $("out").className = "out";
    setText($("meta"), ""); setText($("warn"), "");
    try {
      var data = await postJSON("/api/translate", { text: text, mode: mode });
      setText($("out"), data.text);
      var q = data.quality ? " · " + data.quality.summary : "";
      setText($("meta"),
        data.engine_id + " · " + data.elapsed_ms + " ms · 保护=" + data.protection +
        (data.cached ? " · 命中缓存" : "") +
        (data.attempts > 1 ? " · 尝试" + data.attempts + "次" : "") +
        (data.repair === "rejected" ? " · 格式修复已回退" : "") + q);
      if (data.warnings && data.warnings.length) {
        setText($("warn"), "⚠ " + data.warnings.join("；"));
      }
    } catch (e) {
      setText($("out"), "翻译失败");
      $("out").className = "out err";
      setText($("meta"), e.message);
    } finally {
      $("go").disabled = false;
    }
  };

  $("clear").onclick = function () {
    $("input").value = "";
    setText($("out"), ""); setText($("meta"), ""); setText($("warn"), "");
  };

  // 词典渲染：全部走 createElement + textContent，绝不把词典数据当 HTML 解析
  function renderEntries(entries) {
    var container = $("dict");
    while (container.firstChild) { container.removeChild(container.firstChild); }
    if (!entries.length) { setText(container, "未找到"); return; }

    entries.forEach(function (entry) {
      var head = document.createElement("p");
      var name = document.createElement("span");
      name.className = "entry-head";
      setText(name, entry.headword);
      head.appendChild(name);
      var source = document.createElement("span");
      source.className = "meta";
      setText(source, "  [" + entry.source_id + "]");
      head.appendChild(source);
      container.appendChild(head);

      var list = document.createElement("ul");
      entry.senses.forEach(function (sense) {
        var li = document.createElement("li");
        setText(li, sense);
        list.appendChild(li);
      });
      container.appendChild(list);
    });
  }

  $("lookup").onclick = async function () {
    var term = $("term").value.trim();
    if (!term) { return; }
    var container = $("dict");
    setText(container, "查询中…");
    try {
      var data = await postJSON("/api/lookup", { term: term });
      renderEntries(data.entries || []);
    } catch (e) {
      setText(container, "查词失败：" + e.message);
    }
  };
})();
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 只绑定回环地址() {
        assert_eq!(BIND_HOST, "127.0.0.1", "界面不得监听非回环地址");
    }

    #[test]
    fn 页面不引用外部资源且不使用_innerhtml() {
        assert!(!INDEX_HTML.contains("http://"));
        assert!(!INDEX_HTML.contains("https://"));
        assert!(
            !INDEX_HTML.contains("innerHTML"),
            "词典渲染不得使用 innerHTML"
        );
        assert!(INDEX_HTML.contains("textContent"));
        assert!(INDEX_HTML.contains("createElement"));
        assert!(INDEX_HTML.contains("{{CSRF_TOKEN}}"));
    }

    #[test]
    fn host_只接受本机与当前端口() {
        assert!(host_allowed(Some("127.0.0.1:8765"), 8765));
        assert!(host_allowed(Some("localhost:8765"), 8765));
        assert!(host_allowed(Some("LOCALHOST:8765"), 8765));
        assert!(!host_allowed(Some("127.0.0.1:9999"), 8765));
        assert!(!host_allowed(Some("evil.example:8765"), 8765));
        assert!(!host_allowed(Some("127.0.0.1"), 8765));
        assert!(!host_allowed(None, 8765));
    }

    #[test]
    fn origin_只接受同源() {
        assert!(origin_allowed(None, 8765));
        assert!(origin_allowed(Some("http://127.0.0.1:8765"), 8765));
        assert!(origin_allowed(Some("http://localhost:8765"), 8765));
        assert!(!origin_allowed(Some("http://evil.example"), 8765));
        assert!(!origin_allowed(Some("http://127.0.0.1:9999"), 8765));
        assert!(!origin_allowed(Some("null"), 8765));
    }

    #[test]
    fn csrf_令牌是_256bit_且每次不同() {
        let token = generate_token().expect("OS CSPRNG 应可用");
        assert_eq!(token.len(), 64, "256 bit = 32 字节 = 64 个十六进制字符");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(csrf_matches(Some(&token), &token));
        assert!(!csrf_matches(None, &token));
        assert!(!csrf_matches(Some(""), &token));
        assert!(!csrf_matches(Some(&token[..63]), &token));
        assert_ne!(
            generate_token().expect("OS CSPRNG 应可用"),
            token,
            "令牌必须每次不同"
        );
    }

    #[test]
    fn nonce_是_128bit_且每次不同() {
        let nonce = generate_nonce().expect("OS CSPRNG 应可用");
        assert_eq!(nonce.len(), 32, "128 bit = 16 字节 = 32 个十六进制字符");
        assert_ne!(generate_nonce().expect("OS CSPRNG 应可用"), nonce);
    }

    #[test]
    fn csp_只放行_nonce_且没有_unsafe_inline() {
        let nonce = generate_nonce().unwrap();
        let csp = content_security_policy(&nonce);
        assert!(
            csp.contains(&format!("script-src 'nonce-{nonce}'")),
            "{csp}"
        );
        assert!(csp.contains(&format!("style-src 'nonce-{nonce}'")), "{csp}");
        assert!(csp.contains("default-src 'none'"), "{csp}");
        assert!(csp.contains("connect-src 'self'"), "{csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(
            !csp.contains("unsafe-inline"),
            "不得放行 inline 脚本/样式：{csp}"
        );
        assert!(!csp.contains("unsafe-eval"), "{csp}");
        assert!(!csp.contains("http://"), "{csp}");
        assert!(!csp.contains("https://"), "{csp}");
    }

    #[test]
    fn content_type_必须是_json() {
        assert!(content_type_allowed(Some("application/json")));
        assert!(content_type_allowed(Some(
            "application/json; charset=utf-8"
        )));
        assert!(content_type_allowed(Some("APPLICATION/JSON")));
        assert!(!content_type_allowed(Some("text/html")));
        assert!(!content_type_allowed(Some(
            "application/x-www-form-urlencoded"
        )));
        assert!(!content_type_allowed(None));
    }
}
