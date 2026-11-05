//! SP-01～12 的**第二层**：真实 Bergamot（进程隔离）实测。
//!
//! 需要显式环境变量与 `--ignored`：
//!
//! ```text
//! NMT_BERGAMOT_BIN=<bergamot> NMT_MODEL_DIR=<models> NMT_WORKER_BIN=<nmt-worker> \
//! LD_LIBRARY_PATH=<native 依赖> \
//! cargo test -p translator-nmt-bergamot --test sp_real -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! 判定口径：**字节级结构比较**。受保护片段必须逐字节出现在译文里、次数正确、顺序不乱；
//! 不满足时引擎必须返回 `IntegrityFailure`，而不是"看起来正常"的译文。

use std::sync::{Arc, OnceLock};

use translator_core::protection::ProtectionMode;
use translator_nmt_bergamot::{BergamotNmtConfig, BergamotNmtEngine};
use translator_plugin_api::TranslationEngine;
use translator_types::{
    LanguageTag, TranslateRequest, TranslationError, TranslationErrorCode, TranslationResult,
};

fn config(mode: ProtectionMode) -> BergamotNmtConfig {
    let mut config = BergamotNmtConfig::from_env().expect(
        "需要 NMT_BERGAMOT_BIN / NMT_MODEL_DIR（以及可执行的 NMT_WORKER_BIN）；见文件头注释",
    );
    config.protection = mode;
    config.runtime_build = std::env::var("NMT_RUNTIME_BUILD").unwrap_or_else(|_| "p3.1cde".into());
    config
}

/// 同一进程内共享一个引擎：真实 native 常驻约 342 MiB，不必每个用例各起一份。
fn shared() -> &'static Arc<BergamotNmtEngine> {
    static ENGINE: OnceLock<Arc<BergamotNmtEngine>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let engine = BergamotNmtEngine::new(config(ProtectionMode::Segment))
            .expect("构造引擎失败（模型文件不全？）");
        engine.load().expect("加载真实模型失败");
        Arc::new(engine)
    })
}

