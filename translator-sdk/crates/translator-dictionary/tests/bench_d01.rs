//! D-01 基准：1 万词条规模的查询延迟与常驻内存。
//!
//! 对应资源文档 §10 测试矩阵的 D-01：
//! > 中英 1 万词条索引 | 查询 p50/p95、索引内存 | 字典替换接口不变
//!
//! 默认被 `#[ignore]`，需要显式运行：
//!
//! ```bash
//! cargo test -p translator-dictionary --test bench_d01 -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use pack_dictionary::{pack, PackOptions};
use translator_dictionary::{DictionaryEngine, DictionaryPackage, Provenance};
use translator_plugin_api::TranslationEngine;
use translator_types::{LanguageTag, TranslateRequest};

const ENTRY_COUNT: usize = 10_000;
const QUERY_ROUNDS: usize = 10_000;

/// 生成合成词条。
///
/// 注意：早期版本用 `词00000` 这类连续序号做词条，FST 作为最小 DFA 会把
/// 十个数字位共享成极少数状态，2 万个键的索引只有 202 字节 —— 完全不具
/// 代表性。这里改用固定种子的伪随机汉字词条，让索引密度接近真实词典
/// （CC-CEDICT 实测约 8.8 字节/键），同时保证可复现。
fn synthetic_entries(count: usize) -> Vec<pack_dictionary::RawEntry> {
    // 常用汉字池，取自真实语料的高频字。
    const POOL: &[char] = &[
        '的', '一', '是', '了', '我', '不', '人', '在', '他', '有', '这', '个', '上', '们', '来',
        '到', '时', '大', '地', '为', '子', '中', '你', '说', '生', '国', '年', '着', '就', '那',
        '和', '要', '她', '出', '也', '得', '里', '后', '自', '以', '会', '家', '可', '下', '而',
        '过', '天', '去', '能', '对', '小', '多', '然', '于', '心', '学', '么', '之', '都', '好',
        '看', '起', '发', '当', '没', '成', '只', '如', '事', '把', '还', '用', '第', '样', '道',
        '想', '作', '种', '开', '美', '总', '从', '无', '情', '己', '面', '最', '女', '但', '现',
        '前', '些', '所', '同', '日', '手', '又', '行', '意', '动', '方', '期', '它', '头', '经',
        '长', '儿', '回', '位', '分', '爱', '老', '因', '很', '给', '名', '法', '间', '斯', '知',
        '世', '什', '两', '次', '使', '身', '者', '被', '高', '已', '亲', '其', '进', '此', '话',
        '常', '与', '活', '正', '感', '民', '力', '理', '水', '化', '机', '电', '农', '工', '市',
        '车', '公', '路', '书', '写', '读', '语', '文', '数', '量', '体', '制', '政', '府', '社',
        '区', '报', '告', '问', '题', '答', '案', '计', '划', '管', '理', '产', '品', '服', '务',
    ];

    // 少量简繁映射，让大约一半词条带上繁体键（真实词典也是如此）。
    const TRAD: &[(char, char)] = &[
        ('这', '這'),
        ('个', '個'),
        ('们', '們'),
        ('来', '來'),
        ('时', '時'),
        ('国', '國'),
        ('说', '說'),
        ('过', '過'),
        ('还', '還'),
        ('会', '會'),
        ('学', '學'),
        ('对', '對'),
        ('无', '無'),
        ('后', '後'),
        ('头', '頭'),
        ('经', '經'),
        ('长', '長'),
        ('给', '給'),
        ('间', '間'),
        ('进', '進'),
        ('亲', '親'),
        ('话', '話'),
        ('与', '與'),
        ('开', '開'),
        ('种', '種'),
        ('样', '樣'),
        ('爱', '愛'),
        ('儿', '兒'),
        ('为', '為'),
        ('于', '於'),
        ('现', '現'),
        ('发', '發'),
        ('书', '書'),
        ('写', '寫'),
        ('读', '讀'),
        ('语', '語'),
        ('数', '數'),
        ('体', '體'),
        ('报', '報'),
        ('问', '問'),
        ('题', '題'),
        ('计', '計'),
        ('划', '劃'),
        ('产', '產'),
        ('务', '務'),
    ];

    fn to_traditional(word: &str) -> String {
        word.chars()
            .map(|c| {
                TRAD.iter()
                    .find(|(s, _)| *s == c)
                    .map(|(_, t)| *t)
                    .unwrap_or(c)
            })
            .collect()
    }

    // xorshift64*，固定种子 → 可复现。
    let mut state: u64 = 0x5DEE_CE66_D1CE_1234;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    let mut out = Vec::with_capacity(count);
    let mut seen = std::collections::HashSet::new();
    while out.len() < count {
        let len = 2 + (next() % 3) as usize; // 2~4 字
        let word: String = (0..len)
            .map(|_| POOL[(next() % POOL.len() as u64) as usize])
            .collect();
        // 词条必须唯一，否则合并后不足 1 万条。
        if !seen.insert(word.clone()) {
            continue;
        }
        let i = out.len();
        let traditional = to_traditional(&word);
        out.push(pack_dictionary::RawEntry {
            traditional: if traditional != word {
                Some(traditional)
            } else {
                None
            },
            headword: word,
            pinyin: Some(format!("pin1 yin1 {i}")),
            pos: None,
            definitions: vec![
                format!("synthetic definition {i}"),
                format!("secondary gloss {i}"),
            ],
        });
    }
    out
}

