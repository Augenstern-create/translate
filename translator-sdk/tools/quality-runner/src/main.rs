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

mod metrics;

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

    let cases = match args.limit {
        Some(limit) => cases.into_iter().take(limit).collect(),
        None => cases,
    };

    let mut reports: Vec<(String, Report)> = Vec::new();
    let labels: Vec<(String, Option<PathBuf>)> = match &args.compare {
        Some(list) => list
            .split(',')
            .map(|item| {
                let item = item.trim();
                let label = PathBuf::from(item)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| item.to_string());
                (label, Some(PathBuf::from(item)))
            })
            .collect(),
        None => vec![(
            args.config
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "mock".to_string()),
            args.config.clone(),
        )],
    };

    for (label, config) in &labels {
        let translator = if args.mock {
            mock_translator()
        } else {
            let path = config.clone().ok_or_else(|| {
                "需要 --config <translate.toml>（或用 --mock 跑通链路）".to_string()
            })?;
            let dataset_glossary = if args.glossary_from_dataset {
                Some(write_dataset_glossary(&cases)?)
            } else {
                None
            };
            build_translator(&path, dataset_glossary)?
        };
        let report = evaluate(&cases, &translator, &args)?;
        reports.push((label.clone(), report));
    }

    // 单配置：沿用原来的输出行为
    if reports.len() == 1 {
        let (_, report) = reports.pop().expect("至少一个报告");
        write_outputs(&report, &cases, &args)?;
        println!(
            "评测完成：{} 条，通过 {}，失败 {}；报告写入 {}",
            report.total,
            report.passed,
            report.failed,
            args.out.display()
        );
        return Ok(if report.failed == 0 {
            EXIT_OK
        } else {
            EXIT_FAIL
        });
    }

    // 多配置：每个配置写一份 JSON，另出一份对照表
    let stem = args
        .out
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "compare".to_string());
    let mut table = String::from("# 多模型对照\n\n");
    table.push_str(&format!(
        "数据集：`{}`（{} 条）\n\n",
        args.dataset.display(),
        cases.len()
    ));
    table.push_str("| 配置 | 通过 | chrF++ | BLEU-4 | p50 | p95 | HTTP 调用 | 输入/输出 token |\n|---|---|---|---|---|---|---|---|\n");
    for (label, report) in &reports {
        let chrf = report
            .metrics
            .as_ref()
            .map(|m| format!("{:.2}", m.chrf_pp))
            .unwrap_or_else(|| "—".to_string());
        let bleu = report
            .metrics
            .as_ref()
            .map(|m| format!("{:.2}", m.bleu4))
            .unwrap_or_else(|| "—".to_string());
        table.push_str(&format!(
            "| {} | {}/{} | {} | {} | {} ms | {} ms | {} | {}/{} |\n",
            label,
            report.passed,
            report.total,
            chrf,
            bleu,
            report.latency_p50_ms,
            report.latency_p95_ms,
            report.http_calls,
            report.tokens_input,
            report.tokens_output
        ));
        let path = args.out.with_file_name(format!("{stem}-{label}.json"));
        let text = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    }
    table.push_str("\n> 人工评审：**PENDING_HUMAN**；自动指标不替代人工验收。\n");
    let md_path = args.out.with_file_name(format!("{stem}-compare.md"));
    std::fs::write(&md_path, &table)
        .map_err(|e| format!("写入 {} 失败：{e}", md_path.display()))?;
    println!("{table}");
    println!("对照表：{}", md_path.display());
    let failed: usize = reports.iter().map(|(_, r)| r.failed).sum();
    Ok(if failed == 0 { EXIT_OK } else { EXIT_FAIL })
}

