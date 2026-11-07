//! 把 CC-CEDICT（及其他 TSV 风格词表）编译成词典包。
//!
//! 设计文档 §7.1 要求：**在构建期完成转换，不在用户设备上解析全量 JSON/XML**。
//! 本工具就是那个"构建期"。
//!
//! 输出（`<out_dir>/`）：
//! ```text
//! manifest.json    # 清单 + 来源 + 许可 + sha256
//! index.fst        # term → record_id
//! entries.sqlite   # 释义详情
//! LICENSE.txt      # 许可证原文（取自源文件头部注释）
//! ```

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::Path;

use sha2::{Digest, Sha256};
use translator_dictionary::{
    DictionaryManifest, PackageFile, PayloadKind, Provenance, MANIFEST_SCHEMA_V2, SCHEMA_VERSION,
    SENSE_SEPARATOR,
};

/// 与 [`SENSE_SEPARATOR`] 等价的字符串常量（`join` 需要 `&str`）。
const SENSE_SEP_STR: &str = "\u{1f}";

pub use translator_dictionary::DictionaryError as PackError;

// ============================================================
// 中间表示
// ============================================================

/// 从源词表解析出的一条原始词条。
#[derive(Clone, Debug, PartialEq)]
pub struct RawEntry {
    /// 词条本身（CC-CEDICT 的简体形式）。
    pub headword: String,
    /// 繁体形式，可为空。
    pub traditional: Option<String>,
    /// 拼音，可为空。
    pub pinyin: Option<String>,
    /// 词性，可为空。
    pub pos: Option<String>,
    pub definitions: Vec<String>,
}

/// 解析结果。
pub struct ParsedCedict {
    pub entries: Vec<RawEntry>,
    /// 源文件头部的 `#` 注释块（含许可证声明），用于生成 LICENSE.txt。
    pub header: String,
    /// 源文件声明的版本行（如 `#! version=1`）。
    pub source_version: Option<String>,
    /// 被跳过的畸形行数。
    pub skipped_lines: usize,
}

/// 解析 CC-CEDICT 文本格式：
///
/// ```text
/// 傳統 传统 [chuan2 tong3] /tradition/traditional/
/// ```
///
/// 以 `#` 开头的行视为注释。`limit` 限制读取的词条数（用于生成子集）。
pub fn parse_cedict<R: BufRead>(
    reader: R,
    limit: Option<usize>,
) -> Result<ParsedCedict, PackError> {
    let mut entries = Vec::new();
    let mut header = String::new();
    let mut source_version = None;
    let mut skipped = 0usize;
    let mut in_header = true;

    for line in reader.lines() {
        let line = line?;

        if line.starts_with('#') {
            if in_header {
                header.push_str(&line);
                header.push('\n');
            }
            if let Some(v) = line.strip_prefix("#! version=") {
                source_version = Some(v.trim().to_string());
            }
            continue;
        }

        in_header = false;
        if line.trim().is_empty() {
            continue;
        }

        match parse_line(&line) {
            Some(entry) => {
                entries.push(entry);
                if let Some(limit) = limit {
                    if entries.len() >= limit {
                        break;
                    }
                }
            }
            None => skipped += 1,
        }
    }

    Ok(ParsedCedict {
        entries,
        header,
        source_version,
        skipped_lines: skipped,
    })
}

/// 解析单行。格式不符返回 `None`（计入 skipped，不中断整体转换）。
fn parse_line(line: &str) -> Option<RawEntry> {
    let mut rest = line;

    // 前两个空格分隔的字段：繁体、简体
    let traditional = take_token(&mut rest)?;
    let simplified = take_token(&mut rest)?;

    // [pinyin]
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('[')?;
    let (pinyin, rest) = rest.split_once(']')?;

    // /def1/def2/...
    let rest = rest.trim_start();
    if !rest.starts_with('/') {
        return None;
    }
    let body = rest.trim_matches('/');
    let definitions: Vec<String> = body
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    if simplified.is_empty() || definitions.is_empty() {
        return None;
    }

    Some(RawEntry {
        headword: simplified.to_string(),
        traditional: if traditional != simplified && !traditional.is_empty() {
            Some(traditional.to_string())
        } else {
            None
        },
        pinyin: Some(pinyin.trim().to_string()).filter(|p| !p.is_empty()),
        pos: None,
        definitions,
    })
}

