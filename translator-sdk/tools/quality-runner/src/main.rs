//! T5：批量质量运行器。
//!
//! 输入 JSONL（每条至少 `id` + `source`），输出 JSONL（译文 + 引擎 + 诊断 + 质量检查）。
//! 设计目标：
//!
//! - **一次构建、批量跑**：避免每条样本都启动进程/加载模型；
//! - **失败不中断**：单条错误写成 `error` 字段，后续样本继续；
//! - **输出可 diff**：同一输入在不同配置下的输出按 `id` 对齐；
//! - **不做指标判定**：指标由 Python 脚本计算，运行器只负责产出原始结果。
//!
//! 默认不编译 ORT；需要真实模型时加 `--features ort`。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use translator_analysis::{EnglishLemmatizer, MorphologyAnalyzer};
use translator_api::{
    DictionaryEntry, LanguageTag, LookupRequest, TermPolicy, TranslateRequest, TranslationMode,
    TranslationWarning, Translator,
};

struct Args {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    lookup_input: Option<PathBuf>,
    lookup_output: Option<PathBuf>,
    lookup_exact: bool,
    dictionaries: Vec<PathBuf>,
    rules: Option<PathBuf>,
    terms: Option<PathBuf>,
    templates: Option<PathBuf>,
    composition: Option<PathBuf>,
    semantic_composition: Option<PathBuf>,
    term_policy: TermPolicy,
    mode: TranslationMode,
    text_kind: translator_api::TextKind,
    paragraph_strategy: translator_api::ParagraphStrategy,
    label: String,
    limit: Option<usize>,
    cache_capacity: usize,
    strict_integrity: bool,
    source: Option<LanguageTag>,
    target: LanguageTag,
    policy: translator_api::TranslationPolicy,
    lookup_limit: usize,
    #[cfg(feature = "ort")]
    ort_model_dir: Option<PathBuf>,
    #[cfg(feature = "ort")]
    max_new_tokens: Option<usize>,
    #[cfg(feature = "ort")]
    threads: usize,
    #[cfg(feature = "ort")]
    beam_width: usize,
    #[cfg(feature = "ort")]
    encoder_file: Option<String>,
    #[cfg(feature = "ort")]
    decoder_file: Option<String>,
    #[cfg(feature = "ort")]
    tokenizer_file: Option<String>,
    #[cfg(feature = "ort")]
    model_variant: Option<String>,
    summary: Option<PathBuf>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            lookup_input: None,
            lookup_output: None,
            lookup_exact: false,
            dictionaries: Vec::new(),
            rules: None,
            terms: None,
            templates: None,
            composition: None,
            semantic_composition: None,
            term_policy: TermPolicy::Warn,
            mode: TranslationMode::Precise,
            text_kind: translator_api::TextKind::Auto,
            paragraph_strategy: translator_api::ParagraphStrategy::Auto,
            label: "default".to_string(),
            limit: None,
            cache_capacity: 256,
            strict_integrity: true,
            source: None,
            target: LanguageTag::new("zh-Hans").expect("静态标签合法"),
            policy: translator_api::TranslationPolicy::AutoLocalFirst,
            lookup_limit: 10,
            #[cfg(feature = "ort")]
            ort_model_dir: None,
            #[cfg(feature = "ort")]
            max_new_tokens: None,
            #[cfg(feature = "ort")]
            threads: 1,
            #[cfg(feature = "ort")]
            beam_width: 1,
            #[cfg(feature = "ort")]
            encoder_file: None,
            #[cfg(feature = "ort")]
            decoder_file: None,
            #[cfg(feature = "ort")]
            tokenizer_file: None,
            #[cfg(feature = "ort")]
            model_variant: None,
            summary: None,
        }
    }
}

fn usage() -> &'static str {
    "quality-runner —— 批量翻译/查词质量运行器

用法:
  quality-runner --input <golden.jsonl> --output <out.jsonl> [选项]
  quality-runner --lookup-input <words.txt> --lookup-output <out.jsonl> [选项]

