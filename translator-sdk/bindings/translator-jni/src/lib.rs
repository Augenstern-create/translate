//! P6 JNI 绑定层（契约 v1，见 `docs/39` §1.3 与 ADR-061）。
//!
//! # 这个 crate 只做适配
//!
//! 依赖方向是单向的：`translator-jni → translator-api`。这里**没有**业务逻辑：
//! 路由、预算、资源仓库、规则都在 SDK 内部；本层只负责
//!
//! 1. 把 `jstring` 解成 Rust `String`（并拒绝含 `U+0000` 的输入，见 §"编码"）；
//! 2. 维护**字符串代际句柄**（`"gen:index"`）→ `Arc<Translator>` 的注册表；
//! 3. 把 `TranslationError` 映射成稳定 JSON（不新增错误码）；
//! 4. 在 FFI 边界 `catch_unwind`，保证 panic 不穿过 JNI。
//!
//! # 关闭语义（用户 2026-11-07 明确要求）
//!
//! `nativeCall` 正在执行时另一个线程调 `nativeClose`，**不得释放正在使用的 `Translator`**：
//! 调用路径在查表后立刻 `Arc::clone` 再释放注册表锁，因此：
//! - 新请求：看到 `closed == true` 或句柄已被移除 → `AlreadyClosed`（拒绝新请求）；
//! - 已在飞的请求：持有自己的 `Arc`，会**安全跑完**，最后一个 `Arc` 释放时才 drop。
//!
//! # 编码
//!
//! 句柄/文本都走 `jni` crate：它用 `NewString`(UTF-16) 生成 Java 字符串，
//! 避免了 `NewStringUTF`(Modified UTF-8) 与 Rust 标准 UTF-8 在 Emoji（非 BMP）上的冲突。
//! 输入若含 `U+0000`，Modified UTF-8 会把它编码成 `C0 80`，静默接受等于悄悄改写用户文本，
//! 因此直接返回 `InvalidRequest`。

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use jni::objects::{JClass, JString};
use jni::sys::{jint, jstring};
use jni::JNIEnv;
use serde_json::{json, Value};

use translator_api::{LanguageTag, LookupRequest, Translator};

/// 契约版本。任何不兼容变更都必须 +1，并让旧版本明确失败（不静默猜测）。
const PROTOCOL_VERSION: i64 = 1;

/// 输入上限（J-10）。超过就明确拒绝，不把内存交给调用方决定。
///
/// - 单条文本：64 KiB UTF-8。64 KiB 的中文约 2.1 万字符，远超正常句子；
/// - 整个 payload：256 KiB。深嵌套/超大数组在解析前就被挡住；
/// - lookup 的 `limit`：1000。防止一次查词把整本词典拉进内存。
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_LOOKUP_LIMIT: u64 = 1000;

// ============================================================
// 句柄注册表
// ============================================================

struct Entry {
    translator: Arc<Translator>,
    /// 创建时的原始 config：`resources` 需要 `resource_repo` 之类的路径。
    config: Value,
    /// 这个 Entry 自己的代际。句柄里的 gen 必须与它一致，而不是与全局最新代际一致。
    generation: u64,
    closed: AtomicBool,
    /// 在飞请求的取消令牌（key = 调用方提供的 `request_id`）。
    ///
    /// `nativeCall("translate")` 是同步阻塞的，另一个线程用
    /// `nativeCall("cancel")` 把同一个 id 置为取消即可（G06）。
    cancels: Mutex<HashMap<String, translator_api::CancelToken>>,
}

struct Registry {
    /// 只用于生成唯一的句柄号；**不再**用它淘汰其它存活实例。
    generation: u64,
    next_index: u64,
    entries: HashMap<u64, Arc<Entry>>,
}

/// 同时存活的实例上限。句柄泄漏最终会耗尽内存，因此有一个明确的硬上限。
const MAX_ENTRIES: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
enum HandleError {
    Invalid,
    Stale,
    Closed,
}

