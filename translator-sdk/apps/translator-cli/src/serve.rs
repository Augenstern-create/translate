//! 最小可用翻译界面：本地回环上的单页 Web UI。
//!
//! # 为什么是"内置在 CLI 里的一页网页"
//!
//! 目标是**最小可用**（§18 Phase 6）：用户不需要写代码、不需要理解 crate、
//! 打开浏览器就能翻译和查词。为此不需要引入 GUI 框架，也不需要前端构建链：
//! 一个 `TcpListener` + 一段内嵌 HTML 就够了。
//!
//! # 安全边界（重要）
//!
//! - **只绑定 127.0.0.1**：不接受来自局域网的连接；
//! - 没有鉴权，因此也不允许通过 `--host` 暴露到公网；
//! - 默认端口 8765，可用 `--port` 改；
//! - 该界面会把用户输入发给**已配置的翻译后端**，这一点在页面上明确写出。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use translator_api::Translator;
use translator_types::{LanguageTag, LookupRequest, TextKind, TranslateRequest, TranslationMode};

/// 只监听回环地址：这是安全边界，不是可配置项。
const BIND_HOST: &str = "127.0.0.1";

/// 启动本地 UI，阻塞直到进程退出。
pub fn serve(translator: Arc<Translator>, port: u16) -> Result<(), String> {
    let listener = TcpListener::bind((BIND_HOST, port))
        .map_err(|e| format!("无法绑定 {BIND_HOST}:{port}：{e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("读取监听地址失败：{e}"))?;

    println!("翻译界面已启动：http://{addr}/");
    println!("（仅监听回环地址；按 Ctrl+C 结束。文本会发送到已配置的后端）");
    eprintln!(
        "提示：后端 = {:?}；如需改配置请编辑 translate.toml 后重启",
        translator.backend_ids()
    );

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let translator = Arc::clone(&translator);
        // 每个连接一个线程：翻译可能要几秒，不能把页面卡死
        std::thread::spawn(move || {
            if let Err(err) = handle(stream, &translator) {
                eprintln!("请求处理失败：{err}");
            }
        });
    }
    Ok(())
}

struct HttpRequest {
    method: String,
    path: String,
    body: String,
}

fn handle(mut stream: TcpStream, translator: &Translator) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(180)))?;

    let Some(request) = read_request(&stream)? else {
        return Ok(());
    };

    match (
        request.method.as_str(),
        request.path.split('?').next().unwrap_or(""),
    ) {
        ("GET", "/") | ("GET", "/index.html") => respond_html(&mut stream, INDEX_HTML),
        ("GET", "/api/health") => {
            let body = serde_json::json!({
                "status": "ok",
                "backends": translator.backend_ids(),
                "mode": format!("{:?}", translator.default_options().mode),
            })
            .to_string();
            respond_json(&mut stream, 200, &body)
        }
        ("POST", "/api/translate") => match translate(translator, &request.body) {
            Ok(body) => respond_json(&mut stream, 200, &body),
            Err(message) => respond_json(
                &mut stream,
                400,
                &serde_json::json!({ "error": message }).to_string(),
            ),
        },
        ("POST", "/api/lookup") => match lookup(translator, &request.body) {
            Ok(body) => respond_json(&mut stream, 200, &body),
            Err(message) => respond_json(
                &mut stream,
                400,
                &serde_json::json!({ "error": message }).to_string(),
            ),
        },
        _ => respond_json(
            &mut stream,
            404,
            &serde_json::json!({ "error": "not found" }).to_string(),
        ),
    }
}

fn read_request(stream: &TcpStream) -> std::io::Result<Option<HttpRequest>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(None);
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

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
        if let Some((k, v)) = trimmed.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut body = vec![0u8; content_length.min(1 << 20)];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    Ok(Some(HttpRequest {
        method,
        path,
        body: String::from_utf8_lossy(&body).to_string(),
    }))
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
        "cached": output.diagnostics.cache_hit,
        "protection": output.diagnostics.protection_strategy,
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

fn respond_json(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    respond(
        stream,
        status,
        "application/json; charset=utf-8",
        body.as_bytes(),
    )
}

fn respond_html(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    respond(stream, 200, "text/html; charset=utf-8", body.as_bytes())
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Status",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// 单页界面。刻意不引用任何外部资源：离线也能用。
const INDEX_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>英译中翻译</title>
<style>
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

<hr style="margin:2rem 0; border:none; border-top:1px solid #8883">

<h1 style="font-size:1.1rem">查词</h1>
<div class="row">
  <input type="text" id="term" placeholder="hello" style="flex:1 1 12rem">
  <button id="lookup">查询</button>
</div>
<div id="dict"></div>

<script>
const $ = (id) => document.getElementById(id);

async function postJSON(url, payload) {
  const res = await fetch(url, { method: 'POST', headers: {'content-type':'application/json'},
                                 body: JSON.stringify(payload) });
  const data = await res.json().catch(() => ({ error: '响应不是 JSON' }));
  if (!res.ok) throw new Error(data.error || ('HTTP ' + res.status));
  return data;
}

$('go').onclick = async () => {
  const text = $('input').value.trim();
  if (!text) return;
  const mode = document.querySelector('input[name=mode]:checked').value;
  $('go').disabled = true;
  $('out').textContent = '翻译中…';
  $('meta').textContent = ''; $('warn').textContent = '';
  $('out').className = 'out';
  try {
    const data = await postJSON('/api/translate', { text, mode });
    $('out').textContent = data.text;
    const q = data.quality ? ` · ${data.quality.summary}` : '';
    $('meta').textContent = `${data.engine_id} · ${data.elapsed_ms} ms · 保护=${data.protection}` +
      (data.cached ? ' · 命中缓存' : '') + (data.attempts > 1 ? ` · 尝试${data.attempts}次` : '') + q;
    if (data.warnings && data.warnings.length) $('warn').textContent = '⚠ ' + data.warnings.join('；');
  } catch (e) {
    $('out').textContent = '翻译失败';
    $('out').className = 'out err';
    $('meta').textContent = e.message;
  } finally {
    $('go').disabled = false;
  }
};

$('clear').onclick = () => { $('input').value = ''; $('out').textContent = '';
  $('meta').textContent = ''; $('warn').textContent = ''; };

$('lookup').onclick = async () => {
  const term = $('term').value.trim();
  if (!term) return;
  $('dict').textContent = '查询中…';
  try {
    const data = await postJSON('/api/lookup', { term });
    if (!data.entries.length) { $('dict').textContent = '未找到'; return; }
    $('dict').innerHTML = data.entries.map(e =>
      `<p><b>${e.headword}</b> <span class="meta">[${e.source_id}]</span></p>` +
      '<ul>' + e.senses.map(s => `<li>${s}</li>`).join('') + '</ul>').join('');
  } catch (e) {
    $('dict').textContent = '查词失败：' + e.message;
  }
};
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 页面不引用外部资源() {
        assert!(!INDEX_HTML.contains("http://"));
        assert!(!INDEX_HTML.contains("https://"));
        assert!(INDEX_HTML.contains("/api/translate"));
        assert!(INDEX_HTML.contains("/api/lookup"));
    }

    #[test]
    fn 只绑定回环地址() {
        assert_eq!(BIND_HOST, "127.0.0.1", "界面不得监听非回环地址");
    }
}