选项:
  --dict <dir>            挂载词典包（可重复）
  --rules <file>          挂载规则包
  --terms <file>          挂载输出侧术语表
  --templates <file>      挂载完整句子模板（P16-R）
  --composition <file>   挂载组合式日常句型规则（P19-G）
  --semantic-composition <file> 挂载语义组合/中文生成规则（P20-C）
  --term-policy <p>       off | warn | enforce（默认 warn）
  --mode <m>              precise | fast（默认 precise）
  --text-kind <k>         auto | word | sentence | paragraph | document（默认 auto）
  --paragraph-strategy <s> auto | whole | sentence | group（默认 auto）
  --label <name>          配置标签，写进每条输出
  --limit <n>             只跑前 n 条
  --cache-capacity <n>    查询缓存条数（0 关闭，默认 256）
  --strict-integrity <b>  true | false（默认 true）
  --source <lang>         指定源语言（默认自动识别）
  --target <lang>         目标语言（默认 zh-Hans）
  --policy <p>            auto | local | dictionary（默认 auto）
  --lookup-limit <n>      查词返回条数（默认 10）
  --lookup-exact          只认完整词头/短语 + 词形回退，不做前缀补全（覆盖统计用）
  --summary <file>        额外写一份汇总 JSON

真实 NMT（需 --features ort）:
  --ort-model-dir <dir>   模型目录（encoder/decoder/tokenizer）
  --max-new-tokens <n>    解码上界（默认模型配置 192）
  --threads <n>           intra-op 线程数（默认 1）
  --beam <n>              beam 宽度（1 = 贪心，>1 = 实验性 beam search）
  --encoder-file <f>      编码器文件名（默认 int8）
  --decoder-file <f>      解码器文件名
  --tokenizer-file <f>    分词器文件名
  --model-variant <v>     int8 | fp16 | fp32（推荐 fp32 做质量对照）