fn take_token<'a>(rest: &mut &'a str) -> Option<&'a str> {
    let rest_trimmed = rest.trim_start();
    let end = rest_trimmed.find(char::is_whitespace)?;
    let (token, tail) = rest_trimmed.split_at(end);
    *rest = tail;
    Some(token)
}

// ============================================================
// 打包
// ============================================================

#[derive(Clone, Debug)]
pub struct PackOptions {
    pub id: String,
    pub version: String,
    /// `entries.sqlite` 的存储格式（v1 两张表 / v2 释义内联）。
    ///
    /// 新包默认 v2；要复现历史包（例如给已发布版本对齐 sha256）时显式选 v1。
    pub payload: PayloadKind,
    /// 词条语言，如 `zh-Hans`。
    pub language: String,
    /// 释义语言，如 `["en"]`。
    pub target_languages: Vec<String>,
    pub provenance: Provenance,
    pub license_text: String,
}

#[derive(Clone, Debug)]
pub struct PackStats {
    pub records: u64,
    pub keys: u64,
    pub index_bytes: u64,
    pub db_bytes: u64,
}

/// 待写入的一行记录。用结构体而非长参数列表，避免参数顺序出错。
struct RecordRow<'a> {
    id: u64,
    headword: &'a str,
    traditional: Option<&'a str>,
    pinyin: Option<&'a str>,
    pos: Option<&'a str>,
    definitions: &'a [String],
    source_id: &'a str,
    source_license: &'a str,
}

/// 写入一条记录到 SQLite（v1：entries + senses 两张表）。
fn insert_record(tx: &rusqlite::Transaction<'_>, row: RecordRow<'_>) -> Result<i64, PackError> {
    let id = row.id as i64;
    tx.execute(
        "INSERT INTO entries (id, headword, normalized, traditional, pinyin, pos, source_id, source_license, confidence)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, 1.0)",
        rusqlite::params![
            id,
            row.headword,
            row.traditional,
            row.pinyin,
            row.pos,
            row.source_id,
            row.source_license
        ],
    )?;

    for (idx, definition) in row.definitions.iter().enumerate() {
        tx.execute(
            "INSERT INTO senses (entry_id, idx, definition) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, idx as i64, definition],
        )?;
    }

    Ok(id)
}

/// 写入一条记录到 SQLite（v2：释义内联进 entries.senses）。
fn insert_record_v2(tx: &rusqlite::Transaction<'_>, row: RecordRow<'_>) -> Result<i64, PackError> {
    let id = row.id as i64;
    // U+001F 是 C0 控制符，正常释义里不会出现；出现就拒绝，避免把一条释义劈成两条
    if row.definitions.iter().any(|d| d.contains(SENSE_SEPARATOR)) {
        return Err(PackError::InvalidPackage(format!(
            "词条 {} 的释义里出现了分隔符 U+001F",
            row.headword
        )));
    }
    let senses = row.definitions.join(SENSE_SEP_STR);
    tx.execute(
        "INSERT INTO entries (id, headword, normalized, traditional, pinyin, pos, source_id, source_license, confidence, senses)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, 1.0, ?8)",
        rusqlite::params![
            id,
            row.headword,
            row.traditional,
            row.pinyin,
            row.pos,
            row.source_id,
            row.source_license,
            senses
        ],
    )?;
    Ok(id)
}

/// 通用 JSONL 输入：每行一个对象，字段 `headword` / `definitions` 必填，
/// 可选 `traditional` / `pinyin` / `pos`。供 Kaikki/JMdict 转换器使用。
#[derive(serde::Deserialize)]
struct JsonEntry {
    headword: String,
    #[serde(default)]
    traditional: Option<String>,
    #[serde(default)]
    pinyin: Option<String>,
    #[serde(default)]
    pos: Option<String>,
    definitions: Vec<String>,
}

