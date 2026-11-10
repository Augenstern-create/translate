//! SQLite 学习存储与候选/发布闭环。
//!
//! 设计约束：
//! - **主翻译链不被阻塞**：所有错误都返回 `LearningError`，调用方在 API 层忽略并计数；
//! - **Raw 不等于 Verified**：模型译文只能进入 `proposed_target`，不能直接发布；
//! - **发布可回滚**：发布产物是版本化 JSON + manifest，正式资源由 ResourceStore
//!   的发布流程消费；本模块不直接改词库文件。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use translator_types::FallbackReason;

use crate::types::{
    CandidateKind, KnowledgeCandidate, LearningConsent, LearningInput, LearningStats,
    RecordOutcome, ReleaseInfo, ReleaseStatus, ReviewStatus, SkipReason,
};

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug)]
pub enum LearningError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Invalid(String),
    CandidateNotFound(i64),
    ReleaseNotFound(String),
    InvalidTransition {
        from: ReviewStatus,
        to: ReviewStatus,
    },
    ReleaseGate(String),
}

impl std::fmt::Display for LearningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "学习存储 IO 错误：{e}"),
            Self::Sqlite(e) => write!(f, "学习存储数据库错误：{e}"),
            Self::Json(e) => write!(f, "学习存储 JSON 错误：{e}"),
            Self::Invalid(msg) => write!(f, "学习数据非法：{msg}"),
            Self::CandidateNotFound(id) => write!(f, "候选不存在：{id}"),
            Self::ReleaseNotFound(version) => write!(f, "发布版本不存在：{version}"),
            Self::InvalidTransition { from, to } => {
                write!(f, "非法的审核状态迁移：{from} -> {to}")
            }
            Self::ReleaseGate(msg) => write!(f, "发布门禁未通过：{msg}"),
        }
    }
}

impl std::error::Error for LearningError {}

impl From<std::io::Error> for LearningError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for LearningError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}
impl From<serde_json::Error> for LearningError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// 本地学习存储。线程安全（内部串行化 SQLite 连接）。
pub struct LearningStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
    release_root: PathBuf,
}