impl Registry {
    /// 新建一个 entry，返回 `"generation:index"`。`index` 单调、永不复用。
    fn insert(
        &mut self,
        translator: Arc<Translator>,
        config: Value,
    ) -> Result<String, HandleError> {
        if self.entries.len() >= MAX_ENTRIES {
            return Err(HandleError::Invalid);
        }
        self.generation += 1;
        let generation = self.generation;
        let index = self.next_index;
        self.next_index += 1;
        self.entries.insert(
            index,
            Arc::new(Entry {
                translator,
                config,
                generation,
                closed: AtomicBool::new(false),
                cancels: Mutex::new(HashMap::new()),
            }),
        );
        Ok(format!("{generation}:{index}"))
    }

    /// 按 handle 查 entry：先校验格式，再校验**该 entry 自己的** generation，
    /// 最后看 closed。旧实例不会因为新实例创建而失效。
    fn resolve(&self, raw: &str) -> Result<Arc<Entry>, HandleError> {
        let Some((gen, index)) = parse_handle(raw) else {
            return Err(HandleError::Invalid);
        };
        let Some(entry) = self.entries.get(&index) else {
            return Err(HandleError::Closed);
        };
        if entry.generation != gen {
            return Err(HandleError::Stale);
        }
        if entry.closed.load(Ordering::SeqCst) {
            return Err(HandleError::Closed);
        }
        Ok(Arc::clone(entry))
    }

    /// 只关闭 handle 指向的那个 entry；gen 不匹配时不误伤别人。
    fn close(&mut self, raw: &str) -> Option<Arc<Entry>> {
        let (gen, index) = parse_handle(raw)?;
        let matches = self
            .entries
            .get(&index)
            .map(|e| e.generation == gen)
            .unwrap_or(false);
        if !matches {
            return None;
        }
        self.entries.remove(&index).inspect(|entry| {
            entry.closed.store(true, Ordering::SeqCst);
        })
    }
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            generation: 0,
            next_index: 0,
            entries: HashMap::new(),
        })
    })
}

/// 解析 `"generation:index"`。格式不合法返回 `None`（调用方转成 `InvalidRequest`）。
fn parse_handle(raw: &str) -> Option<(u64, u64)> {
    let (gen, idx) = raw.split_once(':')?;
    Some((gen.parse().ok()?, idx.parse().ok()?))
}

// ============================================================
// 错误与结果信封
// ============================================================

fn envelope_ok(data: Value) -> Value {
    json!({ "protocol_version": PROTOCOL_VERSION, "ok": true, "data": data })
}

fn envelope_err(code: &str, message: impl Into<String>) -> Value {
    json!({
        "protocol_version": PROTOCOL_VERSION,
        "ok": false,
        "error": { "code": code, "message": message.into() }
    })
}

/// `TranslationError` → JSON。**不新增错误码**，直接透传 SDK 的 12 个码。
fn envelope_sdk_err(err: &translator_types::TranslationError) -> Value {
    let mut err_obj = json!({
        "code": err.code.as_str(),
        "message": err.message,
    });
    if !err.suggested_packages.is_empty() {
        err_obj["suggested_packages"] = json!(err.suggested_packages);
    }
    json!({ "protocol_version": PROTOCOL_VERSION, "ok": false, "error": err_obj })
}

// ============================================================
// 配置与调用
// ============================================================

