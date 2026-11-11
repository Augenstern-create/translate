//! 真实 TCP + 真实 HTTP 报文的端到端测试。
//!
//! 覆盖的是"只测 JSON 构造/解析就一定会漏掉"的部分：
//! 鉴权头、状态码分类、重试次数、超时、取消、并发上限、流量顺序。
//!
//! 这些测试**不需要**任何真实密钥或真实转发站：`MockServer` 是本机回环上的真服务端。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use translator_http::client::HttpTransport;
use translator_http::config::HttpSettings;
use translator_http::usage::UsageMeter;
use translator_http::{BackendsFile, HttpErrorKind, HttpTranslationEngine, PromptInput, PromptTemplate};
use translator_plugin_api::{TranslationEngine, TranslationJob};
use translator_types::{
    CancelToken, LanguageTag, TermConstraint, TextKind, TranslateRequest, TranslationErrorCode,
    TranslationMode,
};

#[path = "common/mock_server.rs"]
mod mock_server;

use mock_server::{chat_completion_body, MockRequest, MockResponse, MockServer};

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}
fn zh() -> LanguageTag {
    LanguageTag::new("zh-CN").unwrap()
}

/// 持有 `LanguageTag` 的任务构造器。
///
/// 直接写 `source: &en()` 会借用临时值，编译不过；这里让构造器拥有标签，
/// `build()` 再从自身借用，测试代码就不必到处写 `let s = en();`。
struct JobSpec<'a> {
    text: &'a str,
    glossary: &'a [TermConstraint],
    placeholders: &'a [String],
    retry_hint: Option<&'a str>,
    cancel: Option<&'a CancelToken>,
    max_output_tokens: Option<u32>,
    mode: TranslationMode,
    source: LanguageTag,
    target: LanguageTag,
}

impl<'a> JobSpec<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            glossary: &[],
            placeholders: &[],
            retry_hint: None,
            cancel: None,
            max_output_tokens: None,
            mode: TranslationMode::Precise,
            source: en(),
            target: zh(),
        }
    }

    fn glossary(mut self, glossary: &'a [TermConstraint]) -> Self {
        self.glossary = glossary;
        self
    }

    fn placeholders(mut self, placeholders: &'a [String]) -> Self {
        self.placeholders = placeholders;
        self
    }

    fn retry_hint(mut self, hint: &'a str) -> Self {
        self.retry_hint = Some(hint);
        self
    }

    fn cancel(mut self, token: &'a CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    fn max_output_tokens(mut self, tokens: u32) -> Self {
        self.max_output_tokens = Some(tokens);
        self
    }

    fn build(&self) -> TranslationJob<'_> {
        TranslationJob {
            text: self.text,
            original_text: self.text,
            source: &self.source,
            target: &self.target,
            mode: self.mode,
            text_kind: TextKind::Sentence,
            glossary: self.glossary,
            placeholders: self.placeholders,
            retry_hint: self.retry_hint,
            deadline: None,
            cancel: self.cancel,
            max_output_tokens: self.max_output_tokens,
        }
    }
}

/// 构造一个指向 mock 服务端的引擎。
fn engine_for(base_url: &str, extra: &str) -> HttpTranslationEngine {
    let toml = format!(
        r#"
[backend.test]
base_url = "{base_url}"
model = "configured-model-id"
model_revision = "2026-11-01"
api_key_env = "TRANSLATOR_HTTP_TEST_KEY"
max_retries = {max_retries}
max_concurrent_requests = {concurrency}
timeout_ms = 3000
{extra}
"#,
        max_retries = 0,
        concurrency = 4,
    );
    let file = BackendsFile::from_toml_str(&toml).expect("配置应可解析");
    HttpTranslationEngine::from_file(&file)
        .expect("引擎应可构造")
        .into_iter()
        .next()
        .expect("应有一个后端")
}