pub fn parse_jsonl<R: std::io::BufRead>(
    reader: R,
    limit: Option<usize>,
) -> Result<Vec<RawEntry>, PackError> {
    let mut entries = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let raw: JsonEntry = serde_json::from_str(&line).map_err(|e| {
            PackError::InvalidPackage(format!("JSONL 第 {} 行不是合法对象：{e}", lineno + 1))
        })?;
        if raw.headword.trim().is_empty() {
            return Err(PackError::InvalidPackage(format!(
                "JSONL 第 {} 行 headword 为空",
                lineno + 1
            )));
        }
        if raw.definitions.iter().all(|d| d.trim().is_empty()) {
            return Err(PackError::InvalidPackage(format!(
                "JSONL 第 {} 行 definitions 为空",
                lineno + 1
            )));
        }
        entries.push(RawEntry {
            headword: raw.headword,
            traditional: raw.traditional.filter(|s| !s.trim().is_empty()),
            pinyin: raw.pinyin.filter(|s| !s.trim().is_empty()),
            pos: raw.pos.filter(|s| !s.trim().is_empty()),
            definitions: raw
                .definitions
                .into_iter()
                .filter(|d| !d.trim().is_empty())
                .collect(),
        });
        if let Some(limit) = limit {
            if entries.len() >= limit {
                break;
            }
        }
    }
    Ok(entries)
}