fn pack_options() -> PackOptions {
    PackOptions {
        id: "dictionary.bench.synthetic".to_string(),
        version: "0.0.1".to_string(),
        language: "zh-Hans".to_string(),
        target_languages: vec!["en".to_string()],
        provenance: Provenance {
            name: "SYNTHETIC".to_string(),
            source_url: "https://example.invalid/synthetic".to_string(),
            revision: "bench".to_string(),
            license_spdx: "CC0-1.0".to_string(),
            license_file: "LICENSE.txt".to_string(),
            retrieved_at: "2026-11-05".to_string(),
        },
        license_text: "synthetic benchmark data, CC0\n".to_string(),
    }
}

/// 读取当前进程 RSS（KiB）。
fn rss_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest
                .trim()
                .trim_end_matches(" kB")
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }
    0
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

#[test]
#[ignore = "基准测试，需显式运行 --ignored"]
fn d01_万词条查询基准() {
    println!("\n===== D-01 基准：{ENTRY_COUNT} 词条 =====");
    println!(
        "构建配置        {}",
        if cfg!(debug_assertions) {
            "debug（本项目代码未优化，延迟数字偏悲观）"
        } else {
            "release"
        }
    );

    // 1) 打包
    let dir = tempfile::tempdir().expect("临时目录");
    let entries = synthetic_entries(ENTRY_COUNT);
    let words: Vec<String> = entries.iter().map(|e| e.headword.clone()).collect();
    let t0 = Instant::now();
    let stats = pack(entries, dir.path(), &pack_options()).expect("打包");
    let pack_ms = t0.elapsed().as_millis();
    println!(
        "打包耗时        {pack_ms} ms（记录 {} / 键 {}）",
        stats.records, stats.keys
    );
    println!(
        "index.fst       {} B（{:.1} KB，{:.1} 字节/键）",
        stats.index_bytes,
        stats.index_bytes as f64 / 1024.0,
        stats.index_bytes as f64 / stats.keys as f64
    );
    println!(
        "entries.sqlite  {} B（{:.1} MB）",
        stats.db_bytes,
        stats.db_bytes as f64 / 1_048_576.0
    );

    // 2) 打开（含 FST mmap）
    let rss_before = rss_kib();
    let t0 = Instant::now();
    let package = Arc::new(DictionaryPackage::open(dir.path(), false).expect("打开"));
    let open_ms = t0.elapsed().as_millis();
    let rss_after_open = rss_kib();

    println!("打开耗时        {open_ms} ms");
    println!(
        "索引常驻        {:.1} KB（FST mmap）",
        package.index_bytes() as f64 / 1024.0
    );
    println!(
        "RSS 增量        打开 +{} KiB",
        rss_after_open.saturating_sub(rss_before)
    );

    // 3) 预热
    for i in 0..200 {
        let _ = package.lookup_exact(&words[i % ENTRY_COUNT]);
    }

    // 4) 精确查询延迟（步长与词条数互质，覆盖全表）
    let mut exact_lat = Vec::with_capacity(QUERY_ROUNDS);
    for i in 0..QUERY_ROUNDS {
        let term = &words[(i * 7919) % ENTRY_COUNT];
        let t = Instant::now();
        let hit = package.lookup_exact(term).expect("查询");
        exact_lat.push(t.elapsed().as_nanos());
        assert!(hit.is_some(), "合成数据应全部命中");
    }
    exact_lat.sort_unstable();

    // 5) 前缀查询延迟（含 SQLite 读取多条）
    let mut prefix_lat = Vec::with_capacity(1000);
    for i in 0..1000 {
        // 取词条首字作为前缀，模拟输入法逐字补全
        let term: String = words[(i * 13) % ENTRY_COUNT].chars().take(2).collect();
        let t = Instant::now();
        let hits = package.lookup(&term, 5).expect("前缀查询");
        prefix_lat.push(t.elapsed().as_nanos());
        assert!(!hits.is_empty(), "前缀应至少命中自身");
    }
    prefix_lat.sort_unstable();

    // 6) 引擎层（含 trait 调用与结果封装）
    let engine = DictionaryEngine::new(Arc::clone(&package));
    let source = LanguageTag::new("zh-Hans").unwrap();
    let target = LanguageTag::new("en").unwrap();
    let mut engine_lat = Vec::with_capacity(2000);
    for i in 0..2000 {
        let term = words[(i * 31) % ENTRY_COUNT].clone();
        let req = TranslateRequest::new(term, target.clone()).with_source(source.clone());
        let t = Instant::now();
        let r = pollster::block_on(engine.translate(&req)).expect("引擎翻译");
        engine_lat.push(t.elapsed().as_nanos());
        assert!(!r.text.is_empty());
    }
    engine_lat.sort_unstable();

    // 7) 参考下限：绕开本 crate，直接用 rusqlite 做同样的两次点名查询，
    //    用于判断「剩下的时间到底花在我们的代码上还是 SQLite 上」。
    let mut sqlite_lat = Vec::with_capacity(2000);
    let mut one_query_lat = Vec::with_capacity(2000);
    {
        let conn = rusqlite::Connection::open_with_flags(
            dir.path().join("entries.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("直连数据库");
        let _ = conn.pragma_update(None, "mmap_size", 256 * 1024 * 1024_i64);
        let _ = conn.pragma_update(None, "cache_size", -2000_i64);
        for i in 0..2000 {
            let id = ((i * 17) % ENTRY_COUNT) as i64 + 1;
            let t = Instant::now();
            let head: String = conn
                .prepare_cached("SELECT headword FROM entries WHERE id = ?1")
                .expect("prepare")
                .query_row([id], |r| r.get(0))
                .expect("查询");
            one_query_lat.push(t.elapsed().as_nanos());
            let n: i64 = conn
                .prepare_cached("SELECT count(*) FROM senses WHERE entry_id = ?1")
                .expect("prepare")
                .query_row([id], |r| r.get(0))
                .expect("查询");
            sqlite_lat.push(t.elapsed().as_nanos());
            assert!(!head.is_empty() && n > 0);
        }
    }
    sqlite_lat.sort_unstable();
    one_query_lat.sort_unstable();

    let rss_end = rss_kib();

    println!("\n--- 延迟（纳秒）---");
    println!(
        "精确查询  p50 {:>7}  p95 {:>7}  max {:>8}   （{} 次）",
        percentile(&exact_lat, 0.50),
        percentile(&exact_lat, 0.95),
        exact_lat.last().copied().unwrap_or(0),
        exact_lat.len()
    );
    println!(
        "前缀查询  p50 {:>7}  p95 {:>7}  max {:>8}   （{} 次）",
        percentile(&prefix_lat, 0.50),
        percentile(&prefix_lat, 0.95),
        prefix_lat.last().copied().unwrap_or(0),
        prefix_lat.len()
    );
    println!(
        "引擎调用  p50 {:>7}  p95 {:>7}  max {:>8}   （{} 次）",
        percentile(&engine_lat, 0.50),
        percentile(&engine_lat, 0.95),
        engine_lat.last().copied().unwrap_or(0),
        engine_lat.len()
    );
    println!(
        "裸 SQLite p50 {:>7}  p95 {:>7}  max {:>8}   （{} 次，参考下限）",
        percentile(&sqlite_lat, 0.50),
        percentile(&sqlite_lat, 0.95),
        sqlite_lat.last().copied().unwrap_or(0),
        sqlite_lat.len()
    );
    println!(
        "└ 单条 SQL p50 {:>7}  p95 {:>7}  max {:>8}",
        percentile(&one_query_lat, 0.50),
        percentile(&one_query_lat, 0.95),
        one_query_lat.last().copied().unwrap_or(0)
    );

    println!("\n--- 内存 ---");
    println!(
        "查询全程 RSS 增量  +{} KiB",
        rss_end.saturating_sub(rss_after_open)
    );
    println!("总 RSS            {} KiB", rss_end);

    // 硬性阈值：只用来拦「数量级」的性能回退，不做微基准精度断言。
    // 本机是共享的 i5-8250U（8 线程，load average 常年在 1~3），
    // release 实测 p95 约 17~20μs，其中 14μs 是 SQLite 本身的下限，
    // 因此阈值留 2 倍余量。
    let budget_ns = if cfg!(debug_assertions) {
        100_000
    } else {
        40_000
    };
    assert!(
        percentile(&exact_lat, 0.95) < budget_ns,
        "精确查询 p95 超过 {budget_ns}ns，性能回退"
    );
}

/// 真实包基准：直接压 `resources/zh-en`（完整 CC-CEDICT，12 万词条）。
///
/// 包不存在时自动跳过，方便在 CI 或新机器上直接跑全量测试：
///
/// ```bash
/// cargo test --release -p translator-dictionary --test bench_d01 -- --ignored --nocapture
/// ```
#[test]
#[ignore = "基准测试，需显式运行 --ignored"]
fn d01_真实包_cc_cedict() {
    let root = std::env::var_os("CC_CEDICT_PACKAGE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/zh-en")
        });

    if !root.join("manifest.json").exists() {
        println!("\n[跳过] 未找到真实词典包 {root:?}（可用 CC_CEDICT_PACKAGE 指定）");
        return;
    }

    println!("\n===== D-01 基准：真实包 {} =====", root.display());
    println!(
        "构建配置        {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );

    let rss_before = rss_kib();
    let t0 = Instant::now();
    let package =
        Arc::new(DictionaryPackage::open(&root, true).expect("打开真实包（含 sha256 校验）"));
    let open_verified_ms = t0.elapsed().as_millis();
    let rss_open = rss_kib();

    // 直接从 FST 里流式取出键，作为真实查询词
    let fst_bytes = std::fs::read(root.join("index.fst")).expect("读取 index.fst");
    let map = fst::Map::new(fst_bytes).expect("解析 index.fst");

    println!("词条数          {}", package.entry_count());
    println!("索引键数        {}（简繁双键）", map.len());
    println!(
        "索引            {:.1} KB（{:.1} 字节/键）",
        package.index_bytes() as f64 / 1024.0,
        package.index_bytes() as f64 / map.len() as f64
    );
    println!("打开+sha256校验 {open_verified_ms} ms");
    println!(
        "RSS 增量        打开 +{} KiB（FST 与 DB 都是 mmap，计入的是已触达页）",
        rss_open.saturating_sub(rss_before)
    );

    let mut keys: Vec<String> = Vec::new();
    {
        use fst::Streamer;
        let mut stream = map.stream();
        while let Some((key, _)) = stream.next() {
            if let Ok(s) = std::str::from_utf8(key) {
                keys.push(s.to_string());
            }
            if keys.len() >= QUERY_ROUNDS {
                break;
            }
        }
    }
    println!("取样键数        {}", keys.len());
    assert!(!keys.is_empty(), "索引里应至少有一个键");

    for key in keys.iter().take(200) {
        let _ = package.lookup_exact(key);
    }

    let mut lat = Vec::with_capacity(keys.len());
    for key in &keys {
        let t = Instant::now();
        let hit = package.lookup_exact(key).expect("查询");
        lat.push(t.elapsed().as_nanos());
        assert!(hit.is_some(), "FST 里存在的键必须能读出词条：{key}");
    }
    lat.sort_unstable();

    println!("\n--- 精确查询（真实词条）---");
    println!(
        "p50 {:>7}  p90 {:>7}  p95 {:>7}  p99 {:>7}  max {:>8}   （{} 次）",
        percentile(&lat, 0.50),
        percentile(&lat, 0.90),
        percentile(&lat, 0.95),
        percentile(&lat, 0.99),
        lat.last().copied().unwrap_or(0),
        lat.len()
    );

    let rss_end = rss_kib();
    println!("\n--- 内存 ---");
    println!(
        "查询期间 RSS 增量  +{} KiB",
        rss_end.saturating_sub(rss_open)
    );
    println!("总 RSS            {rss_end} KiB");

    let budget_ns = if cfg!(debug_assertions) {
        400_000
    } else {
        150_000
    };
    assert!(
        percentile(&lat, 0.95) < budget_ns,
        "真实包精确查询 p95 超过 {budget_ns}ns"
    );
}
