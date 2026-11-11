//! OpenAI 兼容 Chat Completions 协议：请求体构造与响应解析。
//!
//! 只实现**一种**已定义清楚的协议（§9.1）。不猜供应商扩展字段：
//! 差异化参数一律通过 `extra_body` 显式传入。
//!
//! 解析原则：宁可报错，不要"看起来能用"。缺 `choices`、`content` 为空、
//! `content` 不是字符串、被 `max_tokens` 截断，全部按
//! [`HttpErrorKind::InvalidResponse`] 处理，而不是返回半句话当作译文。

use serde_json::{json, Value};
use translator_types::TokenUsage;

use crate::config::BackendConfig;
use crate::errors::HttpError;

/// 一条对话消息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptMessage {
    pub role: &'static str,
    pub content: String,
}

impl PromptMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system",
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user",
            content: content.into(),
        }
    }
}

/// 构造 `/chat/completions` 请求体。
pub fn build_request_body(
    cfg: &BackendConfig,
    messages: &[PromptMessage],
    max_tokens: u32,
) -> Value {
    let messages: Vec<Value> = messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();

    let mut body = json!({
        "model": cfg.model,
        "messages": messages,
        "temperature": cfg.temperature,
        "max_tokens": max_tokens,
        "stream": false,
    });

    // 扩展字段：先放供应商风格预设，再放用户显式 extra_body（显式优先）。
    if cfg.disable_thinking {
        if let Some(Value::Object(extra)) = cfg.thinking_style.disable_fields() {
            if let Value::Object(map) = &mut body {
                for (k, v) in extra {
                    map.insert(k, v);
                }
            }
        }
    }
    if let Some(Value::Object(extra)) = &cfg.extra_body {
        if let Value::Object(map) = &mut body {
            for (k, v) in extra {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    body
}

/// 解析后的补全结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatCompletion {
    /// 已做最小净化的译文。
    pub text: String,
    /// 服务端回报的实际模型名（可能带快照后缀）。
    pub model: Option<String>,
    pub usage: Option<TokenUsage>,
    /// `stop` / `length` / `content_filter` …
    pub finish_reason: Option<String>,
}

/// 解析响应体。`status` 由调用方传入（非 2xx 已在别处处理）。
pub fn parse_response(status: u16, body: &str) -> Result<ChatCompletion, HttpError> {
    if !(200..300).contains(&status) {
        return Err(HttpError::from_status(status, body));
    }

    let value: Value = serde_json::from_str(body).map_err(|e| {
        HttpError::invalid_response(format!(
            "响应不是合法 JSON：{e}；响应体开头：{}",
            crate::errors::truncate(body, 120)
        ))
    })?;

    // 有些转发站会用 200 + {"error": {...}} 表达上游失败。
    if let Some(error) = value.get("error") {
        if !error.is_null() {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未知错误");
            let code = error.get("code").and_then(Value::as_str).unwrap_or("");
            return Err(classify_api_error(code, message));
        }
    }

    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| HttpError::invalid_response("响应缺少 choices[0]"))?;

    let content = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HttpError::invalid_response("响应缺少 choices[0].message.content（或不是字符串）")
        })?;

    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(str::to_string);

    let text = sanitize_translation(content);
    if text.is_empty() {
        return Err(HttpError::invalid_response("后端返回了空译文"));
    }
    if finish_reason.as_deref() == Some("length") {
        // 截断的译文**不能**当作成功结果：它必然是半句话，
        // 落库后会变成"看起来正常"的错误数据（§4.5 / §5.5）。
        return Err(HttpError::invalid_response(format!(
            "输出被 max_tokens 截断（finish_reason=length），译文不完整：{}",
            crate::errors::truncate(&text, 40)
        )));
    }
    if finish_reason.as_deref() == Some("content_filter") {
        return Err(HttpError::invalid_response(
            "后端以 content_filter 拒绝了本次翻译",
        ));
    }

    let model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);

    let usage = value.get("usage").map(|u| TokenUsage {
        input_tokens: u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: u
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    });

    Ok(ChatCompletion {
        text,
        model,
        usage,
        finish_reason,
    })
}

/// 把上游业务错误映射成可解释的类别。
///
/// 转发站/供应商的 `code` 字符串不统一，因此只做**保守**匹配：
/// 认不出来的归为 `InvalidResponse`，绝不因此重试（避免重复计费）。
fn classify_api_error(code: &str, message: &str) -> HttpError {
    use crate::errors::HttpErrorKind;
    let lower = format!("{code} {message}").to_lowercase();
    let kind = if lower.contains("insufficient")
        || lower.contains("quota")
        || lower.contains("balance")
        || lower.contains("余额")
        || lower.contains("额度")
    {
        HttpErrorKind::BudgetExceeded
    } else if lower.contains("api key")
        || lower.contains("unauthorized")
        || lower.contains("invalid_api_key")
        || lower.contains("authentication")
    {
        HttpErrorKind::Auth
    } else if lower.contains("rate limit") || lower.contains("too many") {
        HttpErrorKind::RateLimited
    } else {
        HttpErrorKind::InvalidResponse
    };
    HttpError::new(
        kind,
        format!("后端返回错误：{}", crate::errors::truncate(message, 200)),
    )
}