/// 编译词条为词典包。
pub fn pack(
    entries: Vec<RawEntry>,
    out_dir: impl AsRef<Path>,
    opts: &PackOptions,
) -> Result<PackStats, PackError> {
    let out_dir = out_dir.as_ref();
    std::fs::create_dir_all(out_dir)?;

    if opts.payload == PayloadKind::V3 {
        return pack_v3(entries, out_dir, opts);
    }

    // 1) 建索引：键 → record id。同一词条的多条释义合并。
    let mut keys: BTreeMap<String, u64> = BTreeMap::new();
    let mut records: Vec<(RawEntry, Vec<String>)> = Vec::new();

    for entry in entries {
        let mut target_idx: Option<u64> = None;

        // 简体键
        if let Some(&id) = keys.get(&entry.headword) {
            target_idx = Some(id);
        }
        // 注意：**不能**用 traditional/读音键反查已有记录来合并。
        // 日语同音词（はし → 橋/箸/端）各有独立词条；按读音合并会把不同词义的
        // definitions 混进同一条记录。traditional 只作为附加索引键，冲突时保留先到的
        // 记录（单值 FST 无法表达一对多；完整同音多义需要后续 schema 扩展）。

        match target_idx {
            Some(id) => {
                // 已有同名词条：合并释义，去重（id 为 1 起始）
                let merged = &mut records[(id - 1) as usize].1;
                for d in &entry.definitions {
                    if !merged.contains(d) {
                        merged.push(d.clone());
                    }
                }
            }
            None => {
                // 1 起始：SQLite AUTOINCREMENT 语义与显式主键都从 1 开始，
                // 避免 FST 里记录 0 而 SQLite 无 id=0 行的问题。
                let id = records.len() as u64 + 1;
                keys.insert(entry.headword.clone(), id);
                if let Some(trad) = &entry.traditional {
                    keys.entry(trad.clone()).or_insert(id);
                }
                let definitions = entry.definitions.clone();
                records.push((entry, definitions));
            }
        }
    }

    // 2) 写 FST（MapBuilder 要求键按字节序升序，BTreeMap 天然满足）
    let fst_path = out_dir.join("index.fst");
    {
        let file = std::fs::File::create(&fst_path)?;
        let mut writer = std::io::BufWriter::new(file);
        let mut builder = fst::MapBuilder::new(&mut writer)?;
        for (key, id) in &keys {
            builder.insert(key.as_bytes(), *id)?;
        }
        builder.finish()?;
        // BufWriter 在 drop 时 flush；显式 flush 以便随后计算 sha256
        std::io::Write::flush(&mut writer)?;
    }

    // 3) 写 SQLite
    let db_path = out_dir.join("entries.sqlite");
    if db_path.exists() {
        std::fs::remove_file(&db_path)?;
    }
    {
        let mut conn = rusqlite::Connection::open(&db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = OFF;
             PRAGMA synchronous = OFF;",
        )?;
        let tx = conn.transaction()?;
        match opts.payload {
            PayloadKind::V1 => tx.execute_batch(
                "CREATE TABLE entries (
                     id INTEGER PRIMARY KEY,
                     headword TEXT NOT NULL,
                     normalized TEXT NOT NULL,
                     traditional TEXT,
                     pinyin TEXT,
                     pos TEXT,
                     source_id TEXT NOT NULL,
                     source_license TEXT NOT NULL,
                     confidence REAL NOT NULL
                 );
                 CREATE TABLE senses (
                     entry_id INTEGER NOT NULL,
                     idx INTEGER NOT NULL,
                     definition TEXT NOT NULL,
                     PRIMARY KEY (entry_id, idx)
                 );",
            )?,
            PayloadKind::V2 | PayloadKind::V3 => tx.execute_batch(
                "CREATE TABLE entries (
                     id INTEGER PRIMARY KEY,
                     headword TEXT NOT NULL,
                     normalized TEXT NOT NULL,
                     traditional TEXT,
                     pinyin TEXT,
                     pos TEXT,
                     source_id TEXT NOT NULL,
                     source_license TEXT NOT NULL,
                     confidence REAL NOT NULL,
                     senses TEXT NOT NULL
                 );",
            )?,
        }

        let source_id = opts.provenance.name.to_lowercase().replace(' ', "-");
        for (idx, (entry, definitions)) in records.iter().enumerate() {
            let row = RecordRow {
                id: idx as u64 + 1,
                headword: &entry.headword,
                traditional: entry.traditional.as_deref(),
                pinyin: entry.pinyin.as_deref(),
                pos: entry.pos.as_deref(),
                definitions,
                source_id: &source_id,
                source_license: &opts.provenance.license_spdx,
            };
            match opts.payload {
                PayloadKind::V1 => insert_record(&tx, row)?,
                PayloadKind::V2 | PayloadKind::V3 => insert_record_v2(&tx, row)?,
            };
        }

        tx.commit()?;
        // 库文件自报格式：清单与库必须一致，读取侧会当场校验
        conn.execute_batch(&format!(
            "PRAGMA user_version = {};",
            opts.payload.user_version()
        ))?;
        conn.execute_batch("VACUUM;")?;
    }

    // 4) 许可证原文
    let license_path = out_dir.join("LICENSE.txt");
    std::fs::write(&license_path, &opts.license_text)?;

    // 5) 清单
    let index_bytes = std::fs::metadata(&fst_path)?.len();
    let db_bytes = std::fs::metadata(&db_path)?.len();

    let (schema_version, manifest_schema_version, payload_schema_version) = match opts.payload {
        PayloadKind::V1 => (Some(SCHEMA_VERSION), None, None),
        PayloadKind::V2 | PayloadKind::V3 => (
            None,
            Some(MANIFEST_SCHEMA_V2),
            Some(opts.payload.as_str().to_string()),
        ),
    };
    let manifest = DictionaryManifest {
        schema_version,
        manifest_schema_version,
        payload_schema_version,
        id: opts.id.clone(),
        version: opts.version.clone(),
        kind: "dictionary".to_string(),
        language: opts.language.clone(),
        target_languages: opts.target_languages.clone(),
        entry_count: records.len() as u64,
        provenance: opts.provenance.clone(),
        files: vec![
            PackageFile {
                path: "index.fst".to_string(),
                sha256: sha256_file(&fst_path)?,
                bytes: index_bytes,
            },
            PackageFile {
                path: "entries.sqlite".to_string(),
                sha256: sha256_file(&db_path)?,
                bytes: db_bytes,
            },
        ],
    };
    manifest.validate()?;
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    Ok(PackStats {
        records: records.len() as u64,
        keys: keys.len() as u64,
        index_bytes,
        db_bytes,
    })
}