"
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        let mut value = || iter.next().ok_or_else(|| format!("{flag} 缺少参数"));
        match flag.as_str() {
            "-h" | "--help" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            "--input" => args.input = Some(PathBuf::from(value()?)),
            "--output" => args.output = Some(PathBuf::from(value()?)),
            "--lookup-input" => args.lookup_input = Some(PathBuf::from(value()?)),
            "--lookup-output" => args.lookup_output = Some(PathBuf::from(value()?)),
            "--dict" => args.dictionaries.push(PathBuf::from(value()?)),
            "--rules" => args.rules = Some(PathBuf::from(value()?)),
            "--terms" => args.terms = Some(PathBuf::from(value()?)),
            "--templates" => args.templates = Some(PathBuf::from(value()?)),
            "--composition" => args.composition = Some(PathBuf::from(value()?)),
            "--semantic-composition" => args.semantic_composition = Some(PathBuf::from(value()?)),
            "--term-policy" => {
                args.term_policy = match value()?.as_str() {
                    "off" => TermPolicy::Off,
                    "warn" => TermPolicy::Warn,
                    "enforce" => TermPolicy::Enforce,
                    other => return Err(format!("未知 term-policy：{other}")),
                }
            }
            "--mode" => {
                args.mode = match value()?.as_str() {
                    "precise" => TranslationMode::Precise,
                    "fast" => TranslationMode::Fast,
                    other => return Err(format!("未知 mode：{other}")),
                }
            }
            "--text-kind" => {
                args.text_kind = match value()?.as_str() {
                    "auto" => translator_api::TextKind::Auto,
                    "word" => translator_api::TextKind::Word,
                    "sentence" => translator_api::TextKind::Sentence,
                    "paragraph" => translator_api::TextKind::Paragraph,
                    "document" => translator_api::TextKind::Document,
                    other => return Err(format!("未知 text-kind：{other}")),
                }
            }
            "--paragraph-strategy" => {
                args.paragraph_strategy = match value()?.as_str() {
                    "auto" => translator_api::ParagraphStrategy::Auto,
                    "whole" => translator_api::ParagraphStrategy::Whole,
                    "sentence" => translator_api::ParagraphStrategy::Sentence,
                    "group" => translator_api::ParagraphStrategy::Group,
                    "best" => translator_api::ParagraphStrategy::Best,
                    "context" => translator_api::ParagraphStrategy::Context,
                    other => return Err(format!("未知 paragraph-strategy：{other}")),
                }
            }
            "--label" => args.label = value()?,
            "--limit" => args.limit = Some(value()?.parse().map_err(|e| format!("{e}"))?),
            "--cache-capacity" => {
                args.cache_capacity = value()?.parse().map_err(|e| format!("{e}"))?
            }
            "--strict-integrity" => {
                args.strict_integrity = match value()?.as_str() {
                    "true" | "1" => true,
                    "false" | "0" => false,
                    other => return Err(format!("未知 strict-integrity：{other}")),
                }
            }
            "--source" => {
                args.source = Some(LanguageTag::new(value()?).map_err(|e| e.to_string())?)
            }
            "--target" => args.target = LanguageTag::new(value()?).map_err(|e| e.to_string())?,
            "--policy" => {
                args.policy = match value()?.as_str() {
                    "auto" => translator_api::TranslationPolicy::AutoLocalFirst,
                    "local" => translator_api::TranslationPolicy::LocalOnly,
                    "dictionary" => translator_api::TranslationPolicy::DictionaryOnly,
                    other => return Err(format!("未知 policy：{other}")),
                }
            }
            "--lookup-limit" => args.lookup_limit = value()?.parse().map_err(|e| format!("{e}"))?,
            "--lookup-exact" => args.lookup_exact = true,
            "--summary" => args.summary = Some(PathBuf::from(value()?)),
            #[cfg(feature = "ort")]
            "--ort-model-dir" => args.ort_model_dir = Some(PathBuf::from(value()?)),
            #[cfg(feature = "ort")]
            "--max-new-tokens" => {
                args.max_new_tokens = Some(value()?.parse().map_err(|e| format!("{e}"))?)
            }
            #[cfg(feature = "ort")]
            "--threads" => args.threads = value()?.parse().map_err(|e| format!("{e}"))?,
            #[cfg(feature = "ort")]
            "--beam" => args.beam_width = value()?.parse().map_err(|e| format!("{e}"))?,
            #[cfg(feature = "ort")]
            "--encoder-file" => args.encoder_file = Some(value()?),
            #[cfg(feature = "ort")]
            "--decoder-file" => args.decoder_file = Some(value()?),
            #[cfg(feature = "ort")]
            "--tokenizer-file" => args.tokenizer_file = Some(value()?),
            #[cfg(feature = "ort")]
            "--model-variant" => args.model_variant = Some(value()?),
            other => return Err(format!("未知参数：{other}")),
        }
    }

    if args.input.is_some() && args.lookup_input.is_some() {
        return Err("--input 与 --lookup-input 只能二选一".to_string());
    }
    if args.input.is_none() && args.lookup_input.is_none() {
        return Err("需要 --input 或 --lookup-input".to_string());
    }
    Ok(args)
}

