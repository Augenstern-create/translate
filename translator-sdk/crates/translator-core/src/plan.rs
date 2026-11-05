//! `ResolvedExecutionPlan`：**决定译文的全部输入**的显式记录（ADR-023，P3.1-D）。
//!
//! 缓存键不能用 `package_id@version`（ADR-026）：同一份权重配不同解码参数、
//! 不同保护策略、不同规则版本，产出的译文就不是同一个东西。这里把"影响译文的一切"
//! 摊平成一份列表，再用稳定编码求 digest。
//!
//! 三条纪律：
//! 1. **只记录已解析的值**，不记录"用户当时想用什么"——例如 `beam_size` 是实际生效的值；
//! 2. **稳定编码**：字段排序固定、不依赖 `HashMap` 迭代顺序，跨进程/跨机器可复现；
//! 3. **缺失也要显式**：不知道的字段写 `unknown` 并进 digest，而不是省略——
//!    省略会让"旧版本产生的 digest"与"新版本"撞在一起。

use std::collections::BTreeMap;
use std::fmt;

use sha2::{Digest, Sha256};

/// 稳定编码的格式版本。改字段集合/编码方式必须改它，否则旧 digest 会碰撞。
pub const PLAN_SCHEMA: &str = "plan-v1";

/// 一次真实 NMT 执行的全部输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedExecutionPlan {
    pub engine_id: String,
    /// 运行时种类：`bergamot-cli` / `mock` / …
    pub runtime: String,
    /// 运行时构建标识（Bergamot commit）。未知写 `unknown`。
    pub runtime_build: String,
    /// 模型内容摘要（仅诊断用，ADR-026）。
    pub model_revision: String,
    /// 五个文件的逐项摘要（role → sha256）。空表示"没有真实文件"（mock）。
    pub file_digests: BTreeMap<String, String>,
    pub source: String,
    pub target: String,
    pub beam_size: u8,
    pub cpu_threads: u16,
    /// 保护策略版本（`protect-seg-v1` / `protect-mark-v1` / `off`）。
    pub protection: String,
    pub rules_version: Option<String>,
    pub glossary_digest: Option<String>,
    /// 文本流水线版本（规范化/分句规则的版本）。
    pub pipeline_version: u32,
    /// 隐私域：`local` 表示不出本机。
    pub privacy_domain: String,
}

impl ResolvedExecutionPlan {
    /// 规范化文本：`schema` + 排序后的 `key=value`，以 `\n` 分隔。
    ///
    /// 用 BTreeMap 承载所有键，确保顺序稳定；值里的 `\n` 会被转义，
    /// 避免"字段值里带换行"伪造出额外字段（这会让 digest 可被诱导碰撞）。
    pub fn canonical(&self) -> String {
        let mut fields: BTreeMap<String, String> = BTreeMap::new();
        fields.insert("schema".into(), PLAN_SCHEMA.into());
        fields.insert("engine_id".into(), self.engine_id.clone());
        fields.insert("runtime".into(), self.runtime.clone());
        fields.insert("runtime_build".into(), self.runtime_build.clone());
        fields.insert("model_revision".into(), self.model_revision.clone());
        for (role, digest) in &self.file_digests {
            fields.insert(format!("file.{role}"), digest.clone());
        }
        fields.insert("lang.source".into(), self.source.clone());
        fields.insert("lang.target".into(), self.target.clone());
        fields.insert("decode.beam_size".into(), self.beam_size.to_string());
        fields.insert("decode.cpu_threads".into(), self.cpu_threads.to_string());
        fields.insert("protection".into(), self.protection.clone());
        fields.insert(
            "rules_version".into(),
            self.rules_version.clone().unwrap_or_else(|| "none".into()),
        );
        fields.insert(
            "glossary_digest".into(),
            self.glossary_digest
                .clone()
                .unwrap_or_else(|| "none".into()),
        );
        fields.insert("pipeline_version".into(), self.pipeline_version.to_string());
        fields.insert("privacy_domain".into(), self.privacy_domain.clone());

        let mut out = String::new();
        for (key, value) in fields {
            out.push_str(&escape(&key));
            out.push('=');
            out.push_str(&escape(&value));
            out.push('\n');
        }
        out
    }

    /// 稳定 digest（小写十六进制 sha256）。
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.canonical().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// 前 16 个十六进制字符：给日志/诊断用的短标识。
    pub fn short_digest(&self) -> String {
        self.digest().chars().take(16).collect()
    }
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '=' => out.push_str("\\e"),
            _ => out.push(ch),
        }
    }
    out
}

