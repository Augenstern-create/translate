//! 调试 / 基准 / 资源包验证用的命令行工具（设计文档 §3 `apps/translator-cli`）。
//!
//! P0 阶段它只装 Mock 插件，用来证明：
//! **不接任何真实模型、不装任何 native 库，整条 SDK 链路依然可运行。**
//!
//! P1 起支持通过 `--dict` 挂载真实离线词典包（`pack-dictionary` 的产物）。
//!
//! ```text
//! translator-cli translate "hello" --to zh-Hans
//! translator-cli translate "hello world" --to zh-Hans --explain
//! translator-cli lookup cat --lang en
//! translator-cli detect "你好"
//! translator-cli --dict resources/zh-en translate 世界
//! translator-cli --rules fixtures/rules/minimal.json translate 机器人 --explain
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
translator-cli —— 轻量跨平台翻译 SDK 调试工具

用法:
  translator-cli [--dict <包目录>] <子命令> ...

子命令:
  translate <文本> [--from <lang>] [--to <lang>] [--policy <p>] [--explain]
             --to 缺省时按源语言推断：中文→en，其它→zh-Hans
  lookup <词条> [--lang <lang>] [--limit <n>]
  detect <文本>
  capabilities

全局选项:
  --dict <包目录>   挂载离线词典包（不指定则使用内置 Mock 词表）
  --rules <规则文件> 挂载规则包（JSON 或已编译的二进制）

策略 (--policy):
  auto         AutoLocalFirst（默认）
  dictionary   DictionaryOnly
  local        LocalOnly
  remote       RemoteOnly
  engine:<id>  指定引擎

示例:
  translator-cli translate hello
  translator-cli translate \"good morning\" --to zh-Hans --explain
  translator-cli --dict resources/zh-en translate 世界
  translator-cli --dict resources/zh-en lookup 计算机
  translator-cli --rules fixtures/rules/minimal.json translate 机器人 --explain
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    // 抽取全局选项 --dict / --rules
    let (dict, rules) = match take_global_options(&mut args) {
        Ok(pair) => pair,
        Err(code) => return code,
    };

    let translator = match build_translator(dict.as_deref(), rules.as_deref()) {
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

/// 抽出全局选项，剩下的交给子命令解析。
fn take_global_options(
    args: &mut Vec<String>,
) -> Result<(Option<String>, Option<String>), ExitCode> {
    let mut dict = None;
    let mut rules = None;

    for (flag, slot) in [("--dict", &mut dict), ("--rules", &mut rules)] {
        while let Some(pos) = args.iter().position(|a| a == flag) {
            if pos + 1 >= args.len() {
                eprintln!("{flag} 缺少路径参数");
                return Err(ExitCode::from(2));
            }
            *slot = Some(args[pos + 1].clone());
            args.drain(pos..=pos + 1);
        }
    }

    Ok((dict, rules))
}

fn build_translator(
    dict: Option<&str>,
    rules: Option<&str>,
) -> Result<translator_api::Translator, translator_api::TranslationError> {
    let mut builder = translator_api::Translator::builder()
        // 本地 NMT：模拟 Marian（真实模型在 P3 接入）
        .engine(MockNmtEngine::new())
        // 远程 LLM：默认【无】联网授权，用于验证 ADR-004
        .engine(MockRemoteEngine::new(false))
        .backend(MockBackend::new())
        .cache_capacity(64);

    if let Some(path) = rules {
        builder = builder.rules_file(path);
    }

    match dict {
        // 有真实词典包：用它替代 Mock 词表
        Some(dir) => builder = builder.dictionary_package(dir),
        // 无词典包：退回内置 Mock 词表，保证 CLI 随时可用
        None => {
            builder = builder
                .engine(MockDictionaryEngine::new())
                .provider(MockDictionaryProvider::new());
        }
    }

    builder.build()
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

    // 源语言：显式 --from 优先，否则交给 SDK 的语言识别
    let source_tag = match from {
        Some(from) => match LanguageTag::new(&from) {
            Ok(tag) => Some(tag),
            Err(err) => {
                eprintln!("源语言非法：{err}");
                return ExitCode::from(2);
            }
        },
        None => pollster::block_on(translator.detect_language(&text))
            .ok()
            .and_then(|guesses| guesses.into_iter().next())
            .map(|guess| guess.language),
    };

    // 目标语言：--to 优先；否则中文译英文、其它译中文。
    // 这只是调试工具的便利默认值，SDK 本身不做语言对推断。
    let default_target = match &source_tag {
        Some(tag) if tag.as_str().starts_with("zh") => "en",
        _ => "zh-Hans",
    };
    let target = match LanguageTag::new(to.as_deref().unwrap_or(default_target)) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("目标语言非法：{err}");
            return ExitCode::from(2);
        }
    };

    let mut req = TranslateRequest::new(text.clone(), target).with_policy(policy);
    if let Some(tag) = source_tag {
        req = req.with_source(tag);
    }

    if explain {
        // 规则预览需要确定的语言对；源语言识别失败时（req.source 为 None）跳过
        let preview = req
            .source
            .clone()
            .map(|source| translator.explain_rules(&text, &source, &req.target));
        if let Some(preview) = preview {
            if let Some(id) = preview.rules_id.as_deref() {
                println!("规则包：");
                println!("  id        {id}  v{}", preview.version);
                println!("  规则数    {}", preview.rule_count);
                if preview.applied.is_empty() {
                    println!("  生效      （无）");
                } else {
                    println!("  生效      {}", preview.applied.join(", "));
                }
                if preview.text != text {
                    println!("  处理后    {}", preview.text);
                }
                println!();
            }
        }

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