/// 并行跑用例：固定 `jobs` 个线程从同一个原子索引取任务，结果按原顺序落位。
fn evaluate_parallel(
    cases: &[EvalCase],
    translator: &Translator,
    args: &Args,
    jobs: usize,
) -> Result<Vec<CaseResult>, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let jobs = jobs.clamp(1, 16).min(cases.len().max(1));
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<CaseResult>>> =
        Mutex::new((0..cases.len()).map(|_| None).collect());
    let error: Mutex<Option<String>> = Mutex::new(None);

    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::SeqCst);
                if index >= cases.len() {
                    return;
                }
                match evaluate_case(translator, args, &cases[index]) {
                    Ok(record) => {
                        slots.lock().unwrap()[index] = Some(record);
                    }
                    Err(err) => {
                        *error.lock().unwrap() = Some(err);
                        return;
                    }
                }
            });
        }
    });

    if let Some(err) = error.into_inner().unwrap() {
        return Err(err);
    }
    Ok(slots
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|slot| slot.expect("并行执行漏掉了用例"))
        .collect())
}

fn write_outputs(report: &Report, cases: &[EvalCase], args: &Args) -> Result<(), String> {
    if let Some(path) = &args.out_md {
        let markdown = render_markdown(report, cases);
        std::fs::write(path, markdown)
            .map_err(|e| format!("写入 Markdown 报告失败（{}）：{e}", path.display()))?;
        eprintln!("评审包（Markdown）：{}", path.display());
    }
    let text = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&args.out, text).map_err(|e| format!("写入报告失败：{e}"))?;
    Ok(())
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

    out.push_str("| id | 数据集/文档 | 类别 | 结果 | 原文 | 译文 | 机器判定 |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for case in &report.cases {
        let source = cases
            .iter()
            .find(|c| c.id == case.id)
            .map(|c| c.source_text())
            .unwrap_or("");
        let verdict = if case.ok { "✅" } else { "❌" };
        let notes = if case.failures.is_empty() {
            case.notes.clone()
        } else {
            case.failures.join("；")
        };
        let dataset = cases
            .iter()
            .find(|c| c.id == case.id)
            .map(|c| {
                let dataset = if c.dataset_id.is_empty() {
                    "handcrafted"
                } else {
                    &c.dataset_id
                };
                if c.document_id.is_empty() {
                    dataset.to_string()
                } else {
                    format!("{dataset}/{}", c.document_id)
                }
            })
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            case.id,
            dataset,
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

/// 在给定 translator 上跑一遍数据集，产出报告（不做 I/O）。
fn evaluate(cases: &[EvalCase], translator: &Translator, args: &Args) -> Result<Report, String> {
    let mut latencies: Vec<u64> = Vec::new();
    let usage_before = translator.usage();

    let records: Vec<CaseResult> = if args.jobs <= 1 {
        cases
            .iter()
            .map(|case| evaluate_case(translator, args, case))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        evaluate_parallel(cases, translator, args, args.jobs)?
    };
    for record in &records {
        latencies.push(record.elapsed_ms);
    }
    let results = records;
    let passed = results.iter().filter(|r| r.ok).count();
    let failed = results.len() - passed;

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

    let with_reference: Vec<(String, String)> = results
        .iter()
        .zip(cases.iter())
        .filter_map(|(result, case)| {
            case.reference
                .as_ref()
                .filter(|r| !r.trim().is_empty())
                .map(|r| (result.output.clone(), r.clone()))
        })
        .collect();
    let metrics = if with_reference.is_empty() {
        None
    } else {
        let (chrf, bleu) = metrics::corpus_average(&with_reference);
        Some(Metrics {
            with_reference: with_reference.len(),
            chrf_pp: (chrf * 100.0).round() / 100.0,
            bleu4: (bleu * 100.0).round() / 100.0,
            note: "逐句平均；chrF++ 为字符 1-6gram + 词 1-2gram（β=2）；BLEU-4 用空白分词，与 sacrebleu 不可直接比较".to_string(),
        })
    };

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
        metrics,
        // 人工质量测评**没有**执行；不要用自动指标替代它（§16.4）
        human_review: "PENDING_HUMAN".to_string(),
        cases: results,
    };

    Ok(report)
}