impl LearningStore {
    /// 打开/创建数据库。父目录必须可写；不存在的父目录会被创建。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LearningError> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
            release_root: db_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("releases"),
            db_path,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn migrate(&self) -> Result<(), LearningError> {
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS records (
                fingerprint TEXT PRIMARY KEY,
                source_language TEXT NOT NULL,
                target_language TEXT NOT NULL,
                source_text TEXT,
                translated_text TEXT,
                fallback_reason TEXT NOT NULL,
                translation_route TEXT NOT NULL,
                model_identity TEXT,
                knowledge_version TEXT NOT NULL,
                context_key TEXT,
                occurrence_count INTEGER NOT NULL DEFAULT 1,
                review_status TEXT NOT NULL DEFAULT 'raw',
                provenance TEXT NOT NULL,
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_records_reason ON records(fallback_reason);
            CREATE INDEX IF NOT EXISTS idx_records_seen ON records(last_seen);
            CREATE TABLE IF NOT EXISTS candidates (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                key TEXT NOT NULL,
                source_text TEXT NOT NULL,
                proposed_target TEXT,
                fallback_reason TEXT NOT NULL,
                occurrence_count INTEGER NOT NULL DEFAULT 1,
                distinct_contexts INTEGER NOT NULL DEFAULT 1,
                status TEXT NOT NULL DEFAULT 'raw',
                provenance TEXT NOT NULL,
                review_note TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(kind, key)
            );
            CREATE INDEX IF NOT EXISTS idx_candidates_status ON candidates(status);
            CREATE TABLE IF NOT EXISTS releases (
                version TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                published_at INTEGER,
                entry_count INTEGER NOT NULL,
                manifest_sha256 TEXT NOT NULL,
                directory TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS release_entries (
                version TEXT NOT NULL,
                candidate_id INTEGER NOT NULL,
                PRIMARY KEY(version, candidate_id)
            );
            "#,
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    /// 记录一次未覆盖/兜底事实。**隐私关闭时返回 `Skipped(PrivacyOff)` 且不写库。**
    pub fn record(
        &self,
        input: &LearningInput,
        consent: &LearningConsent,
    ) -> Result<RecordOutcome, LearningError> {
        if !consent.enabled() {
            return Ok(RecordOutcome::Skipped(SkipReason::PrivacyOff));
        }
        let source = normalize_source(&input.source_text);
        if source.is_empty() {
            return Ok(RecordOutcome::Skipped(SkipReason::EmptySource));
        }

        // 非词库缺口（用户显式选模型/复杂篇章）仍记录聚合原因，但绝不保存正文。
        // 这用于区分“用户显式选模型”与“真缺词”，避免误导后续建设。
        let useful = input.fallback_reason.is_knowledge_gap();

        // 只在保存正文时做敏感内容检查；不保存正文的聚合记录没有泄露面。
        let store_text = consent.level.stores_source_text() && useful;
        if store_text && contains_sensitive(&source) {
            return Ok(RecordOutcome::Skipped(SkipReason::SensitiveContent));
        }

        let stored_source = store_text.then_some(source.as_str());
        let store_translation =
            useful && (consent.level.stores_translation() || consent.store_model_output);
        let stored_translation = if store_translation {
            input.translated_text.as_deref()
        } else {
            None
        };
        let fingerprint = fingerprint(input, &source);
        let now = now_epoch();
        let reason = input.fallback_reason.as_str();
        let route = input.translation_route.as_str();
        let provenance = if useful {
            "learning:model-fallback"
        } else {
            "learning:aggregate-only"
        };

        let conn = self.conn.lock().expect("学习库锁不可中毒");
        conn.execute(
            r#"INSERT INTO records(
                fingerprint, source_language, target_language, source_text, translated_text,
                fallback_reason, translation_route, model_identity, knowledge_version,
                context_key, occurrence_count, review_status, provenance, first_seen, last_seen
            ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,'raw',?11,?12,?12)
            ON CONFLICT(fingerprint) DO UPDATE SET
                occurrence_count = records.occurrence_count + 1,
                last_seen = excluded.last_seen,
                source_text = COALESCE(records.source_text, excluded.source_text),
                translated_text = COALESCE(excluded.translated_text, records.translated_text),
                model_identity = COALESCE(excluded.model_identity, records.model_identity),
                knowledge_version = excluded.knowledge_version"#,
            params![
                fingerprint,
                input.source_language,
                input.target_language,
                stored_source,
                stored_translation,
                reason,
                route,
                input.model_identity,
                input.knowledge_version,
                input.context_key,
                provenance,
                now,
            ],
        )?;
        let count: i64 = conn.query_row(
            "SELECT occurrence_count FROM records WHERE fingerprint=?1",
            params![fingerprint],
            |row| row.get(0),
        )?;
        Ok(RecordOutcome::Recorded {
            occurrence_count: count as u64,
        })
    }

    /// 删除超过保留期的原始记录；已验证候选保留，避免破坏待发布知识。
    pub fn purge_expired(&self, now: i64) -> Result<u64, LearningError> {
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key='retention_days'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(days) = value.and_then(|v| v.parse::<u32>().ok()) else {
            return Ok(0);
        };
        if days == 0 {
            return Ok(0);
        }
        let cutoff = now.saturating_sub(i64::from(days) * 86_400);
        let deleted = conn.execute("DELETE FROM records WHERE last_seen < ?1", params![cutoff])?;
        conn.execute(
            "DELETE FROM candidates WHERE updated_at < ?1 AND status NOT IN ('verified','released')",
            params![cutoff],
        )?;
        Ok(deleted as u64)
    }

    /// 用当前时间清理超过保留期的原始记录。
    pub fn purge_expired_now(&self) -> Result<u64, LearningError> {
        self.purge_expired(now_epoch())
    }

    pub fn set_retention_days(&self, days: Option<u32>) -> Result<(), LearningError> {
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        match days {
            Some(days) => {
                conn.execute(
                    "INSERT INTO meta(key,value) VALUES('retention_days',?1)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![days.to_string()],
                )?;
            }
            None => {
                conn.execute("DELETE FROM meta WHERE key='retention_days'", [])?;
            }
        }
        Ok(())
    }

    /// 清空学习数据（记录、候选、发布索引）。发布目录下的文件也尽力删除。
    pub fn clear_all(&self) -> Result<(), LearningError> {
        {
            let conn = self.conn.lock().expect("学习库锁不可中毒");
            conn.execute("DELETE FROM records", [])?;
            conn.execute("DELETE FROM candidates", [])?;
            conn.execute("DELETE FROM release_entries", [])?;
            conn.execute("DELETE FROM releases", [])?;
        }
        if self.release_root.exists() {
            for entry in fs::read_dir(&self.release_root)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> Result<LearningStats, LearningError> {
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        let (records, occurrences, with_source, with_translation): (u64, u64, u64, u64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(occurrence_count),0),
                        COALESCE(SUM(source_text IS NOT NULL),0),
                        COALESCE(SUM(translated_text IS NOT NULL),0)
                 FROM records",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let candidates: u64 =
            conn.query_row("SELECT COUNT(*) FROM candidates", [], |row| row.get(0))?;
        let verified: u64 = conn.query_row(
            "SELECT COUNT(*) FROM candidates WHERE status='verified'",
            [],
            |row| row.get(0),
        )?;
        let rejected: u64 = conn.query_row(
            "SELECT COUNT(*) FROM candidates WHERE status='rejected'",
            [],
            |row| row.get(0),
        )?;
        let releases: u64 =
            conn.query_row("SELECT COUNT(*) FROM releases", [], |row| row.get(0))?;

        let mut by_reason = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT fallback_reason, SUM(occurrence_count) FROM records GROUP BY fallback_reason",
        )?;
        let rows = stmt.query_map([], |row| {
            let reason: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((reason, count as u64))
        })?;
        for row in rows {
            let (reason, count) = row?;
            if let Some(reason) = parse_reason(&reason) {
                by_reason.push((reason, count));
            }
        }
        by_reason.sort_by_key(|(reason, _)| reason.as_str());
        Ok(LearningStats {
            records,
            occurrences,
            records_with_source_text: with_source,
            records_with_translation: with_translation,
            candidates,
            verified_candidates: verified,
            rejected_candidates: rejected,
            releases,
            by_reason,
        })
    }

    /// 聚合记录并提取词/短语/模板候选。
    ///
    /// `min_occurrences` 只影响候选优先级，不证明候选正确。
    pub fn extract_candidates(&self, min_occurrences: u64) -> Result<usize, LearningError> {
        let rows = {
            let conn = self.conn.lock().expect("学习库锁不可中毒");
            let mut stmt = conn.prepare(
                "SELECT source_text, translated_text, fallback_reason, context_key,
                        occurrence_count
                 FROM records
                 WHERE source_text IS NOT NULL AND occurrence_count >= ?1
                 ORDER BY occurrence_count DESC, last_seen DESC",
            )?;
            let mapped = stmt.query_map(params![min_occurrences as i64], |row| {
                Ok(RecordRow {
                    source_text: row.get(0)?,
                    translated_text: row.get(1)?,
                    fallback_reason: row.get(2)?,
                    context_key: row.get(3)?,
                    occurrence_count: row.get::<_, i64>(4)? as u64,
                })
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };

        // 先按规范化 key 合并不同大小写/空白；不同 context/reason 不合并。
        let mut grouped: BTreeMap<(CandidateKind, String, String), CandidateSeed> = BTreeMap::new();
        let mut pattern_groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        for row in &rows {
            let Some(text) = row.source_text.as_deref() else {
                continue;
            };
            let Some(reason) = parse_reason(&row.fallback_reason) else {
                continue;
            };
            if !reason.is_knowledge_gap() {
                continue;
            }
            let kind = candidate_kind(text);
            let key = canonical_key(text);
            if key.is_empty() {
                continue;
            }
            let entry = grouped
                .entry((
                    kind,
                    key.clone(),
                    row.context_key.clone().unwrap_or_default(),
                ))
                .or_insert_with(|| CandidateSeed {
                    source_text: text.to_string(),
                    proposed_target: None,
                    reason,
                    occurrences: 0,
                    contexts: BTreeSet::new(),
                });
            entry.occurrences = entry.occurrences.saturating_add(row.occurrence_count);
            entry
                .contexts
                .insert(row.context_key.clone().unwrap_or_default());
            if entry.proposed_target.is_none() {
                if let Some(target) = row.translated_text.as_deref() {
                    if !target.trim().is_empty() {
                        entry.proposed_target = Some(target.to_string());
                    }
                }
            }
            if let Some(pattern) = pattern_signature(text) {
                pattern_groups
                    .entry(pattern.0)
                    .or_default()
                    .insert(pattern.1);
            }
        }

        let mut count = 0usize;
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        for ((kind, key, context), seed) in grouped {
            // 不同上下文不得在 SQLite 的唯一键上合并成同一条可信译法。
            let key = if context.is_empty() {
                key
            } else {
                format!("{key}#ctx:{}", short_hash(&context))
            };
            self.upsert_candidate_locked(
                &conn,
                kind,
                &key,
                &seed.source_text,
                seed.proposed_target.as_deref(),
                seed.reason,
                seed.occurrences,
                seed.contexts.len().max(1) as u64,
                "learning:aggregated",
            )?;
            count += 1;
        }

        // 至少两个不同实例才提出参数化模板候选，且状态仍是 Raw。
        for (template, instances) in pattern_groups {
            if instances.len() < 2 {
                continue;
            }
            let source = instances.iter().next().cloned().unwrap_or_default();
            self.upsert_candidate_locked(
                &conn,
                CandidateKind::Pattern,
                &canonical_key(&template),
                &source,
                None,
                FallbackReason::RuleNotCovered,
                instances.len() as u64,
                instances.len() as u64,
                "learning:pattern-suggestion",
            )?;
            count += 1;
        }
        Ok(count)
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_candidate_locked(
        &self,
        conn: &Connection,
        kind: CandidateKind,
        key: &str,
        source_text: &str,
        proposed_target: Option<&str>,
        reason: FallbackReason,
        occurrences: u64,
        contexts: u64,
        provenance: &str,
    ) -> Result<(), LearningError> {
        let now = now_epoch();
        conn.execute(
            r#"INSERT INTO candidates(
                kind, key, source_text, proposed_target, fallback_reason,
                occurrence_count, distinct_contexts, status, provenance,
                review_note, created_at, updated_at
            ) VALUES(?1,?2,?3,?4,?5,?6,?7,'raw',?8,NULL,?9,?9)
            ON CONFLICT(kind,key) DO UPDATE SET
                source_text=excluded.source_text,
                proposed_target=COALESCE(candidates.proposed_target, excluded.proposed_target),
                fallback_reason=excluded.fallback_reason,
                occurrence_count=MAX(candidates.occurrence_count, excluded.occurrence_count),
                distinct_contexts=MAX(candidates.distinct_contexts, excluded.distinct_contexts),
                updated_at=excluded.updated_at
            "#,
            params![
                kind.as_str(),
                key,
                source_text,
                proposed_target,
                reason.as_str(),
                occurrences as i64,
                contexts as i64,
                provenance,
                now,
            ],
        )?;
        // 聚合本身不等于验证；只有尚未评审的 raw 才升级为 aggregated。
        conn.execute(
            "UPDATE candidates SET status='aggregated'
             WHERE kind=?1 AND key=?2 AND status='raw' AND occurrence_count>=2",
            params![kind.as_str(), key],
        )?;
        Ok(())
    }

    pub fn list_candidates(
        &self,
        status: Option<ReviewStatus>,
        limit: usize,
    ) -> Result<Vec<KnowledgeCandidate>, LearningError> {
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        let sql = if status.is_some() {
            "SELECT id,kind,key,source_text,proposed_target,fallback_reason,
                    occurrence_count,distinct_contexts,status,provenance,review_note,
                    created_at,updated_at
             FROM candidates WHERE status=?1
             ORDER BY occurrence_count DESC, id ASC LIMIT ?2"
        } else {
            "SELECT id,kind,key,source_text,proposed_target,fallback_reason,
                    occurrence_count,distinct_contexts,status,provenance,review_note,
                    created_at,updated_at
             FROM candidates
             ORDER BY occurrence_count DESC, id ASC LIMIT ?1"
        };
        let mut out = Vec::new();
        if let Some(status) = status {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(params![status.as_str(), limit as i64], map_candidate)?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(params![limit as i64], map_candidate)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// 人工/可信来源审核。`Verified` 至少要有一个候选译法；模板候选除外。
    pub fn review_candidate(
        &self,
        id: i64,
        to: ReviewStatus,
        note: Option<&str>,
    ) -> Result<(), LearningError> {
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        let (kind, status, target): (String, String, Option<String>) = conn
            .query_row(
                "SELECT kind,status,proposed_target FROM candidates WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or(LearningError::CandidateNotFound(id))?;
        let from = ReviewStatus::parse(&status)
            .ok_or_else(|| LearningError::Invalid(format!("未知状态：{status}")))?;
        let kind = CandidateKind::parse(&kind)
            .ok_or_else(|| LearningError::Invalid("未知候选类型".into()))?;

        let valid = match (from, to) {
            (_, ReviewStatus::Rejected) => true,
            (ReviewStatus::Raw | ReviewStatus::Aggregated, ReviewStatus::Corroborated) => true,
            (
                ReviewStatus::Raw | ReviewStatus::Aggregated | ReviewStatus::Corroborated,
                ReviewStatus::Verified,
            ) => {
                kind == CandidateKind::Pattern
                    || target
                        .as_deref()
                        .map(|t| !t.trim().is_empty())
                        .unwrap_or(false)
            }
            (ReviewStatus::Verified, ReviewStatus::Released) => true,
            (ReviewStatus::Released, ReviewStatus::Verified) => true,
            _ => false,
        };
        if !valid {
            return Err(LearningError::InvalidTransition { from, to });
        }
        if to == ReviewStatus::Verified
            && kind != CandidateKind::Pattern
            && target
                .as_deref()
                .map(|t| t.trim().is_empty())
                .unwrap_or(true)
        {
            return Err(LearningError::ReleaseGate(
                "候选缺少 proposed_target，不能标为 verified".to_string(),
            ));
        }
        conn.execute(
            "UPDATE candidates SET status=?1, review_note=?2, updated_at=?3 WHERE id=?4",
            params![to.as_str(), note, now_epoch(), id],
        )?;
        Ok(())
    }

    /// 将已验证候选写入 staging 版本目录。未验证候选会被拒绝。
    pub fn stage_release(
        &self,
        version: &str,
        candidate_ids: &[i64],
    ) -> Result<ReleaseInfo, LearningError> {
        if version.trim().is_empty()
            || version.contains('/')
            || version.contains('\\')
            || version.contains("..")
        {
            return Err(LearningError::Invalid(format!("非法版本号：{version}")));
        }
        let candidates = {
            let conn = self.conn.lock().expect("学习库锁不可中毒");
            let mut out = Vec::new();
            for id in candidate_ids {
                let candidate = conn
                    .query_row(
                        "SELECT id,kind,key,source_text,proposed_target,fallback_reason,
                                occurrence_count,distinct_contexts,status,provenance,review_note,
                                created_at,updated_at
                         FROM candidates WHERE id=?1",
                        params![id],
                        map_candidate,
                    )
                    .optional()?
                    .ok_or(LearningError::CandidateNotFound(*id))?;
                if !candidate.status.can_release() {
                    return Err(LearningError::ReleaseGate(format!(
                        "候选 {} 状态为 {}，不是 verified",
                        id, candidate.status
                    )));
                }
                out.push(candidate);
            }
            out
        };
        if candidates.is_empty() {
            return Err(LearningError::ReleaseGate("发布版本不能为空".to_string()));
        }

        let directory = self.release_root.join(version);
        fs::create_dir_all(&directory)?;
        let created_at = now_epoch();
        let manifest = serde_json::json!({
            "schema_version": 1,
            "kind": "knowledge.candidates",
            "version": version,
            "created_at": created_at,
            "source": "translator-learning",
            "entry_count": candidates.len(),
            "entries": candidates.iter().map(|c| serde_json::json!({
                "kind": c.kind.as_str(),
                "key": c.key,
                "source": c.source_text,
                "target": c.proposed_target,
                "provenance": c.provenance,
                "review_status": c.status.as_str(),
                "review_note": c.review_note,
            })).collect::<Vec<_>>(),
        });
        let bytes = serde_json::to_vec_pretty(&manifest)?;
        let knowledge_path = directory.join("knowledge.json");
        fs::write(&knowledge_path, &bytes)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let knowledge_digest = format!("{:x}", hasher.finalize());

        // 产物直接兼容 ResourceStore 的 v2 清单，可被 `install_from_dir` 安装、
        // 激活和回滚；它不是词库文件，不会自动进入正式词典。
        let resource_manifest = serde_json::json!({
            "manifest_schema_version": 2,
            "payload_schema_version": "knowledge.json.v1",
            "id": "knowledge.learning.en-zh",
            "version": version,
            "kind": "knowledge",
            "language": "en",
            "target_languages": ["zh-Hans"],
            "entry_count": candidates.len(),
            "provenance": {
                "name": "translator-learning staged candidates",
                "source_url": "local://translator-learning",
                "revision": version,
                "license_spdx": "LicenseRef-Proprietary",
                "license_file": "LICENSE.txt",
                "retrieved_at": created_at.to_string(),
            },
            "files": [{
                "path": "knowledge.json",
                "sha256": knowledge_digest,
                "bytes": bytes.len(),
            }],
            "dependencies": [],
            "platforms": [],
        });
        let manifest_bytes = serde_json::to_vec_pretty(&resource_manifest)?;
        fs::write(directory.join("manifest.json"), &manifest_bytes)?;
        fs::write(
            directory.join("LICENSE.txt"),
            "Staged candidate knowledge produced by translator-learning.\n             These entries are verified candidates, not automatically merged into a dictionary.\n",
        )?;
        let mut hasher = Sha256::new();
        hasher.update(&manifest_bytes);
        let digest = format!("{:x}", hasher.finalize());

        {
            let conn = self.conn.lock().expect("学习库锁不可中毒");
            conn.execute(
                "INSERT INTO releases(version,status,created_at,published_at,entry_count,
                                      manifest_sha256,directory)
                 VALUES(?1,'staged',?2,NULL,?3,?4,?5)",
                params![
                    version,
                    created_at,
                    candidates.len() as i64,
                    digest,
                    directory.to_string_lossy().to_string()
                ],
            )?;
            for candidate in &candidates {
                conn.execute(
                    "INSERT INTO release_entries(version,candidate_id) VALUES(?1,?2)",
                    params![version, candidate.id],
                )?;
            }
        }
        self.release_info(version)
    }

    /// 发布一个 staged 版本，并把之前的 published 标记为 superseded。
    pub fn publish_release(&self, version: &str) -> Result<ReleaseInfo, LearningError> {
        let now = now_epoch();
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM releases WHERE version=?1",
                params![version],
                |row| row.get(0),
            )
            .optional()?;
        let status = status.ok_or_else(|| LearningError::ReleaseNotFound(version.to_string()))?;
        if status != "staged" {
            return Err(LearningError::ReleaseGate(format!(
                "版本 {version} 状态为 {status}，只有 staged 可以发布"
            )));
        }
        conn.execute(
            "UPDATE releases SET status='superseded' WHERE status='published'",
            [],
        )?;
        conn.execute(
            "UPDATE releases SET status='published', published_at=?1 WHERE version=?2",
            params![now, version],
        )?;
        conn.execute(
            "UPDATE candidates SET status='released', updated_at=?1
             WHERE id IN (SELECT candidate_id FROM release_entries WHERE version=?2)",
            params![now, version],
        )?;
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('active_release',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![version],
        )?;
        drop(conn);
        self.release_info(version)
    }

    /// 回滚到上一个曾经发布过的版本（不含当前版本）；没有则返回错误。
    pub fn rollback_release(&self) -> Result<ReleaseInfo, LearningError> {
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        let current: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key='active_release'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let previous: Option<String> = conn
            .query_row(
                "SELECT version FROM releases
                 WHERE status IN ('published','superseded') AND version<>?1
                 ORDER BY COALESCE(published_at,created_at) DESC, created_at DESC LIMIT 1",
                params![current.as_deref().unwrap_or("")],
                |row| row.get(0),
            )
            .optional()?;
        let previous =
            previous.ok_or_else(|| LearningError::ReleaseGate("没有可回滚的版本".to_string()))?;
        let old_active: Option<String> = current;
        conn.execute(
            "UPDATE releases SET status='superseded' WHERE status='published'",
            [],
        )?;
        conn.execute(
            "UPDATE releases SET status='published', published_at=?1 WHERE version=?2",
            params![now_epoch(), previous],
        )?;
        if let Some(old) = old_active {
            conn.execute(
                "UPDATE candidates SET status='verified'
                 WHERE id IN (SELECT candidate_id FROM release_entries WHERE version=?1)",
                params![old],
            )?;
        }
        conn.execute(
            "UPDATE candidates SET status='released'
             WHERE id IN (SELECT candidate_id FROM release_entries WHERE version=?1)",
            params![previous],
        )?;
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='active_release'",
            params![previous],
        )?;
        drop(conn);
        self.release_info(&previous)
    }

    pub fn release_info(&self, version: &str) -> Result<ReleaseInfo, LearningError> {
        let conn = self.conn.lock().expect("学习库锁不可中毒");
        let (status, created_at, published_at, entry_count, digest, directory): (
            String,
            i64,
            Option<i64>,
            i64,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT status,created_at,published_at,entry_count,manifest_sha256,directory
                 FROM releases WHERE version=?1",
                params![version],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| LearningError::ReleaseNotFound(version.to_string()))?;
        Ok(ReleaseInfo {
            version: version.to_string(),
            status: ReleaseStatus::parse(&status)
                .ok_or_else(|| LearningError::Invalid(format!("未知发布状态：{status}")))?,
            created_at,
            published_at,
            entry_count: entry_count as u64,
            manifest_sha256: digest,
            directory,
        })
    }

    pub fn active_release(&self) -> Result<Option<ReleaseInfo>, LearningError> {
        let version = {
            let conn = self.conn.lock().expect("学习库锁不可中毒");
            let value: Option<String> = conn
                .query_row(
                    "SELECT value FROM meta WHERE key='active_release'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            value
        };
        match version {
            Some(version) => self.release_info(&version).map(Some),
            None => Ok(None),
        }
    }
}

#[derive(Clone)]
struct RecordRow {
    source_text: Option<String>,
    translated_text: Option<String>,
    fallback_reason: String,
    context_key: Option<String>,
    occurrence_count: u64,
}

struct CandidateSeed {
    source_text: String,
    proposed_target: Option<String>,
    reason: FallbackReason,
    occurrences: u64,
    contexts: BTreeSet<String>,
}

fn map_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeCandidate> {
    let kind: String = row.get(1)?;
    let status: String = row.get(8)?;
    let reason: String = row.get(5)?;
    let invalid = |what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("未知{what}"),
            )),
        )
    };
    Ok(KnowledgeCandidate {
        id: row.get(0)?,
        kind: CandidateKind::parse(&kind).ok_or_else(|| invalid("候选类型"))?,
        key: row.get(2)?,
        source_text: row.get(3)?,
        proposed_target: row.get(4)?,
        fallback_reason: parse_reason(&reason).ok_or_else(|| invalid("兜底原因"))?,
        occurrence_count: row.get::<_, i64>(6)? as u64,
        distinct_contexts: row.get::<_, i64>(7)? as u64,
        status: ReviewStatus::parse(&status).ok_or_else(|| invalid("审核状态"))?,
        provenance: row.get(9)?,
        review_note: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn parse_reason(raw: &str) -> Option<FallbackReason> {
    Some(match raw {
        "dictionary_miss" => FallbackReason::DictionaryMiss,
        "phrase_miss" => FallbackReason::PhraseMiss,
        "sense_ambiguous" => FallbackReason::SenseAmbiguous,
        "rule_not_covered" => FallbackReason::RuleNotCovered,
        "rule_rejected" => FallbackReason::RuleRejected,
        "complex_syntax" => FallbackReason::ComplexSyntax,
        "long_context" => FallbackReason::LongContext,
        "explicit_model_request" => FallbackReason::ExplicitModelRequest,
        "engine_fallback" => FallbackReason::EngineFallback,
        _ => return None,
    })
}

fn normalize_source(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn canonical_key(text: &str) -> String {
    normalize_source(text)
        .trim_end_matches(['.', '!', '?', '。', '！', '？'])
        .to_ascii_lowercase()
}

fn fingerprint(input: &LearningInput, normalized_source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"learning-record-v1\0");
    hasher.update(normalized_source.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.source_language.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.target_language.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.fallback_reason.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(input.context_key.as_deref().unwrap_or("").as_bytes());
    format!("{:x}", hasher.finalize())
}

fn short_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
        .chars()
        .take(12)
        .collect()
}

fn candidate_kind(text: &str) -> CandidateKind {
    if text.split_whitespace().count() <= 1 {
        CandidateKind::Word
    } else {
        CandidateKind::Phrase
    }
}

/// 从重复出现的“同一句框”提出参数化模板候选。
///
/// 只处理最保守的形式：`Please restart the X.` → `Please restart the {object}.`
/// 不自动把结果当正式规则；它只是 Raw 候选，仍需正反例与人工审核。
fn pattern_signature(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim();
    let body = trimmed.trim_end_matches(['.', '!', '?', '。', '！', '？']);
    let words: Vec<&str> = body.split_whitespace().collect();
    if words.len() < 4 || words.len() > 12 {
        return None;
    }
    let lower0 = words[0].to_ascii_lowercase();
    if lower0 != "please" && lower0 != "the" && lower0 != "this" && lower0 != "that" {
        return None;
    }
    let last = *words.last()?;
    if last.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut prefix = words[..words.len() - 1].join(" ");
    prefix.push_str(" {object}.");
    Some((prefix, last.to_ascii_lowercase()))
}

fn contains_sensitive(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "api_key",
        "apikey",
        "api-key",
        "secret_key",
        "private_key",
        "password=",
        "password:",
        "passwd=",
        "authorization:",
        "bearer ",
        "access_token",
        "refresh_token",
        "-----begin",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 指纹区分上下文与原因() {
        let base = LearningInput {
            source_language: "en".into(),
            target_language: "zh-Hans".into(),
            source_text: "bank".into(),
            translated_text: None,
            fallback_reason: FallbackReason::DictionaryMiss,
            translation_route: translator_types::TranslationRoute::LocalNmt,
            model_identity: None,
            knowledge_version: "v1".into(),
            context_key: Some("river".into()),
        };
        let a = fingerprint(&base, "bank");
        let mut b = base.clone();
        b.context_key = Some("money".into());
        let b = fingerprint(&b, "bank");
        assert_ne!(a, b, "不同上下文不得合并为同一条可信译法");
    }

    #[test]
    fn 敏感内容标记可识别() {
        assert!(contains_sensitive("Authorization: Bearer xyz"));
        assert!(!contains_sensitive("The server is ready."));
    }

    #[test]
    fn 模板候选只接受保守句式() {
        assert_eq!(
            pattern_signature("Please restart the service."),
            Some((
                "Please restart the {object}.".to_string(),
                "service".to_string()
            ))
        );
        assert!(pattern_signature("This is fine.").is_none());
    }
}