fn build_translator(config: &Value) -> Result<Translator, Value> {
    let build = || -> Result<Translator, translator_types::TranslationError> {
        let mut builder = Translator::builder();

        // 测试用 mock 引擎：只有显式 `"mock_engines": true` 才注册，
        // 这样默认产物里没有"假翻译"，也方便 J-08/J-09 验证门禁。
        if config
            .get("mock_engines")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            builder = builder
                .engine(translator_api::mocks::MockNmtEngine::new())
                .engine(translator_api::mocks::MockRemoteEngine::new(false));
            if config.get("dict_package").and_then(Value::as_str).is_none() {
                builder = builder.provider(translator_api::mocks::MockDictionaryProvider::new());
            }
        }

        if let Some(dir) = config.get("dict_package").and_then(Value::as_str) {
            builder = builder.dictionary_package(dir);
        }
        if let Some(path) = config.get("rules_file").and_then(Value::as_str) {
            builder = builder.rules_file(path);
        }
        if let Some(bytes) = config.get("memory_budget_bytes").and_then(Value::as_u64) {
            builder = builder.memory_budget(Some(bytes));
        }
        if let Some(policy) = config.get("term_policy").and_then(Value::as_str) {
            let parsed = match policy {
                "off" => translator_api::TermPolicy::Off,
                "warn" => translator_api::TermPolicy::Warn,
                "enforce" => translator_api::TermPolicy::Enforce,
                other => {
                    return Err(translator_types::TranslationError::new(
                        translator_types::TranslationErrorCode::InvalidRequest,
                        format!("term_policy 只能是 off|warn|enforce：{other}"),
                    ))
                }
            };
            builder = builder.term_policy(parsed);
        }
        builder.build()
    };

    build().map_err(|e| envelope_sdk_err(&e))
}

fn platform_name() -> String {
    let os = if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

fn call_capabilities(translator: &Translator) -> Value {
    let engines: Vec<Value> = translator
        .capabilities()
        .into_iter()
        .map(|c| {
            json!({
                "engine_id": c.engine_id,
                "capabilities": c.capabilities.iter().map(|x| x.as_str()).collect::<Vec<_>>(),
                "priority": c.priority,
                "license": c.license,
            })
        })
        .collect();

    // 运行时状态如实报告：Android 上没有编译进任何真实 NMT 后端（见 docs/36）。
    let runtimes = json!([
        { "id": "runtime.bergamot", "available": cfg!(feature = "nmt-bergamot"),
          "reason": if cfg!(feature = "nmt-bergamot") { Value::Null } else { json!("not_compiled") } },
        { "id": "runtime.ort", "available": cfg!(feature = "nmt-ort"),
          "reason": if cfg!(feature = "nmt-ort") { Value::Null } else { json!("not_compiled") } }
    ]);

    envelope_ok(json!({
        "platform": platform_name(),
        "binding": { "multi_instance": true, "max_instances": MAX_ENTRIES },
        "engines": engines,
        "runtimes": runtimes,
    }))
}

/// 解析 payload：先卡字节数，再要求是 JSON 对象。
///
/// `serde_json` 默认递归上限 128 层；深嵌套会在这里以 `InvalidRequest` 失败，
/// 不需要自己写深度检查（但要保证错误信息可诊断）。
fn parse_payload(raw: &str) -> Result<Value, Value> {
    if raw.len() > MAX_PAYLOAD_BYTES {
        return Err(envelope_err(
            "InvalidRequest",
            format!("payload 超过上限：{} 字节 > {MAX_PAYLOAD_BYTES}", raw.len()),
        ));
    }
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| envelope_err("InvalidRequest", format!("payload 非法：{e}")))?;
    if !value.is_object() {
        return Err(envelope_err(
            "InvalidRequest",
            "payload 必须是 JSON 对象（数组/标量不接受）",
        ));
    }
    Ok(value)
}

