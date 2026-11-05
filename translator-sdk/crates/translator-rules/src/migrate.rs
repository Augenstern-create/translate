//! 规则包元数据检查与离线迁移。
//!
//! 设计约束（审查决定，见 `docs/05`）：
//!
//! - `pipeline_version: 1` 的规则包**不可执行**（旧语义允许规则改写 URL/邮箱/代码）；
//! - 但资源管理器仍需要**识别**它、**查看元数据**、并**迁移**它；
//! - 迁移必须在新的执行语义下**重新跑通金样本**，不允许"改个版本号就发布"。
//!
//! 因此这里提供两条与"加载并执行"分开的路径：
//!
//! | 函数 | 是否执行规则 | 用途 |
//! |---|---|---|
//! | [`inspect`] | 否 | 列出元数据，不加载到内存执行 |
//! | [`migrate_json`] | 是（金样本） | 产出可执行的新包，样本不过就失败 |

use serde_json::Value;

use crate::{RuleEngine, RuleError};

/// 规则包元数据（不执行规则即可获得）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleMetadata {
    pub id: String,
    pub version: u32,
    /// 省略时为当前语义版本。
    pub pipeline_version: u16,
    pub rules: usize,
    pub license: Option<String>,
    /// 能否被本 SDK 执行（`false` 表示只能查看/迁移）。
    pub executable: bool,
    /// 是否是二进制形态（magic `TRULE\0`）。
    pub binary: bool,
}

impl RuleMetadata {
    /// 为什么不可执行（给人看的理由）。
    pub fn blocked_reason(&self) -> Option<String> {
        if self.executable {
            return None;
        }
        Some(format!(
            "pipeline_version={} 已废弃（当前 {}）：旧语义允许规则改写 URL/邮箱/代码，\
             请用 migrate-rules 迁移并重跑金样本",
            self.pipeline_version,
            crate::CURRENT_PIPELINE_VERSION
        ))
    }
}

/// 读元数据：**不执行规则**，因此旧包也能被资源管理器识别。
pub fn inspect(path: impl AsRef<std::path::Path>) -> Result<RuleMetadata, RuleError> {
    let bytes = std::fs::read(path.as_ref())?;
    inspect_bytes(&bytes)
}

pub fn inspect_bytes(bytes: &[u8]) -> Result<RuleMetadata, RuleError> {
    let (json, binary) = if bytes.starts_with(b"TRULE\0") {
        let layout = crate::binary_payload(bytes)?;
        (layout, true)
    } else {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| RuleError::Corrupt(format!("规则文件不是 UTF-8：{e}")))?;
        (text.to_string(), false)
    };

    let value: Value = serde_json::from_str(&json)?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if id.is_empty() {
        return Err(RuleError::EmptyId);
    }
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    let pipeline_version = value
        .get("pipeline_version")
        .and_then(Value::as_u64)
        .map(|v| v as u16)
        .unwrap_or(crate::CURRENT_PIPELINE_VERSION);
    let rules = value
        .get("rules")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let license = value
        .get("license")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(RuleMetadata {
        id,
        version,
        pipeline_version,
        rules,
        license,
        executable: pipeline_version == crate::CURRENT_PIPELINE_VERSION,
        binary,
    })
}

/// 迁移报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReport {
    pub id: String,
    pub version: u32,
    pub from_pipeline_version: u16,
    pub to_pipeline_version: u16,
    pub rules: usize,
    /// 迁移时实际执行过的金样本数量。
    pub samples_checked: usize,
    /// 是否改动了内容（已经是当前版本时为 `false`）。
    pub changed: bool,
}