/// v3：一个键可对应多个 entry_id（`postings` 表）。FST 仍存首个 id 用于存在性检查，
/// 真正查询走 `postings(key, rank, entry_id)`。
fn pack_v3(
    entries: Vec<RawEntry>,
    out_dir: &Path,
    opts: &PackOptions,
) -> Result<PackStats, PackError> {
    let mut records: Vec<(RawEntry, Vec<String>)> = Vec::new();
    let mut by_headword: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();
    let mut keys: std::collections::BTreeMap<String, Vec<u64>> = std::collections::BTreeMap::new();

    for entry in entries {
        if let Some(&id) = by_headword.get(&entry.headword) {
            let merged = &mut records[(id - 1) as usize].1;
            for d in &entry.definitions {
                if !merged.contains(d) {
                    merged.push(d.clone());
                }
            }
            continue;
        }
        let id = records.len() as u64 + 1;
        by_headword.insert(entry.headword.clone(), id);
        keys.entry(entry.headword.clone()).or_default().push(id);
        if let Some(trad) = &entry.traditional {
            keys.entry(trad.clone()).or_default().push(id);
        }
        let definitions = entry.definitions.clone();
        records.push((entry, definitions));
    }

    let fst_path = out_dir.join("index.fst");
    {
        let file = std::fs::File::create(&fst_path)?;
        let mut writer = std::io::BufWriter::new(file);
        let mut builder = fst::MapBuilder::new(&mut writer)?;
        for (key, ids) in &keys {
            builder.insert(key.as_bytes(), *ids.first().unwrap_or(&1))?;
        }
        builder.finish()?;
        std::io::Write::flush(&mut writer)?;
    }

    let db_path = out_dir.join("entries.sqlite");
    if db_path.exists() {
        std::fs::remove_file(&db_path)?;
    }
    {
        let mut conn = rusqlite::Connection::open(&db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = OFF;
             PRAGMA synchronous = OFF;",
        )?;
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE entries (
                 id INTEGER PRIMARY KEY,
                 headword TEXT NOT NULL,
                 normalized TEXT NOT NULL,
                 traditional TEXT,
                 pinyin TEXT,
                 pos TEXT,
                 source_id TEXT NOT NULL,
                 source_license TEXT NOT NULL,
                 confidence REAL NOT NULL,
                 senses TEXT NOT NULL
             );
             CREATE TABLE postings (
                 key TEXT NOT NULL,
                 rank INTEGER NOT NULL,
                 entry_id INTEGER NOT NULL,
                 PRIMARY KEY (key, rank)
             );
             CREATE INDEX postings_entry ON postings(entry_id);",
        )?;
        let source_id = opts.provenance.name.to_lowercase().replace(' ', "-");
        for (idx, (entry, definitions)) in records.iter().enumerate() {
            insert_record_v2(
                &tx,
                RecordRow {
                    id: idx as u64 + 1,
                    headword: &entry.headword,
                    traditional: entry.traditional.as_deref(),
                    pinyin: entry.pinyin.as_deref(),
                    pos: entry.pos.as_deref(),
                    definitions,
                    source_id: &source_id,
                    source_license: &opts.provenance.license_spdx,
                },
            )?;
        }
        for (key, ids) in &keys {
            for (rank, id) in ids.iter().enumerate() {
                tx.execute(
                    "INSERT INTO postings (key, rank, entry_id) VALUES (?1, ?2, ?3)",
                    rusqlite::params![key, rank as i64, *id as i64],
                )?;
            }
        }
        tx.commit()?;
        conn.execute_batch("PRAGMA user_version = 3;")?;
        conn.execute_batch("VACUUM;")?;
    }

    let license_path = out_dir.join("LICENSE.txt");
    std::fs::write(&license_path, &opts.license_text)?;
    let index_bytes = std::fs::metadata(&fst_path)?.len();
    let db_bytes = std::fs::metadata(&db_path)?.len();
    let manifest = DictionaryManifest {
        schema_version: None,
        manifest_schema_version: Some(MANIFEST_SCHEMA_V2),
        payload_schema_version: Some(PayloadKind::V3.as_str().to_string()),
        id: opts.id.clone(),
        version: opts.version.clone(),
        kind: "dictionary".to_string(),
        language: opts.language.clone(),
        target_languages: opts.target_languages.clone(),
        entry_count: records.len() as u64,
        provenance: opts.provenance.clone(),
        files: vec![
            PackageFile {
                path: "index.fst".to_string(),
                sha256: sha256_file(&fst_path)?,
                bytes: index_bytes,
            },
            PackageFile {
                path: "entries.sqlite".to_string(),
                sha256: sha256_file(&db_path)?,
                bytes: db_bytes,
            },
        ],
    };
    manifest.validate()?;
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    Ok(PackStats {
        records: records.len() as u64,
        keys: keys.len() as u64,
        index_bytes,
        db_bytes,
    })
}

