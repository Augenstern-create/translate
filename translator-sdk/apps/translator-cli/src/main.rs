//! 调试 / 基准 / 资源包验证用的命令行工具（设计文档 §3 `apps/translator-cli`）。
//!
//! P0 阶段它只装 Mock 插件，用来证明：
//! **不接任何真实模型、不装任何 native 库，整条 SDK 链路依然可运行。**
//!
//! ```text
//! translator-cli translate "hello" --to zh-Hans
//! translator-cli translate "hello world" --to zh-Hans --explain
//! translator-cli lookup cat --lang en
//! translator-cli detect "你好"
//! translator-cli capabilities
//! ```

use std::process::ExitCode;

use translator_api::{
    mocks::{
        MockBackend, MockDictionaryEngine, MockDictionaryProvider, MockNmtEngine, MockRemoteEngine,
    },
    LanguageTag, LookupRequest, TranslateRequest, TranslationErrorCode, TranslationPolicy,
};

const USAGE: &str = "\
translator-cli —— 轻量跨平台翻译 SDK 调试工具（P0 / Mock 模式）

用法:
  translator-cli translate <文本> [--from <lang>] [--to <lang>] [--policy <p>] [--explain]
  translator-cli lookup <词条> [--lang <lang>] [--limit <n>]
  translator-cli detect <文本>
  translator-cli capabilities

策略 (--policy):
  auto         AutoLocalFirst（默认）
  dictionary   DictionaryOnly
  local        LocalOnly
  remote       RemoteOnly
  engine:<id>  指定引擎