/// 把旧语义的规则包迁移成当前语义。
///
/// 只做一件事：把 `pipeline_version` 设为当前版本，然后**用新语义重新编译**——
/// 编译过程会跑全部金样本，任何一条不通过都会失败，不会产出半成品。
///
/// 语义变化的影响面：规则不再改写 URL/邮箱/代码里的内容。因此旧包若有依赖
/// "改写受保护内容"的样本，会在这里失败——这正是我们想要的失败。
pub fn migrate_json(json: &str) -> Result<(String, MigrationReport), RuleError> {
    let mut value: Value = serde_json::from_str(json)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| RuleError::Corrupt("规则包顶层必须是 JSON 对象".to_string()))?;

    let from = object
        .get("pipeline_version")
        .and_then(Value::as_u64)
        .map(|v| v as u16)
        .unwrap_or(crate::CURRENT_PIPELINE_VERSION);

    let changed = from != crate::CURRENT_PIPELINE_VERSION;
    object.insert(
        "pipeline_version".to_string(),
        Value::from(crate::CURRENT_PIPELINE_VERSION),
    );

    let migrated = serde_json::to_string_pretty(&value)?;

    // 关键一步：用新语义重新编译（等价于重跑金样本）
    let engine = RuleEngine::from_json(&migrated)?;
    let report = MigrationReport {
        id: engine.id().to_string(),
        version: engine.version(),
        from_pipeline_version: from,
        to_pipeline_version: crate::CURRENT_PIPELINE_VERSION,
        rules: engine.rule_count(),
        samples_checked: engine.sample_count(),
        changed,
    };

    Ok((migrated, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: &str = r#"{
        "schema_version": 1,
        "id": "rules.test.legacy",
        "version": 3,
        "pipeline_version": 1,
        "rules": [
            { "id": "term.robot", "language_pairs": [["zh-Hans", "en"]],
              "action": { "kind": "replace", "from": "机器人", "to": "robot" },
              "samples": [{ "input": "机器人", "expect": "robot" }] }
        ]
    }"#;

    #[test]
    fn 旧包能被识别元数据但不可执行() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.json");
        std::fs::write(&path, LEGACY).unwrap();

        let meta = inspect(&path).expect("旧包必须能被识别");
        assert_eq!(meta.id, "rules.test.legacy");
        assert_eq!(meta.version, 3);
        assert_eq!(meta.pipeline_version, 1);
        assert!(!meta.executable);
        assert!(!meta.binary);
        assert_eq!(meta.rules, 1);
        assert!(meta.blocked_reason().is_some());

        // 但不可执行
        assert!(matches!(
            RuleEngine::load(&path),
            Err(RuleError::UnsupportedPipelineVersion(1))
        ));
    }

    #[test]
    fn 迁移后重跑金样本并产出可执行包() {
        let (migrated, report) = migrate_json(LEGACY).expect("应能迁移");
        assert_eq!(report.from_pipeline_version, 1);
        assert_eq!(report.to_pipeline_version, crate::CURRENT_PIPELINE_VERSION);
        assert_eq!(report.rules, 1);
        assert_eq!(report.samples_checked, 1);
        assert!(report.changed);
        assert!(migrated.contains("\"pipeline_version\": 2"));

        // 迁移产物必须能被正常加载并执行
        let engine = RuleEngine::from_json(&migrated).expect("迁移后应可执行");
        let zh = translator_types::LanguageTag::new("zh-Hans").unwrap();
        let en = translator_types::LanguageTag::new("en").unwrap();
        assert_eq!(engine.apply("机器人", &zh, &en).text, "robot");
    }

    #[test]
    fn 金样本过不了就不产出迁移结果() {
        // 旧包靠金样本记录了"URL 也被改写"的行为；新语义下必须失败
        let legacy = r#"{
            "schema_version": 1,
            "id": "rules.test.url",
            "version": 1,
            "pipeline_version": 1,
            "rules": [
                { "id": "term.llm", "language_pairs": [["en", "zh-Hans"]],
                  "action": { "kind": "replace", "from": "LLM", "to": "large language model" },
                  "samples": [{ "input": "https://example.com/LLM", "expect": "https://example.com/large language model" }] }
            ]
        }"#;
        let err = migrate_json(legacy).unwrap_err();
        assert!(matches!(err, RuleError::SampleFailed { .. }), "实际：{err}");
    }

    #[test]
    fn 已是当前版本的包迁移是幂等的() {
        let current = LEGACY.replace("\"pipeline_version\": 1,", "");
        let (migrated, report) = migrate_json(&current).expect("应能迁移");
        assert!(!report.changed, "已经是当前语义时 changed 应为 false");
        assert!(RuleEngine::from_json(&migrated).is_ok());
    }

    #[test]
    fn 二进制形态也能读元数据() {
        let current = LEGACY.replace("\"pipeline_version\": 1,", "");
        let engine = RuleEngine::from_json(&current).expect("编译");
        let meta = inspect_bytes(&engine.to_bytes()).expect("二进制元数据");
        assert!(meta.binary);
        assert!(meta.executable);
        assert_eq!(meta.id, "rules.test.legacy");
    }
}
