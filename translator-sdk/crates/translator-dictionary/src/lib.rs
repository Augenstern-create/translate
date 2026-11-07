//! 离线词典包：FST 索引 + SQLite 释义库，只读、mmap 友好。
//!
//! 对应设计文档 §7.1「统一词条数据模型」与 §9「Resource & Model Manager」。
//!
//! # 包结构
//!
//! ```text
//! <package_dir>/
//! ├── manifest.json    # 清单：语言、来源、许可证、校验和
//! ├── index.fst        # 精确匹配 + 前缀检索（mmap 只读）
//! ├── entries.sqlite   # 释义详情
//! └── LICENSE.txt      # 许可证原文
//! ```
//!
//! # 设计取舍
//!
//! - **索引与释义分离**：FST 只有 term → record_id，常驻内存极小；
//!   释义按需从 SQLite 读，避免启动时把所有词条载入内存（§9.3 的内存预算要求）。
//! - **每条释义都带 `source_id`**：许可证来源可追溯（§7.1），
//!   不允许二次打包后丢失来源。
//! - **索引键 = 规范化后的词条**，同一条目可同时支持简体与繁体键。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fst::{Automaton, IntoStreamer, Streamer};
use serde::{Deserialize, Serialize};
use translator_types::{
    DictionaryEntry, LanguageTag, Sense, TranslationError, TranslationErrorCode,
};

pub use engine::{DictionaryEngine, DictionaryProviderImpl};

mod engine;

/// v1 清单协议版本（P1 词典包，字段名 `schema_version`）。
pub const SCHEMA_VERSION: u32 = 1;
/// v2 清单协议版本（字段名 `manifest_schema_version`，与 payload 版本分离）。
pub const MANIFEST_SCHEMA_V2: u32 = 2;

/// `entries.sqlite` 的存储格式。
///
/// | 值 | 结构 | 读取成本 |
/// |---|---|---|
/// | `dictionary.sqlite.v1` | `entries` + `senses` 两张表，读一个词条要 2 次 B-tree 查找 | 基准 |
/// | `dictionary.sqlite.v2` | 释义内联进 `entries.senses`（U+001F 分隔），1 次查找 | 更快 |
///
/// v2 的代价是**失去逐条释义的行**：只读场景下没人按释义做查询，
/// 因此这个交换是划算的（实测见 `docs/07` §3.3）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    V1,
    V2,
    /// 多值索引：FST 仍存首个 id，SQLite `postings` 表按键保存全部 entry_id。
    V3,
}

impl PayloadKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "dictionary.sqlite.v1",
            Self::V2 => "dictionary.sqlite.v2",
            Self::V3 => "dictionary.sqlite.v3",
        }
    }

    /// 库文件的 `PRAGMA user_version`。v1 保持 0（历史包没有写过这个值）。
    pub const fn user_version(self) -> i64 {
        match self {
            Self::V1 => 0,
            Self::V2 => 2,
            Self::V3 => 3,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "dictionary.sqlite.v1" => Some(Self::V1),
            "dictionary.sqlite.v2" => Some(Self::V2),
            "dictionary.sqlite.v3" => Some(Self::V3),
            _ => None,
        }
    }
}

/// v2 内联释义的分隔符（ASCII Unit Separator）。
///
/// 选它而不是 `\n` 或 `|`：释义文本里可能出现任何可打印字符，
/// 而 U+001F 是 C0 控制符，真实语料里不会自然出现。
pub const SENSE_SEPARATOR: char = '\u{1f}';

/// SQLite mmap 上限（字节）。与 `PRAGMA mmap_size` 一致。
///
/// 这是**虚拟映射上限**，不是 RSS：SQLite 只会把真正读到的页换入。
const MMAP_SIZE_BYTES: u64 = 256 * 1024 * 1024;

/// SQLite 页缓存配置（KiB）。负值表示 KiB，见 SQLite `PRAGMA cache_size`。
///
/// 抽成常量是为了让「打开时配置的值」与「内存估算里假设的值」不可能漂移——
/// 之前这两处一旦不一致，估算就等于撒谎。
const SQLITE_CACHE_KIB: i64 = 2000;