fn build_translator(args: &Args) -> Result<Translator, String> {
    let mut builder = Translator::builder()
        .translation_mode(args.mode)
        .strict_integrity(args.strict_integrity)
        .cache_capacity(args.cache_capacity)
        .lemmatizer(Arc::new(EnglishLemmatizer::new()))
        .term_policy(args.term_policy);

    for dict in &args.dictionaries {
        builder = builder.dictionary_package(dict);
    }
    if let Some(rules) = &args.rules {
        builder = builder.rules_file(rules);
    }
    if let Some(terms) = &args.terms {
        builder = builder.term_table_file(terms);
    }
    if let Some(templates) = &args.templates {
        builder = builder.templates_file(templates);
    }
    if let Some(composition) = &args.composition {
        builder = builder.composition_file(composition);
    }
    if let Some(semantic) = &args.semantic_composition {
        builder = builder.semantic_composition_file(semantic);
    }

    #[cfg(feature = "ort")]
    if let Some(dir) = &args.ort_model_dir {
        let mut config = translator_api::nmt_ort::OrtNmtConfig::new(
            dir,
            LanguageTag::new("en").expect("静态标签合法"),
            args.target.clone(),
        );
        config.threads = args.threads;
        config.beam_width = args.beam_width;
        if let Some(variant) = &args.model_variant {
            config = config.with_variant(match variant.as_str() {
                "int8" => translator_api::nmt_ort::ModelVariant::Int8,
                "fp16" => translator_api::nmt_ort::ModelVariant::Fp16,
                "fp32" => translator_api::nmt_ort::ModelVariant::Fp32,
                other => return Err(format!("未知 model-variant：{other}")),
            });
        }
        if args.encoder_file.is_some()
            || args.decoder_file.is_some()
            || args.tokenizer_file.is_some()
        {
            config = config.with_files(
                args.encoder_file
                    .clone()
                    .unwrap_or_else(|| translator_api::nmt_ort::ENCODER_FILE.to_string()),
                args.decoder_file
                    .clone()
                    .unwrap_or_else(|| translator_api::nmt_ort::DECODER_FILE.to_string()),
                args.tokenizer_file
                    .clone()
                    .unwrap_or_else(|| translator_api::nmt_ort::TOKENIZER_FILE.to_string()),
            );
        }
        if let Some(max_new) = args.max_new_tokens {
            config.max_new_tokens = max_new;
        }
        builder = builder.nmt_ort(config);
    }

    builder.build().map_err(|e| e.to_string())
}

fn warning_str(warning: &TranslationWarning) -> String {
    match warning {
        TranslationWarning::PlaceholderRestoreFailed(detail) => {
            format!("placeholder_restore_failed: {detail}")
        }
        TranslationWarning::LanguageAssumed(lang) => format!("language_assumed: {lang}"),
        TranslationWarning::CacheHit => "cache_hit".to_string(),
        TranslationWarning::TermMismatch(detail) => format!("term_mismatch: {detail}"),
        TranslationWarning::PivotRoute(detail) => format!("pivot_route: {detail}"),
        TranslationWarning::SegmentFailed(detail) => format!("segment_failed: {detail}"),
    }
}

fn protected_kind(kind: translator_api::ProtectedKind) -> &'static str {
    match kind {
        translator_api::ProtectedKind::Url => "url",
        translator_api::ProtectedKind::Email => "email",
        translator_api::ProtectedKind::TemplateVar => "template_var",
        translator_api::ProtectedKind::Code => "code",
        translator_api::ProtectedKind::Number => "number",
    }
}

