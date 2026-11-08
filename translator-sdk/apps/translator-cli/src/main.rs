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
//! translator-cli --store .resources resource install path/to/package
//! translator-cli --store .resources --dict-id dictionary.cc-cedict.zh-en lookup 计算机
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
  resource install <目录>              安装资源包到仓库（校验 + 原子落位）
  resource list                        列出仓库里的资源与激活版本
  resource inspect <id>                查看某个资源的版本与历史
  resource activate <id> <version>     切换激活版本（不删除旧版本）
  resource rollback <id>               回滚到上一个激活过的版本
  resource uninstall <id> <version>    删除非激活版本（激活版本拒绝删除）
  resource cleanup <id> [--keep <n>]   保留激活 + 最近 n 个历史版本
  resource lease [<id> [<version>]]    列出租约（谁在用哪个版本）
  resource lease reclaim               回收已死进程留下的租约
  resource audit [--prune]             三方对账（state ↔ installed ↔ staging）；--prune 删孤儿目录
  memory                               打印内存预算与实际挂载情况
  translate <文本> [--from <lang>] [--to <lang>] [--policy <p>] [--explain]
             --to 缺省时按源语言推断：中文→en，其它→zh-Hans
  lookup <词条> [--lang <lang>] [--limit <n>]
  detect <文本>
  capabilities

全局选项:
  --dict <包目录>    直接挂载一个词典包目录
  --dict-id <资源 id> 从资源仓库挂载该资源的**激活版本**
  --store <仓库目录>  资源仓库根目录（默认 .resources）
  --memory-budget <MiB>  峰值内存上限；超限时 build() 直接 OutOfBudget
  --mode <m>         翻译模式：precise（默认）| fast
  --text-kind <k>    文本类型：auto（默认）| word | sentence | paragraph | document
  --paragraph-strategy <s>  段落策略：auto（默认）| whole | sentence | group
  --terms <术语表>   输出侧术语表（JSON，P2.4）
  --term-policy <p>  术语策略：off | warn | enforce（默认 warn：只报告不改写）
  --rules <规则文件>  挂载规则包（JSON 或已编译的二进制）