/// 跑一条用例。可并行调用：`Translator` 是线程安全的。
fn evaluate_case(
    translator: &Translator,
    args: &Args,
    case: &EvalCase,
) -> Result<CaseResult, String> {
    let started = std::time::Instant::now();
    let request = TranslateRequest::new(
        case.source_text().to_string(),
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
                        .filter(|issue| issue.severity == translator_api::QualitySeverity::Critical)
                        .map(|issue| {
                            format!("SDK 判定致命问题 {}：{}", issue.kind.as_str(), issue.detail)
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
                category: case.effective_category().to_string(),
                ok,
                output: result.text.clone(),
                engine_id: result.engine_id.clone(),
                elapsed_ms: elapsed,
                failures,
                error: None,
                dataset_id: case.dataset_id.clone(),
                document_id: case.document_id.clone(),
                notes: case.notes.clone(),
            }
        }
        Err(err) => CaseResult {
            id: case.id.clone(),
            category: case.effective_category().to_string(),
            ok: false,
            output: String::new(),
            engine_id: String::new(),
            elapsed_ms: elapsed,
            failures: vec![format!("翻译失败：{}（{}）", err.message, err.code)],
            error: Some(err.code.as_str().to_string()),
            dataset_id: case.dataset_id.clone(),
            document_id: case.document_id.clone(),
            notes: case.notes.clone(),
        },
    };
    Ok(record)
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
    /// 旧格式（自建用例）用 `text`；外部导入用 `source`。
    #[serde(default)]
    text: String,
    /// 外部数据集字段（与 `text` 二选一）。
    #[serde(default)]
    source: String,
    /// 参考译文；没有时为 `null`（人工评审待办）。
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    dataset_id: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    document_id: String,
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

impl EvalCase {
    /// 源文：优先 `source`，兼容旧格式的 `text`。
    fn source_text(&self) -> &str {
        if self.source.is_empty() {
            &self.text
        } else {
            &self.source
        }
    }

    /// 类别：外部数据没有 category，用 domain 兜底。
    fn effective_category(&self) -> &str {
        if self.category.is_empty() {
            if self.domain.is_empty() {
                "external"
            } else {
                &self.domain
            }
        } else {
            &self.category
        }
    }
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

#[derive(Clone, Debug, Serialize)]
struct Metrics {
    /// 参与计算的样本数（有参考译文的）。
    with_reference: usize,
    /// 逐句 chrF++ 的平均值（0–100）。
    chrf_pp: f64,
    /// 逐句 BLEU-4 的平均值（0–100，空白分词，仅供参考）。
    bleu4: f64,
    /// 口径说明：不要把这里的数字当作 sacrebleu 结果。
    note: String,
}

#[derive(Serialize)]
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
    /// 自动指标（有参考译文时才计算）。chrF++ 是主指标，BLEU 仅供参考。
    metrics: Option<Metrics>,
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
    /// 数据来源（`wmt24pp` / `ntrex128` / 空表示自建用例）。
    #[serde(default)]
    dataset_id: String,
    /// 文档边界（外部数据集才有）。
    #[serde(default)]
    document_id: String,
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
    /// 可选：逗号分隔的多个配置文件，逐一跑同一数据集并输出对照表。
    compare: Option<String>,
    /// 并发执行的用例数（默认 1；远程后端建议 4）。
    jobs: usize,
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
        let mut compare: Option<String> = None;
        let mut jobs: usize = 1;

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
                "--compare" => {
                    compare = Some(next("--compare")?);
                    i += 2;
                }
                "--jobs" => {
                    jobs = next("--jobs")?
                        .parse()
                        .map_err(|_| "--jobs 必须是整数".to_string())?;
                    i += 2;
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
            compare,
            jobs,
            config,
            mock,
            mode,
            limit,
        })
    }
}