fn run_engine(
    engine: &HttpTranslationEngine,
    job: &TranslationJob<'_>,
) -> Result<translator_plugin_api::EngineOutput, translator_types::TranslationError> {
    pollster::block_on(engine.translate_job(job))
}

#[test]
fn 成功翻译并发送正确报文() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "test-key-not-a-real-secret");
    let seen: Arc<Mutex<Vec<MockRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let server = MockServer::start(move |req| {
        sink.lock().unwrap().push(req);
        MockResponse::completion("你好，世界。")
    });

    let engine = engine_for(&server.base_url(), "");
    let out = run_engine(&engine, &JobSpec::new("Hello, world.").build()).expect("应成功");

    assert_eq!(out.text, "你好，世界。");
    assert_eq!(out.model_revision.as_deref(), Some("2026-11-01"));
    assert_eq!(out.usage.map(|u| u.total()), Some(44));

    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/chat/completions");
    assert_eq!(
        req.header("authorization"),
        Some("Bearer test-key-not-a-real-secret")
    );
    let body = req.json().unwrap();
    assert_eq!(body["model"], "configured-model-id");
    assert_eq!(body["messages"][1]["content"], "Hello, world.");
    assert_eq!(body["stream"], false);
}

#[test]
fn 密钥缺失时不发请求且报鉴权错误() {
    let counter = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&counter);
    let server = MockServer::start(move |_| {
        hits.fetch_add(1, Ordering::SeqCst);
        MockResponse::completion("不该被调用")
    });
    let toml = format!(
        r#"
[backend.test]
base_url = "{}"
model = "m"
api_key_env = "TRANSLATOR_HTTP_UNSET_KEY_98765"
max_retries = 0
"#,
        server.base_url()
    );
    let file = BackendsFile::from_toml_str(&toml).unwrap();
    let engine = HttpTranslationEngine::from_file(&file).unwrap().remove(0);

    let err = run_engine(&engine, &JobSpec::new("Hello.").build()).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::AuthenticationFailed);
    assert_eq!(counter.load(Ordering::SeqCst), 0, "不应发起网络请求");
    // 错误信息里不能出现密钥（本来就没有），也不能只有一句空话
    assert!(err.message.contains("TRANSLATOR_HTTP_UNSET_KEY_98765"));
}

#[test]
fn 限流会被有限重试后成功() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let counter = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&counter);
    let server = MockServer::start(move |_| {
        let n = hits.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            MockResponse::json(429, r#"{"error":{"message":"rate limit"}}"#)
        } else {
            MockResponse::completion("重试成功")
        }
    });

    let toml = format!(
        r#"
[backend.test]
base_url = "{}"
model = "m"
api_key_env = "TRANSLATOR_HTTP_TEST_KEY"
max_retries = 1
"#,
        server.base_url()
    );
    let file = BackendsFile::from_toml_str(&toml).unwrap();
    let engine = HttpTranslationEngine::from_file(&file).unwrap().remove(0);

    let out = run_engine(&engine, &JobSpec::new("Hello.").build()).expect("重试后应成功");
    assert_eq!(out.text, "重试成功");
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    assert_eq!(engine.usage().retries, 1);
}

#[test]
fn 鉴权失败不重试() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let counter = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&counter);
    let server = MockServer::start(move |_| {
        hits.fetch_add(1, Ordering::SeqCst);
        MockResponse::json(401, r#"{"error":{"message":"bad key"}}"#)
    });

    let toml = format!(
        r#"
[backend.test]
base_url = "{}"
model = "m"
api_key_env = "TRANSLATOR_HTTP_TEST_KEY"
max_retries = 3
"#,
        server.base_url()
    );
    let file = BackendsFile::from_toml_str(&toml).unwrap();
    let engine = HttpTranslationEngine::from_file(&file).unwrap().remove(0);

    let err = run_engine(&engine, &JobSpec::new("Hello.").build()).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::AuthenticationFailed);
    assert_eq!(counter.load(Ordering::SeqCst), 1, "鉴权错误不得重试");
}

