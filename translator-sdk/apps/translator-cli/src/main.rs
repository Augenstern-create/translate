//! 翻译 SDK 的命令行客户端。
//!
//! 这是"最小可用界面"（§18 Phase 6）：用户不需要理解内部 crate，也不需要写代码。
//!
//! ```text
//! translator-cli translate "Hello, world."
//! translator-cli translate --file input.txt --json
//! translator-cli lookup hello --limit 3
//! translator-cli config check
//! translator-cli doctor
//! translator-cli usage
//! ```
//!
//! # 默认是"真的翻译"
//!
//! 旧 CLI 默认装 Mock 插件，所以看起来能跑、其实没有翻译任何东西。现在默认读
//! `translate.toml` 并调用真实后端；没有配置就**明确报错**，不再给出假结果。
//!
//! 配置查找顺序：
//!
//! 1. `--config <path>`
//! 2. 环境变量 `TRANSLATE_CONFIG`
//! 3. 当前目录的 `translate.toml`
//!
//! # 安全
//!
//! - 密钥只从环境变量读取（`api_key_env`），配置文件里不出现；
//! - 不把原文写进任何日志；`--json` 输出由调用方决定是否保存。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;
use translator_api::{Translator, TranslatorBuilder};
use translator_cli::serve;
use translator_http::{BackendConfig, BackendsFile, HttpSettings};
use translator_types::{
    LanguageTag, LookupRequest, TextKind, TranslateRequest, TranslationError, TranslationMode,
};

