//! 兼容性判定。
//!
//! 原则：**不理解就拒绝**。资源加载器不允许"尽量试试"，因为一个半读成功的词典
//! 比一个明确报错的缺失词典更危险——它会把错误数据当成译文来源。

use std::collections::BTreeMap;

use crate::error::ResourceError;
use crate::manifest::{ManifestSchema, PayloadSchema, ResourceKind, ResourceManifest};

/// 宿主能力：当前 SDK 支持什么。
#[derive(Clone, Debug)]
pub struct HostSupport {
    pub manifest_schemas: Vec<u32>,
    pub payload_schemas: Vec<PayloadSchema>,
    pub platforms: Vec<String>,
}

impl HostSupport {
    /// 本 SDK 当前能力。
    pub fn current() -> Self {
        Self {
            manifest_schemas: vec![
                crate::manifest::MANIFEST_SCHEMA_V1,
                crate::manifest::MANIFEST_SCHEMA_V2,
            ],
            payload_schemas: vec![
                PayloadSchema::DictionarySqliteV1,
                PayloadSchema::DictionarySqliteV2,
                PayloadSchema::DictionarySqliteV3,
                PayloadSchema::RulesJsonV1,
            ],
            platforms: vec![current_platform()],
        }
    }

    /// 测试用：只支持给定 payload。
    pub fn with_payload_schemas(mut self, schemas: Vec<PayloadSchema>) -> Self {
        self.payload_schemas = schemas;
        self
    }
}

/// 当前平台标签，与清单里 `platforms` 的写法一致（`linux-x86_64` 这类）。
pub fn current_platform() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!("{os}-{arch}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompatibilityIssue {
    UnsupportedManifestSchema(u32),
    UnsupportedPayloadSchema(String),
    UnsupportedPlatform(String),
    MissingDependency {
        id: String,
        requirement: String,
    },
    /// 依赖存在但版本不满足。
    DependencyVersionMismatch {
        id: String,
        required: String,
        installed: String,
    },
    /// 已知 kind 与 payload 不匹配（例如本 SDK 没有 model 的读取器）。
    KindNotReadable(String),
}