/// 每个 SQLite 连接 + 语句缓存 + 分配的固定开销估计（字节）。
///
/// 这是一个**猜测值**，没有实测依据（实测只测到进程整体 RSS，见 `docs/04` §6.2）。
/// 宁可写成一个显式的、有名字的常量，也不要把未知量悄悄摊到别处。
const SQLITE_CONNECTION_OVERHEAD_BYTES: u64 = 256 * 1024;

// ============================================================
// 错误
// ============================================================

#[derive(Debug)]
pub enum DictionaryError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Fst(fst::Error),
    Json(serde_json::Error),
    /// 包结构不符合约定（缺文件、清单字段非法等）。
    InvalidPackage(String),
    /// 校验和不匹配 —— 设计文档 §12.1 要求必须能检出。
    IntegrityFailure {
        file: String,
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for DictionaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO 错误：{e}"),
            Self::Sqlite(e) => write!(f, "SQLite 错误：{e}"),
            Self::Fst(e) => write!(f, "FST 错误：{e}"),
            Self::Json(e) => write!(f, "清单解析失败：{e}"),
            Self::InvalidPackage(msg) => write!(f, "资源包非法：{msg}"),
            Self::IntegrityFailure {
                file,
                expected,
                actual,
            } => write!(f, "校验失败：{file} 期望 {expected}，实际 {actual}"),
        }
    }
}

impl std::error::Error for DictionaryError {}

impl From<std::io::Error> for DictionaryError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for DictionaryError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<fst::Error> for DictionaryError {
    fn from(e: fst::Error) -> Self {
        Self::Fst(e)
    }
}

impl From<serde_json::Error> for DictionaryError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<DictionaryError> for TranslationError {
    fn from(e: DictionaryError) -> Self {
        let code = match &e {
            DictionaryError::IntegrityFailure { .. } => TranslationErrorCode::IntegrityFailure,
            _ => TranslationErrorCode::BackendFailed,
        };
        TranslationError::new(code, e.to_string())
    }
}

// ============================================================
// 清单
// ============================================================

/// 资源来源与许可信息。设计文档 §9 要求每个包都能独立审核。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    /// 人类可读名称，如 `CC-CEDICT`。
    pub name: String,
    pub source_url: String,
    /// 数据版本或 revision（不允许填 `REQUIRED`）。
    pub revision: String,
    /// SPDX 标识，如 `CC-BY-SA-4.0`。
    pub license_spdx: String,
    /// 许可证原文文件名。
    pub license_file: String,
    /// 获取时间（ISO 8601）。
    pub retrieved_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

/// 词典包清单（v1 / v2 统一表示）。
///
/// 三个版本字段都是 `Option`：v1 包只有 `schema_version`，
/// v2 包只有 `manifest_schema_version` + `payload_schema_version`。
/// 序列化时 `None` 不写出，避免又造出"两种字段都写"的歧义包。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DictionaryManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_schema_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_schema_version: Option<String>,
    pub id: String,
    pub version: String,
    /// 固定为 `dictionary`。
    pub kind: String,
    /// 词条语言，如 `zh-Hans`。
    pub language: String,
    /// 释义语言，如 `["en"]`。
    pub target_languages: Vec<String>,
    pub entry_count: u64,
    pub provenance: Provenance,
    pub files: Vec<PackageFile>,
}