fn sha256_file(path: &Path) -> Result<String, PackError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# CC-CEDICT
# License: CC BY-SA 4.0
#! version=1
貓 猫 [mao1] /cat/CL:隻|只[zhi1]/
你好 你好 [ni3 hao3] /hello/hi/
";

    fn opts() -> PackOptions {
        PackOptions {
            id: "dictionary.test.zh-en".to_string(),
            version: "0.0.1".to_string(),
            payload: PayloadKind::V1,
            language: "zh-Hans".to_string(),
            target_languages: vec!["en".to_string()],
            provenance: Provenance {
                name: "CC-CEDICT".to_string(),
                source_url: "https://cc-cedict.org/".to_string(),
                revision: "test".to_string(),
                license_spdx: "CC-BY-SA-4.0".to_string(),
                license_file: "LICENSE.txt".to_string(),
                retrieved_at: "2026-11-05".to_string(),
            },
            license_text: "# CC-CEDICT\nLicense: CC BY-SA 4.0\n".to_string(),
        }
    }

    #[test]
    fn 能解析标准行() {
        let parsed =
            parse_cedict(std::io::Cursor::new("貓 猫 [mao1] /cat/ feline/\n"), None).unwrap();
        let entry = &parsed.entries[0];
        assert_eq!(entry.headword, "猫");
        assert_eq!(entry.traditional.as_deref(), Some("貓"));
        assert_eq!(entry.pinyin.as_deref(), Some("mao1"));
        assert_eq!(entry.definitions, vec!["cat", "feline"]);
    }

    #[test]
    fn 畸形行应被跳过而不是中断() {
        let text = "壞行\n貓 猫 [mao1] /cat/\n";
        let parsed = parse_cedict(std::io::Cursor::new(text), None).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.skipped_lines, 1);
    }

    #[test]
    fn limit参数生效() {
        let parsed = parse_cedict(std::io::Cursor::new(SAMPLE), Some(1)).unwrap();
        assert_eq!(parsed.entries.len(), 1);
    }

    #[test]
    fn 头部注释被保留为许可证() {
        let parsed = parse_cedict(std::io::Cursor::new(SAMPLE), None).unwrap();
        assert!(parsed.header.contains("CC BY-SA"));
        assert_eq!(parsed.source_version.as_deref(), Some("1"));
    }

    #[test]
    fn 打包后可被重新打开并查询() {
        let dir = tempfile::tempdir().unwrap();
        let parsed = parse_cedict(std::io::Cursor::new(SAMPLE), None).unwrap();
        let stats = pack(parsed.entries, dir.path(), &opts()).unwrap();

        assert_eq!(stats.records, 2);
        // 简体 + 繁体两个键
        assert_eq!(stats.keys, 3);

        let pkg = translator_dictionary::DictionaryPackage::open(dir.path(), true).unwrap();
        assert_eq!(pkg.entry_count(), 2);

        let entry = pkg.lookup_exact("猫").unwrap().unwrap();
        assert_eq!(entry.senses[0].definition, "cat");
        assert_eq!(entry.source_license, "CC-BY-SA-4.0");

        // 繁体也能查到同一条目
        let trad = pkg.lookup_exact("貓").unwrap().unwrap();
        assert_eq!(trad.headword, "猫");
    }

    #[test]
    fn jsonl_输入可被解析与打包() {
        let jsonl =
            "{\"headword\":\"bank\",\"pos\":\"noun\",\"definitions\":[\"銀行\",\"河岸\"]}\n";
        let entries = parse_jsonl(std::io::Cursor::new(jsonl), None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].definitions.len(), 2);

        let dir = tempfile::tempdir().unwrap();
        let mut o = opts();
        o.id = "dictionary.test.en-zh".into();
        o.language = "en".into();
        o.target_languages = vec!["zh".into()];
        let stats = pack(entries, dir.path(), &o).unwrap();
        assert_eq!(stats.records, 1);
        let pkg = translator_dictionary::DictionaryPackage::open(dir.path(), true).unwrap();
        assert_eq!(pkg.lookup_exact("bank").unwrap().unwrap().senses.len(), 2);
    }

    #[test]
    fn 同读音不同词头不得合并词义() {
        let jsonl = concat!(
            "{\"headword\":\"橋\",\"traditional\":\"はし\",\"definitions\":[\"bridge\"]}\n",
            "{\"headword\":\"箸\",\"traditional\":\"はし\",\"definitions\":[\"chopsticks\"]}\n",
        );
        let entries = parse_jsonl(std::io::Cursor::new(jsonl), None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut o = opts();
        o.id = "dictionary.test.ja-en".into();
        o.language = "ja".into();
        o.target_languages = vec!["en".into()];
        let stats = pack(entries, dir.path(), &o).unwrap();
        assert_eq!(stats.records, 2);
        let pkg = translator_dictionary::DictionaryPackage::open(dir.path(), true).unwrap();
        assert_eq!(
            pkg.lookup_exact("橋").unwrap().unwrap().senses[0].definition,
            "bridge"
        );
        assert_eq!(
            pkg.lookup_exact("箸").unwrap().unwrap().senses[0].definition,
            "chopsticks"
        );
        // 单值读音索引只能指向其中一条，但绝不能把两条词义混在一起
        let hashi = pkg.lookup_exact("はし").unwrap().unwrap();
        assert!(hashi.senses.len() == 1, "同音词不得被合并：{hashi:?}");
    }

    #[test]
    fn v3_同读音多值索引可返回全部词条() {
        let jsonl = concat!(
            "{\"headword\":\"橋\",\"traditional\":\"はし\",\"definitions\":[\"bridge\"]}\n",
            "{\"headword\":\"箸\",\"traditional\":\"はし\",\"definitions\":[\"chopsticks\"]}\n",
            "{\"headword\":\"端\",\"traditional\":\"はし\",\"definitions\":[\"edge\"]}\n",
        );
        let entries = parse_jsonl(std::io::Cursor::new(jsonl), None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut o = opts();
        o.id = "dictionary.test.ja-en.v3".into();
        o.language = "ja".into();
        o.target_languages = vec!["en".into()];
        o.payload = PayloadKind::V3;
        let stats = pack(entries, dir.path(), &o).unwrap();
        assert_eq!(stats.records, 3);
        let pkg = translator_dictionary::DictionaryPackage::open(dir.path(), true).unwrap();
        let all = pkg.lookup_exact_all("はし").unwrap();
        assert_eq!(all.len(), 3, "同音键应返回全部词条：{all:?}");
        let defs: Vec<_> = all
            .iter()
            .map(|e| e.senses[0].definition.as_str())
            .collect();
        assert!(defs.contains(&"bridge"));
        assert!(defs.contains(&"chopsticks"));
        assert!(defs.contains(&"edge"));
        // v2 兼容读取仍由现有测试覆盖；v3 的首条稳定为 rank 0（橋）
        assert_eq!(pkg.lookup_exact("はし").unwrap().unwrap().headword, "橋");
    }

    #[test]
    fn 校验和不匹配应被检出() {
        let dir = tempfile::tempdir().unwrap();
        let parsed = parse_cedict(std::io::Cursor::new(SAMPLE), None).unwrap();
        pack(parsed.entries, dir.path(), &opts()).unwrap();

        // 篡改索引文件
        let fst_path = dir.path().join("index.fst");
        let mut bytes = std::fs::read(&fst_path).unwrap();
        if let Some(last) = bytes.last_mut() {
            *last = last.wrapping_add(1);
        }
        std::fs::write(&fst_path, bytes).unwrap();

        let err = translator_dictionary::DictionaryPackage::open(dir.path(), true).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("校验") || msg.contains("校验失败"),
            "应报校验失败，实际：{msg}"
        );
    }
}