#[test]
fn 服务端故障重试耗尽后报不可用() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let counter = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&counter);
    let server = MockServer::start(move |_| {
        hits.fetch_add(1, Ordering::SeqCst);
        MockResponse::json(503, "service unavailable")
    });

    let toml = format!(
        r#"
[backend.test]
base_url = "{}"
model = "m"
api_key_env = "TRANSLATOR_HTTP_TEST_KEY"
max_retries = 2
"#,
        server.base_url()
    );
    let file = BackendsFile::from_toml_str(&toml).unwrap();
    let engine = HttpTranslationEngine::from_file(&file).unwrap().remove(0);

    let err = run_engine(&engine, &JobSpec::new("Hello.").build()).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::BackendUnavailable);
    assert_eq!(counter.load(Ordering::SeqCst), 3, "首次 + 2 次重试");
}

#[test]
fn 截断输出不会被当成成功() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let server = MockServer::start(|_| {
        MockResponse::json(
            200,
            r#"{"choices":[{"message":{"content":"半句话"},"finish_reason":"length"}]}"#,
        )
    });
    let engine = engine_for(&server.base_url(), "");
    let err = run_engine(&engine, &JobSpec::new("Hello.").build()).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::InvalidBackendResponse);
}

#[test]
fn 超时映射为后端超时() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let server = MockServer::start(|_| {
        MockResponse::completion("太慢了").with_delay(Duration::from_millis(600))
    });
    let toml = format!(
        r#"
[backend.test]
base_url = "{}"
model = "m"
api_key_env = "TRANSLATOR_HTTP_TEST_KEY"
max_retries = 0
timeout_ms = 120
"#,
        server.base_url()
    );
    let file = BackendsFile::from_toml_str(&toml).unwrap();
    let engine = HttpTranslationEngine::from_file(&file).unwrap().remove(0);

    let err = run_engine(&engine, &JobSpec::new("Hello.").build()).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::BackendTimeout);
}

#[test]
fn 取消后立即返回而不是等后端() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let server = MockServer::start(|_| {
        MockResponse::completion("迟到").with_delay(Duration::from_millis(800))
    });
    let engine = engine_for(&server.base_url(), "");

    let token = CancelToken::new();
    let token_for_thread = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        token_for_thread.cancel();
    });

    let spec = JobSpec::new("Hello.").cancel(&token);

    let start = Instant::now();
    let err = pollster::block_on(engine.translate_job(&spec.build())).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::Cancelled);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "取消应立即生效，实际耗时 {:?}",
        start.elapsed()
    );
}

#[test]
fn 并发上限生效() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (cur, hi) = (Arc::clone(&current), Arc::clone(&peak));
    let server = MockServer::start(move |_| {
        let now = cur.fetch_add(1, Ordering::SeqCst) + 1;
        hi.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(120));
        cur.fetch_sub(1, Ordering::SeqCst);
        MockResponse::completion("好的")
    });

    let toml = format!(
        r#"
[backend.test]
base_url = "{}"
model = "m"
api_key_env = "TRANSLATOR_HTTP_TEST_KEY"
max_retries = 0
max_concurrent_requests = 1
"#,
        server.base_url()
    );
    let file = BackendsFile::from_toml_str(&toml).unwrap();
    let engine = Arc::new(HttpTranslationEngine::from_file(&file).unwrap().remove(0));

    let mut handles = Vec::new();
    for _ in 0..3 {
        let engine = Arc::clone(&engine);
        handles.push(std::thread::spawn(move || {
            let r = run_engine(&engine, &JobSpec::new("Hello.").build());
            assert!(r.is_ok(), "{r:?}");
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), 1, "并发闸门应把并发压到 1");
    assert_eq!(engine.usage().requests, 3);
}

#[test]
fn 术语与占位符进入系统消息而原文只出现一次() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let seen: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);
    let server = MockServer::start(move |req| {
        *sink.lock().unwrap() = req.json();
        MockResponse::completion("请访问⟦P1⟧并输入大语言模型。")
    });

    let engine = engine_for(&server.base_url(), "");
    let glossary = vec![TermConstraint {
        source: "large language model".to_string(),
        target: "大语言模型".to_string(),
        strict: true,
        matched: "phrase".to_string(),
    }];
    let placeholders = vec!["⟦P1⟧".to_string()];
    let spec = JobSpec::new("Please visit ⟦P1⟧ and try the large language model.")
        .glossary(&glossary)
        .placeholders(&placeholders)
        .retry_hint("上次丢了 ⟦P1⟧")
        .max_output_tokens(512);

    let out = pollster::block_on(engine.translate_job(&spec.build())).expect("应成功");
    assert!(out.text.contains("大语言模型"));

    let body = seen.lock().unwrap().clone().expect("应收到请求");
    let system = body["messages"][0]["content"].as_str().unwrap();
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(system.contains("large language model"));
    assert!(system.contains("大语言模型"));
    assert!(system.contains("⟦P1⟧"));
    assert!(system.contains("Correction for this attempt"));
    assert_eq!(user, "Please visit ⟦P1⟧ and try the large language model.");
    assert_eq!(body["max_tokens"], 512);
}

