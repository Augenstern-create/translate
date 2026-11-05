//! P4-05：**语义与结构回归**（SP-01～12、LC-01～10 中适用于进程内 ORT 的部分）。
//!
//! 与 Bergamot 那套（`crates/translator-nmt-bergamot/tests/sp_real.rs`）的区别先说清：
//! 判定口径完全一致（**字节级结构比较**，受保护片段逐字节出现、次数正确、顺序不乱），
//! 但本后端**跑在宿主进程内**，所以"native 死亡""进程隔离""租约"这一族用例**不适用**，
//! 适用/不适用的判定写在 `docs/26` 的 LC 表里，不在这里假装通过。
//!
//! ```bash
//! export P4_ORT_MODEL_DIR=/path/to/ort-model
//! cargo test -p translator-nmt-ort --features ort-runtime --test p4_contract \
//!     -- --ignored --test-threads=1 --nocapture
//! ```

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use translator_core::protection::{ProtectionMode, ProtectionPlan};
use translator_nmt_ort::{OrtNmtConfig, OrtNmtEngine, DECODER_FILE, ENCODER_FILE, TOKENIZER_FILE};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest, TranslationErrorCode, TranslationResult};

fn model_dir() -> PathBuf {
    PathBuf::from(std::env::var("P4_ORT_MODEL_DIR").expect("需要 P4_ORT_MODEL_DIR"))
}

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn config() -> OrtNmtConfig {
    OrtNmtConfig::new(model_dir(), en(), zh()).with_threads(1)
}

/// 同一进程内共享一个引擎：加载一次就要 ~1.3 s、常驻 ~313 MiB，不必每个用例各起一份。
fn shared() -> &'static Arc<OrtNmtEngine> {
    static ENGINE: OnceLock<Arc<OrtNmtEngine>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let engine = OrtNmtEngine::new(config()).expect("构造引擎失败");
        engine.load().expect("加载真实模型失败");
        Arc::new(engine)
    })
}

fn translate(engine: &OrtNmtEngine, text: &str) -> Result<TranslationResult, TranslationErrorCode> {
    let req = TranslateRequest::new(text, zh()).with_source(en());
    pollster::block_on(engine.translate(&req)).map_err(|e| e.code)
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// 结构断言：受保护片段按原文顺序逐字节出现，且次数与原文一致。
fn assert_structure(source: &str, output: &str) {
    let plan = ProtectionPlan::build(source);
    let mut cursor = 0usize;
    for item in plan.items() {
        let found = output[cursor..]
            .find(&item.original)
            .unwrap_or_else(|| panic!("受保护片段 {:?} 丢失或乱序：{output:?}", item.original));
        cursor += found + item.original.len();
        assert_eq!(
            count(output, &item.original),
            count(source, &item.original),
            "受保护片段 {:?} 次数不符：{output:?}",
            item.original
        );
    }
}

// ------------------------------------------------------------ SP-01～07

#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn sp01_03_模板变量单个多种重复都完整() {
    let engine = shared();
    for source in [
        "Hello {name}!",
        "call {order_id} for {name}",
        "{name} and {name}",
    ] {
        let out = translate(engine, source).expect("翻译失败").text;
        assert_structure(source, &out);
        println!("{source} -> {out}");
    }
}

#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn sp04_05_url_email_查询串精确保留() {
    let engine = shared();
    for source in [
        "Visit https://example.com/docs today.",
        "See https://example.com/a?b=1&c=2#frag now.",
        "Write to hi@example.com before Friday.",
        "Mixed: {plan} at https://x.dev/y and mail hi@example.com.",
    ] {
        let out = translate(engine, source).expect("翻译失败").text;
        assert_structure(source, &out);
        println!("{source} -> {out}");
    }
}

#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn sp06_07_反引号代码整体保留且内嵌url不重复() {
    let engine = shared();
    for source in [
        "Run `cargo test --all-features` before committing.",
        "Fetch `https://example.com/api?x=1` and cache it.",
    ] {
        let out = translate(engine, source).expect("翻译失败").text;
        assert_structure(source, &out);
        println!("{source} -> {out}");
    }
}

// ------------------------------------------------------------ SP-10～12