impl std::fmt::Display for CompatibilityIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedManifestSchema(v) => write!(f, "不支持 manifest_schema_version={v}"),
            Self::UnsupportedPayloadSchema(v) => write!(f, "不支持 payload_schema_version={v}"),
            Self::UnsupportedPlatform(p) => write!(f, "不支持平台 {p}"),
            Self::MissingDependency { id, requirement } => {
                write!(f, "缺少依赖 {id} {requirement}")
            }
            Self::DependencyVersionMismatch {
                id,
                required,
                installed,
            } => write!(f, "依赖 {id} 需要 {required}，已安装 {installed}"),
            Self::KindNotReadable(kind) => write!(f, "本 SDK 没有 {kind} 类型的读取器"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompatibilityReport {
    pub issues: Vec<CompatibilityIssue>,
}

impl CompatibilityReport {
    pub fn is_compatible(&self) -> bool {
        self.issues.is_empty()
    }
}

/// 判断资源能否被本宿主加载。
///
/// `installed` 是"已安装资源"的 `id -> version`，用来检查依赖。
pub fn check(
    manifest: &ResourceManifest,
    host: &HostSupport,
    installed: &BTreeMap<String, String>,
) -> CompatibilityReport {
    let mut issues = Vec::new();

    let schema = match manifest.manifest_schema {
        ManifestSchema::V1 => crate::manifest::MANIFEST_SCHEMA_V1,
        ManifestSchema::V2 => crate::manifest::MANIFEST_SCHEMA_V2,
    };
    if !host.manifest_schemas.contains(&schema) {
        issues.push(CompatibilityIssue::UnsupportedManifestSchema(schema));
    }

    if !host.payload_schemas.contains(&manifest.payload_schema) {
        issues.push(CompatibilityIssue::UnsupportedPayloadSchema(
            manifest.payload_schema.as_str().to_string(),
        ));
    }

    // 平台：空 = 所有平台；含 "*" = 所有平台
    if !manifest.platforms.is_empty() && !manifest.platforms.iter().any(|p| p == "*") {
        let platform = current_platform();
        if !manifest.platforms.contains(&platform) {
            issues.push(CompatibilityIssue::UnsupportedPlatform(platform));
        }
    }

    // kind 与 payload 的一致性：已知 kind 必须有读取器
    match (&manifest.kind, &manifest.payload_schema) {
        (ResourceKind::Model | ResourceKind::Analyzer, _) => {
            issues.push(CompatibilityIssue::KindNotReadable(
                manifest.kind.as_str().to_string(),
            ));
        }
        (ResourceKind::Unknown(raw), _) => {
            issues.push(CompatibilityIssue::KindNotReadable(raw.clone()));
        }
        _ => {}
    }

    for dep in &manifest.dependencies {
        match installed.get(&dep.id) {
            None => issues.push(CompatibilityIssue::MissingDependency {
                id: dep.id.clone(),
                requirement: dep
                    .version
                    .clone()
                    .unwrap_or_else(|| "(任意版本)".to_string()),
            }),
            Some(version) => {
                if let Some(required) = &dep.version {
                    if required != version {
                        issues.push(CompatibilityIssue::DependencyVersionMismatch {
                            id: dep.id.clone(),
                            required: required.clone(),
                            installed: version.clone(),
                        });
                    }
                }
            }
        }
    }

    CompatibilityReport { issues }
}

/// 兼容性不通过时转成错误（保留第一条原因，其余拼在信息里）。
pub fn ensure_compatible(report: &CompatibilityReport) -> Result<(), ResourceError> {
    let Some(first) = report.issues.first() else {
        return Ok(());
    };
    let detail = report
        .issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("；");
    Err(match first {
        CompatibilityIssue::UnsupportedManifestSchema(v) => {
            ResourceError::UnsupportedManifestSchema(*v)
        }
        CompatibilityIssue::UnsupportedPayloadSchema(v) => {
            ResourceError::UnsupportedPayloadSchema(v.clone())
        }
        CompatibilityIssue::UnsupportedPlatform(p) => ResourceError::UnsupportedPlatform(p.clone()),
        CompatibilityIssue::MissingDependency { id, requirement } => {
            ResourceError::MissingDependency {
                id: id.clone(),
                requirement: requirement.clone(),
            }
        }
        _ => ResourceError::InvalidManifest(detail),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(v: &str) -> ResourceManifest {
        let files = if v == "v1" {
            r#""files": [
                { "path": "index.fst", "sha256": "0000000000000000000000000000000000000000000000000000000000000000", "bytes": 1 },
                { "path": "entries.sqlite", "sha256": "1111111111111111111111111111111111111111111111111111111111111111", "bytes": 1 }
            ]"#
        } else {
            r#""files": [
                { "path": "index.fst", "sha256": "0000000000000000000000000000000000000000000000000000000000000000", "bytes": 1 },
                { "path": "entries.sqlite", "sha256": "1111111111111111111111111111111111111111111111111111111111111111", "bytes": 1 }
            ],
            "dependencies": [{ "id": "rules.base", "version": "1.0.0" }],
            "platforms": ["*"]"#
        };
        let head = if v == "v1" {
            r#""schema_version": 1"#
        } else {
            r#""manifest_schema_version": 2, "payload_schema_version": "dictionary.sqlite.v1""#
        };
        let json = format!(
            r#"{{ {head},
                "id": "dictionary.x", "version": "1.0.0", "kind": "dictionary",
                "language": "zh-Hans", "target_languages": ["en"], "entry_count": 1,
                "provenance": {{ "name": "X", "source_url": "https://example.invalid",
                  "revision": "r1", "license_spdx": "CC0-1.0", "license_file": "LICENSE.txt",
                  "retrieved_at": "2026-11-05" }},
                {files} }}"#
        );
        ResourceManifest::parse(&json).expect("测试清单应可解析")
    }

    #[test]
    fn v1包在当前宿主下兼容() {
        let m = manifest("v1");
        let report = check(&m, &HostSupport::current(), &BTreeMap::new());
        assert!(report.is_compatible(), "{:?}", report.issues);
    }

    #[test]
    fn 未知payload版本不兼容() {
        let mut m = manifest("v2");
        m.payload_schema = PayloadSchema::Unknown("dictionary.sqlite.v9".into());
        let report = check(&m, &HostSupport::current(), &BTreeMap::new());
        assert!(!report.is_compatible());
        assert!(matches!(
            report.issues[0],
            CompatibilityIssue::UnsupportedPayloadSchema(_)
        ));
        assert!(ensure_compatible(&report).is_err());
    }

    #[test]
    fn 缺依赖不兼容() {
        let m = manifest("v2");
        let report = check(&m, &HostSupport::current(), &BTreeMap::new());
        assert!(!report.is_compatible());
        assert!(matches!(
            report.issues[0],
            CompatibilityIssue::MissingDependency { .. }
        ));
    }

    #[test]
    fn 依赖版本不符不兼容() {
        let m = manifest("v2");
        let mut installed = BTreeMap::new();
        installed.insert("rules.base".to_string(), "0.9.0".to_string());
        let report = check(&m, &HostSupport::current(), &installed);
        assert!(matches!(
            report.issues[0],
            CompatibilityIssue::DependencyVersionMismatch { .. }
        ));

        installed.insert("rules.base".to_string(), "1.0.0".to_string());
        let report = check(&m, &HostSupport::current(), &installed);
        assert!(report.is_compatible(), "{:?}", report.issues);
    }

    #[test]
    fn 平台不匹配不兼容() {
        let mut m = manifest("v2");
        m.platforms = vec!["plan9-mips".to_string()];
        let report = check(&m, &HostSupport::current(), &{
            let mut installed = BTreeMap::new();
            installed.insert("rules.base".to_string(), "1.0.0".to_string());
            installed
        });
        assert!(report
            .issues
            .iter()
            .any(|i| matches!(i, CompatibilityIssue::UnsupportedPlatform(_))));
    }

    #[test]
    fn 没有读取器的kind不兼容() {
        let mut m = manifest("v1");
        m.kind = ResourceKind::Model;
        let report = check(&m, &HostSupport::current(), &BTreeMap::new());
        assert!(report
            .issues
            .iter()
            .any(|i| matches!(i, CompatibilityIssue::KindNotReadable(_))));
    }
}