impl fmt::Display for ResolvedExecutionPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} / {}@{} / {}→{} / beam={} threads={} / {} / plan={}",
            self.engine_id,
            self.runtime,
            self.runtime_build,
            self.source,
            self.target,
            self.beam_size,
            self.cpu_threads,
            self.protection,
            self.short_digest()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ResolvedExecutionPlan {
        let mut files = BTreeMap::new();
        files.insert("model".to_string(), "aa".repeat(32));
        files.insert("src_vocab".to_string(), "bb".repeat(32));
        ResolvedExecutionPlan {
            engine_id: "nmt.bergamot".into(),
            runtime: "bergamot-cli".into(),
            runtime_build: "9271618".into(),
            model_revision: "nmt.enzh.mozilla@3.0#4e5accc1".into(),
            file_digests: files,
            source: "en".into(),
            target: "zh-Hans".into(),
            beam_size: 1,
            cpu_threads: 1,
            protection: "protect-seg-v1".into(),
            rules_version: None,
            glossary_digest: None,
            pipeline_version: 2,
            privacy_domain: "local".into(),
        }
    }

    #[test]
    fn 同一输入给出同一digest且顺序稳定() {
        let a = base();
        let b = base();
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.canonical(), b.canonical());
        assert_eq!(a.digest().len(), 64);
    }

    #[test]
    fn 任何影响译文的字段变化都会改digest() {
        let base_digest = base().digest();
        let mut variants = Vec::new();

        let mut p = base();
        p.beam_size = 4;
        variants.push(("beam", p));
        let mut p = base();
        p.protection = "protect-mark-v1".into();
        variants.push(("protection", p));
        let mut p = base();
        p.rules_version = Some("rules.zh-en@2".into());
        variants.push(("rules", p));
        let mut p = base();
        p.file_digests.insert("model".into(), "cc".repeat(32));
        variants.push(("file_digest", p));
        let mut p = base();
        p.target = "zh-Hant".into();
        variants.push(("target", p));
        let mut p = base();
        p.pipeline_version = 3;
        variants.push(("pipeline", p));
        let mut p = base();
        p.privacy_domain = "remote-allowed".into();
        variants.push(("privacy", p));

        for (label, plan) in variants {
            assert_ne!(plan.digest(), base_digest, "{label} 改变必须改 digest");
        }
    }

    /// 金值锁定：digest 是**对外可见的缓存身份**，任何无意的编码变化都会让所有缓存失效，
    /// 因此这里钉死一个具体值。要改它，必须同时改 `PLAN_SCHEMA` 版本并在报告里说明。
    #[test]
    fn digest金值锁定() {
        assert_eq!(
            base().digest(),
            "67d2768e997f63a71aa81d5b2364ee5ae04deaa836bc87b616e182c9ad762f60"
        );
        assert_eq!(base().short_digest(), "67d2768e997f63a7");
        // 键按字典序排列，`schema` 不是第一个：编码是"排版稳定"，不是"语义顺序"
        let canonical = base().canonical();
        assert!(canonical.starts_with("decode.beam_size=1\n"));
        assert!(canonical.contains(&format!("schema={PLAN_SCHEMA}\n")));
        // 14 个固定字段 + 每个模型文件一行（这里 2 个文件）
        assert_eq!(
            canonical.lines().count(),
            14 + base().file_digests.len(),
            "字段数固定：新增字段必须同时改 PLAN_SCHEMA"
        );
    }

    #[test]
    fn 字段值里的换行不能伪造额外字段() {
        let mut a = base();
        a.rules_version = Some("x\ny=z".into());
        let b = {
            let mut p = base();
            p.rules_version = Some("x".into());
            p.glossary_digest = Some("y=z".into());
            p
        };
        assert_ne!(a.digest(), b.digest(), "转义必须阻止字段注入");
        assert!(!a.canonical().contains("\ny=z"), "{}", a.canonical());
    }

    #[test]
    fn 缺失的可选字段也进digest() {
        let mut a = base();
        a.rules_version = None;
        let mut b = base();
        b.rules_version = Some("none".into());
        // "none" 是保留值：显式写 "none" 与缺失视为同一语义
        assert_eq!(a.digest(), b.digest());
        // 但与真的规则版本不同
        let mut c = base();
        c.rules_version = Some("rules.v1".into());
        assert_ne!(a.digest(), c.digest());
    }

    #[test]
    fn 展示串里带短digest() {
        let plan = base();
        let text = plan.to_string();
        assert!(text.contains("nmt.bergamot"), "{text}");
        assert!(text.contains(&plan.short_digest()), "{text}");
        assert_eq!(plan.short_digest().len(), 16);
    }
}