示例:
  translator-cli translate hello --to zh-Hans
  translator-cli translate \"good morning\" --to zh-Hans --explain
  translator-cli translate 你好 --to en
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let translator = match build_translator() {
        Ok(t) => t,
        Err(err) => {
            eprintln!("插件注册失败：{err}");
            return ExitCode::FAILURE;
        }
    };

    match args[0].as_str() {
        "translate" => cmd_translate(&translator, &args[1..]),
        "lookup" => cmd_lookup(&translator, &args[1..]),
        "detect" => cmd_detect(&translator, &args[1..]),
        "capabilities" => cmd_capabilities(&translator),
        other => {
            eprintln!("未知子命令：{other}\n");
            print!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn build_translator() -> Result<translator_api::Translator, translator_api::TranslationError> {
    translator_api::Translator::builder()
        // 词典：只做精确词条
        .engine(MockDictionaryEngine::new())
        // 本地 NMT：模拟 Marian
        .engine(MockNmtEngine::new())
        // 远程 LLM：默认【无】联网授权，用于验证 ADR-004
        .engine(MockRemoteEngine::new(false))
        .provider(MockDictionaryProvider::new())
        .backend(MockBackend::new())
        .cache_capacity(64)
        .build()
}

fn cmd_translate(translator: &translator_api::Translator, args: &[String]) -> ExitCode {
    let mut text: Option<String> = None;
    let mut from: Option<String> = None;
    let mut to: Option<String> = None;
    let mut policy = TranslationPolicy::AutoLocalFirst;
    let mut explain = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from" => {
                i += 1;
                from = args.get(i).cloned();
            }
            "--to" => {
                i += 1;
                to = args.get(i).cloned();
            }
            "--policy" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("auto") => policy = TranslationPolicy::AutoLocalFirst,
                    Some("dictionary") => policy = TranslationPolicy::DictionaryOnly,
                    Some("local") => policy = TranslationPolicy::LocalOnly,
                    Some("remote") => policy = TranslationPolicy::RemoteOnly,
                    Some(other) if other.starts_with("engine:") => {
                        policy = TranslationPolicy::ExplicitEngine(
                            other.trim_start_matches("engine:").to_string(),
                        );
                    }
                    Some(other) => {
                        eprintln!("未知策略：{other}");
                        return ExitCode::from(2);
                    }
                    None => {
                        eprintln!("--policy 缺少取值");
                        return ExitCode::from(2);
                    }
                }
            }
            "--explain" => explain = true,
            flag if flag.starts_with("--") => {
                eprintln!("未知选项：{flag}");
                return ExitCode::from(2);
            }
            positional => {
                text = Some(match text.take() {
                    Some(prev) => format!("{prev} {positional}"),
                    None => positional.to_string(),
                });
            }
        }
        i += 1;
    }

    let Some(text) = text else {
        eprintln!("缺少待翻译文本\n");
        print!("{USAGE}");
        return ExitCode::from(2);
    };

    let target = match LanguageTag::new(to.as_deref().unwrap_or("zh-Hans")) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("目标语言非法：{err}");
            return ExitCode::from(2);
        }
    };

    let mut req = TranslateRequest::new(text, target).with_policy(policy);
    if let Some(from) = from {
        match LanguageTag::new(&from) {
            Ok(tag) => req = req.with_source(tag),
            Err(err) => {
                eprintln!("源语言非法：{err}");
                return ExitCode::from(2);
            }
        }
    }

    if explain {
        match translator.explain_route(&req) {
            Ok(d) => {
                println!("路由决策：");
                println!("  引擎      {}", d.engine_id);
                println!("  能力      {}", d.capability);
                println!("  理由      {}", d.reason);
                println!();
            }
            Err(err) => {
                eprintln!("路由失败：{err}");
                return ExitCode::FAILURE;
            }
        }
    }

    match pollster::block_on(translator.translate(&req)) {
        Ok(result) => {
            println!("译文    {}", result.text);
            println!("引擎    {}", result.engine_id);
            if let Some(model) = &result.model_id {
                println!("模型    {model}");
            }
            println!("语言    {} → {}", result.source, result.target);
            println!("耗时    {} ms", result.elapsed_ms);
            if !result.warnings.is_empty() {
                println!("警告    {:?}", result.warnings);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("翻译失败：{err}");
            // 用退出码区分"资源未安装"与其它错误，便于脚本判断
            if err.code == TranslationErrorCode::ResourceNotInstalled {
                ExitCode::from(3)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn cmd_lookup(translator: &translator_api::Translator, args: &[String]) -> ExitCode {
    let mut term: Option<String> = None;
    let mut lang: Option<String> = None;
    let mut limit = 10usize;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lang" => {
                i += 1;
                lang = args.get(i).cloned();
            }
            "--limit" => {
                i += 1;
                limit = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(10);
            }
            positional => term = Some(positional.to_string()),
        }
        i += 1;
    }

    let Some(term) = term else {
        eprintln!("缺少查询词条\n");
        return ExitCode::from(2);
    };

    let mut req = LookupRequest::new(term).with_limit(limit);
    if let Some(lang) = lang {
        match LanguageTag::new(&lang) {
            Ok(tag) => req = req.with_language(tag),
            Err(err) => {
                eprintln!("语言非法：{err}");
                return ExitCode::from(2);
            }
        }
    }

    match pollster::block_on(translator.lookup(&req)) {
        Ok(entries) if entries.is_empty() => {
            println!("未找到词条");
            ExitCode::SUCCESS
        }
        Ok(entries) => {
            for (idx, entry) in entries.iter().enumerate() {
                println!(
                    "[{}] {} ({})  置信度 {:.2}",
                    idx + 1,
                    entry.headword,
                    entry.language,
                    entry.confidence
                );
                for sense in &entry.senses {
                    println!("    - {}   [来源: {}]", sense.definition, sense.source_id);
                }
                println!("    许可: {}", entry.source_license);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("查词失败：{err}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_detect(translator: &translator_api::Translator, args: &[String]) -> ExitCode {
    let text = args.join(" ");
    if text.trim().is_empty() {
        eprintln!("缺少待识别文本\n");
        return ExitCode::from(2);
    }

    match pollster::block_on(translator.detect_language(&text)) {
        Ok(guesses) => {
            for g in guesses {
                println!("{}  置信度 {:.2}", g.language, g.confidence);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("识别失败：{err}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_capabilities(translator: &translator_api::Translator) -> ExitCode {
    let caps = translator.capabilities();
    if caps.is_empty() {
        println!("（没有已注册的引擎）");
        return ExitCode::SUCCESS;
    }

    println!("已注册引擎：");
    for info in caps {
        let names: Vec<&str> = info.capabilities.iter().map(|c| c.as_str()).collect();
        println!(
            "  {:<20} {:<40} 优先级 {:>4}  许可 {}",
            info.engine_id,
            names.join(", "),
            info.priority,
            info.license
        );
    }
    ExitCode::SUCCESS
}