fn translate_record(translator: &Translator, args: &Args, id: &str, source_text: &str) -> Value {
    let mut req =
        TranslateRequest::new(source_text, args.target.clone()).with_policy(args.policy.clone());
    if let Some(source) = &args.source {
        req = req.with_source(source.clone());
    }

    let started = std::time::Instant::now();
    let options = translator_api::TranslateOptions {
        mode: args.mode,
        text_kind: args.text_kind,
        paragraph_strategy: args.paragraph_strategy,
        strategy: None,
        context_id: None,
        context_text: None,
    };
    match pollster::block_on(translator.translate_with_diagnostics_options(&req, &options)) {
        Ok(out) => {
            let quality = out.diagnostics.quality.as_ref();
            let mut record = Map::new();
            record.insert("id".to_string(), json!(id));
            record.insert("label".to_string(), json!(args.label));
            record.insert("mode".to_string(), json!(args.mode.as_str()));
            record.insert(
                "text_kind".to_string(),
                json!(out.diagnostics.text_kind.as_str()),
            );
            record.insert("strategy".to_string(), json!(out.diagnostics.strategy));
            record.insert("route".to_string(), json!(out.diagnostics.route.as_str()));
            record.insert(
                "fallback_reason".to_string(),
                json!(out.diagnostics.fallback_reason.map(|r| r.as_str())),
            );
            record.insert(
                "model_invoked".to_string(),
                json!(out.diagnostics.model_invoked),
            );
            record.insert(
                "dictionary_hit".to_string(),
                json!(out.diagnostics.dictionary_hit),
            );
            record.insert(
                "rule_matched".to_string(),
                json!(out.diagnostics.rule_matched),
            );
            record.insert("source".to_string(), json!(source_text));
            record.insert("output".to_string(), json!(out.result.text));
            record.insert("engine_id".to_string(), json!(out.result.engine_id));
            record.insert("model_id".to_string(), json!(out.result.model_id));
            record.insert("elapsed_ms".to_string(), json!(out.result.elapsed_ms));
            record.insert("wall_ms".to_string(), json!(started.elapsed().as_millis()));
            record.insert("cache_hit".to_string(), json!(out.diagnostics.cache_hit));
            record.insert(
                "warnings".to_string(),
                json!(out
                    .result
                    .warnings
                    .iter()
                    .map(warning_str)
                    .collect::<Vec<_>>()),
            );
            record.insert(
                "quality".to_string(),
                match quality {
                    Some(q) => json!({
                        "mode": q.mode.as_str(),
                        "numbers_in_source": q.numbers_in_source,
                        "numbers_preserved": q.numbers_preserved,
                        "fatal": q.has_fatal(),
                        "issues": q.issues.iter().map(|issue| json!({
                            "kind": issue.kind.as_str(),
                            "severity": issue.severity.as_str(),
                            "detail": issue.detail,
                        })).collect::<Vec<_>>(),
                    }),
                    None => Value::Null,
                },
            );
            record.insert(
                "diagnostics".to_string(),
                json!({
                    "engine_id": out.diagnostics.engine_id,
                    "capability": out.diagnostics.capability.as_str(),
                    "route_reason": out.diagnostics.route_reason,
                    "rules_version": out.diagnostics.rules_version,
                    "resource_fingerprint": out.diagnostics.resource_fingerprint,
                    "evidence": out.diagnostics.evidence.as_ref().map(|e| e.summary()),
                    "units": out.diagnostics.units.iter().map(|u| json!({
                        "index": u.index,
                        "span": [u.span.start, u.span.end],
                        "kind": u.kind.as_str(),
                        "route": u.route.as_str(),
                        "engine_id": u.engine_id,
                        "reason": u.reason,
                        "model_invoked": u.model_invoked,
                        "fallback_reason": u.fallback_reason.map(|r| r.as_str()),
                    })).collect::<Vec<_>>(),
                    "rules_applied": out.diagnostics.rules.as_ref().map(|r| r.applied.clone()),
                    "protected_spans": out.diagnostics.protected_spans.iter().map(|s| json!({
                        "kind": protected_kind(s.kind),
                        "start": s.start,
                        "end": s.end,
                    })).collect::<Vec<_>>(),
                    "restore_missing": out.diagnostics.restore_missing,
                    "term": out.diagnostics.term.as_ref().map(|t| json!({
                        "id": t.id,
                        "version": t.version,
                        "policy": t.policy.as_str(),
                        "replaced": t.replaced,
                        "violations": t.violations.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
                    })),
                }),
            );
            record.insert("error".to_string(), Value::Null);
            Value::Object(record)
        }
        Err(err) => json!({
            "id": id,
            "label": args.label,
            "mode": args.mode.as_str(),
            "source": source_text,
            "output": Value::Null,
            "engine_id": Value::Null,
            "model_id": Value::Null,
            "elapsed_ms": started.elapsed().as_millis(),
            "wall_ms": started.elapsed().as_millis(),
            "cache_hit": false,
            "warnings": [],
            "quality": Value::Null,
            "diagnostics": Value::Null,
            "error": {
                "code": err.code.as_str(),
                "message": err.message,
                "suggested_packages": err.suggested_packages,
            },
        }),
    }
}

