//! 资源清单：v1（P1 词典包）与 v2（协议/存储版本分离）统一表示。
//!
//! # 为什么要有两个版本号（ADR-010）
//!
//! P1 的词典清单只有一个 `schema_version`，它同时承担了两件事：
//!
//! 1. **资源协议**：这个包是谁、什么版本、依赖什么、目标平台、怎么校验；
//! 2. **payload 存储格式**：`entries.sqlite` 的表结构。
//!
//! 两者变化频率完全不同：协议可能因为要加"签名""平台约束"而升级，
//! 而 SQLite 表结构可以十年不动；反之我们要给词典加个"释义内联列"，也不该逼所有
//! 资源格式陪着升一次版本。于是 v2 拆成：
//!
//! ```text
//! manifest_schema_version: 2              // 协议
//! payload_schema_version:  "dictionary.sqlite.v1"   // 存储
//! ```
//!
//! # v1 兼容策略
//!
//! v1 包**只读兼容**：解析成统一表示后，payload 版本按 `kind` 推断
//! （`dictionary` → `dictionary.sqlite.v1`）。旧包不需要重新打包，
//! 但也不会被"升级"回去写文件。
//!
//! 这条与"旧规则包（`pipeline_version: 1`）不可执行"是**两件独立的事**：
//! 前者是数据格式兼容，后者是不安全行为的兼容（ADR-009）。

use serde::Deserialize;

use crate::error::{ResourceError, ResourceResult};

/// v2 协议版本。
pub const MANIFEST_SCHEMA_V2: u32 = 2;
/// v1 协议版本（P1 词典包）。
pub const MANIFEST_SCHEMA_V1: u32 = 1;

/// payload 存储格式。未知值必须**原样保留字符串**，好让错误信息说清楚。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayloadSchema {
    /// `entries.sqlite` 两张表：entries + senses。
    DictionarySqliteV1,
    /// `entries.sqlite` 释义内联进 `entries.senses`（P2.5）。
    DictionarySqliteV2,
    /// `entries.sqlite` + `postings` 多值索引（P9.1-S）。
    DictionarySqliteV3,
    /// 规则包 JSON（`translator-rules` 的 schema_version=1）。
    RulesJsonV1,
    Unknown(String),
}

