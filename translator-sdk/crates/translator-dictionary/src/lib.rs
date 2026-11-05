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

/// 包格式版本。不兼容变更必须递增。
pub const SCHEMA_VERSION: u32 = 1;

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

/// 词典包清单。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DictionaryManifest {
    pub schema_version: u32,
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
    /// 校验清单自洽性。**拒绝任何 `REQUIRED` 占位符**（设计文档 §9）。
    pub fn validate(&self) -> Result<(), DictionaryError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(DictionaryError::InvalidPackage(format!(
                "schema_version 应为 {SCHEMA_VERSION}，实际 {}",
                self.schema_version
            )));
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

        let conn = rusqlite::Connection::open_with_flags(
            root.join("entries.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // 让 SQLite 尽量用 mmap 读页，降低 RSS
        let _ = conn.pragma_update(None, "mmap_size", 256 * 1024 * 1024_i64);
        let _ = conn.pragma_update(None, "cache_size", -2000_i64);

        let language = LanguageTag::new(&manifest.language)
            .map_err(|e| DictionaryError::InvalidPackage(format!("清单语言非法：{e}")))?;

        Ok(Self {
            root,
            manifest,
            language,
            index,
            db: Mutex::new(conn),
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

    /// 精确匹配。
    pub fn lookup_exact(&self, term: &str) -> Result<Option<DictionaryEntry>, DictionaryError> {
        let Some(id) = self.index.get(term.as_bytes()) else {
            return Ok(None);
        };
        Ok(Some(self.load_entry(id)?))
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

        if let Some(id) = self.index.get(term.as_bytes()) {
            seen.insert(id);
            out.push(self.load_entry(id)?);
            if out.len() >= limit {
                return Ok(out);
            }
        }

        // 前缀检索：仅在精确命中不足时补充
        let prefix = fst::automaton::Str::new(term).starts_with();
        let mut stream = self.index.search(prefix).into_stream();
        while let Some((_, id)) = stream.next() {
            if !seen.insert(id) {
                continue; // 该 record 已出现过
            }
            out.push(self.load_entry(id)?);
            if out.len() >= limit {
                break;
            }
        }

        Ok(out)
    }

    /// 按 record id 读取完整词条。
    fn load_entry(&self, id: u64) -> Result<DictionaryEntry, DictionaryError> {
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

        let mut readings = Vec::new();
        if let Some(p) = pinyin {
            readings.push(p);
        }
        if let Some(t) = traditional {
            readings.push(format!("繁体：{t}"));
        }

        Ok(DictionaryEntry {
            headword,
            normalized,
            language: self.language.clone(),
            readings,
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