impl DictionaryManifest {
    /// 这份清单声明的 payload 存储格式。
    ///
    /// 判定顺序刻意写死，不做"都试一遍"：
    ///
    /// 1. 有 `payload_schema_version` → 按它解析，认不出就**报错**（不猜）；
    /// 2. 否则看 `schema_version == 1` → v1（P1 历史包）；
    /// 3. 其余情况报错。
    ///
    /// 这条规则与资源层的 `ResourceManifest` 一致：v1 是数据兼容，不是行为兼容。
    pub fn payload_kind(&self) -> Result<PayloadKind, DictionaryError> {
        if let Some(raw) = &self.payload_schema_version {
            return PayloadKind::parse(raw).ok_or_else(|| {
                DictionaryError::InvalidPackage(format!("不支持的 payload_schema_version={raw}"))
            });
        }
        match (self.schema_version, self.manifest_schema_version) {
            (Some(SCHEMA_VERSION), _) => Ok(PayloadKind::V1),
            (None, Some(MANIFEST_SCHEMA_V2)) => Err(DictionaryError::InvalidPackage(
                "manifest_schema_version=2 的包必须声明 payload_schema_version".to_string(),
            )),
            (None, Some(other)) => Err(DictionaryError::InvalidPackage(format!(
                "不支持的 manifest_schema_version={other}"
            ))),
            (Some(other), _) => Err(DictionaryError::InvalidPackage(format!(
                "不支持的 schema_version={other}（本 SDK 支持 {SCHEMA_VERSION}）"
            ))),
            (None, None) => Err(DictionaryError::InvalidPackage(
                "清单里没有 schema_version / manifest_schema_version，无法判断格式".to_string(),
            )),
        }
    }

    /// 校验清单自洽性。**拒绝任何 `REQUIRED` 占位符**（设计文档 §9）。
    pub fn validate(&self) -> Result<(), DictionaryError> {
        // 版本字段本身是否自洽（也顺带挡住"两个版本字段都写"的歧义包）
        self.payload_kind()?;
        if self.schema_version.is_some() && self.manifest_schema_version.is_some() {
            return Err(DictionaryError::InvalidPackage(
                "同时出现 schema_version 与 manifest_schema_version，无法判断协议版本".to_string(),
            ));
        }
        if self.kind != "dictionary" {
            return Err(DictionaryError::InvalidPackage(format!(
                "kind 应为 dictionary，实际 {}",
                self.kind
            )));
        }
        let placeholder_fields = [
            ("provenance.revision", self.provenance.revision.as_str()),
            (
                "provenance.license_spdx",
                self.provenance.license_spdx.as_str(),
            ),
            ("provenance.source_url", self.provenance.source_url.as_str()),
        ];
        for (field, value) in placeholder_fields {
            if value.trim().is_empty() || value.contains("REQUIRED") || value.contains("TBD") {
                return Err(DictionaryError::InvalidPackage(format!(
                    "{field} 未填写真实值（当前：{value:?}）"
                )));
            }
        }
        if self.files.is_empty() {
            return Err(DictionaryError::InvalidPackage("files 不得为空".into()));
        }
        Ok(())
    }

    /// 资源内容摘要：`id@version` 之外再带上逐文件 sha256。
    ///
    /// 用途是缓存隔离（ADR-011 / P8.3 EP-01）：同一个 `id@version` 被重打包成
    /// 不同内容时，不能让旧译文命中。摘要不包含绝对路径，跨机器稳定。
    pub fn content_digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.id.as_bytes());
        hasher.update(b"@");
        hasher.update(self.version.as_bytes());
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        for file in files {
            hasher.update(b"\n");
            hasher.update(file.path.as_bytes());
            hasher.update(b"\0");
            hasher.update(file.sha256.as_bytes());
        }
        let digest = format!("{:x}", hasher.finalize());
        digest[..16].to_string()
    }
}

// ============================================================
// 资源包
// ============================================================

/// 已打开的只读词典包。
pub struct DictionaryPackage {
    root: PathBuf,
    manifest: DictionaryManifest,
    /// 词条语言在打开时解析一次，避免每次查询都做字符串规范化。
    language: LanguageTag,
    index: fst::Map<memmap2::Mmap>,
    db: Mutex<rusqlite::Connection>,
    /// 释义库文件大小（打开时取一次 metadata，用于内存上限估算）。
    db_bytes: u64,
    /// 释义库的存储格式（决定读路径）。
    payload: PayloadKind,
}