fn payload_text(payload: &Value) -> Result<String, Value> {
    let text = payload
        .get("text")
        .or_else(|| payload.get("term"))
        .and_then(Value::as_str)
        .ok_or_else(|| envelope_err("InvalidRequest", "payload 缺少 text 字段"))?;
    if text.len() > MAX_TEXT_BYTES {
        return Err(envelope_err(
            "InvalidRequest",
            format!("文本超过上限：{} 字节 > {MAX_TEXT_BYTES}", text.len()),
        ));
    }
    if text.contains('\0') {
        return Err(envelope_err(
            "InvalidRequest",
            "文本含 NUL：JNI 的 Modified UTF-8 会把它编码成 C0 80，静默接受等于悄悄改写用户文本",
        ));
    }
    // 控制字符：明确拒绝 C0（保留制表/换行/回车）与 DEL。
    // 理由：控制字符会破坏日志与终端，且没有真实的翻译语义；
    // 换行是正常段落分隔，因此不拒绝。
    if text
        .chars()
        .any(|c| (c.is_control() && !matches!(c, '\t' | '\n' | '\r')) || c == '\u{7f}')
    {
        return Err(envelope_err(
            "InvalidRequest",
            "文本含不允许的控制字符（仅允许 TAB/LF/CR）",
        ));
    }
    if text.trim().is_empty() {
        return Err(envelope_err("InvalidRequest", "文本为空"));
    }
    Ok(text.to_string())
}

fn call_detect(translator: &Translator, payload: &Value) -> Value {
    let text = match payload_text(payload) {
        Ok(t) => t,
        Err(e) => return e,
    };
    match pollster::block_on(translator.detect_language(&text)) {
        Ok(guesses) => envelope_ok(json!({
            "guesses": guesses
                .iter()
                .map(|g| json!({ "language": g.language.as_str(), "confidence": g.confidence }))
                .collect::<Vec<_>>(),
        })),
        Err(e) => envelope_sdk_err(&e),
    }
}

fn call_lookup(translator: &Translator, payload: &Value) -> Value {
    let term = match payload_text(payload) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let language = match payload.get("language").and_then(Value::as_str) {
        Some(tag) => match LanguageTag::new(tag) {
            Ok(t) => Some(t),
            Err(e) => return envelope_sdk_err(&e),
        },
        None => None,
    };
    let limit = payload.get("limit").and_then(Value::as_u64).unwrap_or(10);
    if limit > MAX_LOOKUP_LIMIT {
        return envelope_err(
            "InvalidRequest",
            format!("limit 超过上限：{limit} > {MAX_LOOKUP_LIMIT}"),
        );
    }
    let limit = limit as usize;

    let req = LookupRequest {
        term,
        language,
        limit,
    };
    match pollster::block_on(translator.lookup(&req)) {
        Ok(entries) => envelope_ok(json!({
            "entries": entries
                .iter()
                .map(|e| json!({
                    "headword": e.headword,
                    "normalized": e.normalized,
                    "language": e.language.as_str(),
                    "readings": e.readings,
                    "parts_of_speech": e.parts_of_speech,
                    "senses": e.senses.iter().map(|s| json!({
                        "definition": s.definition, "source_id": s.source_id
                    })).collect::<Vec<_>>(),
                    "source_id": e.source_id,
                    "source_license": e.source_license,
                    "confidence": e.confidence,
                }))
                .collect::<Vec<_>>(),
        })),
        Err(e) => envelope_sdk_err(&e),
    }
}

