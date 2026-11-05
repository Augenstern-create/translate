//! P3.2-D/E：执行计划摘要作键的缓存（LC-09）+ 空闲回收 + 生命周期观测。
//!
//! 需要真实模型（`NMT_BERGAMOT_BIN` / `NMT_MODEL_DIR` / `NMT_WORKER_BIN`）：
//!
//! ```text
//! cargo test -p translator-nmt-bergamot --test p32_cache -- --ignored --test-threads=1 --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use translator_core::protection::ProtectionMode;
use translator_nmt_bergamot::{BergamotNmtConfig, BergamotNmtEngine, LoadState};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest, TranslationResult};

fn config() -> BergamotNmtConfig {
    let mut config = BergamotNmtConfig::from_env()
        .expect("需要 NMT_BERGAMOT_BIN / NMT_MODEL_DIR / NMT_WORKER_BIN");
    config.protection = ProtectionMode::Segment;
    config
}

fn translate(engine: &BergamotNmtEngine, text: &str) -> TranslationResult {
    let req = TranslateRequest::new(text, LanguageTag::new("zh-Hans").unwrap())
        .with_source(LanguageTag::new("en").unwrap());
    pollster::block_on(engine.translate(&req)).expect("翻译失败")
}

/// 把模型目录复制一份（49.5 MiB），用于"内容变了"的实验。
fn copy_model_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // 只复制五个模型文件（目录里还有测试用的 yml/txt）
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("model.")
            && !name.starts_with("srcvocab")
            && !name.starts_with("trgvocab")
            && !name.starts_with("lex.")
            && !name.starts_with("nonbreaking_prefix")
        {
            continue;
        }
        std::fs::copy(&path, dst.join(&name)).unwrap();
    }
}

#[test]
#[ignore = "需要真实 Bergamot 模型"]
fn d01_相同请求第二次命中缓存() {
    let engine = BergamotNmtEngine::new(config()).expect("构造失败");
    let first = translate(&engine, "Hello world!");
    let second = translate(&engine, "Hello world!");
    let stats = engine.cache_stats();
    println!(
        "第一次 {} ms / 第二次 {} ms / 命中 {} 未命中 {} 条目 {}",
        first.elapsed_ms, second.elapsed_ms, stats.hits, stats.misses, stats.entries
    );
    assert_eq!(first.text, second.text);
    // 只有"查过"才算一次未命中：第一次调用前还没有计划摘要（模型没加载过），
    // 因此它既不是命中也不是未命中，而是"先加载再算"。
    assert_eq!(stats.hits, 1, "第二次应当命中");
    assert_eq!(stats.entries, 1, "缓存里应当只有一条");
    assert!(
        second.elapsed_ms <= first.elapsed_ms,
        "命中不该比不命中更慢（{} vs {}）",
        second.elapsed_ms,
        first.elapsed_ms
    );
}

#[test]
#[ignore = "需要真实 Bergamot 模型"]
fn d02_模型内容变化后计划摘要随之变化() {
    let src = PathBuf::from(std::env::var("NMT_MODEL_DIR").unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let copy = tmp.path().join("models-copy");
    copy_model_dir(&src, &copy);

    let mut base = config();
    base.model_dir = copy.clone();
    let engine_a = Arc::new(BergamotNmtEngine::new(base.clone()).expect("构造 A 失败"));
    let out_a = translate(&engine_a, "Hello world!");
    let digest_a = {
        let identity = engine_a.identity().expect("A 未加载");
        engine_a.plan(&identity, ProtectionMode::Segment).digest()
    };

    // 改一个**非权重**文件：既能改变摘要，又不会让 native 加载失败
    let prefix = copy.join("nonbreaking_prefix.en");
    let mut body = std::fs::read_to_string(&prefix).unwrap();
    body.push_str("p32-marker\n");
    std::fs::write(&prefix, body).unwrap();

    let engine_b = Arc::new(BergamotNmtEngine::new(base).expect("构造 B 失败"));
    let out_b = translate(&engine_b, "Hello world!");
    let digest_b = {
        let identity = engine_b.identity().expect("B 未加载");
        engine_b.plan(&identity, ProtectionMode::Segment).digest()
    };

    println!("A 摘要 {} / B 摘要 {}", &digest_a[..16], &digest_b[..16]);
    println!("A 译文 {:?} / B 译文 {:?}", out_a.text, out_b.text);
    assert_ne!(digest_a, digest_b, "模型文件内容变了，计划摘要必须变");
    // 这条断言是**故意**的：`model_id`（= content_digest）只覆盖权重文件，
    // 换 tokenizer / 短名单 / prefix 表时它**不变**。这正是 ADR-026 说的
    // "content_digest 不能当缓存键"——缓存键必须用覆盖五文件的计划摘要。
    assert_eq!(
        out_a.model_id, out_b.model_id,
        "若这里开始不等，说明 content_digest 已扩展到支持文件，应一并复核 ADR-026 的结论"
    );
    // B 是全新引擎，缓存必须是空的：没有跨"内容版本"的旧结果
    let stats_b = engine_b.cache_stats();
    assert_eq!(stats_b.hits, 0, "内容不同的旧结果绝不能被命中：{stats_b:?}");
    assert_eq!(stats_b.entries, 1, "B 只缓存自己这一次的结果：{stats_b:?}");
}

#[test]
#[ignore = "需要真实 Bergamot 模型"]
fn e01_空闲回收会卸载模型且下次请求自动重载() {
    let engine = BergamotNmtEngine::new(config()).expect("构造失败");
    let first = translate(&engine, "Hello world!");
    assert!(matches!(engine.state(), LoadState::Loaded { .. }));

    // TTL 设为 0：只要"现在"没有在飞请求就必须回收
    let evicted = engine.evict_if_idle(Duration::ZERO).expect("回收失败");
    assert!(evicted, "空闲且无在飞请求时必须真的卸载");
    assert_eq!(engine.state(), LoadState::Unloaded);

    let second = translate(&engine, "Hello world!");
    assert_eq!(first.text, second.text, "重载后译文必须一致");
    // 缓存键（计划摘要）在卸载后仍然有效 → 这一句应当直接命中，不再重载模型
    let stats = engine.cache_stats();
    println!(
        "卸载后再翻译：{} ms，命中 {} 未命中 {}，模型状态 {:?}",
        second.elapsed_ms,
        stats.hits,
        stats.misses,
        engine.state() == LoadState::Unloaded
    );
    assert_eq!(stats.hits, 1, "卸载后同一句应当命中缓存：{stats:?}");
    assert!(
        matches!(engine.state(), LoadState::Unloaded),
        "命中缓存时不该为了答案而重新加载模型"
    );
}

#[test]
#[ignore = "需要真实 Bergamot 模型"]
fn e02_加载状态带快照大小与准备耗时() {
    let engine = BergamotNmtEngine::new(config()).expect("构造失败");
    translate(&engine, "Hello world!");
    match engine.state() {
        LoadState::Loaded {
            snapshot_bytes,
            preparation_ms,
            uptime_ms,
            identity,
        } => {
            println!(
                "快照 {} MiB / 准备 {} ms / 已存活 {} ms / 模型 {}",
                snapshot_bytes / (1024 * 1024),
                preparation_ms,
                uptime_ms,
                identity.model_revision
            );
            assert!(snapshot_bytes > 40 * 1024 * 1024, "五文件合计应约 49.5 MiB");
            assert!(preparation_ms > 0);
        }
        LoadState::Unloaded => panic!("刚翻译完却是 Unloaded"),
    }
}