#[test]
fn 传输层直接调用可以拿到原始状态与响应体() {
    let server = MockServer::start(|_| MockResponse::json(418, "teapot"));
    let meter = UsageMeter::new();
    let transport = HttpTransport::new(
        &HttpSettings::default(),
        0,
        1,
        Duration::from_secs(2),
        Arc::clone(&meter),
    );
    let outcome = pollster::block_on(transport.send(
        translator_http::HttpRequest {
            url: format!("{}/chat/completions", server.base_url()),
            api_key: None,
            body: serde_json::json!({"x": 1}),
            timeout: Duration::from_secs(2),
        },
        None,
        None,
    ))
    .expect("传输层不把 4xx 当传输错误");
    assert_eq!(outcome.status, 418);
    assert_eq!(outcome.body, "teapot");
}

#[test]
fn 连不上的端口报不可用() {
    let meter = UsageMeter::new();
    let transport = HttpTransport::new(
        &HttpSettings::default(),
        0,
        1,
        Duration::from_millis(500),
        Arc::clone(&meter),
    );
    let err = pollster::block_on(transport.send(
        translator_http::HttpRequest {
            url: "http://127.0.0.1:9/v1/chat/completions".to_string(),
            api_key: None,
            body: serde_json::json!({}),
            timeout: Duration::from_millis(500),
        },
        None,
        None,
    ))
    .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::Unavailable);
}

#[test]
fn 提示词模板可以注入自定义内容() {
    let prompt = PromptTemplate::embedded_precise();
    let messages = prompt.build_messages(
        &PromptInput {
            text: "Hello.",
            glossary: &[],
            placeholders: &[],
            retry_hint: None,
        },
        "zh-CN",
    );
    assert!(messages[0].content.contains("Simplified Chinese"));
}

#[test]
fn 简单请求走_translate_也能工作() {
    std::env::set_var("TRANSLATOR_HTTP_TEST_KEY", "k");
    let server = MockServer::start(|_| MockResponse::completion("你好。"));
    let engine = engine_for(&server.base_url(), "");
    let req = TranslateRequest::new("Hello.", zh()).with_source(en());
    let result = pollster::block_on(engine.translate(&req)).expect("应成功");
    assert_eq!(result.text, "你好。");
    assert_eq!(result.engine_id, "http.test");
}

#[test]
fn 响应体里带_bom_或前后空白的_json_不被误判() {
    // 真实转发站偶尔在 JSON 前加空白；解析必须容忍。
    let body = format!("  \n{}", chat_completion_body("你好。"));
    let parsed = translator_http::parse_response(200, &body).expect("应能解析");
    assert_eq!(parsed.text, "你好。");
}