真实本地 NMT（需以 --features nmt 编译）:
  --nmt                    挂载 Bergamot 引擎（缺省仍用 Mock）
  --nmt-native <path>      bergamot native 可执行文件（或 NMT_BERGAMOT_BIN）
  --nmt-model-dir <dir>    模型目录（或 NMT_MODEL_DIR）
  --nmt-worker <path>      nmt-worker 可执行文件（或 NMT_WORKER_BIN）
  --nmt-snapshot-root <d>  快照根目录（或 NMT_SNAPSHOT_ROOT）
  --nmt-protection <p>     segment | marker（默认 segment）
  --nmt-threads <n>        native 线程数（默认 1）
  --nmt-beam <n>           beam size（默认 1）
  --nmt-idle-ttl <ms>      空闲多久后允许回收模型（默认不自动回收）
  --nmt-source <lang>      引擎语言对源（默认 en）
  --nmt-target <lang>      引擎语言对目标（默认 zh-Hans）

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
  translator-cli --store .resources resource install resources/zh-en
  translator-cli --store .resources --dict-id dictionary.cc-cedict.zh-en lookup 计算机
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    // 抽取全局选项
    let options = match take_global_options(&mut args) {
        Ok(o) => o,
        Err(code) => return code,
    };

    // resource 子命令不需要构建 Translator（它管理的是磁盘上的资源）
    if args[0] == "resource" {
        return cmd_resource(&options.store, &args[1..]);
    }

    let (translator, banner) = match build_translator(&options) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("插件注册失败：{err}");
            return ExitCode::FAILURE;
        }
    };

    match args[0].as_str() {
        "translate" => cmd_translate(&translator, &args[1..], &banner),
        "lookup" => cmd_lookup(&translator, &args[1..]),
        "detect" => cmd_detect(&translator, &args[1..]),
        "capabilities" => cmd_capabilities(&translator),
        "memory" => cmd_memory(&translator),
        other => {
            eprintln!("未知子命令：{other}\n");
            print!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// 全局选项。
struct GlobalOptions {
    dict: Option<String>,
    dict_id: Option<String>,
    store: String,
    rules: Option<String>,
    /// 峰值内存上限（字节）。
    memory_budget: Option<u64>,
    /// 输出侧术语表文件。
    terms: Option<String>,
    term_policy: translator_api::TermPolicy,
    /// 翻译模式（docs/57 §4）：默认 Precise。
    mode: translator_api::TranslationMode,
    /// 文本类型（docs/62 §3）：默认 Auto。
    text_kind: translator_api::TextKind,
    /// 段落策略：默认 Auto。
    paragraph_strategy: translator_api::ParagraphStrategy,
    /// 真实本地 NMT 选项（`--nmt*`）。
    nmt: NmtOptions,
}

/// 真实本地 NMT 的全局选项。只有以 `--features nmt` 编译出的二进制才认得它们。
#[derive(Default)]
struct NmtOptions {
    enabled: bool,
    worker: Option<String>,
    native: Option<String>,
    model_dir: Option<String>,
    snapshot_root: Option<String>,
    protection: Option<String>,
    threads: Option<u16>,
    beam: Option<u8>,
    source: Option<String>,
    target: Option<String>,
    idle_ttl_ms: Option<u64>,
}

/// 运行时身份横幅（`--diagnose` 时打印）。不含 pid、socket、权重路径。
#[derive(Default)]
struct RuntimeBanner {
    lines: Vec<(&'static str, String)>,
    /// 真实引擎句柄：`--diagnose` 时用来读缓存统计与加载状态。
    #[cfg(feature = "nmt")]
    engine: Option<std::sync::Arc<translator_api::nmt_bergamot::BergamotNmtEngine>>,
}

const DEFAULT_STORE: &str = ".resources";

/// 抽出全局选项，剩下的交给子命令解析。
fn take_global_options(args: &mut Vec<String>) -> Result<GlobalOptions, ExitCode> {
    let mut options = GlobalOptions {
        dict: None,
        dict_id: None,
        store: DEFAULT_STORE.to_string(),
        rules: None,
        memory_budget: None,
        terms: None,
        term_policy: translator_api::TermPolicy::default(),
        mode: translator_api::TranslationMode::Precise,
        text_kind: translator_api::TextKind::Auto,
        paragraph_strategy: translator_api::ParagraphStrategy::Auto,
        nmt: NmtOptions::default(),
    };

    let take = |args: &mut Vec<String>, flag: &str| -> Option<String> {
        let mut value = None;
        while let Some(pos) = args.iter().position(|a| a == flag) {
            if pos + 1 >= args.len() {
                return None;
            }
            value = Some(args[pos + 1].clone());
            args.drain(pos..=pos + 1);
        }
        value
    };

    options.dict = take(args, "--dict");
    options.dict_id = take(args, "--dict-id");
    options.rules = take(args, "--rules");
    if let Some(store) = take(args, "--store") {
        options.store = store;
    }
    options.terms = take(args, "--terms");
    if let Some(raw) = take(args, "--mode") {
        options.mode = match raw.as_str() {
            "precise" => translator_api::TranslationMode::Precise,
            "fast" => translator_api::TranslationMode::Fast,
            other => {
                eprintln!("--mode 只能是 precise | fast：{other}");
                return Err(ExitCode::from(2));
            }
        };
    }
    if let Some(raw) = take(args, "--text-kind") {
        options.text_kind = match raw.as_str() {
            "auto" => translator_api::TextKind::Auto,
            "word" => translator_api::TextKind::Word,
            "sentence" => translator_api::TextKind::Sentence,
            "paragraph" => translator_api::TextKind::Paragraph,
            "document" => translator_api::TextKind::Document,
            other => {
                eprintln!(
                    "--text-kind 只能是 auto | word | sentence | paragraph | document：{other}"
                );
                return Err(ExitCode::from(2));
            }
        };
    }
    if let Some(raw) = take(args, "--paragraph-strategy") {
        options.paragraph_strategy = match raw.as_str() {
            "auto" => translator_api::ParagraphStrategy::Auto,
            "whole" => translator_api::ParagraphStrategy::Whole,
            "sentence" => translator_api::ParagraphStrategy::Sentence,
            "group" => translator_api::ParagraphStrategy::Group,
            other => {
                eprintln!("--paragraph-strategy 只能是 auto | whole | sentence | group：{other}");
                return Err(ExitCode::from(2));
            }
        };
    }
    if let Some(raw) = take(args, "--term-policy") {
        options.term_policy = match raw.as_str() {
            "off" => translator_api::TermPolicy::Off,
            "warn" => translator_api::TermPolicy::Warn,
            "enforce" => translator_api::TermPolicy::Enforce,
            other => {
                eprintln!("--term-policy 只能是 off | warn | enforce：{other}");
                return Err(ExitCode::from(2));
            }
        };
    }
    if let Some(raw) = take(args, "--memory-budget") {
        match raw.parse::<u64>() {
            Ok(mib) if mib > 0 => options.memory_budget = Some(mib * 1024 * 1024),
            _ => {
                eprintln!("--memory-budget 需要正整数（单位 MiB）：{raw}");
                return Err(ExitCode::from(2));
            }
        }
    }

    options.nmt.worker = take(args, "--nmt-worker");
    options.nmt.native = take(args, "--nmt-native");
    options.nmt.model_dir = take(args, "--nmt-model-dir");
    options.nmt.snapshot_root = take(args, "--nmt-snapshot-root");
    options.nmt.protection = take(args, "--nmt-protection");
    options.nmt.source = take(args, "--nmt-source");
    options.nmt.target = take(args, "--nmt-target");
    if let Some(raw) = take(args, "--nmt-threads") {
        match raw.parse::<u16>() {
            Ok(n) if n > 0 => options.nmt.threads = Some(n),
            _ => {
                eprintln!("--nmt-threads 需要正整数：{raw}");
                return Err(ExitCode::from(2));
            }
        }
    }
    if let Some(raw) = take(args, "--nmt-idle-ttl") {
        match raw.parse::<u64>() {
            Ok(ms) if ms > 0 => options.nmt.idle_ttl_ms = Some(ms),
            _ => {
                eprintln!("--nmt-idle-ttl 需要正整数（毫秒）：{raw}");
                return Err(ExitCode::from(2));
            }
        }
    }
    if let Some(raw) = take(args, "--nmt-beam") {
        match raw.parse::<u8>() {
            Ok(n) if n > 0 => options.nmt.beam = Some(n),
            _ => {
                eprintln!("--nmt-beam 需要 1..=255：{raw}");
                return Err(ExitCode::from(2));
            }
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--nmt") {
        options.nmt.enabled = true;
        args.remove(pos);
    }
    if !options.nmt.enabled
        && (options.nmt.worker.is_some()
            || options.nmt.native.is_some()
            || options.nmt.model_dir.is_some()
            || options.nmt.protection.is_some())
    {
        eprintln!("--nmt-* 需要与 --nmt 一起使用");
        return Err(ExitCode::from(2));
    }

    if options.dict.is_some() && options.dict_id.is_some() {
        eprintln!("--dict 与 --dict-id 只能用一个");
        return Err(ExitCode::from(2));
    }

    Ok(options)
}

/// 构造 `Translator`：把全局选项翻成 builder 调用。
///
/// `--dict-id` 走 `dictionary_from_store`：目录由仓库解析，并且整个进程存续期间
/// 持有租约——另一个进程想 `uninstall` 这个版本会被 `ResourceLeased` 拒绝（P2.3）。
fn build_translator(
    options: &GlobalOptions,
) -> Result<(translator_api::Translator, RuntimeBanner), translator_api::TranslationError> {
    let mut builder = translator_api::Translator::builder()
        // 本地 NMT：模拟 Marian（真实模型在 P3 接入）
        .engine(MockNmtEngine::new())
        // 远程 LLM：默认【无】联网授权，用于验证 ADR-004
        .engine(MockRemoteEngine::new(false))
        .backend(MockBackend::new())
        .translation_mode(options.mode)
        .text_kind(options.text_kind)
        .paragraph_strategy(options.paragraph_strategy)
        .cache_capacity(64);

    // 真实本地 NMT：显式传 `--nmt` 才挂载；未编译该特性时给出明确提示而不是静默降级
    let banner = match register_nmt(builder, &options.nmt) {
        Ok((next, banner)) => {
            builder = next;
            banner
        }
        Err(err) => return Err(err),
    };

    if let Some(path) = &options.rules {
        builder = builder.rules_file(path);
    }
    if let Some(bytes) = options.memory_budget {
        builder = builder.memory_budget(Some(bytes));
    }
    if let Some(path) = &options.terms {
        builder = builder
            .term_table_file(path)
            .term_policy(options.term_policy);
    }

    match (&options.dict, &options.dict_id) {
        // 有真实词典包目录：直接挂载
        (Some(dir), _) => builder = builder.dictionary_package(dir),
        // 从资源仓库挂载激活版本（持租约）
        (None, Some(id)) => {
            let store = translator_resource::ResourceStore::open(&options.store).map_err(|e| {
                translator_api::TranslationError::new(
                    translator_api::TranslationErrorCode::RuntimeInitFailed,
                    format!("打开资源仓库失败（{}）：{e}", options.store),
                )
            })?;
            builder = builder.dictionary_from_store(&store, id);
        }
        // 都没有：退回内置 Mock 词表，保证 CLI 随时可用
        (None, None) => {
            builder = builder
                .engine(MockDictionaryEngine::new())
                .provider(MockDictionaryProvider::new());
        }
    }

    builder.build().map(|translator| (translator, banner))
}

/// 把 `--nmt*` 翻成 `BergamotNmtConfig` 并挂上引擎；返回身份横幅。
#[cfg(feature = "nmt")]
fn register_nmt(
    builder: translator_api::TranslatorBuilder,
    options: &NmtOptions,
) -> Result<(translator_api::TranslatorBuilder, RuntimeBanner), translator_api::TranslationError> {
    use translator_api::nmt_bergamot::{BergamotNmtConfig, BergamotNmtEngine};

    if !options.enabled {
        return Ok((builder, RuntimeBanner::default()));
    }

    let env = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    let native = options
        .native
        .clone()
        .or_else(|| env("NMT_BERGAMOT_BIN"))
        .ok_or_else(|| {
            translator_api::TranslationError::new(
                translator_api::TranslationErrorCode::RuntimeInitFailed,
                "缺少 --nmt-native 或环境变量 NMT_BERGAMOT_BIN",
            )
        })?;
    let model_dir = options
        .model_dir
        .clone()
        .or_else(|| env("NMT_MODEL_DIR"))
        .ok_or_else(|| {
            translator_api::TranslationError::new(
                translator_api::TranslationErrorCode::RuntimeInitFailed,
                "缺少 --nmt-model-dir 或环境变量 NMT_MODEL_DIR",
            )
        })?;
    let worker = options
        .worker
        .clone()
        .or_else(|| env("NMT_WORKER_BIN"))
        .unwrap_or_else(|| "nmt-worker".to_string());

    let mut config = BergamotNmtConfig::new(worker, native, model_dir);
    if let Some(root) = options
        .snapshot_root
        .clone()
        .or_else(|| env("NMT_SNAPSHOT_ROOT"))
    {
        config.snapshot_root = std::path::PathBuf::from(root);
    }
    if let Some(raw) = &options.protection {
        config.protection = match raw.as_str() {
            "segment" => translator_api::nmt_bergamot::ProtectionMode::Segment,
            "marker" => translator_api::nmt_bergamot::ProtectionMode::Marker,
            other => {
                return Err(translator_api::TranslationError::new(
                    translator_api::TranslationErrorCode::InvalidRequest,
                    format!("--nmt-protection 只能是 segment | marker：{other}"),
                ));
            }
        };
    }
    if let Some(threads) = options.threads {
        config.cpu_threads = threads;
    }
    if let Some(beam) = options.beam {
        config.beam_size = beam;
    }
    if let Some(source) = &options.source {
        config.source = LanguageTag::new(source)?;
    }
    if let Some(target) = &options.target {
        config.target = LanguageTag::new(target)?;
    }
    if let Some(ms) = options.idle_ttl_ms {
        config.idle_ttl = Some(std::time::Duration::from_millis(ms));
    }

    let engine = std::sync::Arc::new(BergamotNmtEngine::new(config)?);
    // 显式加载：把"模型坏了"暴露在启动阶段，而不是第一条译文时
    let identity = engine.load()?;

    let mut banner = RuntimeBanner::default();
    banner
        .lines
        .push(("引擎", translator_api::nmt_bergamot::ENGINE_ID.to_string()));
    banner
        .lines
        .push(("模型版本", identity.model_revision.clone()));
    banner.lines.push((
        "运行时",
        identity.runtime.clone().unwrap_or_else(|| "unknown".into()),
    ));
    banner.lines.push((
        "构建",
        identity
            .runtime_build
            .clone()
            .unwrap_or_else(|| "unknown".into()),
    ));
    banner.lines.push((
        "线程/beam",
        format!("{}/{}", identity.threads, identity.beam_size),
    ));
    banner.lines.push((
        "保护策略",
        match engine.config().protection {
            translator_api::nmt_bergamot::ProtectionMode::Segment => "分段（受保护跨度不进模型）",
            translator_api::nmt_bergamot::ProtectionMode::Marker => {
                "标记（受保护跨度以占位符进模型）"
            }
        }
        .to_string(),
    ));
    banner.lines.push((
        "策略版本",
        translator_api::nmt_bergamot::protection_label().to_string(),
    ));
    // 执行计划摘要 = 未来的缓存身份（ADR-027）：模型五文件摘要 + 解码参数 + 语言对 + 保护策略
    let plan = engine.plan(&identity, engine.config().protection);
    banner.lines.push((
        "计划摘要",
        format!(
            "{}（plan-v1，隐私域 {}）",
            plan.short_digest(),
            plan.privacy_domain
        ),
    ));

    banner.engine = Some(engine.clone());
    Ok((builder.engine(engine), banner))
}

#[cfg(not(feature = "nmt"))]
fn register_nmt(
    builder: translator_api::TranslatorBuilder,
    options: &NmtOptions,
) -> Result<(translator_api::TranslatorBuilder, RuntimeBanner), translator_api::TranslationError> {
    if options.enabled {
        return Err(translator_api::TranslationError::new(
            translator_api::TranslationErrorCode::CapabilityUnavailable,
            "本二进制未启用 nmt 特性：请用 `cargo build -p translator-cli --features nmt` 重新编译",
        ));
    }
    Ok((builder, RuntimeBanner::default()))
}

/// 平台一行诊断（不依赖 feature：没有 `nmt` 时也说得清）。
fn platform_line() -> String {
    let os = std::env::consts::OS;
    #[cfg(feature = "nmt")]
    {
        let support = translator_api::nmt_bergamot::platform::current();
        format!("{os}（{}）", support.reason())
    }
    #[cfg(not(feature = "nmt"))]
    {
        format!("{os}（本次构建未启用 nmt feature，未挂载真实引擎）")
    }
}

fn cmd_translate(
    translator: &translator_api::Translator,
    args: &[String],
    banner: &RuntimeBanner,
) -> ExitCode {
    let mut text: Option<String> = None;
    let mut from: Option<String> = None;
    let mut to: Option<String> = None;
    let mut policy = TranslationPolicy::AutoLocalFirst;
    let mut explain = false;
    let mut diagnose = false;

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
            "--diagnose" => diagnose = true,
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

    let outcome = if diagnose {
        pollster::block_on(translator.translate_with_diagnostics(&req)).map(|out| out.result)
    } else {
        pollster::block_on(translator.translate(&req))
    };

    match outcome {
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
            if diagnose {
                // 平台门禁：非 Linux 上真实 NMT 会明确报 CapabilityUnavailable，
                // 所以诊断输出必须把"当前平台是否被验证过"直接写出来（P3.4）
                println!();
                println!("平台    {}", platform_line());
                // 再跑一次只是为了拿诊断；缓存命中所以没有额外计算成本
                if !banner.lines.is_empty() {
                    println!();
                    println!("运行时  （真实本地 NMT）");
                    for (key, value) in &banner.lines {
                        println!("  {key:<8}{value}");
                    }
                }
                #[cfg(feature = "nmt")]
                if let Some(engine) = &banner.engine {
                    let stats = engine.cache_stats();
                    println!();
                    println!(
                        "引擎缓存 命中 {} / 未命中 {} / 条目 {} / 淘汰 {}（键=执行计划摘要）",
                        stats.hits, stats.misses, stats.entries, stats.evictions
                    );
                    match engine.state() {
                        translator_api::nmt_bergamot::LoadState::Loaded {
                            snapshot_bytes,
                            preparation_ms,
                            uptime_ms,
                            ..
                        } => println!(
                            "模型状态   已加载（快照 {} MiB / 准备 {} ms / 已存活 {} ms）",
                            snapshot_bytes / (1024 * 1024),
                            preparation_ms,
                            uptime_ms
                        ),
                        translator_api::nmt_bergamot::LoadState::Unloaded => {
                            println!("模型状态   未加载")
                        }
                    }
                }
                if let Ok(out) = pollster::block_on(translator.translate_with_diagnostics(&req)) {
                    println!();
                    println!("诊断    {}", out.diagnostics.summary());
                    for span in &out.diagnostics.protected_spans {
                        println!(
                            "  受保护  [{}, {})  {}",
                            span.start,
                            span.end,
                            match span.kind {
                                translator_api::ProtectedKind::Url => "URL",
                                translator_api::ProtectedKind::Email => "邮箱",
                                translator_api::ProtectedKind::TemplateVar => "模板变量",
                                translator_api::ProtectedKind::Code => "代码",
                                translator_api::ProtectedKind::Number => "数字",
                            }
                        );
                    }
                    if let Some(term) = &out.diagnostics.term {
                        for violation in &term.violations {
                            println!("  术语    {violation}");
                        }
                    }
                }
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

fn cmd_resource(store_dir: &str, args: &[String]) -> ExitCode {
    let Some(action) = args.first() else {
        eprintln!(
            "resource 需要子命令：install | list | inspect | activate | rollback | uninstall | cleanup | lease | audit"
        );
        return ExitCode::from(2);
    };

    let store = match translator_resource::ResourceStore::open(store_dir) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("打开资源仓库失败（{store_dir}）：{err}");
            return ExitCode::FAILURE;
        }
    };

    match action.as_str() {
        "install" => {
            let Some(path) = args.get(1) else {
                eprintln!("resource install 需要包目录");
                return ExitCode::from(2);
            };
            // 安装前顺手清掉上次中断留下的 staging
            if let Ok(report) = store.recover() {
                if report.removed_staging > 0 {
                    println!("已清理 {} 个遗留安装目录", report.removed_staging);
                }
                if report.reclaimed_leases > 0 {
                    println!("已回收 {} 个失效租约", report.reclaimed_leases);
                }
                if report.removed_temp_files > 0 {
                    println!("已清理 {} 个状态临时文件", report.removed_temp_files);
                }
            }
            match store.install_from_dir(path) {
                Ok(outcome) => {
                    println!(
                        "已安装 {}@{}（{} / {}）",
                        outcome.resource.id,
                        outcome.resource.version,
                        outcome.resource.kind.as_str(),
                        outcome.resource.payload_schema
                    );
                    if outcome.activated {
                        println!("已激活（此前没有激活版本）");
                    } else {
                        println!(
                            "未激活：当前激活版本仍为 {}；用 `resource activate {} {}` 切换",
                            outcome
                                .previous_active
                                .unwrap_or_else(|| "（无）".to_string()),
                            outcome.resource.id,
                            outcome.resource.version
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("安装失败：{err}");
                    ExitCode::FAILURE
                }
            }
        }
        "list" => match store.list() {
            Ok(list) if list.is_empty() => {
                println!("（仓库为空）");
                ExitCode::SUCCESS
            }
            Ok(list) => {
                for status in list {
                    println!(
                        "{}  激活={}  历史=[{}]",
                        status.id,
                        status.active.unwrap_or_else(|| "（无）".to_string()),
                        status.history.join(", ")
                    );
                    for version in status.versions {
                        let leases = store
                            .leases(&status.id, &version.version)
                            .map(|l| l.into_iter().filter(|x| x.is_active()).count())
                            .unwrap_or(0);
                        println!(
                            "  - {}{}  {}  {}{}",
                            version.version,
                            if version.active { "（激活）" } else { "" },
                            version.kind.as_str(),
                            version.installed_at,
                            if leases > 0 {
                                format!("  租约={leases}")
                            } else {
                                String::new()
                            }
                        );
                    }
                }
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("读取资源列表失败：{err}");
                ExitCode::FAILURE
            }
        },
        "inspect" => {
            let Some(id) = args.get(1) else {
                eprintln!("resource inspect 需要资源 id");
                return ExitCode::from(2);
            };
            match store.inspect(id) {
                Ok(status) => {
                    println!("资源    {}", status.id);
                    println!(
                        "激活    {}",
                        status.active.unwrap_or_else(|| "（无）".to_string())
                    );
                    println!("历史    [{}]", status.history.join(", "));
                    for version in status.versions {
                        println!(
                            "版本    {}{}  {}  {}  {}",
                            version.version,
                            if version.active { "（激活）" } else { "" },
                            version.kind.as_str(),
                            version.payload_schema,
                            version.path.display()
                        );
                        for lease in store.leases(id, &version.version).unwrap_or_default() {
                            println!(
                                "        租约  {}  {}",
                                lease.describe(),
                                if lease.is_active() {
                                    "（有效）"
                                } else {
                                    "（已失效，可 reclaim）"
                                }
                            );
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("查询失败：{err}");
                    ExitCode::FAILURE
                }
            }
        }
        "activate" => {
            let (Some(id), Some(version)) = (args.get(1), args.get(2)) else {
                eprintln!("resource activate 需要 <id> <version>");
                return ExitCode::from(2);
            };
            match store.activate(id, version) {
                Ok(()) => {
                    println!("已激活 {id}@{version}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("激活失败：{err}");
                    ExitCode::FAILURE
                }
            }
        }
        "rollback" => {
            let Some(id) = args.get(1) else {
                eprintln!("resource rollback 需要 <id>");
                return ExitCode::from(2);
            };
            match store.rollback(id) {
                Ok(version) => {
                    println!("已回滚 {id} 到 {version}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("回滚失败：{err}");
                    ExitCode::FAILURE
                }
            }
        }
        "uninstall" => {
            let (Some(id), Some(version)) = (args.get(1), args.get(2)) else {
                eprintln!("resource uninstall 需要 <id> <version>");
                return ExitCode::from(2);
            };
            match store.uninstall(id, version) {
                Ok(()) => {
                    println!("已删除 {id}@{version}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    // 拒绝删除激活版本属于正常保护，不是崩溃
                    eprintln!("卸载失败：{err}");
                    ExitCode::FAILURE
                }
            }
        }
        "cleanup" => {
            let Some(id) = args.get(1) else {
                eprintln!("resource cleanup 需要 <id> [--keep <n>]");
                return ExitCode::from(2);
            };
            let keep = args
                .iter()
                .position(|a| a == "--keep")
                .and_then(|pos| args.get(pos + 1))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1);
            match store.cleanup(id, keep) {
                Ok(report) => {
                    if report.removed.is_empty() {
                        println!("没有需要清理的版本");
                    } else {
                        println!("已删除：{}", report.removed.join(", "));
                    }
                    if !report.kept_leased.is_empty() {
                        println!("因有效租约保留：");
                        for (version, holders) in &report.kept_leased {
                            println!("  - {version}（{}）", holders.join("; "));
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("清理失败：{err}");
                    ExitCode::FAILURE
                }
            }
        }
        "lease" => {
            // resource lease [<id> [<version>]] | resource lease reclaim
            if args.get(1).map(String::as_str) == Some("reclaim") {
                return match store.reclaim_stale_leases() {
                    Ok(0) => {
                        println!("没有失效租约");
                        ExitCode::SUCCESS
                    }
                    Ok(n) => {
                        println!("已回收 {n} 个失效租约");
                        ExitCode::SUCCESS
                    }
                    Err(err) => {
                        eprintln!("回收失败：{err}");
                        ExitCode::FAILURE
                    }
                };
            }
            let leases = match (args.get(1), args.get(2)) {
                (Some(id), Some(version)) => store.leases(id, version),
                _ => store.all_leases(),
            };
            match leases {
                Ok(list) if list.is_empty() => {
                    println!("（没有租约）");
                    ExitCode::SUCCESS
                }
                Ok(list) => {
                    for lease in list {
                        println!(
                            "{}@{}  {}  {}",
                            lease.id,
                            lease.version,
                            lease.describe(),
                            match lease.alive {
                                Some(true) => "有效",
                                Some(false) => "已失效（可 reclaim）",
                                None => "无法判定存活（按占用处理）",
                            }
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("读取租约失败：{err}");
                    ExitCode::FAILURE
                }
            }
        }
        "audit" => {
            // 对账：只报告，不改动；--prune 才删孤儿目录（P2-S / S-05）
            let prune = args.iter().any(|a| a == "--prune");
            let audit = match store.audit() {
                Ok(audit) => audit,
                Err(err) => {
                    eprintln!("对账失败：{err}");
                    return ExitCode::FAILURE;
                }
            };
            println!("{}", audit.summary());
            println!();
            println!(
                "记录缺目录 {} / 孤儿目录 {} / 激活缺失 {} / staging {} / 临时文件 {} / 租约 {}（失效 {}）",
                audit.missing_dirs.len(),
                audit.orphan_dirs.len(),
                audit.active_missing.len(),
                audit.staging_dirs.len(),
                audit.stale_tmp_files.len(),
                audit.lease_files,
                audit.stale_leases
            );
            for item in &audit.missing_dirs {
                println!("  缺失：{item}");
            }
            for item in &audit.orphan_dirs {
                println!("  孤儿：{item}");
            }
            for item in &audit.active_missing {
                println!("  激活缺失：{item}");
            }

            if prune {
                match store.prune_orphans() {
                    Ok(removed) if removed.is_empty() => println!("没有需要删除的孤儿目录"),
                    Ok(removed) => {
                        for item in removed {
                            println!("已删除孤儿目录：{item}");
                        }
                        println!("staging / 临时文件残留可用 `recover()` 或下次安装自动清理。");
                    }
                    Err(err) => {
                        eprintln!("删除孤儿目录失败：{err}");
                        return ExitCode::FAILURE;
                    }
                }
                return ExitCode::SUCCESS;
            }

            if audit.needs_attention() {
                println!();
                println!("注：对账只报告不改动。孤儿目录可用 `resource audit --prune` 删除；");
                println!("    「记录缺目录」是数据丢失，需要人决定重装还是清记录——不替你猜。");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("未知 resource 子命令：{other}");
            ExitCode::from(2)
        }
    }
}

/// 打印内存预算与逐项明细（P2.3）。
fn cmd_memory(translator: &translator_api::Translator) -> ExitCode {
    let report = translator.memory_report();
    println!("{}", report.summary());
    match report.limit_bytes {
        Some(limit) => println!("上限    {} MiB", limit / (1024 * 1024)),
        None => println!("上限    （未设置：只记账不拒绝）"),
    }
    println!("已用    {} MiB", report.used_bytes / (1024 * 1024));
    for (label, bytes) in &report.entries {
        println!("  - {label}  {} MiB", bytes / (1024 * 1024));
    }
    println!();
    println!("注：以上是各组件自报的下界估计，不是 RSS；实测数据见 docs/04 §6.2。");
    ExitCode::SUCCESS
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