impl DictionaryPackage {
    /// 打开包并校验完整性。
    ///
    /// `verify` 为真时逐文件比对 sha256（较慢，建议仅在安装时执行）。
    pub fn open(root: impl AsRef<Path>, verify: bool) -> Result<Self, DictionaryError> {
        let root = root.as_ref().to_path_buf();

        let manifest_path = root.join("manifest.json");
        let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|e| {
            DictionaryError::InvalidPackage(format!("无法读取 {}: {e}", manifest_path.display()))
        })?;
        let manifest: DictionaryManifest = serde_json::from_str(&manifest_text)?;
        manifest.validate()?;

        for file in &manifest.files {
            let path = root.join(&file.path);
            let meta = std::fs::metadata(&path).map_err(|e| {
                DictionaryError::InvalidPackage(format!("缺少文件 {}: {e}", path.display()))
            })?;
            if meta.len() != file.bytes {
                return Err(DictionaryError::IntegrityFailure {
                    file: file.path.clone(),
                    expected: format!("{} 字节", file.bytes),
                    actual: format!("{} 字节", meta.len()),
                });
            }
            if verify {
                let actual = sha256_file(&path)?;
                if actual != file.sha256 {
                    return Err(DictionaryError::IntegrityFailure {
                        file: file.path.clone(),
                        expected: file.sha256.clone(),
                        actual,
                    });
                }
            }
        }

        let index_file = std::fs::File::open(root.join("index.fst"))?;
        // SAFETY: 包在打开后按约定只读；文件被外部修改属于使用错误。
        let mmap = unsafe { memmap2::Mmap::map(&index_file)? };
        let index = fst::Map::new(mmap)?;

        // 清单声明的 payload 格式决定读路径；打开前先确认，避免读到一半才发现不匹配
        let payload = manifest.payload_kind()?;

        let conn = rusqlite::Connection::open_with_flags(
            root.join("entries.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        // 清单说的格式必须与文件自报的格式一致：
        // 打包时把 v2 的库配上 v1 的清单（或反过来）是最容易犯的错，
        // 这种包读起来会"看起来能用但释义为空"，必须当场拒绝。
        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version != payload.user_version() {
            return Err(DictionaryError::InvalidPackage(format!(
                "清单声明 {}（期望 user_version={}），但 entries.sqlite 的 user_version={}",
                payload.as_str(),
                payload.user_version(),
                user_version
            )));
        }
        // 让 SQLite 尽量用 mmap 读页，降低 RSS
        let _ = conn.pragma_update(None, "mmap_size", MMAP_SIZE_BYTES as i64);
        let _ = conn.pragma_update(None, "cache_size", -SQLITE_CACHE_KIB);

        let language = LanguageTag::new(&manifest.language)
            .map_err(|e| DictionaryError::InvalidPackage(format!("清单语言非法：{e}")))?;

        let db_bytes = std::fs::metadata(root.join("entries.sqlite"))
            .map(|m| m.len())
            .unwrap_or(0);

        Ok(Self {
            root,
            manifest,
            language,
            index,
            db: Mutex::new(conn),
            db_bytes,
            payload,
        })
    }

    pub fn manifest(&self) -> &DictionaryManifest {
        &self.manifest
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entry_count(&self) -> u64 {
        self.manifest.entry_count
    }

    /// 释义库的存储格式。
    pub fn payload_kind(&self) -> PayloadKind {
        self.payload
    }

    /// 词条语言，例如 `zh-Hans`。
    pub fn language(&self) -> &str {
        &self.manifest.language
    }

    /// 已解析的词条语言。
    pub fn language_tag(&self) -> &LanguageTag {
        &self.language
    }

    /// 索引占用的常驻字节数（FST mmap 的大小）。
    pub fn index_bytes(&self) -> u64 {
        self.index.as_fst().as_bytes().len() as u64
    }

    /// 释义库文件大小（不是常驻内存：SQLite 按需读页）。
    pub fn db_file_bytes(&self) -> u64 {
        self.db_bytes
    }

    /// 内存占用估算（P2.3，供 `MemoryBudget` 记账）。
    ///
    /// **这是下界，不是保证值**，理由见返回值里各字段的说明：
    ///
    /// | 字段 | 含义 | 准确性 |
    /// |---|---|---|
    /// | `index_bytes` | FST mmap 的文件大小 | 上界（mmap 只读，可能从未换入） |
    /// | `page_cache_bytes` | `PRAGMA cache_size` 配置值 | 精确（就是配置的那个数） |
    /// | `overhead_bytes` | 连接/语句/分配器 | **猜的**，无实测 |
    ///
    /// 实测结论（`docs/04` §6.2，本机、真实 CC-CEDICT 121,584 条）：
    /// 包 1.7 MB 索引 + 19.1 MB 释义库，查词后进程 RSS **+9.8 MB**，
    /// 总占用 24.3–24.9 MB。也就是说真实 RSS 与「文件总大小」同量级，
    /// 远大于本函数给出的下界——因此预算判定只能说「肯定不够」，
    /// 不能说「够用」。设计文档 §9.3 明确禁止把预算数字当保证值。
    pub fn memory_footprint(&self) -> MemoryFootprint {
        let index_bytes = self.index_bytes();
        let page_cache_bytes = (SQLITE_CACHE_KIB as u64) * 1024;
        MemoryFootprint {
            index_bytes,
            page_cache_bytes,
            overhead_bytes: SQLITE_CONNECTION_OVERHEAD_BYTES,
            // 最坏情况：索引全部换入 + 释义库被 mmap 全读进来
            ceiling_bytes: index_bytes + self.db_bytes + page_cache_bytes,
            db_file_bytes: self.db_bytes,
        }
    }

    /// 精确匹配。
    pub fn lookup_exact(&self, term: &str) -> Result<Option<DictionaryEntry>, DictionaryError> {
        Ok(self.lookup_exact_all(term)?.into_iter().next())
    }

    /// 精确匹配的全部词条。
    ///
    /// v1/v2 一个键最多一条；v3 通过 `postings(key, rank, entry_id)` 返回同音/同形
    /// 的全部词条（例如 はし → 橋/箸/端）。排序由 `rank` 保证稳定。
    pub fn lookup_exact_all(&self, term: &str) -> Result<Vec<DictionaryEntry>, DictionaryError> {
        let Some(_first) = self.index.get(term.as_bytes()) else {
            return Ok(Vec::new());
        };
        let ids = self.entry_ids_for_key(term)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.load_entry(id)?);
        }
        Ok(out)
    }

