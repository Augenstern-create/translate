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
use translator_types::{LanguageTag, TextKind, TranslateRequest, TranslationMode};

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
        build_translator(&path)?
    };

    let cases = match args.limit {
        Some(limit) => cases.into_iter().take(limit).collect(),
        None => cases,
    };

    let mut results = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut latencies = Vec::new();

    for case in &cases {
        let started = std::time::Instant::now();
        let request = TranslateRequest::new(
            case.text.clone(),
            LanguageTag::new(&case.target_lang).map_err(|e| e.message)?,
        )
        .with_source(LanguageTag::new(&case.source_lang).map_err(|e| e.message)?);

        let mut options = translator.default_options().clone();
        options.text_kind = TextKind::Sentence;
        options.mode = args.mode;

        let outcome = pollster::block_on(translator.translate_with_options(&request, &options));
        let elapsed = started.elapsed().as_millis() as u64;
        latencies.push(elapsed);

        let record = match outcome {
            Ok(output) => {
                let mut failures = Vec::new();
                for needle in &case.must_preserve {
                    if !output.text.contains(needle) {
                        failures.push(format!("缺少受保护内容 {needle}"));
                    }
                }
                for needle in &case.must_keep_numbers {
                    if !output.text.contains(needle) {
                        // 数字可能被写成中文；这里只做"字面保留"的严格判定，
                        // 语义等价由 SDK 的 quality::number 负责（已在下游执行）
                        failures.push(format!("关键数字未字面保留 {needle}"));
                    }
                }
                for term in &case.glossary {
                    if !output.text.contains(&term.target) {
                        failures.push(format!("术语未遵循 {} → {}", term.source, term.target));
                    }
                }
                let ok = failures.is_empty();
                CaseResult {
                    id: case.id.clone(),
                    category: case.category.clone(),
                    ok,
                    output: output.text.clone(),
                    engine_id: output.engine_id.clone(),
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

    let report = Report {
        dataset: args.dataset.display().to_string(),
        mode: format!("{:?}", args.mode),
        mock: args.mock,
        total: results.len(),
        passed,
        failed,
        latency_p50_ms: latency_p50,
        latency_max_ms: latency_max,
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

fn mock_translator() -> Translator {
    let builder = Translator::builder()
        .engine(translator_plugin_api::mocks::MockNmtEngine::new())
        .expect("注册 mock 引擎失败");
    let builder = builder.cache_capacity(0);
    builder.build().expect("构建 mock translator 失败")
}

fn build_translator(path: &PathBuf) -> Result<Translator, String> {
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
    if let Some(glossary) = &file.glossary {
        builder = builder.glossary_file(glossary).map_err(|e| e.message)?;
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
    latency_max_ms: u64,
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
            config,
            mock,
            mode,
            limit,
        })
    }
}