const EXIT_OK: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(Failure::Usage(msg)) => {
            eprintln!("用法错误：{msg}");
            eprintln!();
            print_help();
            ExitCode::from(EXIT_USAGE)
        }
        Err(Failure::App(err)) => {
            eprintln!("错误：{err}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

enum Failure {
    Usage(String),
    App(String),
}

impl From<TranslationError> for Failure {
    fn from(err: TranslationError) -> Self {
        Failure::App(format!("{}（{}）", err.message, err.code))
    }
}

impl From<translator_http::HttpError> for Failure {
    fn from(err: translator_http::HttpError) -> Self {
        Failure::App(err.to_string())
    }
}

impl From<std::io::Error> for Failure {
    fn from(err: std::io::Error) -> Self {
        Failure::App(format!("IO 错误：{err}"))
    }
}

fn run(args: &[String]) -> Result<u8, Failure> {
    let Some(command) = args.first().map(String::as_str) else {
        print_help();
        return Ok(EXIT_USAGE);
    };

    let global = GlobalArgs::parse(&args[1..])?;
    match command {
        "translate" | "t" => cmd_translate(&global, &args[1..]),
        "lookup" | "l" => cmd_lookup(&global, &args[1..]),
        "config" => cmd_config(&global, &args[1..]),
        "doctor" => cmd_doctor(&global),
        "usage" => cmd_usage(&global),
        "serve" => cmd_serve(&global, &args[1..]),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(EXIT_OK)
        }
        "version" | "--version" | "-V" => {
            println!("translator-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(EXIT_OK)
        }
        other => Err(Failure::Usage(format!("未知命令：{other}"))),
    }
}

fn print_help() {
    println!(
        "translator-cli {} — 英译中翻译与词典查询

用法：
  translator-cli [--config <path>] <命令> [参数]

命令：
  translate <文本>            翻译一段文本
  translate --file <路径>     翻译文件内容
  translate --stdin           翻译标准输入
  lookup <词>                 查询词典
  config check                校验配置文件
  config show                 显示生效配置（密钥永远不显示）
  doctor                      环境自检（配置/密钥/词典/提示词）
  usage                       显示本次会话的用量与估算费用
  serve [--port 8765]         启动本地翻译页面（仅监听 127.0.0.1）
  help                        显示本帮助

通用选项：
  --config <path>             配置文件（默认 translate.toml，或环境变量 TRANSLATE_CONFIG）
  --json                      以 JSON 输出
  --mode precise|fast         翻译模式（默认 precise）

translate 选项：
  --text-kind sentence|word|paragraph|auto   文本类型
  --timeout <毫秒>            超时覆盖
  （术语表/模板/词典由配置文件决定，不通过命令行临时指定）

lookup 选项：
  --limit <n>                 最多返回条目数（默认 5）
  --lang <tag>                词典语言（默认 en）",
        env!("CARGO_PKG_VERSION")
    );
}

// ============================================================
// 参数
// ============================================================

struct GlobalArgs {
    config: Option<PathBuf>,
    json: bool,
}

impl GlobalArgs {
    fn parse(args: &[String]) -> Result<Self, Failure> {
        let mut config = None;
        let mut json = false;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--config" => {
                    let value = args
                        .get(i + 1)
                        .ok_or_else(|| Failure::Usage("--config 需要路径".to_string()))?;
                    config = Some(PathBuf::from(value));
                    i += 2;
                }
                "--json" => {
                    json = true;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        Ok(Self { config, json })
    }

    fn config_path(&self) -> PathBuf {
        if let Some(path) = &self.config {
            return path.clone();
        }
        if let Ok(path) = std::env::var("TRANSLATE_CONFIG") {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
        PathBuf::from("translate.toml")
    }
}

// ============================================================
// 配置文件
// ============================================================

/// CLI 侧的完整配置：后端 + 术语表 + 模板 + 词典。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    http: HttpSettings,
    #[serde(default)]
    backend: BTreeMap<String, BackendConfig>,
    #[serde(default)]
    translation: TranslationSection,
    #[serde(default)]
    cache: CacheSection,
    /// 术语表 JSON 路径。
    #[serde(default)]
    glossary: Option<PathBuf>,
    /// 模板 JSON 路径。
    #[serde(default)]
    templates: Option<PathBuf>,
    /// 词典包根目录。
    #[serde(default)]
    dictionary: Option<PathBuf>,
    /// 规则包路径。
    #[serde(default)]
    rules: Option<PathBuf>,
    /// 模板作用域：`app` / `page`。
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    page: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TranslationSection {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_input_chars: Option<usize>,
    /// 一次逻辑请求允许的物理 HTTP 调用数（分段与重试共享）。
    #[serde(default)]
    max_http_calls_per_request: Option<u32>,
    #[serde(default)]
    strict_integrity: Option<bool>,
    /// `disabled` / `on-local-unavailable` / `on-local-failure`
    #[serde(default)]
    remote_fallback: Option<String>,
    /// 默认后端 id（`remote` / `local`，或完整 `http.xxx`）。
    #[serde(default)]
    preferred_backend: Option<String>,
    /// 隐私隔离域（不同域不共享缓存）。
    #[serde(default)]
    privacy_scope: Option<String>,
    /// 术语策略：`off` / `warn` / `enforce`。
    #[serde(default)]
    term_policy: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheSection {
    #[serde(default)]
    capacity: Option<usize>,
    /// 把缓存持久化到该文件（会写入译文，默认关闭）。
    #[serde(default)]
    file: Option<PathBuf>,
}

fn load_file_config(path: &Path) -> Result<FileConfig, Failure> {
    if !path.exists() {
        return Err(Failure::App(format!(
            "配置文件不存在：{}。请复制 docs/development.md 里的示例，或指定 --config",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path)?;
    let mut config: FileConfig =
        toml::from_str(&text).map_err(|e| Failure::App(format!("配置文件解析失败：{e}")))?;
    // 用 map 的键补齐空 id
    for (name, backend) in config.backend.iter_mut() {
        if backend.id.trim().is_empty() {
            backend.id = name.clone();
        }
    }
    Ok(config)
}

fn build_translator(config: &FileConfig) -> Result<Translator, Failure> {
    let mut builder = Translator::builder();

    let backends = BackendsFile {
        http: config.http.clone(),
        backend: config.backend.clone(),
    };
    builder = builder.http_backends(&backends)?;

    if let Some(mode) = &config.translation.mode {
        builder = builder.translation_mode(parse_mode(mode)?);
    }
    if let Some(timeout) = config.translation.timeout_ms {
        builder = builder.timeout_ms(timeout);
    }
    if let Some(chars) = config.translation.max_input_chars {
        builder = builder.max_input_chars(chars);
    }
    if let Some(calls) = config.translation.max_http_calls_per_request {
        builder = builder.max_http_calls_per_request(calls);
    }
    // [http] 段：全局并发与队列容量（0/缺省表示用安全默认值）
    if config.http.max_concurrent_requests > 0 {
        builder = builder.http_global_concurrency(config.http.max_concurrent_requests);
    }
    if config.http.max_queue_size > 0 {
        builder = builder.http_queue_size(config.http.max_queue_size);
    }
    if let Some(strict) = config.translation.strict_integrity {
        builder = builder.strict_integrity(strict);
    }
    if let Some(policy) = &config.translation.remote_fallback {
        builder = builder.remote_fallback(parse_fallback(policy)?);
    }
    // 只有一个启用后端时自动设为默认：用户既然写明了 base_url，
    // 就等同于授权使用它（否则单后端配置会因为"远程未授权"直接不可用）。
    let default_backend = config
        .backend
        .iter()
        .find(|(_, backend)| backend.enabled)
        .map(|(name, _)| name.clone());
    match (&config.translation.preferred_backend, default_backend) {
        (Some(backend), _) => builder = builder.preferred_backend(normalize_backend_id(backend)),
        (None, Some(name)) => builder = builder.preferred_backend(normalize_backend_id(&name)),
        (None, None) => {}
    }
    if let Some(scope) = &config.translation.privacy_scope {
        builder = builder.privacy_scope(scope.clone());
    }
    if let Some(policy) = &config.translation.term_policy {
        builder = builder.term_policy(parse_term_policy(policy)?);
    }
    if let Some(capacity) = config.cache.capacity {
        builder = builder.cache_capacity(capacity);
    }
    if let Some(path) = &config.cache.file {
        builder = builder.cache_file(path.clone());
    }
    if let Some(path) = &config.glossary {
        builder = builder.glossary_file(path)?;
    }
    if let Some(path) = &config.templates {
        builder = builder.templates_file(path)?;
    }
    if let Some(path) = &config.rules {
        builder = builder.rules_file(path)?;
    }
    if let Some(root) = &config.dictionary {
        builder = builder.dictionary_package(root.clone())?;
    }
    match (&config.app, &config.page) {
        (Some(app), Some(page)) => {
            builder = builder.template_scope(translator_api::TemplateScope::app_page(
                app.clone(),
                page.clone(),
            ))
        }
        (Some(app), None) => {
            builder = builder.template_scope(translator_api::TemplateScope::app(app.clone()))
        }
        _ => {}
    }

    builder.build().map_err(Failure::from)
}

fn normalize_backend_id(id: &str) -> String {
    if id.starts_with("http.") || id.starts_with("dict.") || id.starts_with("template:") {
        id.to_string()
    } else {
        format!("http.{id}")
    }
}

fn parse_mode(value: &str) -> Result<TranslationMode, Failure> {
    match value.to_ascii_lowercase().as_str() {
        "precise" | "p" => Ok(TranslationMode::Precise),
        "fast" | "f" => Ok(TranslationMode::Fast),
        other => Err(Failure::Usage(format!(
            "未知模式 {other}（可选 precise / fast）"
        ))),
    }
}

fn parse_text_kind(value: &str) -> Result<TextKind, Failure> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Ok(TextKind::Auto),
        "sentence" => Ok(TextKind::Sentence),
        "word" | "phrase" => Ok(TextKind::Word),
        "paragraph" => Ok(TextKind::Paragraph),
        "document" => Ok(TextKind::Document),
        other => Err(Failure::Usage(format!("未知文本类型 {other}"))),
    }
}

fn parse_fallback(value: &str) -> Result<translator_types::RemoteFallbackPolicy, Failure> {
    use translator_types::RemoteFallbackPolicy as P;
    match value.to_ascii_lowercase().as_str() {
        "disabled" | "off" => Ok(P::Disabled),
        "on-local-unavailable" | "on_unavailable" => Ok(P::OnLocalUnavailable),
        "on-local-failure" | "on_failure" => Ok(P::OnLocalFailure),
        other => Err(Failure::Usage(format!("未知远程回退策略 {other}"))),
    }
}

fn parse_term_policy(value: &str) -> Result<translator_api::TermPolicy, Failure> {
    use translator_api::TermPolicy as P;
    match value.to_ascii_lowercase().as_str() {
        "off" => Ok(P::Off),
        "warn" => Ok(P::Warn),
        "enforce" => Ok(P::Enforce),
        other => Err(Failure::Usage(format!("未知术语策略 {other}"))),
    }
}

// ============================================================
// translate
// ============================================================

fn cmd_translate(global: &GlobalArgs, args: &[String]) -> Result<u8, Failure> {
    let mut text: Option<String> = None;
    let mut file: Option<PathBuf> = None;
    let mut stdin = false;
    let mut mode: Option<TranslationMode> = None;
    let mut kind = TextKind::Sentence;
    let mut timeout: Option<u64> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--file" | "-f" => {
                file =
                    Some(PathBuf::from(args.get(i + 1).ok_or_else(|| {
                        Failure::Usage("--file 需要路径".to_string())
                    })?));
                i += 2;
            }
            "--stdin" => {
                stdin = true;
                i += 1;
            }
            "--mode" => {
                mode =
                    Some(parse_mode(args.get(i + 1).ok_or_else(|| {
                        Failure::Usage("--mode 需要值".to_string())
                    })?)?);
                i += 2;
            }
            "--text-kind" => {
                kind = parse_text_kind(
                    args.get(i + 1)
                        .ok_or_else(|| Failure::Usage("--text-kind 需要值".to_string()))?,
                )?;
                i += 2;
            }
            "--timeout" => {
                timeout = Some(
                    args.get(i + 1)
                        .ok_or_else(|| Failure::Usage("--timeout 需要值".to_string()))?
                        .parse()
                        .map_err(|_| Failure::Usage("--timeout 必须是毫秒整数".to_string()))?,
                );
                i += 2;
            }
            "--config" | "--json" => i += 2, // 已在全局参数里处理
            other if other.starts_with("--") => {
                return Err(Failure::Usage(format!("未知选项 {other}")))
            }
            other => {
                if text.is_none() {
                    text = Some(other.to_string());
                    i += 1;
                } else {
                    return Err(Failure::Usage("只能翻译一段文本，请加引号".to_string()));
                }
            }
        }
    }

    let source_text = if let Some(path) = file {
        std::fs::read_to_string(&path)?
    } else if stdin {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        buffer
    } else {
        text.ok_or_else(|| Failure::Usage("缺少要翻译的文本".to_string()))?
    };

    if source_text.trim().is_empty() {
        return Err(Failure::Usage("文本为空".to_string()));
    }

    let config = load_file_config(&global.config_path())?;
    let translator = build_translator(&config)?;

    let zh = LanguageTag::new("zh-CN")?;
    let mut request =
        TranslateRequest::new(source_text, zh.clone()).with_source(LanguageTag::new("en")?);
    if let Some(timeout) = timeout {
        request = request.with_timeout_ms(timeout);
    }

    let mut options = translator.default_options().clone();
    options.text_kind = kind;
    if let Some(mode) = mode {
        options.mode = mode;
    }

    let started = std::time::Instant::now();
    let output =
        pollster::block_on(translator.translate_with_diagnostics_options(&request, &options))
            .map_err(|e| Failure::App(format!("翻译失败：{}（{}）", e.message, e.code)))?;
    let elapsed = started.elapsed();
    let result = &output.result;
    let diagnostics = &output.diagnostics;

    if global.json {
        let value = serde_json::json!({
            "text": result.text,
            "source": result.source.as_str(),
            "target": result.target.as_str(),
            "engine_id": result.engine_id,
            "model_id": result.model_id,
            "elapsed_ms": result.elapsed_ms,
            "cached": diagnostics.cache_hit,
            "attempts": diagnostics.attempts,
            "http_calls": diagnostics.http_calls,
            "warnings": result.warnings.iter().map(|w| format!("{w:?}")).collect::<Vec<_>>(),
            "quality": diagnostics.quality.as_ref().map(|q| serde_json::json!({
                "issues": q.issues.len(),
                "fatal": q.has_fatal(),
                "summary": q.summary(),
            })),
            "protection": diagnostics.protection_strategy,
            "route": diagnostics.route.as_str(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
    } else {
        println!("{}", result.text);
        eprintln!(
            "— 引擎 {} · {} ms · {}",
            result.engine_id,
            elapsed.as_millis(),
            diagnostics
                .quality
                .as_ref()
                .map(|q| q.summary())
                .unwrap_or_default()
        );
        for warning in &result.warnings {
            eprintln!("⚠ {warning:?}");
        }
    }
    Ok(EXIT_OK)
}

// ============================================================
// lookup
// ============================================================

fn cmd_lookup(global: &GlobalArgs, args: &[String]) -> Result<u8, Failure> {
    let mut term: Option<String> = None;
    let mut limit = 5usize;
    let mut lang = "en".to_string();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--limit" => {
                limit = args
                    .get(i + 1)
                    .ok_or_else(|| Failure::Usage("--limit 需要值".to_string()))?
                    .parse()
                    .map_err(|_| Failure::Usage("--limit 必须是整数".to_string()))?;
                i += 2;
            }
            "--lang" => {
                lang = args
                    .get(i + 1)
                    .ok_or_else(|| Failure::Usage("--lang 需要值".to_string()))?
                    .clone();
                i += 2;
            }
            "--config" | "--json" => i += 2,
            other if other.starts_with("--") => {
                return Err(Failure::Usage(format!("未知选项 {other}")))
            }
            other => {
                if term.is_none() {
                    term = Some(other.to_string());
                    i += 1;
                } else {
                    return Err(Failure::Usage("只能查询一个词".to_string()));
                }
            }
        }
    }

    let term = term.ok_or_else(|| Failure::Usage("缺少要查询的词".to_string()))?;
    let config = load_file_config(&global.config_path())?;
    let translator = build_translator(&config)?;

    let mut request = LookupRequest::new(term.clone());
    if let Ok(tag) = LanguageTag::new(&lang) {
        request = request.with_language(tag);
    }
    request = request.with_limit(limit);

    let entries = pollster::block_on(translator.lookup(&request))
        .map_err(|e| Failure::App(format!("查词失败：{}（{}）", e.message, e.code)))?;

    if global.json {
        let value: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "headword": entry.headword,
                    "source_id": entry.source_id,
                    "confidence": entry.confidence,
                    "senses": entry.senses.iter().map(|s| serde_json::json!({
                        "definition": s.definition,
                        "source_id": s.source_id,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
    } else if entries.is_empty() {
        println!("（未找到「{term}」）");
    } else {
        for entry in &entries {
            println!("{}  [{}]", entry.headword, entry.source_id);
            for (idx, sense) in entry.senses.iter().enumerate() {
                println!("  {}. {}", idx + 1, sense.definition);
            }
        }
    }
    Ok(EXIT_OK)
}

// ============================================================
// config / doctor / usage
// ============================================================

fn cmd_config(global: &GlobalArgs, args: &[String]) -> Result<u8, Failure> {
    let sub = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str);
    match sub {
        Some("check") => {
            let path = global.config_path();
            let config = load_file_config(&path)?;
            // 真正走一遍装配：配置错误、密钥缺失、提示词缺失都会在这里暴露
            let translator = build_translator(&config)?;
            if global.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "config": path.display().to_string(),
                        "summary": translator.config_summary(),
                        "backends": translator.backend_ids(),
                    })
                );
            } else {
                println!("✓ 配置有效：{}", path.display());
                println!("  {}", translator.config_summary());
            }
            Ok(EXIT_OK)
        }
        Some("show") => {
            let path = global.config_path();
            let config = load_file_config(&path)?;
            println!("配置文件：{}", path.display());
            for (name, backend) in &config.backend {
                println!(
                    "  后端 {name}: enabled={} model={} revision={} endpoint={} key_env={}",
                    backend.enabled,
                    backend.model,
                    backend.model_revision.as_deref().unwrap_or("<未设置>"),
                    backend.endpoint(),
                    backend
                        .api_key_env
                        .as_deref()
                        .unwrap_or("TRANSLATE_API_KEY"),
                );
            }
            println!("  术语表：{:?}", config.glossary);
            println!("  模板：{:?}", config.templates);
            println!("  词典：{:?}", config.dictionary);
            println!("  规则：{:?}", config.rules);
            Ok(EXIT_OK)
        }
        _ => Err(Failure::Usage(
            "config 需要子命令：check | show".to_string(),
        )),
    }
}