#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn sp10_含emoji与中日韩混排不崩且结构完好() {
    let engine = shared();
    // 只要不是"受保护跨度"，这些字符就得靠模型自己带出来（见下一条 D-71）。
    let source = "Deploy 🚀 to 東京 and {env} 🎯 now.";
    let out = translate(engine, source)
        .expect("含 emoji 的句子不该失败")
        .text;
    assert_structure(source, out.as_str());
    assert!(!out.is_empty());
    println!("{source} -> {out}");
}

/// **D-71 的特征化测试**：词表外的字符（emoji 等）**不保证保留**。
///
/// 这不是"通过"，而是把已知缺陷钉成可执行事实：候选模型的联合词表里没有 emoji，
/// 它们只能变成 `<unk>`，译文里退化成下划线（实测 `部署 至 _______{env}现在`）。
/// P3 的 Bergamot 能原样带出这些字符，所以这是**本候选的退化**，不是"本地 NMT 都这样"。
/// 修法只有两条：走标记模式（本后端不支持，D-61）或改核心保护策略（冻结，ADR-044）。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn d71_词表外字符不保证保留() {
    let engine = shared();
    let out = translate(engine, "Deploy 🚀 now.").expect("翻译失败").text;
    println!("Deploy 🚀 now. -> {out}");
    assert!(
        !out.contains('🚀'),
        "如果这条开始失败，说明 D-71 被修好了（emoji 能带出来了）：请更新 docs/26 与 D-71"
    );
}

#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn sp11_多行长文本换行与顺序保留() {
    let engine = shared();
    let source = "Line one with {a}.\nLine two with https://example.com/b.\nLine three.";
    let out = translate(engine, source).expect("翻译失败").text;
    assert_structure(source, &out);
    assert_eq!(count(&out, "\n"), 2, "换行数必须保留：{out:?}");
    println!("{source:?} -> {out:?}");
}

#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn sp12_全保护与空白输入不崩溃() {
    let engine = shared();

    // 整句都是受保护片段：没有任何纯文本可送进模型，必须原样返回而不是崩溃/空串
    let all = "{a} https://example.com `x` {b}";
    let out = translate(engine, all).expect("全保护输入不该失败").text;
    assert_structure(all, &out);

    // 空白：显式 InvalidRequest，不把空串当译文
    let err = translate(engine, "   ").expect_err("空文本必须报错");
    assert_eq!(err, TranslationErrorCode::InvalidRequest);
    println!("all-protected -> {out:?}");
}

// ------------------------------------------------------------ SP-08/09

/// SP-08/09（冲突/伪造标记、丢失/复制/篡改 → `IntegrityFailure`）在**标记模式**下才成立，
/// 而本后端不支持标记模式（D-61）。这里断言的是"**明确拒绝**"，不是"结构完好"。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn sp08_09_标记模式显式不支持而不是假装通过() {
    let mut cfg = config();
    cfg.protection = ProtectionMode::Marker;
    let err = OrtNmtEngine::new(cfg).expect_err("标记模式必须被拒绝");
    assert_eq!(err.code, TranslationErrorCode::CapabilityUnavailable);
    assert!(err.message.contains("protect-seg-v1"), "{}", err.message);
}

// ------------------------------------------------------------ LC-01/02/03/06/09

/// LC-01：并发加载同一个模型，只能有一份会话，身份必须一致。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn lc01_并发加载只有一份会话() {
    let engine = Arc::new(OrtNmtEngine::new(config()).expect("构造失败"));
    let mut handles = Vec::new();
    for _ in 0..10 {
        let e = engine.clone();
        handles.push(std::thread::spawn(move || e.load().expect("并发加载失败")));
    }
    let mut ids: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().expect("线程 panic").model_revision)
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 1, "并发加载得到的身份必须唯一：{ids:?}");
    assert!(engine.is_loaded());
}

/// LC-02：翻译进行中 unload，不得死锁，也不得把正在跑的那次结果弄坏。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn lc02_翻译中卸载不死锁() {
    let engine = Arc::new(OrtNmtEngine::new(config()).expect("构造失败"));
    engine.load().expect("加载失败");

    let e = engine.clone();
    let worker = std::thread::spawn(move || translate(&e, "The cat is on the table."));
    // 让 worker 先进到解码里，再请求卸载
    std::thread::sleep(std::time::Duration::from_millis(50));
    engine.unload().expect("卸载失败");

    let out = worker.join().expect("worker panic");
    // 两种结果都可接受：要么这次翻译正常完成（unload 等到它跑完），要么显式失败；
    // 不可接受的是死锁、panic 或"半截译文"。
    match out {
        Ok(text) => {
            assert!(!text.text.is_empty());
            println!("翻译先完成：{}", text.text);
        }
        Err(code) => {
            assert_eq!(code, TranslationErrorCode::CapabilityUnavailable);
            println!("卸载先完成，翻译显式失败：{code:?}");
        }
    }
}