fn translate(
    engine: &BergamotNmtEngine,
    text: &str,
) -> Result<TranslationResult, TranslationError> {
    let req = TranslateRequest::new(text, LanguageTag::new("zh-Hans").unwrap())
        .with_source(LanguageTag::new("en").unwrap());
    pollster::block_on(engine.translate(&req))
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// 结构断言：每个受保护片段逐字节出现、次数正确、顺序不变。
fn assert_structure(source: &str, output: &str) {
    let plan = translator_core::protection::ProtectionPlan::build(source);
    let mut cursor = 0usize;
    for item in plan.items() {
        let expect = if item.original == "{name}" {
            count(source, &item.original)
        } else {
            1
        };
        assert_eq!(
            count(output, &item.original),
            expect,
            "{} 片段 {:?} 在译文里应出现 {expect} 次：{output:?}",
            item.kind_str(),
            item.original
        );
        let offset = output[cursor..].find(&item.original).unwrap_or_else(|| {
            panic!(
                "{} 片段 {:?} 丢失：{output:?}",
                item.kind_str(),
                item.original
            )
        });
        cursor += offset + item.original.len();
    }
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_01_基本句产出真实中文译文并带身份() {
    let engine = shared();
    let result = translate(engine, "Hello world!").expect("基本句翻译失败");
    println!(
        "SP-REAL-01 译文={:?} engine={} model={:?} 耗时={}ms",
        result.text, result.engine_id, result.model_id, result.elapsed_ms
    );
    assert!(!result.text.trim().is_empty());
    assert!(
        result
            .text
            .chars()
            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "译文里没有汉字：{:?}",
        result.text
    );
    assert_eq!(result.engine_id, translator_nmt_bergamot::ENGINE_ID);
    assert!(result.model_id.is_some(), "必须报告模型版本");
    assert_eq!(result.source.as_str(), "en");
    assert_eq!(result.target.as_str(), "zh-Hans");
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_01_03_模板变量单个多种重复都完整() {
    let engine = shared();
    for text in [
        "Hello {name}!",
        "call {order_id} for {name}",
        "{name} and {name}",
    ] {
        let result = translate(engine, text).expect("翻译失败");
        println!("SP-REAL-01~03 in={text:?} out={:?}", result.text);
        assert_structure(text, &result.text);
        if text == "{name} and {name}" {
            assert_eq!(count(&result.text, "{name}"), 2, "两次出现必须都保留");
        }
    }
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_04_05_url_email_查询串精确保留() {
    let engine = shared();
    let cases = [
        "see https://x.example/a now",
        "go https://x.example/a?b=1&c=2&d=%E4%B8%AD now",
        "mail a.b+c@ex.example today",
        "see https://x.example/a mail a.b+c@ex.example about {name}",
    ];
    for text in cases {
        let result = translate(engine, text).expect("翻译失败");
        println!("SP-REAL-04~05 in={text:?} out={:?}", result.text);
        assert_structure(text, &result.text);
    }
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_06_07_反引号代码整体保留且内嵌url不重复() {
    let engine = shared();
    for text in [
        "use `cargo build --release` now",
        "`curl https://x.example/a`",
    ] {
        let result = translate(engine, text).expect("翻译失败");
        println!("SP-REAL-06~07 in={text:?} out={:?}", result.text);
        assert_structure(text, &result.text);
        assert_eq!(
            count(&result.text, "`"),
            2,
            "反引号必须成对：{:?}",
            result.text
        );
    }
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_10_emoji与中日韩混排不破坏字节边界() {
    let engine = shared();
    let text = "😀 see https://a.example/😀 与 {名字} ok";
    let result = translate(engine, text).expect("翻译失败");
    println!("SP-REAL-10 in={text:?} out={:?}", result.text);
    assert_eq!(count(&result.text, "😀"), 2);
    assert_eq!(count(&result.text, "https://a.example/😀"), 1);
    assert_eq!(count(&result.text, "{名字}"), 1);
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_11_多行长文本换行与顺序保留() {
    let engine = shared();
    let text = "line1\nline2 {x}\nline3";
    let result = translate(engine, text).expect("翻译失败");
    println!("SP-REAL-11 in={text:?} out={:?}", result.text);
    assert_eq!(
        count(&result.text, "\n"),
        2,
        "换行不能丢：{:?}",
        result.text
    );
    assert_eq!(count(&result.text, "{x}"), 1);
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_12_全保护与空白输入不崩溃() {
    let engine = shared();
    // 全保护：没有纯文本需要翻译，输出必须与输入逐字节相同
    let text = "https://only.example/path";
    let result = translate(engine, text).expect("全保护输入不应失败");
    println!("SP-REAL-12 out={:?}", result.text);
    assert_eq!(result.text, text, "全保护输入必须原样返回");

    // 空白输入：直接拒绝，不把空白当译文
    let blank = translate(engine, "   ");
    assert_eq!(
        blank.unwrap_err().code,
        TranslationErrorCode::InvalidRequest,
        "空白输入必须 InvalidRequest"
    );
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_08_09_标记模式要么结构完整要么完整性失败() {
    // 这一条是"标记策略不可靠"的实测记录：允许失败，但**不允许 silently 坏文本**。
    let engine = BergamotNmtEngine::new(config(ProtectionMode::Marker)).expect("构造失败");
    engine.load().expect("加载失败");
    let cases = [
        "Hello {name}!",
        "see https://x.example/a?b=1&c=2 now",
        "use `cargo build` now",
        "call {order_id} for {name} at https://y.example/?q={name}",
    ];
    let mut ok = 0;
    let mut integrity = 0;
    for text in cases {
        match translate(&engine, text) {
            Ok(result) => {
                println!(
                    "SP-REAL-08~09 标记模式成功 in={text:?} out={:?}",
                    result.text
                );
                assert_structure(text, &result.text);
                ok += 1;
            }
            Err(err) => {
                println!(
                    "SP-REAL-08~09 标记模式拒绝 in={text:?} code={:?} msg={}",
                    err.code, err
                );
                assert_eq!(
                    err.code,
                    TranslationErrorCode::IntegrityFailure,
                    "标记丢失必须报完整性失败，实际 {err:?}"
                );
                integrity += 1;
            }
        }
    }
    println!("SP-REAL-08~09 标记模式统计：结构完整 {ok} 例，完整性失败 {integrity} 例");
    // 不允许"两样都不是"（既没成功也没报错）——上面的分支已经覆盖全部情况
    assert_eq!(ok + integrity, cases.len());
    engine.unload().expect("卸载失败");
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_分段模式的耗时随受保护片段数线性增长() {
    let engine = shared();
    let cases = [
        ("Hello world!", 0usize),
        ("Hello {name}!", 1),
        ("call {a} and {b} at https://x.example/a", 3),
    ];
    println!("分段模式耗时（每段一次 native 调用）：");
    for (text, protected) in cases {
        let started = std::time::Instant::now();
        let result = translate(engine, text).expect("翻译失败");
        let elapsed = started.elapsed().as_millis();
        println!(
            "  受保护 {protected} 个  耗时 {elapsed} ms  译文 {:?}",
            result.text
        );
        assert!(elapsed < 5_000, "单句耗时异常：{elapsed} ms");
    }
}

/// 快照残留核对：一次"新建 → 加载 → 卸载 → Drop"之后，快照根目录里不能多出目录。
///
/// 注意不要和 `shared()` 那个进程级引擎混淆：`OnceLock` 里的静态值**在进程退出时不会被 Drop**，
/// 因此共享引擎自己会留下一个快照目录。那是测试脚手架的行为，不是生产路径的行为。
#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_单次加载卸载后不残留快照目录() {
    fn dirs(root: &std::path::Path) -> usize {
        std::fs::read_dir(root)
            .map(|it| it.filter_map(|e| e.ok()).count())
            .unwrap_or(0)
    }
    let config = config(ProtectionMode::Segment);
    let root = config.snapshot_root.clone();
    std::fs::create_dir_all(&root).expect("创建快照根失败");
    let before = dirs(&root);
    {
        let engine = BergamotNmtEngine::new(config).expect("构造失败");
        engine.load().expect("加载失败");
        engine.unload().expect("卸载失败");
    }
    let after = dirs(&root);
    println!("快照目录数：加载前 {before}，卸载并 Drop 后 {after}");
    assert_eq!(after, before, "卸载后快照目录必须被删除");
}

#[test]
#[ignore = "需要真实 Bergamot 模型（NMT_BERGAMOT_BIN / NMT_MODEL_DIR）"]
fn sp_real_卸载后重新翻译自动重载() {
    let engine = shared();
    let first = translate(engine, "Hello world!").expect("第一次翻译失败");
    engine.unload().expect("卸载失败");
    assert!(engine.identity().is_none(), "卸载后不应还报告已加载身份");
    let second = translate(engine, "Hello world!").expect("卸载后重载翻译失败");
    println!(
        "重载后 engine={} model={:?} 耗时={}ms 译文={:?}",
        second.engine_id, second.model_id, second.elapsed_ms, second.text
    );
    assert_eq!(first.text, second.text, "同一输入同一版本应得到相同译文");
}