fn cmd_doctor(global: &GlobalArgs) -> Result<u8, Failure> {
    let path = global.config_path();
    println!("配置文件：{}", path.display());
    let mut problems = 0usize;

    let config = match load_file_config(&path) {
        Ok(config) => {
            println!("  ✓ 可读取并解析");
            config
        }
        Err(err) => {
            let Failure::App(msg) = err else {
                return Err(err);
            };
            println!("  ✗ {msg}");
            return Ok(EXIT_ERROR);
        }
    };

    if config.backend.is_empty() {
        println!("  ✗ 没有配置任何 [backend.<name>]");
        problems += 1;
    }
    for (name, backend) in &config.backend {
        if !backend.enabled {
            println!("  · 后端 {name} 已禁用");
            continue;
        }
        if let Err(err) = backend.validate() {
            println!("  ✗ 后端 {name} 配置非法：{err}");
            problems += 1;
            continue;
        }
        println!("  ✓ 后端 {name} → {}", backend.endpoint());
        match backend.resolve_api_key() {
            Ok(Some(_)) => println!("    ✓ 密钥已从环境变量读取"),
            Ok(None) => println!("    · 未配置密钥（本地服务正常）"),
            Err(err) => {
                println!("    ✗ {err}");
                problems += 1;
            }
        }
        if backend.model_revision.is_none() {
            println!("    ! 未设置 model_revision：缓存无法区分模型替换");
        }
    }

    for (label, path) in [
        ("术语表", config.glossary.as_ref()),
        ("模板", config.templates.as_ref()),
        ("词典", config.dictionary.as_ref()),
        ("规则", config.rules.as_ref()),
    ] {
        if let Some(path) = path {
            if path.exists() {
                println!("  ✓ {label}：{}", path.display());
            } else {
                println!("  ✗ {label} 不存在：{}", path.display());
                problems += 1;
            }
        }
    }

    if problems == 0 {
        println!("结论：未发现问题");
        Ok(EXIT_OK)
    } else {
        println!("结论：{problems} 个问题需要处理");
        Ok(EXIT_ERROR)
    }
}