    fn entry_ids_for_key(&self, key: &str) -> Result<Vec<u64>, DictionaryError> {
        if self.payload != PayloadKind::V3 {
            return Ok(self.index.get(key.as_bytes()).into_iter().collect());
        }
        let conn = self
            .db
            .lock()
            .map_err(|_| DictionaryError::InvalidPackage("数据库连接锁被污染".to_string()))?;
        let mut stmt =
            conn.prepare("SELECT entry_id FROM postings WHERE key = ?1 ORDER BY rank ASC")?;
        let rows = stmt.query_map([key], |row| row.get::<_, i64>(0))?;
        let mut ids = Vec::new();
        for id in rows {
            ids.push(id? as u64);
        }
        Ok(ids)
    }

    /// 精确匹配 + 前缀补全，最多返回 `limit` 条。
    pub fn lookup(
        &self,
        term: &str,
        limit: usize,
    ) -> Result<Vec<DictionaryEntry>, DictionaryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        // 同一词条可能同时以简体键与繁体键存在，必须按 record id 去重
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();

        let mut push_ids =
            |ids: Vec<u64>, out: &mut Vec<DictionaryEntry>| -> Result<bool, DictionaryError> {
                for id in ids {
                    if !seen.insert(id) {
                        continue;
                    }
                    out.push(self.load_entry(id)?);
                    if out.len() >= limit {
                        return Ok(true);
                    }
                }
                Ok(false)
            };

        if self.index.get(term.as_bytes()).is_some()
            && push_ids(self.entry_ids_for_key(term)?, &mut out)?
        {
            return Ok(out);
        }