/// 最小净化：只去掉**明显不是译文**的外壳，不改写语义。
///
/// 1. 去掉整体包裹的 ``` 代码围栏（含首行语言标记）；
/// 2. 去掉 `Translation:` / `译文：` 这类行首标签；
/// 3. 折叠首尾空白。
///
/// 刻意**不**做的事：不加/删标点、不替换术语、不"润色"。那些属于质量门禁，
/// 不属于协议层。
pub fn sanitize_translation(raw: &str) -> String {
    let mut text = raw.trim().to_string();

    if text.starts_with("```") {
        if let Some(first_newline) = text.find('\n') {
            let after = &text[first_newline + 1..];
            if let Some(end) = after.rfind("```") {
                text = after[..end].trim().to_string();
            }
        }
    }

    for label in [
        "Translation:",
        "Translated text:",
        "译文：",
        "译文:",
        "中文翻译：",
        "中文翻译:",
    ] {
        if let Some(rest) = text.strip_prefix(label) {
            text = rest.trim().to_string();
            break;
        }
    }

    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThinkingStyle;

    fn cfg() -> BackendConfig {
        let mut cfg: BackendConfig = toml::from_str(
            r#"
base_url = "https://relay.example/v1"
model = "configured-model-id"
"#,
        )
        .unwrap();
        cfg.id = "remote".into();
        cfg.temperature = 0.0;
        cfg
    }

    #[test]
    fn 请求体包含协议必需字段() {
        let body = build_request_body(
            &cfg(),
            &[PromptMessage::system("sys"), PromptMessage::user("hello")],
            512,
        );
        assert_eq!(body["model"], "configured-model-id");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hello");
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(body["stream"], false);
        assert_eq!(body["temperature"], 0.0);
    }

    #[test]
    fn 关闭思考按选定风格注入字段() {
        let mut c = cfg();
        c.disable_thinking = true;
        c.thinking_style = ThinkingStyle::ChatTemplate;
        let body = build_request_body(&c, &[], 16);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);

        c.thinking_style = ThinkingStyle::None;
        let body = build_request_body(&c, &[], 16);
        assert!(body.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn 显式扩展字段覆盖预设() {
        let mut c = cfg();
        c.disable_thinking = true;
        c.thinking_style = ThinkingStyle::ReasoningEffort;
        c.extra_body = Some(serde_json::json!({"reasoning_effort": "low", "top_p": 0.5}));
        let body = build_request_body(&c, &[], 16);
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["top_p"], 0.5);
    }

    #[test]
    fn 正常响应解析出译文与用量() {
        let body = r#"{
          "id":"x","model":"configured-model-id-2026-11-01",
          "choices":[{"index":0,"message":{"role":"assistant","content":"你好，世界。"},"finish_reason":"stop"}],
          "usage":{"prompt_tokens":21,"completion_tokens":7,"total_tokens":28}
        }"#;
        let out = parse_response(200, body).unwrap();
        assert_eq!(out.text, "你好，世界。");
        assert_eq!(out.model.as_deref(), Some("configured-model-id-2026-11-01"));
        assert_eq!(
            out.usage,
            Some(TokenUsage {
                input_tokens: 21,
                output_tokens: 7
            })
        );
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn 截断的译文必须报错而不是返回半句() {
        let body =
            r#"{"choices":[{"message":{"content":"这是一句被截"},"finish_reason":"length"}]}"#;
        let err = parse_response(200, body).unwrap_err();
        assert!(err.message.contains("截断"), "{err}");
        assert_eq!(err.kind, crate::errors::HttpErrorKind::InvalidResponse);
    }

    #[test]
    fn 空译文与缺_choices_都是非法响应() {
        let empty = r#"{"choices":[{"message":{"content":"   "},"finish_reason":"stop"}]}"#;
        assert_eq!(
            parse_response(200, empty).unwrap_err().kind,
            crate::errors::HttpErrorKind::InvalidResponse
        );
        let missing = r#"{"model":"m"}"#;
        assert_eq!(
            parse_response(200, missing).unwrap_err().kind,
            crate::errors::HttpErrorKind::InvalidResponse
        );
        let not_json = "<html>502 Bad Gateway</html>";
        assert_eq!(
            parse_response(200, not_json).unwrap_err().kind,
            crate::errors::HttpErrorKind::InvalidResponse
        );
    }

    #[test]
    fn 非_2xx_按状态码分类() {
        assert_eq!(
            parse_response(401, "{}").unwrap_err().kind,
            crate::errors::HttpErrorKind::Auth
        );
        assert_eq!(
            parse_response(429, "{}").unwrap_err().kind,
            crate::errors::HttpErrorKind::RateLimited
        );
        assert_eq!(
            parse_response(502, "bad gateway").unwrap_err().kind,
            crate::errors::HttpErrorKind::Unavailable
        );
    }

    #[test]
    fn 两百状态里的业务错误也被识别() {
        let body = r#"{"error":{"message":"Insufficient balance","code":"insufficient_quota"}}"#;
        assert_eq!(
            parse_response(200, body).unwrap_err().kind,
            crate::errors::HttpErrorKind::BudgetExceeded
        );
        let body = r#"{"error":{"message":"Invalid API key","code":"invalid_api_key"}}"#;
        assert_eq!(
            parse_response(200, body).unwrap_err().kind,
            crate::errors::HttpErrorKind::Auth
        );
    }

    #[test]
    fn 净化只去掉外壳不改语义() {
        assert_eq!(sanitize_translation("  你好。  "), "你好。");
        assert_eq!(sanitize_translation("```\n你好。\n```"), "你好。");
        assert_eq!(sanitize_translation("```chinese\n你好。\n```"), "你好。");
        assert_eq!(sanitize_translation("译文：你好。"), "你好。");
        // 句子内部的英文标签不能被误删
        assert_eq!(
            sanitize_translation("请把 Translation: 字段填好。"),
            "请把 Translation: 字段填好。"
        );
    }
}
