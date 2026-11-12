//! 评测运行器：把 `fixtures/eval/*.jsonl` 跑过真实后端，输出**可复现**报告。
//!
//! # 它检查什么、不检查什么
//!
//! 检查（机器可判定，必须 100% 通过）：
//!
//! - `must_preserve`：URL / 邮箱 / 模板变量 / 代码 / 标签必须逐字出现；
//! - `must_keep_numbers`：编号/日期/百分比/金额等关键数字必须保留；
//! - `glossary`：术语约束是否被满足；
//! - 空译文、保护标记残留（由 SDK 的完整性门禁负责，运行器只记录结果）。
//!
//! **不**检查：译文是否准确、自然、地道。那需要人读（`reference_zh` 目前为 `null`，
//! 状态 `PENDING_HUMAN`）。运行器绝不把"自动指标通过"说成"质量验收通过"。
//!
//! # 用法
//!
//! ```text
//! quality-runner --dataset fixtures/eval/smoke.jsonl --config translate.toml --out report.json
//! quality-runner --dataset fixtures/eval/smoke.jsonl --mock --limit 3 --out smoke.json
//! ```
//!
//! 退出码：0 = 所有机器可判定项通过；1 = 有失败；2 = 用法/配置错误。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use translator_api::Translator;
use translator_http::{BackendConfig, BackendsFile, HttpSettings};
use translator_types::{
    LanguageTag, TextKind, TranslateRequest, TranslationMode, TranslationPolicy,
};

