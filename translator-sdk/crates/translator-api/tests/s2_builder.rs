//! S2 审查修复：`TranslatorBuilder` 的 HTTP 装配路径。
//!
//! 重点验证两件事：
//!
//! 1. 通过**正式 Builder 入口**创建的多后端共享同一个 `HttpRuntime`；
//! 2. `http_global_concurrency` / `http_queue_size` 真正生效（不是只写进配置）。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use translator_api::{
    LanguageTag, TranslateRequest, TranslationErrorCode, TranslationPolicy, Translator,
};

// ============================================================
// 本地 mock 模型服务端（带并发峰值统计与延迟）
// ============================================================

struct MockModel {
    port: u16,
    peak: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
}

impl MockModel {
    fn start(delay_ms: u64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 mock 端口失败");
        let port = listener.local_addr().unwrap().port();
        let peak = Arc::new(AtomicUsize::new(0));
        let hits = Arc::new(AtomicUsize::new(0));
        let current = Arc::new(AtomicUsize::new(0));
        let (peak_thread, hits_thread, current_thread) =
            (Arc::clone(&peak), Arc::clone(&hits), Arc::clone(&current));

        std::thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(stream) = incoming else { continue };
                let peak = Arc::clone(&peak_thread);
                let hits = Arc::clone(&hits_thread);
                let current = Arc::clone(&current_thread);
                std::thread::spawn(move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(delay_ms));
                    current.fetch_sub(1, Ordering::SeqCst);
                    let _ = respond(stream);
                });
            }
        });

        Self { port, peak, hits }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn respond(mut stream: TcpStream) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader};

    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    // 必须把请求**读完整**再响应：只读头部就回包时，Windows 上会出现
    // "客户端仍在写 body、服务端已关连接"的竞态（Peer disconnected）。
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
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
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
    }

    let body = serde_json::json!({
        "id": "chatcmpl-test",
        "model": "mock-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "你好。" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 8, "completion_tokens": 3, "total_tokens": 11 }
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

fn two_backend_toml(base_url: &str) -> String {
    format!(
        r#"
[backend.a]
base_url = "{base_url}"
model = "m"
model_revision = "r1"
api_key_env = ""
max_retries = 0
max_concurrent_requests = 8
timeout_ms = 8000

[backend.b]
base_url = "{base_url}"
model = "m"
model_revision = "r1"
api_key_env = ""
max_retries = 0
max_concurrent_requests = 8
timeout_ms = 8000
"#
    )
}

fn request_for(engine: &str, text: &str) -> TranslateRequest {
    TranslateRequest::new(text, LanguageTag::new("zh-CN").unwrap())
        .with_source(LanguageTag::new("en").unwrap())
        .with_policy(TranslationPolicy::ExplicitEngine(engine.to_string()))
}

// ============================================================
// 用例
// ============================================================

#[test]
fn builder_装配两个后端并共享全局并发() {
    let model = MockModel::start(120);
    let translator = Translator::builder()
        .http_config_toml(&two_backend_toml(&model.base_url()))
        .expect("配置应可解析")
        .http_global_concurrency(2)
        .http_queue_size(16)
        .build()
        .expect("应可构造");

    assert_eq!(translator.backend_ids(), vec!["http.a", "http.b"]);

    let translator = Arc::new(translator);
    let mut handles = Vec::new();
    for index in 0..8 {
        let translator = Arc::clone(&translator);
        handles.push(std::thread::spawn(move || {
            let engine = if index % 2 == 0 { "http.a" } else { "http.b" };
            pollster::block_on(translator.translate(&request_for(engine, "Hello."))).map(|r| r.text)
        }));
    }
    for handle in handles {
        assert_eq!(handle.join().unwrap().unwrap(), "你好。");
    }

    assert_eq!(model.hits(), 8);
    assert!(
        model.peak() <= 2,
        "经 Builder 装配的两个后端必须共享全局并发 2，实测峰值 {}",
        model.peak()
    );
}

#[test]
fn builder_的队列上限真正生效() {
    // 全局并发 1、队列 1、服务端 400ms：4 个并发请求中必然有被拒绝的
    let model = MockModel::start(400);
    let translator = Translator::builder()
        .http_config_toml(&two_backend_toml(&model.base_url()))
        .expect("配置应可解析")
        .http_global_concurrency(1)
        .http_queue_size(1)
        .build()
        .expect("应可构造");

    let translator = Arc::new(translator);
    let started = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let translator = Arc::clone(&translator);
        handles.push(std::thread::spawn(move || {
            pollster::block_on(translator.translate(&request_for("http.a", "Hello.")))
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

    assert!(rejected >= 1, "队列上限 1 时应至少拒绝一个请求");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "拒绝必须是立即的（实测 {:?}）",
        started.elapsed()
    );
}

#[test]
fn 两个后端各自仍是独立的引擎() {
    let model = MockModel::start(10);
    let translator = Translator::builder()
        .http_config_toml(&two_backend_toml(&model.base_url()))
        .expect("配置应可解析")
        .build()
        .expect("应可构造");

    let capabilities = translator.capabilities();
    let ids: Vec<&str> = capabilities.iter().map(|c| c.engine_id.as_str()).collect();
    assert!(ids.contains(&"http.a"), "{ids:?}");
    assert!(ids.contains(&"http.b"), "{ids:?}");

    // 显式指定引擎时，两个后端都能独立工作
    let text_a = pollster::block_on(translator.translate(&request_for("http.a", "Hello.")))
        .expect("后端 a 应可用");
    let text_b = pollster::block_on(translator.translate(&request_for("http.b", "Hello.")))
        .expect("后端 b 应可用");
    assert_eq!(text_a.text, "你好。");
    assert_eq!(text_b.text, "你好。");
}