        // 前缀检索：仅在精确命中不足时补充。v3 的每个键可能对应多条记录。
        let prefix = fst::automaton::Str::new(term).starts_with();
        let mut stream = self.index.search(prefix).into_stream();
        while let Some((key, _)) = stream.next() {
            let key = String::from_utf8_lossy(key).to_string();
            if push_ids(self.entry_ids_for_key(&key)?, &mut out)? {
                break;
            }
        }

        Ok(out)
    }

    /// 按 record id 读取完整词条。按 payload 格式分派。
    fn load_entry(&self, id: u64) -> Result<DictionaryEntry, DictionaryError> {
        match self.payload {
            PayloadKind::V1 => self.load_entry_v1(id),
            PayloadKind::V2 | PayloadKind::V3 => self.load_entry_v2(id),
        }
    }

    /// v2：单次查询取回全部字段（释义内联，U+001F 分隔）。
    fn load_entry_v2(&self, id: u64) -> Result<DictionaryEntry, DictionaryError> {
        let conn = self
            .db
            .lock()
            .map_err(|_| DictionaryError::InvalidPackage("数据库连接锁被污染".to_string()))?;

        let (headword, normalized, traditional, pinyin, pos, source_id, source_license, confidence, senses) = conn
            .prepare_cached(
                "SELECT headword, normalized, traditional, pinyin, pos, source_id, source_license, confidence, senses \
                 FROM entries WHERE id = ?1",
            )?
            .query_row([id as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, f64>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(|e| {
                DictionaryError::InvalidPackage(format!("record {id} 读取失败：{e}"))
            })?;

        let senses = senses
            .split(SENSE_SEPARATOR)
            .filter(|s| !s.is_empty())
            .map(|definition| Sense {
                definition: definition.to_string(),
                source_id: source_id.clone(),
            })
            .collect();

        Ok(DictionaryEntry {
            headword,
            normalized,
            language: self.language.clone(),
            readings: build_readings(pinyin, traditional),
            parts_of_speech: pos
                .map(|p| p.split('|').map(str::to_string).collect())
                .unwrap_or_default(),
            senses,
            source_id,
            source_license,
            confidence: confidence as f32,
        })
    }

    /// v1：两条查询（entries + senses）。
    fn load_entry_v1(&self, id: u64) -> Result<DictionaryEntry, DictionaryError> {
        let conn = self
            .db
            .lock()
            .map_err(|_| DictionaryError::InvalidPackage("数据库连接锁被污染".to_string()))?;

        let (headword, normalized, traditional, pinyin, pos, source_id, source_license, confidence) = conn
            .prepare_cached(
                "SELECT headword, normalized, traditional, pinyin, pos, source_id, source_license, confidence \
                 FROM entries WHERE id = ?1",
            )?
            .query_row([id as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, f64>(7)?,
                ))
            })
            .map_err(|e| {
                DictionaryError::InvalidPackage(format!("record {id} 读取失败：{e}"))
            })?;

        // 语句缓存：D-01 基准显示每次重新 prepare 占单次查询耗时的四成以上
        let mut stmt =
            conn.prepare_cached("SELECT definition FROM senses WHERE entry_id = ?1 ORDER BY idx")?;
        let senses = stmt
            .query_map([id as i64], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|definition| Sense {
                definition,
                source_id: source_id.clone(),
            })
            .collect();

        Ok(DictionaryEntry {
            headword,
            normalized,
            language: self.language.clone(),
            readings: build_readings(pinyin, traditional),
            parts_of_speech: pos
                .map(|p| p.split('|').map(str::to_string).collect())
                .unwrap_or_default(),
            senses,
            source_id,
            source_license,
            confidence: confidence as f32,
        })
    }
}

impl std::fmt::Debug for DictionaryPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DictionaryPackage")
            .field("root", &self.root)
            .field("id", &self.manifest.id)
            .field("language", &self.manifest.language)
            .field("entry_count", &self.manifest.entry_count)
            .finish()
    }
}

/// 由 `pinyin` / `traditional` 组装 `readings`。v1 与 v2 共用，避免两条读路径分叉。
fn build_readings(pinyin: Option<String>, traditional: Option<String>) -> Vec<String> {
    let mut readings = Vec::new();
    if let Some(p) = pinyin {
        readings.push(p);
    }
    if let Some(t) = traditional {
        readings.push(format!("繁体：{t}"));
    }
    readings
}