impl PayloadSchema {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "dictionary.sqlite.v1" => Self::DictionarySqliteV1,
            "dictionary.sqlite.v2" => Self::DictionarySqliteV2,
            "dictionary.sqlite.v3" => Self::DictionarySqliteV3,
            "rules.json.v1" => Self::RulesJsonV1,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::DictionarySqliteV1 => "dictionary.sqlite.v1",
            Self::DictionarySqliteV2 => "dictionary.sqlite.v2",
            Self::DictionarySqliteV3 => "dictionary.sqlite.v3",
            Self::RulesJsonV1 => "rules.json.v1",
            Self::Unknown(raw) => raw,
        }
    }

    /// 本 SDK 能否读取该 payload 格式。
    pub fn is_supported(&self) -> bool {
        matches!(
            self,
            Self::DictionarySqliteV1
                | Self::DictionarySqliteV2
                | Self::DictionarySqliteV3
                | Self::RulesJsonV1
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Dictionary,
    Rules,
    Model,
    Analyzer,
    Unknown(String),
}

impl ResourceKind {
    fn parse(raw: &str) -> Self {
        match raw {
            "dictionary" => Self::Dictionary,
            "rules" => Self::Rules,
            "model" => Self::Model,
            "analyzer" => Self::Analyzer,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Dictionary => "dictionary",
            Self::Rules => "rules",
            Self::Model => "model",
            Self::Analyzer => "analyzer",
            Self::Unknown(raw) => raw,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestSchema {
    V1,
    V2,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub name: String,
    pub source_url: String,
    pub revision: String,
    pub license_spdx: String,
    pub license_file: String,
    pub retrieved_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackageFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

/// 依赖：`id` + 可选的最低版本（当前只做相等/存在性检查，见 `compat`）。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawV1 {
    schema_version: u32,
    id: String,
    version: String,
    kind: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    target_languages: Vec<String>,
    #[serde(default)]
    entry_count: Option<u64>,
    provenance: Provenance,
    files: Vec<PackageFile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawV2 {
    manifest_schema_version: u32,
    payload_schema_version: String,
    id: String,
    version: String,
    kind: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    target_languages: Vec<String>,
    #[serde(default)]
    entry_count: Option<u64>,
    provenance: Provenance,
    files: Vec<PackageFile>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
    #[serde(default)]
    platforms: Vec<String>,
}

/// 统一后的资源清单。
#[derive(Clone, Debug)]
pub struct ResourceManifest {
    pub manifest_schema: ManifestSchema,
    pub payload_schema: PayloadSchema,
    pub id: String,
    pub version: String,
    pub kind: ResourceKind,
    pub language: Option<String>,
    pub target_languages: Vec<String>,
    pub entry_count: Option<u64>,
    pub provenance: Provenance,
    pub files: Vec<PackageFile>,
    pub dependencies: Vec<Dependency>,
    /// 空表示"所有平台"。
    pub platforms: Vec<String>,
}

impl ResourceManifest {
    /// 解析清单 JSON，自动识别 v1 / v2。
    ///
    /// 识别方式：看有没有 `manifest_schema_version` 字段，而不是"先按 v2 试、失败再按 v1 试"——
    /// 后者会把字段拼错的 v2 误当成 v1 解析成功，属于"猜"，本 SDK 不做。
    pub fn parse(json: &str) -> ResourceResult<Self> {
        let value: serde_json::Value = serde_json::from_str(json)?;

        if value.get("manifest_schema_version").is_some() && value.get("schema_version").is_some() {
            return Err(ResourceError::InvalidManifest(
                "同时出现 manifest_schema_version 与 schema_version，无法判断协议版本".to_string(),
            ));
        }

        if value.get("manifest_schema_version").is_some() {
            let raw: RawV2 = serde_json::from_value(value)?;
            if raw.manifest_schema_version != MANIFEST_SCHEMA_V2 {
                return Err(ResourceError::UnsupportedManifestSchema(
                    raw.manifest_schema_version,
                ));
            }
            let manifest = Self {
                manifest_schema: ManifestSchema::V2,
                payload_schema: PayloadSchema::parse(&raw.payload_schema_version),
                id: raw.id,
                version: raw.version,
                kind: ResourceKind::parse(&raw.kind),
                language: raw.language,
                target_languages: raw.target_languages,
                entry_count: raw.entry_count,
                provenance: raw.provenance,
                files: raw.files,
                dependencies: raw.dependencies,
                platforms: raw.platforms,
            };
            manifest.validate()?;
            return Ok(manifest);
        }

        let raw: RawV1 = serde_json::from_value(value)?;
        if raw.schema_version != MANIFEST_SCHEMA_V1 {
            return Err(ResourceError::UnsupportedManifestSchema(raw.schema_version));
        }
        let kind = ResourceKind::parse(&raw.kind);
        // v1 没有 payload 版本字段：按 kind 推断，便于上层统一处理
        let payload_schema = match kind {
            ResourceKind::Dictionary => PayloadSchema::DictionarySqliteV1,
            ResourceKind::Rules => PayloadSchema::RulesJsonV1,
            _ => PayloadSchema::Unknown(format!("v1:{}", raw.kind)),
        };
        let manifest = Self {
            manifest_schema: ManifestSchema::V1,
            payload_schema,
            id: raw.id,
            version: raw.version,
            kind,
            language: raw.language,
            target_languages: raw.target_languages,
            entry_count: raw.entry_count,
            provenance: raw.provenance,
            files: raw.files,
            dependencies: Vec::new(),
            platforms: Vec::new(),
        };
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> ResourceResult<()> {
        if self.id.trim().is_empty() {
            return Err(ResourceError::InvalidManifest("id 不能为空".to_string()));
        }
        if self.version.trim().is_empty() {
            return Err(ResourceError::InvalidManifest(
                "version 不能为空".to_string(),
            ));
        }
        if self.files.is_empty() {
            return Err(ResourceError::InvalidManifest(
                "files 不能为空：没有文件就无从校验".to_string(),
            ));
        }
        for file in &self.files {
            if file.path.trim().is_empty() {
                return Err(ResourceError::InvalidManifest(
                    "files[].path 为空".to_string(),
                ));
            }
            if file.sha256.len() != 64 || !file.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ResourceError::InvalidManifest(format!(
                    "{} 的 sha256 不是 64 位十六进制",
                    file.path
                )));
            }
            if is_unsafe_relative(&file.path) {
                return Err(ResourceError::UnsafePath(file.path.clone()));
            }
        }
        for dep in &self.dependencies {
            if dep.id.trim().is_empty() {
                return Err(ResourceError::InvalidManifest("依赖的 id 为空".to_string()));
            }
        }
        if !matches!(self.payload_schema, PayloadSchema::Unknown(_)) {
            // 已知 payload 必须有对应文件，否则读的时候才发现就太晚了
            let expected = self.payload_schema.as_str();
            let is_dictionary = matches!(
                self.payload_schema,
                PayloadSchema::DictionarySqliteV1
                    | PayloadSchema::DictionarySqliteV2
                    | PayloadSchema::DictionarySqliteV3
            );
            let has = self.files.iter().any(|f| {
                (is_dictionary && f.path == "entries.sqlite")
                    || (expected == "rules.json.v1" && f.path.ends_with(".json"))
            });
            if !has {
                return Err(ResourceError::InvalidManifest(format!(
                    "payload {expected} 缺少对应文件"
                )));
            }
        }
        Ok(())
    }

    /// `id@version`。缓存指纹、日志都用这个形式。
    pub fn fingerprint(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    pub fn is_v1(&self) -> bool {
        matches!(self.manifest_schema, ManifestSchema::V1)
    }
}

/// 绝对路径、`..`、以 `/` 开头、含盘符，一律视为穿越。
pub(crate) fn is_unsafe_relative(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') {
        return true;
    }
    if path.contains(':') {
        // Windows 盘符或 NTFS 备用数据流
        return true;
    }
    path.split(['/', '\\'])
        .any(|part| part == ".." || part == ".")
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = r#"{
        "schema_version": 1,
        "id": "dictionary.cc-cedict.zh-en",
        "version": "0.1.0",
        "kind": "dictionary",
        "language": "zh-Hans",
        "target_languages": ["en"],
        "entry_count": 121584,
        "provenance": {
            "name": "CC-CEDICT", "source_url": "https://example.invalid/",
            "revision": "snapshot-2026-11-05", "license_spdx": "CC-BY-SA-4.0",
            "license_file": "LICENSE.txt", "retrieved_at": "2026-11-05"
        },
        "files": [
            { "path": "index.fst", "sha256": "0000000000000000000000000000000000000000000000000000000000000000", "bytes": 10 },
            { "path": "entries.sqlite", "sha256": "1111111111111111111111111111111111111111111111111111111111111111", "bytes": 20 }
        ]
    }"#;

    const V2: &str = r#"{
        "manifest_schema_version": 2,
        "payload_schema_version": "dictionary.sqlite.v1",
        "id": "dictionary.cc-cedict.zh-en",
        "version": "0.2.0",
        "kind": "dictionary",
        "language": "zh-Hans",
        "target_languages": ["en"],
        "entry_count": 121584,
        "provenance": {
            "name": "CC-CEDICT", "source_url": "https://example.invalid/",
            "revision": "snapshot-2026-11-05", "license_spdx": "CC-BY-SA-4.0",
            "license_file": "LICENSE.txt", "retrieved_at": "2026-11-05"
        },
        "files": [
            { "path": "index.fst", "sha256": "0000000000000000000000000000000000000000000000000000000000000000", "bytes": 10 },
            { "path": "entries.sqlite", "sha256": "1111111111111111111111111111111111111111111111111111111111111111", "bytes": 20 }
        ],
        "dependencies": [],
        "platforms": ["*"]
    }"#;

    #[test]
    fn v1清单可读且推断出payload版本() {
        let m = ResourceManifest::parse(V1).expect("v1 应兼容读取");
        assert!(m.is_v1());
        assert_eq!(m.payload_schema, PayloadSchema::DictionarySqliteV1);
        assert_eq!(m.kind, ResourceKind::Dictionary);
        assert_eq!(m.fingerprint(), "dictionary.cc-cedict.zh-en@0.1.0");
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn v2清单解析出显式payload版本() {
        let m = ResourceManifest::parse(V2).expect("v2 应可解析");
        assert_eq!(m.manifest_schema, ManifestSchema::V2);
        assert_eq!(m.payload_schema, PayloadSchema::DictionarySqliteV1);
        assert_eq!(m.version, "0.2.0");
    }

    #[test]
    fn 未知manifest版本被拒收() {
        let json = V2.replace(
            "\"manifest_schema_version\": 2",
            "\"manifest_schema_version\": 9",
        );
        assert!(matches!(
            ResourceManifest::parse(&json),
            Err(ResourceError::UnsupportedManifestSchema(9))
        ));
    }

    #[test]
    fn 未知payload版本不被静默接受() {
        let json = V2.replace("dictionary.sqlite.v1", "dictionary.sqlite.v9");
        let m = ResourceManifest::parse(&json).expect("语法合法，解析阶段不拒绝");
        assert_eq!(
            m.payload_schema,
            PayloadSchema::Unknown("dictionary.sqlite.v9".into())
        );
        assert!(!m.payload_schema.is_supported(), "兼容性检查必须拒绝它");
    }

    #[test]
    fn 同时出现两个协议版本字段时拒收() {
        let json = V2.replace(
            "\"payload_schema_version\"",
            "\"schema_version\": 1, \"payload_schema_version\"",
        );
        assert!(matches!(
            ResourceManifest::parse(&json),
            Err(ResourceError::InvalidManifest(_))
        ));
    }

    #[test]
    fn 路径穿越被拒收() {
        for bad in ["../evil.fst", "/etc/passwd", "a/../../b", "C:evil", "./x"] {
            let json = V2.replace("index.fst", bad);
            let result = ResourceManifest::parse(&json);
            assert!(
                matches!(result, Err(ResourceError::UnsafePath(_))),
                "{bad} 应被拒收，实际：{result:?}"
            );
        }
    }

    #[test]
    fn 缺少payload文件时拒收() {
        let json = V2.replace("\"path\": \"entries.sqlite\"", "\"path\": \"other.sqlite\"");
        assert!(matches!(
            ResourceManifest::parse(&json),
            Err(ResourceError::InvalidManifest(_))
        ));
    }

    #[test]
    fn sha256格式不合法时拒收() {
        let json = V2.replace(&"0".repeat(64), "zz");
        assert!(matches!(
            ResourceManifest::parse(&json),
            Err(ResourceError::InvalidManifest(_))
        ));
    }

    #[test]
    fn 字段拼错不会被当成v1静默接受() {
        // v2 多写一个未知字段 → deny_unknown_fields 报错，而不是降级解析
        let json = V2.replace("\"id\":", "\"idd\":");
        assert!(ResourceManifest::parse(&json).is_err());
    }
}