fn cmd_serve(global: &GlobalArgs, args: &[String]) -> Result<u8, Failure> {
    let mut port = 8765u16;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                port = args
                    .get(i + 1)
                    .ok_or_else(|| Failure::Usage("--port 需要值".to_string()))?
                    .parse()
                    .map_err(|_| Failure::Usage("--port 必须是 1-65535".to_string()))?;
                i += 2;
            }
            "--config" | "--json" => i += 2,
            other if other.starts_with("--") => {
                return Err(Failure::Usage(format!("未知选项 {other}")))
            }
            _ => i += 1,
        }
    }
    let config = load_file_config(&global.config_path())?;
    let translator = std::sync::Arc::new(build_translator(&config)?);
    serve::serve(translator, port).map_err(Failure::App)?;
    Ok(EXIT_OK)
}

fn cmd_usage(global: &GlobalArgs) -> Result<u8, Failure> {
    let config = load_file_config(&global.config_path())?;
    let translator = build_translator(&config)?;
    let usage = translator.usage();
    if global.json {
        let value: Vec<serde_json::Value> = usage
            .iter()
            .map(|(name, snapshot)| {
                serde_json::json!({
                    "backend": name,
                    "logical_requests": snapshot.logical_requests,
                    "http_calls_started": snapshot.http_calls_started,
                    "http_calls_completed": snapshot.http_calls_completed,
                    "transport_retries": snapshot.transport_retries,
                    "content_retries": snapshot.content_retries,
                    "failed_calls": snapshot.failed_calls,
                    "cancelled_requests": snapshot.cancelled_requests,
                    "potentially_billable_unknown_calls": snapshot
                        .potentially_billable_unknown_calls,
                    "input_tokens": snapshot.tokens.input_tokens,
                    "output_tokens": snapshot.tokens.output_tokens,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
    } else if usage.is_empty() {
        println!("没有 HTTP 后端，无用量可显示");
    } else {
        println!("注意：客户端计数只用于提醒，可信计费边界在服务端。");
        for (name, snapshot) in usage {
            println!("  后端 {name}: {}", snapshot.summary());
        }
        if let Some(cost) = translator.estimated_cost_usd() {
            println!("  估算费用：${cost:.4}（按配置单价；未配置单价的后端不计入）");
        }
    }
    Ok(EXIT_OK)
}

/// 供测试使用的库内入口：从 TOML 文本构建 translator。
///
/// 放在 CLI 里而不是 SDK 里，是为了让"配置格式"只有 CLI 一个解释者。
pub fn build_from_toml(text: &str) -> Result<Translator, String> {
    let mut config: FileConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    for (name, backend) in config.backend.iter_mut() {
        if backend.id.trim().is_empty() {
            backend.id = name.clone();
        }
    }
    build_translator(&config).map_err(|e| match e {
        Failure::App(msg) | Failure::Usage(msg) => msg,
    })
}

#[allow(dead_code)]
fn builder_for_tests() -> TranslatorBuilder {
    Translator::builder()
}