fn entry_json(entry: &DictionaryEntry) -> Value {
    json!({
        "headword": entry.headword,
        "normalized": entry.normalized,
        "language": entry.language.as_str(),
        "readings": entry.readings,
        "pos": entry.parts_of_speech,
        "senses": entry.senses.iter().map(|s| s.definition.clone()).collect::<Vec<_>>(),
        "source_id": entry.source_id,
        "source_license": entry.source_license,
        "confidence": entry.confidence,
    })
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("参数错误：{err}\n\n{}", usage());
            std::process::exit(2);
        }
    };

    let translator = match build_translator(&args) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("构建 Translator 失败：{err}");
            std::process::exit(1);
        }
    };

    let mut success = 0usize;
    let mut errors = 0usize;
    let mut cache_hits = 0usize;
    let mut total_wall_ms = 0u128;

    if let Some(input) = &args.input {
        let output_path = args.output.as_ref().expect("--input 需要 --output");
        let text = std::fs::read_to_string(input).unwrap_or_else(|e| {
            eprintln!("读取 {} 失败：{e}", input.display());
            std::process::exit(1);
        });
        let mut records: Vec<Value> = Vec::new();
        for (lineno, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let value: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("第 {} 行 JSON 非法：{e}", lineno + 1);
                    errors += 1;
                    continue;
                }
            };
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("line-{}", lineno + 1));
            let source_text = value
                .get("source")
                .or_else(|| value.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if source_text.trim().is_empty() {
                eprintln!("{id}: source 为空，跳过");
                errors += 1;
                continue;
            }
            let record = translate_record(&translator, &args, &id, source_text);
            if record.get("error").map(Value::is_null).unwrap_or(false) {
                success += 1;
                if record
                    .get("cache_hit")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    cache_hits += 1;
                }
                total_wall_ms += record.get("wall_ms").and_then(Value::as_u64).unwrap_or(0) as u128;
            } else {
                errors += 1;
            }
            records.push(record);
            if let Some(limit) = args.limit {
                if records.len() >= limit {
                    break;
                }
            }
            if records.len() % 50 == 0 {
                eprintln!("  已完成 {} 条（错误 {errors}）", records.len());
            }
        }
        write_jsonl(output_path, &records);
        eprintln!(
            "翻译完成：{} 条，成功 {success}，错误 {errors}，缓存命中 {cache_hits}",
            records.len()
        );
    }

    if let Some(input) = &args.lookup_input {
        let output_path = args
            .lookup_output
            .as_ref()
            .expect("--lookup-input 需要 --lookup-output");
        let text = std::fs::read_to_string(input).unwrap_or_else(|e| {
            eprintln!("读取 {} 失败：{e}", input.display());
            std::process::exit(1);
        });
        let mut records: Vec<Value> = Vec::new();
        // 覆盖统计必须用 exact：`lookup()` 会做前缀补全，不能拿前缀结果
        // 冒充完整短语命中（docs/70 §4.4）。
        let exact_packages = if args.lookup_exact {
            let mut pkgs = Vec::new();
            for dir in &args.dictionaries {
                match translator_api::dictionary::DictionaryPackage::open(dir, true) {
                    Ok(pkg) => pkgs.push(pkg),
                    Err(err) => {
                        eprintln!("打开词典 {} 失败：{err}", dir.display());
                        std::process::exit(1);
                    }
                }
            }
            Some(pkgs)
        } else {
            None
        };
        let lemmatizer = EnglishLemmatizer::new();
        for (lineno, line) in text.lines().enumerate() {
            let word = line.trim();
            if word.is_empty() || word.starts_with('#') {
                continue;
            }
            let started = std::time::Instant::now();
            if let Some(packages) = &exact_packages {
                let mut entries = Vec::new();
                let mut candidates = vec![word.to_string()];
                candidates.extend(lemmatizer.lemmas(word).into_iter().take(5));
                let mut exact = false;
                let mut lemma_hit = false;
                for candidate in &candidates {
                    for package in packages {
                        match package.lookup_exact_all(candidate) {
                            Ok(found) => {
                                for entry in found {
                                    if entry.headword.eq_ignore_ascii_case(word) {
                                        exact = true;
                                    } else if !candidate.eq_ignore_ascii_case(word) {
                                        lemma_hit = true;
                                    }
                                    if !entries.iter().any(|e: &DictionaryEntry| {
                                        e.headword == entry.headword
                                            && e.source_id == entry.source_id
                                    }) {
                                        entries.push(entry);
                                    }
                                }
                            }
                            Err(err) => {
                                errors += 1;
                                eprintln!("{word}: exact lookup 失败：{err}");
                            }
                        }
                    }
                    if !entries.is_empty() {
                        break;
                    }
                }
                let found = !entries.is_empty();
                if found {
                    success += 1;
                } else {
                    errors += 1;
                }
                records.push(json!({
                    "word": word,
                    "line": lineno + 1,
                    "found": found,
                    "exact": exact,
                    "lemma_hit": lemma_hit,
                    "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
                    "wall_ms": started.elapsed().as_millis(),
                    "error": Value::Null,
                }));
            } else {
                let req = LookupRequest::new(word)
                    .with_language(LanguageTag::new("en").expect("静态标签合法"))
                    .with_limit(args.lookup_limit);
                match pollster::block_on(translator.lookup(&req)) {
                    Ok(entries) => {
                        success += 1;
                        records.push(json!({
                            "word": word,
                            "line": lineno + 1,
                            "found": !entries.is_empty(),
                            "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
                            "wall_ms": started.elapsed().as_millis(),
                            "error": Value::Null,
                        }));
                    }
                    Err(err) => {
                        errors += 1;
                        records.push(json!({
                            "word": word,
                            "line": lineno + 1,
                            "found": false,
                            "entries": [],
                            "wall_ms": started.elapsed().as_millis(),
                            "error": { "code": err.code.as_str(), "message": err.message },
                        }));
                    }
                }
            }
            if let Some(limit) = args.limit {
                if records.len() >= limit {
                    break;
                }
            }
        }
        write_jsonl(output_path, &records);
        eprintln!(
            "查词完成：{} 条，命中 {success}，错误 {errors}",
            records.len()
        );
    }

    let route_stats = translator.route_stats();
    let reliable_units = route_stats
        .dictionary_hits
        .saturating_add(route_stats.rule_matches);
    let model_units = route_stats
        .nmt_calls
        .saturating_add(route_stats.remote_calls);
    let total_units = route_stats.executions.max(1);
    let summary = json!({
        "label": args.label,
        "mode": args.mode.as_str(),
        "success": success,
        "errors": errors,
        "cache_hits": cache_hits,
        "total_wall_ms": total_wall_ms,
        "resource_fingerprint": translator.resource_fingerprint(),
        "route_stats": {
            "executions": route_stats.executions,
            "dictionary_hits": route_stats.dictionary_hits,
            "rule_matches": route_stats.rule_matches,
            "nmt_calls": route_stats.nmt_calls,
            "remote_calls": route_stats.remote_calls,
            "nmt_fallbacks": route_stats.nmt_fallbacks,
            "remote_fallbacks": route_stats.remote_fallbacks,
            "cache_hits": route_stats.cache_hits,
            "best_candidates": route_stats.best_candidates,
            "failures": route_stats.failures,
        },
        "coverage": {
            "reliable_units": reliable_units,
            "model_units": model_units,
            "reliable_coverage": reliable_units as f64 / total_units as f64,
            "model_ratio": model_units as f64 / total_units as f64,
        },
    });
    if let Some(path) = &args.summary {
        std::fs::write(path, serde_json::to_string_pretty(&summary).unwrap()).ok();
    }
    println!("{}", summary);
}

fn write_jsonl(path: &PathBuf, records: &[Value]) {
    let body = records
        .iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, format!("{body}\n")).unwrap_or_else(|e| {
        eprintln!("写入 {} 失败：{e}", path.display());
        std::process::exit(1);
    });
}