/// LC-03 / LC-09：同 id 但**文件内容不同**必须得到不同身份（没有缓存可串，身份也不许复用）。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn lc03_09_内容不同则身份不同() {
    let src = model_dir();
    let tmp = tempfile::tempdir().expect("临时目录失败");
    let copy = tmp.path().join("model");
    std::fs::create_dir(&copy).expect("建目录失败");
    for name in [ENCODER_FILE, DECODER_FILE, TOKENIZER_FILE] {
        let from = src.join(name);
        let to = copy.join(name);
        // 优先硬链接（省 111 MiB 拷贝），失败再退回复制
        if std::fs::hard_link(&from, &to).is_err() {
            std::fs::copy(&from, &to).expect("复制模型文件失败");
        }
    }

    let original = OrtNmtEngine::new(OrtNmtConfig::new(&copy, en(), zh()))
        .expect("构造失败")
        .load()
        .expect("加载失败")
        .model_revision;

    // 断链后改写 tokenizer.json（追加一个换行：字节变了、JSON 仍合法）
    let tk = copy.join(TOKENIZER_FILE);
    std::fs::remove_file(&tk).expect("断链失败");
    let mut text = std::fs::read_to_string(src.join(TOKENIZER_FILE)).expect("读 tokenizer 失败");
    text.push('\n');
    std::fs::write(&tk, text).expect("写 tokenizer 失败");

    let mutated = OrtNmtEngine::new(OrtNmtConfig::new(&copy, en(), zh()))
        .expect("构造失败")
        .load()
        .expect("加载失败")
        .model_revision;

    assert_ne!(
        original, mutated,
        "文件内容变了，身份却一样：任何缓存都会串味"
    );
    println!("{original} -> {mutated}");
}

/// LC-06：连续 10 次加载/卸载不得无限堆积（这是"进程内跑 native"最容易翻车的地方）。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn lc06_反复加载卸载不无限堆积() {
    fn rss_mib() -> f64 {
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let kib: u64 = s
            .lines()
            .find(|l| l.starts_with("VmRSS:"))
            .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
            .unwrap_or(0);
        kib as f64 / 1024.0
    }

    let engine = OrtNmtEngine::new(config()).expect("构造失败");
    let mut marks = Vec::new();
    for cycle in 0..10 {
        engine.load().expect("加载失败");
        let out = translate(&engine, "Hello world!").expect("翻译失败");
        assert!(!out.text.is_empty());
        engine.unload().expect("卸载失败");
        assert!(!engine.is_loaded(), "第 {cycle} 轮卸载后仍是已加载");
        marks.push(rss_mib());
    }
    let first = marks[1]; // 第 0 轮含进程首次抬升
    let last = *marks.last().unwrap();
    println!("RSS 逐轮（MiB）：{marks:?}");
    assert!(
        last - first < 200.0,
        "10 轮加载/卸载后 RSS 涨了 {:.1} MiB（{first:.1} → {last:.1}）：arena 没还回去",
        last - first
    );
}

/// LC-04（deadline）、LC-05（native 死亡）、LC-07（依赖租约）、LC-08（安装期 kill -9）
/// 在本后端**不适用**：没有 worker 进程、没有安装流程、没有租约。
///
/// 这条测试把"不适用"写成了可执行的事实：`timeout_ms` 目前被忽略（D-63），
/// 因此它必须能**证明**自己没生效，而不是让读者以为超时已被处理。
#[test]
#[ignore = "需要真实 ORT 模型资产（P4_ORT_MODEL_DIR）"]
fn lc04_超时字段当前被忽略() {
    let engine = shared();
    let mut req = TranslateRequest::new("The cat is on the table.", zh()).with_source(en());
    req.timeout_ms = Some(1); // 1 ms，必然"超时"
    let out = pollster::block_on(engine.translate(&req));
    assert!(
        out.is_ok(),
        "如果这条开始失败，说明超时真的实现了：请更新 docs/26 与 D-63"
    );
    println!(
        "timeout_ms=1 仍照常返回：{}（D-63 未实现取消，超时需宿主在外层等）",
        out.unwrap().text
    );
}