const EXIT_OK: u8 = 0;
const EXIT_FAIL: u8 = 1;
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("错误：{err}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn run() -> Result<u8, String> {
    let args = Args::parse(std::env::args().skip(1).collect())?;
    let cases = load_dataset(&args.dataset)?;
    if cases.is_empty() {
        return Err(format!("数据集为空：{}", args.dataset.display()));
    }

    let translator = if args.mock {
        mock_translator()
    } else {
        let path = args
            .config
            .clone()
            .ok_or_else(|| "需要 --config <translate.toml>（或用 --mock 跑通链路）".to_string())?;
        let dataset_glossary = if args.glossary_from_dataset {
            Some(write_dataset_glossary(&cases)?)
        } else {
            None
        };
        build_translator(&path, dataset_glossary)?
    };

    let cases = match args.limit {
        Some(limit) => cases.into_iter().take(limit).collect(),
        None => cases,
    };

    let mut results = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut latencies = Vec::new();
    let usage_before = translator.usage();

    for case in &cases {
        let started = std::time::Instant::now();
        let request = TranslateRequest::new(
            case.text.clone(),
            LanguageTag::new(&case.target_lang).map_err(|e| e.message)?,
        )
        .with_source(LanguageTag::new(&case.source_lang).map_err(|e| e.message)?)
        // 评测对象是远程 LLM：必须显式授权，不能用默认的"本地优先、绝不自动回退远程"
        .with_policy(TranslationPolicy::RemoteOnly);

        let mut options = translator.default_options().clone();
        options.text_kind = TextKind::Sentence;
        options.mode = args.mode;

        let outcome =
            pollster::block_on(translator.translate_with_diagnostics_options(&request, &options));
        let elapsed = started.elapsed().as_millis() as u64;
        latencies.push(elapsed);

        let record = match outcome {
            Ok(output) => {
                let result = &output.result;
                let mut failures = Vec::new();
                // 1) 字面约束：URL / 邮箱 / 变量 / 代码 / 标签必须逐字出现
                for needle in &case.must_preserve {
                    if !result.text.contains(needle) {
                        failures.push(format!("缺少受保护内容 {needle}"));
                    }
                }
                // 2) 数字约束：先看字面，再看 SDK 的语义判定
                //    （`$39.99` 写成 `39.99美元`、`2:00` 写成 `凌晨2点` 都是正确的）
                let critical_numbers: Vec<String> = output
                    .diagnostics
                    .quality
                    .as_ref()
                    .map(|quality| {
                        quality
                            .issues
                            .iter()
                            .filter(|issue| {
                                issue.severity == translator_api::QualitySeverity::Critical
                            })
                            .map(|issue| {
                                format!(
                                    "SDK 判定致命问题 {}：{}",
                                    issue.kind.as_str(),
                                    issue.detail
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // SDK 语义层报告的"数字/金额缺失或新增"（Major）在**用例声明了数字**时算失败：
                // 这样币种丢失、金额被改会被抓住，而 SDK 对 "one hand"/"every second"
                // 这类词形歧义的启发式判断不会污染结果（那些用例本身不声明数字）。
                if !case.must_keep_numbers.is_empty() {
                    if let Some(quality) = output.diagnostics.quality.as_ref() {
                        for issue in &quality.issues {
                            if issue.severity != translator_api::QualitySeverity::Major {
                                continue;
                            }
                            if matches!(
                                issue.kind,
                                translator_api::QualityIssueKind::NumberMissing
                                    | translator_api::QualityIssueKind::NumberAdded
                                    | translator_api::QualityIssueKind::NumberRelationMismatch
                            ) {
                                failures.push(format!(
                                    "数字/金额问题 {}：{}",
                                    issue.kind.as_str(),
                                    issue.detail
                                ));
                            }
                        }
                    }
                }
                for needle in &case.must_keep_numbers {
                    if !result.text.contains(needle) && !critical_numbers.is_empty() {
                        failures.push(format!("关键数字未保留 {needle}"));
                    }
                }
                failures.extend(critical_numbers);
                // 3) 术语
                for term in &case.glossary {
                    if !result.text.contains(&term.target) {
                        failures.push(format!("术语未遵循 {} → {}", term.source, term.target));
                    }
                }
                let ok = failures.is_empty();
                CaseResult {
                    id: case.id.clone(),
                    category: case.category.clone(),
                    ok,
                    output: result.text.clone(),
                    engine_id: result.engine_id.clone(),
                    elapsed_ms: elapsed,
                    failures,
                    error: None,
                    notes: case.notes.clone(),
                }
            }
            Err(err) => CaseResult {
                id: case.id.clone(),
                category: case.category.clone(),
                ok: false,
                output: String::new(),
                engine_id: String::new(),
                elapsed_ms: elapsed,
                failures: vec![format!("翻译失败：{}（{}）", err.message, err.code)],
                error: Some(err.code.as_str().to_string()),
                notes: case.notes.clone(),
            },
        };

        if record.ok {
            passed += 1;
        } else {
            failed += 1;
        }
        results.push(record);
    }

    let mut by_category: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for record in &results {
        let slot = by_category.entry(record.category.clone()).or_insert((0, 0));
        if record.ok {
            slot.0 += 1;
        } else {
            slot.1 += 1;
        }
    }
    latencies.sort_unstable();
    let latency_p50 = latencies.get(latencies.len() / 2).copied().unwrap_or(0);
    let latency_max = latencies.last().copied().unwrap_or(0);

    let usage_after = translator.usage();
    let _ = &usage_before;
    let report = Report {
        dataset: args.dataset.display().to_string(),
        mode: format!("{:?}", args.mode),
        mock: args.mock,
        total: results.len(),
        passed,
        failed,
        latency_p50_ms: latency_p50,
        latency_p95_ms: percentile(&latencies, 95),
        latency_max_ms: latency_max,
        http_calls: usage_after.iter().map(|(_, u)| u.http_calls_started).sum(),
        content_retries: usage_after.iter().map(|(_, u)| u.content_retries).sum(),
        tokens_input: usage_after.iter().map(|(_, u)| u.tokens.input_tokens).sum(),
        tokens_output: usage_after
            .iter()
            .map(|(_, u)| u.tokens.output_tokens)
            .sum(),
        estimated_cost_usd: translator.estimated_cost_usd(),
        by_category: by_category
            .into_iter()
            .map(|(k, (ok, bad))| CategoryStat {
                category: k,
                passed: ok,
                failed: bad,
            })
            .collect(),
        // 人工质量测评**没有**执行；不要用自动指标替代它（§16.4）
        human_review: "PENDING_HUMAN".to_string(),
        cases: results,
    };

    if let Some(path) = &args.out_md {
        let markdown = render_markdown(&report, &cases);
        std::fs::write(path, markdown)
            .map_err(|e| format!("写入 Markdown 报告失败（{}）：{e}", path.display()))?;
        eprintln!("评审包（Markdown）：{}", path.display());
    }

    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(&args.out, text).map_err(|e| format!("写入报告失败：{e}"))?;

    println!(
        "评测完成：{} 条，通过 {}，失败 {}；报告写入 {}",
        report.total,
        report.passed,
        report.failed,
        args.out.display()
    );
    println!("注意：人工质量评审状态 = PENDING_HUMAN（自动指标不能替代人工验收）");

    Ok(if report.failed == 0 {
        EXIT_OK
    } else {
        EXIT_FAIL
    })
}

/// 延迟百分位（排序后取最近秩）。
fn percentile(values: &[u64], p: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = (sorted.len() - 1) * p / 100;
    sorted[rank]
}

/// 生成人工评审包：原文/译文对照 + 机器判定的失败项。
///
/// 明确标注 `PENDING_HUMAN`：自动指标通过不等于质量验收通过。
fn render_markdown(report: &Report, cases: &[EvalCase]) -> String {
    let mut out = String::new();
    out.push_str("# 翻译质量评审包\n\n");
    out.push_str(&format!("- 数据集：`{}`\n", report.dataset));
    out.push_str(&format!(
        "- 模式：{}（mock={}）\n",
        report.mode, report.mock
    ));
    out.push_str(&format!(
        "- 机器可判定项：{} / {} 通过（失败 {}）\n",
        report.passed, report.total, report.failed
    ));
    out.push_str(&format!(
        "- 延迟：p50 {} ms / p95 {} ms / max {} ms\n",
        report.latency_p50_ms, report.latency_p95_ms, report.latency_max_ms
    ));
    out.push_str(&format!(
        "- HTTP 调用 {} 次，内容重试 {} 次，token {}/{}\n",
        report.http_calls, report.content_retries, report.tokens_input, report.tokens_output
    ));
    match report.estimated_cost_usd {
        Some(cost) => out.push_str(&format!(
            "- 估算费用：${cost:.6}（按配置单价，最终以服务端账单为准）\n"
        )),
        None => out.push_str("- 估算费用：未配置单价（不做估算）\n"),
    }
    out.push_str(&format!("- 人工评审：**{}**\n\n", report.human_review));
    out.push_str("> 自动指标只覆盖\"必须保留的内容\"，不代表译文准确、自然。\n\n");

    out.push_str("| id | 类别 | 结果 | 原文 | 译文 | 机器判定 |\n");
    out.push_str("|---|---|---|---|---|---|\n");
    for case in &report.cases {
        let source = cases
            .iter()
            .find(|c| c.id == case.id)
            .map(|c| c.text.as_str())
            .unwrap_or("");
        let verdict = if case.ok { "✅" } else { "❌" };
        let notes = if case.failures.is_empty() {
            case.notes.clone()
        } else {
            case.failures.join("；")
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            case.id,
            case.category,
            verdict,
            escape_table(source),
            escape_table(&case.output),
            escape_table(&notes)
        ));
    }
    out
}

fn escape_table(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn mock_translator() -> Translator {
    let builder = Translator::builder()
        .engine(translator_plugin_api::mocks::MockNmtEngine::new())
        .expect("注册 mock 引擎失败");
    let builder = builder.cache_capacity(0);
    builder.build().expect("构建 mock translator 失败")
}

fn build_translator(
    path: &PathBuf,
    dataset_glossary: Option<PathBuf>,
) -> Result<Translator, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取配置失败：{e}"))?;
    let file: RunnerConfig = toml::from_str(&text).map_err(|e| format!("配置解析失败：{e}"))?;
    let mut builder = Translator::builder();
    let backends = BackendsFile {
        http: file.http,
        backend: file.backend,
    };
    builder = builder
        .http_backends(&backends)
        .map_err(|e| format!("后端配置非法：{}", e.message))?;
    if let Some(glossary) = dataset_glossary.as_ref().or(file.glossary.as_ref()) {
        builder = builder.glossary_file(glossary).map_err(|e| e.message)?;
        // 评测关注"术语约束是否被遵守"，因此用 enforce；
        // 注意 enforce 只让**不满足**的翻译失败，不会替换译文用词。
        builder = builder.term_policy(translator_api::TermPolicy::Enforce);
    }
    if let Some(templates) = &file.templates {
        builder = builder.templates_file(templates).map_err(|e| e.message)?;
    }
    builder.build().map_err(|e| e.message)
}

// ============================================================
// 数据与报告
// ============================================================

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    #[serde(default)]
    category: String,
    text: String,
    #[serde(default = "default_source")]
    source_lang: String,
    #[serde(default = "default_target")]
    target_lang: String,
    #[serde(default)]
    must_preserve: Vec<String>,
    #[serde(default)]
    must_keep_numbers: Vec<String>,
    #[serde(default)]
    glossary: Vec<GlossaryPair>,
    #[serde(default)]
    notes: String,
}

/// 把数据集里声明的术语对写成临时术语表（JSON），用于 `Enforce` 评测。
fn write_dataset_glossary(cases: &[EvalCase]) -> Result<PathBuf, String> {
    let mut entries = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for case in cases {
        for pair in &case.glossary {
            let key = (pair.source.clone(), pair.target.clone());
            if !seen.insert(key) {
                continue;
            }
            entries.push(serde_json::json!({
                "id": format!("eval-{}", entries.len() + 1),
                "source": pair.source,
                "target": pair.target,
                "strict": true,
            }));
        }
    }
    if entries.is_empty() {
        return Err("数据集里没有 glossary 字段，无法生成术语表".to_string());
    }
    let spec = serde_json::json!({
        "schema_version": 1,
        "id": "eval.dataset",
        "version": 1,
        "entries": entries,
    });
    let path = std::env::temp_dir().join("augenstern-eval-glossary.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&spec).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写入临时术语表失败：{e}"))?;
    eprintln!(
        "术语表（来自数据集）：{} 条 → {}",
        entries.len(),
        path.display()
    );
    Ok(path)
}

fn default_source() -> String {
    "en".to_string()
}
fn default_target() -> String {
    "zh-CN".to_string()
}

#[derive(Debug, Deserialize)]
struct GlossaryPair {
    source: String,
    target: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // 与 CLI 共用同一份配置格式，未用到的段落允许存在
struct RunnerConfig {
    #[serde(default)]
    http: HttpSettings,
    #[serde(default)]
    backend: BTreeMap<String, BackendConfig>,
    #[serde(default)]
    glossary: Option<PathBuf>,
    #[serde(default)]
    templates: Option<PathBuf>,
    /// 允许出现但本工具不用的段落（避免复制 CLI 配置时解析失败）
    #[serde(default)]
    translation: Option<toml::Value>,
    #[serde(default)]
    cache: Option<toml::Value>,
    #[serde(default)]
    dictionary: Option<PathBuf>,
    #[serde(default)]
    rules: Option<PathBuf>,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    page: Option<String>,
}

fn load_dataset(path: &PathBuf) -> Result<Vec<EvalCase>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取数据集 {} 失败：{e}", path.display()))?;
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let case: EvalCase = serde_json::from_str(line)
            .map_err(|e| format!("第 {} 行不是合法 JSON：{e}", idx + 1))?;
        out.push(case);
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
struct Report {
    dataset: String,
    mode: String,
    mock: bool,
    total: usize,
    passed: usize,
    failed: usize,
    latency_p50_ms: u64,
    latency_p95_ms: u64,
    latency_max_ms: u64,
    /// 本次运行实际发出的物理 HTTP 调用数。
    http_calls: u64,
    /// 内容完整性重试次数。
    content_retries: u64,
    /// token 用量（所有后端合计）。
    tokens_input: u64,
    tokens_output: u64,
    /// 费用估算（美元）。未配置单价时为 `null`，不要当成账单。
    estimated_cost_usd: Option<f64>,
    by_category: Vec<CategoryStat>,
    /// 人工质量评审状态。自动指标通过不等于质量验收通过。
    human_review: String,
    cases: Vec<CaseResult>,
}

#[derive(Debug, Serialize)]
struct CategoryStat {
    category: String,
    passed: usize,
    failed: usize,
}

#[derive(Debug, Serialize)]
struct CaseResult {
    id: String,
    category: String,
    ok: bool,
    output: String,
    engine_id: String,
    elapsed_ms: u64,
    failures: Vec<String>,
    error: Option<String>,
    notes: String,
}

// ============================================================
// 参数
// ============================================================

struct Args {
    dataset: PathBuf,
    out: PathBuf,
    /// 可选：同时输出 Markdown 评审包（原文/译文对照，给人读）。
    out_md: Option<PathBuf>,
    /// 用数据集里的 `glossary` 字段生成临时术语表，并按 `Enforce` 跑。
    glossary_from_dataset: bool,
    config: Option<PathBuf>,
    mock: bool,
    mode: TranslationMode,
    limit: Option<usize>,
}

impl Args {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        let mut dataset = None;
        let mut out = None;
        let mut config = None;
        let mut mock = false;
        let mut mode = TranslationMode::Precise;
        let mut limit = None;
        let mut out_md = None;
        let mut glossary_from_dataset = false;

        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            let next = |name: &str| -> Result<String, String> {
                let value = args.get(i + 1).ok_or_else(|| format!("{name} 需要值"))?;
                Ok(value.clone())
            };
            match arg {
                "--dataset" | "-d" => {
                    dataset = Some(PathBuf::from(next("--dataset")?));
                    i += 2;
                }
                "--out" | "-o" => {
                    out = Some(PathBuf::from(next("--out")?));
                    i += 2;
                }
                "--config" | "-c" => {
                    config = Some(PathBuf::from(next("--config")?));
                    i += 2;
                }
                "--out-md" => {
                    out_md = Some(PathBuf::from(next("--out-md")?));
                    i += 2;
                }
                "--limit" => {
                    limit = Some(
                        next("--limit")?
                            .parse()
                            .map_err(|_| "--limit 必须是整数".to_string())?,
                    );
                    i += 2;
                }
                "--mode" => {
                    let value = next("--mode")?;
                    mode = match value.as_str() {
                        "precise" => TranslationMode::Precise,
                        "fast" => TranslationMode::Fast,
                        _ => return Err("--mode 只能是 precise 或 fast".to_string()),
                    };
                    i += 2;
                }
                "--mock" => {
                    mock = true;
                    i += 1;
                }
                "--glossary-from-dataset" => {
                    glossary_from_dataset = true;
                    i += 1;
                }
                "--help" | "-h" => {
                    println!(
                        "quality-runner --dataset <jsonl> --out <report.json> \
                         [--config <translate.toml>] [--mode precise|fast] [--limit N] [--mock]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("未知参数：{other}")),
            }
        }

        Ok(Self {
            dataset: dataset.ok_or("缺少 --dataset")?,
            out: out.ok_or("缺少 --out")?,
            out_md,
            glossary_from_dataset,
            config,
            mock,
            mode,
            limit,
        })
    }
}