fn call_translate(entry: &Entry, payload: &Value) -> Value {
    let text = match payload_text(payload) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let Some(target_raw) = payload.get("target").and_then(Value::as_str) else {
        return envelope_err("InvalidRequest", "translate 需要 target 字段");
    };
    let target = match LanguageTag::new(target_raw) {
        Ok(t) => t,
        Err(e) => return envelope_sdk_err(&e),
    };
    let source = match payload.get("source").and_then(Value::as_str) {
        Some(tag) => match LanguageTag::new(tag) {
            Ok(t) => Some(t),
            Err(e) => return envelope_sdk_err(&e),
        },
        None => None,
    };
    let policy = match payload.get("policy").and_then(Value::as_str) {
        None | Some("auto") => translator_api::TranslationPolicy::AutoLocalFirst,
        Some("dictionary") => translator_api::TranslationPolicy::DictionaryOnly,
        Some("local") => translator_api::TranslationPolicy::LocalOnly,
        Some("remote") => translator_api::TranslationPolicy::RemoteOnly,
        Some(other) if other.starts_with("engine:") => {
            translator_api::TranslationPolicy::ExplicitEngine(
                other.trim_start_matches("engine:").to_string(),
            )
        }
        Some(other) => return envelope_err("InvalidRequest", format!("未知 policy：{other}")),
    };

    let mut req = translator_api::TranslateRequest::new(text, target);
    req.source = source;
    req.policy = policy;
    if let Some(ms) = payload.get("timeout_ms").and_then(Value::as_u64) {
        req.timeout_ms = Some(ms);
    }

    // request_id 可选；传了就可以被另一个线程 cancel。
    // 非法值明确拒绝，而不是静默忽略（否则宿主以为能取消，实际不能）。
    let request_id = match payload.get("request_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if !id.is_empty() && id.len() <= 128 => Some(id.clone()),
        Some(_) => return envelope_err("InvalidRequest", "request_id 必须是 1..=128 字节的字符串"),
    };
    let token = request_id.as_ref().map(|id| {
        let mut cancels = entry.cancels.lock().unwrap_or_else(|e| e.into_inner());
        cancels.entry(id.clone()).or_default().clone()
    });

    let outcome = match &token {
        Some(token) => pollster::block_on(entry.translator.translate_cancellable(&req, token)),
        None => pollster::block_on(entry.translator.translate(&req)),
    };
    if let Some(id) = &request_id {
        entry
            .cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    match outcome {
        Ok(out) => envelope_ok(json!({
            "text": out.text,
            "source": out.source.as_str(),
            "target": out.target.as_str(),
            "engine_id": out.engine_id,
            "model_id": out.model_id,
            "elapsed_ms": out.elapsed_ms,
        })),
        Err(e) => envelope_sdk_err(&e),
    }
}

/// 取消一个在飞/尚未开始的请求。幂等；取消不存在的 id 也返回成功。
fn call_cancel(entry: &Entry, payload: &Value) -> Value {
    let Some(request_id) = payload
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 128)
    else {
        return envelope_err("InvalidRequest", "cancel 需要 1..=128 字节的 request_id");
    };
    let mut cancels = entry.cancels.lock().unwrap_or_else(|e| e.into_inner());
    let inflight = cancels.contains_key(request_id);
    // 上限：取消一个已经结束的 id 会留下一个永远不会被消费的令牌，
    // 不允许它无限增长。超过上限时清掉一个最旧的（HashMap 无序，仅作兜底）。
    if !inflight && cancels.len() >= 64 {
        if let Some(key) = cancels.keys().next().cloned() {
            cancels.remove(&key);
        }
    }
    let token = cancels.entry(request_id.to_string()).or_default().clone();
    let already = token.is_cancelled();
    token.cancel();
    envelope_ok(json!({
        "request_id": request_id,
        "cancelled": true,
        "already_cancelled": already,
        "inflight": inflight,
    }))
}