/// 单份词典包的内存估算。字段含义见 [`DictionaryPackage::memory_footprint`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryFootprint {
    pub index_bytes: u64,
    pub page_cache_bytes: u64,
    pub overhead_bytes: u64,
    /// 最坏情况（索引 + 释义库 + 页缓存全部常驻）。
    pub ceiling_bytes: u64,
    pub db_file_bytes: u64,
}

impl MemoryFootprint {
    /// 记账用的下界（`MemoryBudget` 只需要这一项）。
    pub fn accounting_bytes(&self) -> u64 {
        self.index_bytes + self.page_cache_bytes + self.overhead_bytes
    }
}

impl std::fmt::Display for MemoryFootprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "索引 {} KiB + 页缓存 {} KiB + 固定开销 {} KiB = {} KiB（最坏 {} KiB）",
            self.index_bytes / 1024,
            self.page_cache_bytes / 1024,
            self.overhead_bytes / 1024,
            self.accounting_bytes() / 1024,
            self.ceiling_bytes / 1024
        )
    }
}

/// 只读清单（不打开索引与数据库）。
///
/// 资源管理器与缓存指纹需要 id/version 时用它，避免为了取一个版本号
/// 就把 mmap 和 SQLite 都打开一遍。
pub fn read_manifest(root: impl AsRef<Path>) -> Result<DictionaryManifest, DictionaryError> {
    let path = root.as_ref().join("manifest.json");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        DictionaryError::InvalidPackage(format!("无法读取 {}: {e}", path.display()))
    })?;
    let manifest: DictionaryManifest = serde_json::from_str(&text)?;
    manifest.validate()?;
    Ok(manifest)
}

/// 计算文件 sha256。
pub fn sha256_file(path: impl AsRef<Path>) -> Result<String, DictionaryError> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path.as_ref())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// 构造 [`DictionaryEngine`] 与 [`DictionaryProviderImpl`]。
pub fn open_as_plugins(
    root: impl AsRef<Path>,
) -> Result<
    (
        std::sync::Arc<DictionaryEngine>,
        std::sync::Arc<DictionaryProviderImpl>,
    ),
    DictionaryError,
> {
    let package = std::sync::Arc::new(DictionaryPackage::open(root, false)?);
    Ok((
        std::sync::Arc::new(DictionaryEngine::new(std::sync::Arc::clone(&package))),
        std::sync::Arc::new(DictionaryProviderImpl::new(package)),
    ))
}

#[cfg(test)]
mod p83_tests {
    use super::*;

    fn manifest(sha: &str) -> DictionaryManifest {
        DictionaryManifest {
            schema_version: None,
            manifest_schema_version: Some(MANIFEST_SCHEMA_V2),
            payload_schema_version: Some("dictionary.sqlite.v1".into()),
            id: "dictionary.test.zh-en".into(),
            version: "1.0.0".into(),
            kind: "dictionary".into(),
            language: "zh-Hans".into(),
            target_languages: vec!["en".into()],
            entry_count: 1,
            provenance: Provenance {
                name: "TEST".into(),
                source_url: "https://example.invalid".into(),
                revision: "r1".into(),
                license_spdx: "CC0-1.0".into(),
                license_file: "LICENSE.txt".into(),
                retrieved_at: "2026-11-07".into(),
            },
            files: vec![
                PackageFile {
                    path: "entries.sqlite".into(),
                    sha256: sha.into(),
                    bytes: 1,
                },
                PackageFile {
                    path: "index.fst".into(),
                    sha256: sha.into(),
                    bytes: 1,
                },
            ],
        }
    }

    #[test]
    fn 同id版本不同内容摘要不同() {
        let a = manifest(&"a".repeat(64));
        let b = manifest(&"b".repeat(64));
        assert_eq!(a.content_digest(), a.content_digest());
        assert_ne!(a.content_digest(), b.content_digest());
        assert_eq!(a.content_digest().len(), 16);
    }
}