fn call_resources(config: &Value) -> Value {
    let Some(repo) = config.get("resource_repo").and_then(Value::as_str) else {
        // 没配仓库就如实返回空清单，不做假数据。
        return envelope_ok(json!({ "items": [], "note": "resource_repo_not_configured" }));
    };
    match translator_resource::ResourceStore::open(repo) {
        Ok(store) => match store.list() {
            Ok(items) => envelope_ok(json!({
                "items": items
                    .iter()
                    .map(|r| json!({
                        "id": r.id,
                        "active": r.active,
                        "history": r.history,
                        "versions": r.versions.iter().map(|v| json!({
                            "id": v.id, "version": v.version,
                            "payload_schema": v.payload_schema, "active": v.active,
                        })).collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>(),
            })),
            Err(e) => envelope_err("ResourceNotInstalled", format!("{e}")),
        },
        Err(e) => envelope_err("ResourceNotInstalled", format!("{e}")),
    }
}

// ============================================================
// FFI：把 Result<Value, ()> 收成 jstring
// ============================================================

fn to_jstring(env: &mut JNIEnv, value: &Value) -> jstring {
    let text = serde_json::to_string(value).unwrap_or_else(|e| {
        format!(
            r#"{{"protocol_version":1,"ok":false,"error":{{"code":"BackendFailed","message":"序列化失败：{e}"}}}}"#
        )
    });
    match env.new_string(text) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// 统一入口：取输入字符串 → 执行 → 输出 JSON。panic 在这里被拦住。
fn guarded_call<F>(env: &mut JNIEnv, body: F) -> jstring
where
    F: FnOnce(&mut JNIEnv) -> Value,
{
    let result = catch_unwind(AssertUnwindSafe(|| body(env)));

    match result {
        Ok(value) => to_jstring(env, &value),
        Err(_) => {
            let value = envelope_err(
                "BackendFailed",
                "native panic：已在 JNI 边界拦截（不应发生，请附日志报告）",
            );
            to_jstring(env, &value)
        }
    }
}

// ============================================================
// 导出符号（类名与包名冻结，见 docs/39 §1.3）
// ============================================================

/// `nativeProtocolVersion()` → 契约版本（整数）。
#[no_mangle]
pub extern "system" fn Java_ai_ridge_translator_NativeBridge_nativeProtocolVersion(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    PROTOCOL_VERSION as jint
}

/// `nativeCreate(configJson)` → `{"ok":true,"data":{"handle":"g:i"}}` 或错误信封。
#[no_mangle]
pub extern "system" fn Java_ai_ridge_translator_NativeBridge_nativeCreate(
    mut env: JNIEnv,
    _class: JClass,
    config: JString,
) -> jstring {
    guarded_call(&mut env, move |env| {
        let raw: String = match env.get_string(&config) {
            Ok(s) => s.into(),
            Err(e) => return envelope_err("InvalidRequest", format!("读 config 失败：{e}")),
        };
        let config: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return envelope_err("InvalidRequest", format!("config 不是合法 JSON：{e}")),
        };
        match config.get("protocol_version").and_then(Value::as_i64) {
            Some(v) if v == PROTOCOL_VERSION => {}
            Some(v) => {
                return envelope_err(
                    "InvalidRequest",
                    format!("protocol_version 不兼容：客户端 {v}，本端 {PROTOCOL_VERSION}"),
                )
            }
            None => return envelope_err("InvalidRequest", "config 缺少 protocol_version"),
        }

        let translator = match build_translator(&config) {
            Ok(t) => Arc::new(t),
            Err(e) => return e,
        };

        let handle = {
            let mut reg = match registry().lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            match reg.insert(translator, config.clone()) {
                Ok(handle) => handle,
                Err(_) => {
                    return envelope_err("OutOfBudget", "同时存活的 Translator 实例已达到上限")
                }
            }
        };

        json!({
            "protocol_version": PROTOCOL_VERSION,
            "ok": true,
            "data": { "handle": handle, "platform": platform_name() }
        })
    })
}

/// `nativeCall(handle, method, payloadJson)`。
#[no_mangle]
pub extern "system" fn Java_ai_ridge_translator_NativeBridge_nativeCall(
    mut env: JNIEnv,
    _class: JClass,
    handle: JString,
    method: JString,
    payload: JString,
) -> jstring {
    guarded_call(&mut env, move |env| {
        let handle_raw: String = match env.get_string(&handle) {
            Ok(s) => s.into(),
            Err(e) => return envelope_err("InvalidRequest", format!("读 handle 失败：{e}")),
        };
        let method_raw: String = match env.get_string(&method) {
            Ok(s) => s.into(),
            Err(e) => return envelope_err("InvalidRequest", format!("读 method 失败：{e}")),
        };
        let payload_raw: String = match env.get_string(&payload) {
            Ok(s) => s.into(),
            Err(e) => return envelope_err("InvalidRequest", format!("读 payload 失败：{e}")),
        };

        if parse_handle(&handle_raw).is_none() {
            return envelope_err("InvalidRequest", "handle 格式非法（应为 generation:index）");
        }

        // 只在锁内做查表与 Arc::clone；SDK 调用一律在锁外（docs/38 §4）。
        let entry = {
            let reg = match registry().lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            match reg.resolve(&handle_raw) {
                Ok(entry) => entry,
                Err(HandleError::Invalid) => {
                    return envelope_err(
                        "InvalidRequest",
                        "handle 格式非法（应为 generation:index）",
                    )
                }
                Err(HandleError::Stale) => {
                    return envelope_err("AlreadyClosed", "句柄代际与实例不匹配（已关闭或伪造）")
                }
                Err(HandleError::Closed) => return envelope_err("AlreadyClosed", "句柄已关闭"),
            }
        };

        match method_raw.as_str() {
            "capabilities" => call_capabilities(&entry.translator),
            "detect" => match parse_payload(&payload_raw) {
                Ok(payload) => call_detect(&entry.translator, &payload),
                Err(e) => e,
            },
            "lookup" => match parse_payload(&payload_raw) {
                Ok(payload) => call_lookup(&entry.translator, &payload),
                Err(e) => e,
            },
            "translate" => match parse_payload(&payload_raw) {
                Ok(payload) => call_translate(&entry, &payload),
                Err(e) => e,
            },
            "cancel" => match parse_payload(&payload_raw) {
                Ok(payload) => call_cancel(&entry, &payload),
                Err(e) => e,
            },
            "resources" => call_resources(&entry.config),
            other => envelope_err(
                "InvalidRequest",
                format!(
                    "未知 method：{other}（v1 支持 capabilities/detect/lookup/translate/cancel/resources）"
                ),
            ),
        }
    })
}

/// `nativeClose(handle)`：幂等；在飞请求持有自己的 `Arc`，不会被提前释放。
#[no_mangle]
pub extern "system" fn Java_ai_ridge_translator_NativeBridge_nativeClose(
    mut env: JNIEnv,
    _class: JClass,
    handle: JString,
) -> jstring {
    guarded_call(&mut env, move |env| {
        let handle_raw: String = match env.get_string(&handle) {
            Ok(s) => s.into(),
            Err(e) => return envelope_err("InvalidRequest", format!("读 handle 失败：{e}")),
        };
        if parse_handle(&handle_raw).is_none() {
            return envelope_err("InvalidRequest", "handle 格式非法（应为 generation:index）");
        }

        let removed = {
            let mut reg = match registry().lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            reg.close(&handle_raw)
        };

        match removed {
            Some(entry) => {
                // 先标记关闭：新请求在入口就被拒；在飞请求继续持有 Arc。
                entry.closed.store(true, Ordering::SeqCst);
                let closed_again = Arc::strong_count(&entry) > 1;
                drop(entry);
                json!({
                    "protocol_version": PROTOCOL_VERSION,
                    "ok": true,
                    "data": { "closed": true, "inflight_kept": closed_again }
                })
            }
            None => json!({
                "protocol_version": PROTOCOL_VERSION,
                "ok": true,
                "data": { "closed": true, "already_closed": true }
            }),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 句柄解析拒绝非法格式() {
        assert_eq!(parse_handle("1:0"), Some((1, 0)));
        assert_eq!(parse_handle("1:0:2"), None);
        assert_eq!(parse_handle("x:0"), None);
        assert_eq!(parse_handle("10"), None);
    }

    #[test]
    fn 错误信封带协议版本与错误码() {
        let v = envelope_err("InvalidRequest", "文本为空");
        assert_eq!(v["protocol_version"], json!(PROTOCOL_VERSION));
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["error"]["code"], json!("InvalidRequest"));
    }

    #[test]
    fn 成功信封形状稳定() {
        let v = envelope_ok(json!({ "items": [] }));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"]["items"], json!([]));
    }

    #[test]
    fn j10_超长文本与payload在上限处拒绝() {
        let long = "a".repeat(MAX_TEXT_BYTES + 1);
        let err = payload_text(&json!({ "text": long })).unwrap_err();
        assert_eq!(err["error"]["code"], "InvalidRequest");
        assert!(err["error"]["message"].as_str().unwrap().contains("上限"));

        let ok = "a".repeat(MAX_TEXT_BYTES);
        assert_eq!(
            payload_text(&json!({ "text": ok })).unwrap().len(),
            MAX_TEXT_BYTES
        );

        let raw = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        let err = parse_payload(&raw).unwrap_err();
        assert!(err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("payload 超过上限"));
    }

    #[test]
    fn j10_控制字符明确拒绝但保留换行() {
        for bad in ["a\u{1}b", "a\u{7f}b"] {
            let err = payload_text(&json!({ "text": bad })).unwrap_err();
            assert!(
                err["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("控制字符"),
                "{err}"
            );
        }
        assert!(payload_text(&json!({ "text": "a\nb\tc\r" })).is_ok());
    }

    #[test]
    fn j10_畸形json与深嵌套受控失败() {
        assert!(parse_payload("{not json").is_err());
        assert!(parse_payload("[1,2,3]").is_err());
        let deep = format!("{}1{}", "[".repeat(200), "]".repeat(200));
        let err = parse_payload(&deep).unwrap_err();
        assert_eq!(err["error"]["code"], "InvalidRequest");
    }

    fn empty_registry() -> Registry {
        Registry {
            generation: 0,
            next_index: 0,
            entries: HashMap::new(),
        }
    }

    fn dummy_translator() -> Translator {
        Translator::builder()
            .build()
            .expect("空 Translator 应能构建")
    }

    #[test]
    fn m01_两个实例可以同时存活() {
        let mut reg = empty_registry();
        let a = reg.insert(Arc::new(dummy_translator()), json!({ "name": "a" }));
        let b = reg.insert(Arc::new(dummy_translator()), json!({ "name": "b" }));
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(a, "1:0");
        assert_eq!(b, "2:1");
        assert!(reg.resolve(&a).is_ok(), "创建 B 不得让 A 失效");
        assert!(reg.resolve(&b).is_ok());
    }

    #[test]
    fn m03_m04_关闭只影响目标实例且索引不复用() {
        let mut reg = empty_registry();
        let a = reg.insert(Arc::new(dummy_translator()), json!({})).unwrap();
        let b = reg.insert(Arc::new(dummy_translator()), json!({})).unwrap();
        let closed = reg.close(&a).expect("A 应被关闭");
        assert!(closed.closed.load(Ordering::SeqCst));
        assert_eq!(reg.resolve(&a).err().unwrap(), HandleError::Closed);
        assert!(reg.resolve(&b).is_ok(), "关闭 A 不得影响 B");

        let c = reg.insert(Arc::new(dummy_translator()), json!({})).unwrap();
        assert_eq!(c, "3:2", "index 单调、不复用");
        assert_eq!(reg.resolve(&a).err().unwrap(), HandleError::Closed);
        assert!(reg.resolve(&b).is_ok());
        assert!(reg.resolve(&c).is_ok());
    }

    #[test]
    fn m06_伪造或错代句柄受控拒绝() {
        let mut reg = empty_registry();
        let a = reg.insert(Arc::new(dummy_translator()), json!({})).unwrap();
        assert_eq!(
            reg.resolve("not-a-handle").err().unwrap(),
            HandleError::Invalid
        );
        assert_eq!(reg.resolve("9999:0").err().unwrap(), HandleError::Stale);
        // 用错误的 gen 关闭不得误伤真实 entry
        assert!(reg.close("9999:0").is_none());
        assert!(reg.resolve(&a).is_ok());
        assert_eq!(reg.resolve("1:9999").err().unwrap(), HandleError::Closed);
    }

    #[test]
    fn 平台名带架构() {
        let p = platform_name();
        assert!(p.contains('-'), "{p}");
    }
}
