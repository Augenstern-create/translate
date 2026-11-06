//! Bergamot 配置生成：**白名单、类型化**，不接受任何用户提供的 YAML/CLI 片段。
//!
//! `docs/14` §3.4 的 P0（B-P0-05）：Marian 的配置能打开大量行为（路径、工作区、批处理、
//! 甚至外部文件），把用户字符串拼进 YAML 等于把 native 的配置面直接暴露给调用方。
//! 所以这里只做一件事：从已经校验过的模型集合 + 少数枚举化参数，**生成**一份配置。
//!
//! 生成的值刻意与 P3.0 参考链路一致（`fixtures/nmt/config.enzh.bergamot.yml.example`）：
//! 单句、贪心、mini-batch 1、workspace 128、`--cpu-threads 1`。任何偏离都要先有实测。

use std::path::Path;

use crate::error::RuntimeError;
use crate::model::{FileRole, ModelSpec};

/// 允许调用方调整的那几个参数：**全部是数值或语言标签**，没有自由文本路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BergamotOptions {
    pub cpu_threads: u16,
    pub beam_size: u8,
    /// `max-length`（Marian 的句子长度上限，按子词算）。P3.0 用 100。
    pub max_input_chars: u32,
    pub source_language: String,
    pub target_language: String,
}

impl Default for BergamotOptions {
    fn default() -> Self {
        Self {
            cpu_threads: 1,
            beam_size: 1,
            max_input_chars: 100,
            source_language: "en".to_string(),
            target_language: "zh-Hans".to_string(),
        }
    }
}

impl BergamotOptions {
    /// 边界检查：范围之外直接拒绝，而不是"尽力而为"。
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.cpu_threads == 0 || self.cpu_threads > 64 {
            return Err(RuntimeError::InitFailed(format!(
                "cpu_threads 必须在 1..=64，收到 {}",
                self.cpu_threads
            )));
        }
        if self.beam_size == 0 || self.beam_size > 16 {
            return Err(RuntimeError::InitFailed(format!(
                "beam_size 必须在 1..=16，收到 {}",
                self.beam_size
            )));
        }
        if self.max_input_chars < 8 || self.max_input_chars > 1024 {
            return Err(RuntimeError::InitFailed(format!(
                "max_input_chars 必须在 8..=1024，收到 {}",
                self.max_input_chars
            )));
        }
        for (label, value) in [
            ("source_language", &self.source_language),
            ("target_language", &self.target_language),
        ] {
            if value.is_empty()
                || value.len() > 32
                || !value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(RuntimeError::InitFailed(format!(
                    "{label} 只允许 ASCII 字母数字与 `-`/`_`：{value:?}"
                )));
            }
        }
        Ok(())
    }
}

/// YAML 双引号标量的安全渲染。
///
/// 路径来自已校验的快照目录，理论上安全；但"理论上"不是理由：
/// 控制字符、引号、反斜杠都会破坏 YAML，必须显式拒绝或转义。
fn yaml_path(path: &str, role: FileRole) -> Result<String, RuntimeError> {
    if !Path::new(path).is_absolute() {
        return Err(RuntimeError::InitFailed(format!(
            "{role} 路径必须是绝对路径：{path}"
        )));
    }
    if path.chars().any(|c| c.is_control()) {
        return Err(RuntimeError::InitFailed(format!(
            "{role} 路径含控制字符，拒绝写入配置"
        )));
    }
    // 双引号里只需要转义 \ 和 "
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"{escaped}\""))
}

/// 生成 Bergamot 的 `--model-config-paths` 所需的 YAML。
///
/// 注意 `shortlist` 是**独立键**而不是 `models` 的第二项：P3.0 实测把 lex 写进 `models`
/// 会让 Marian 把它当第二个模型解析二进制头，直接 `abort()`（`docs/11` §2.5）。
pub fn render_bergamot_config(
    spec: &ModelSpec,
    options: &BergamotOptions,
) -> Result<String, RuntimeError> {
    options.validate()?;

    let model = yaml_path(&spec.model_path, FileRole::Model)?;
    let src = yaml_path(&spec.src_vocab, FileRole::SourceVocab)?;
    let trg = yaml_path(&spec.trg_vocab, FileRole::TargetVocab)?;

    let mut out = String::new();
    out.push_str("# 由 translator-runtime::config 生成（白名单字段，非用户 YAML）\n");
    out.push_str(&format!("# model_revision: {}\n", spec.model_revision));
    out.push_str(&format!(
        "# languages: {} -> {}\n\n",
        options.source_language, options.target_language
    ));
    out.push_str("models:\n");
    out.push_str(&format!("  - {model}\n\n"));

    if let Some(lex) = &spec.lex_shortlist {
        let lex = yaml_path(lex, FileRole::LexicalShortlist)?;
        out.push_str("shortlist:\n");
        out.push_str(&format!("  - {lex}\n"));
        // 50 50 false true 与 Firefox 训练时一致（P3.0 记录）
        out.push_str("  - 50\n  - 50\n  - false\n  - true\n\n");
    }

    out.push_str("vocabs:\n");
    out.push_str(&format!("  - {src}\n  - {trg}\n\n"));

    out.push_str(&format!("beam-size: {}\n", options.beam_size));
    out.push_str("normalize: 1.0\n");
    out.push_str("word-penalty: 0\n");
    out.push_str(&format!("max-length: {}\n\n", options.max_input_chars));

    out.push_str("mini-batch: 1\n");
    out.push_str("maxi-batch: 1\n");
    out.push_str("maxi-batch-sort: none\n");
    out.push_str("workspace: 128\n\n");

    out.push_str("ssplit-mode: sentence\n");
    if let Some(prefix) = &spec.split_prefix {
        let prefix = yaml_path(prefix, FileRole::SplitPrefix)?;
        out.push_str(&format!("ssplit-prefix-file: {prefix}\n"));
    }
    out.push_str("alignment: soft\n");
    out.push_str("max-length-break: 128\n");
    out.push_str("max-length-factor: 2.0\n");
    out.push_str("mini-batch-words: 1024\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn spec(dir: &str) -> ModelSpec {
        ModelSpec {
            package_id: "nmt.test.enzh".into(),
            version: "3.0".into(),
            model_path: format!("{dir}/model.bin"),
            src_vocab: format!("{dir}/srcvocab.spm"),
            trg_vocab: format!("{dir}/trgvocab.spm"),
            lex_shortlist: Some(format!("{dir}/lex.bin")),
            split_prefix: None,
            threads: 1,
            beam_size: 1,
            model_revision: "nmt.test.enzh@3.0#deadbeef".into(),
            digests: BTreeMap::new(),
        }
    }

    // 下面这几条的夹具用 POSIX 绝对路径（`/snap/abc`）。Windows 上它不是绝对路径，配置渲染会先
    // 因"必须是绝对路径"失败；而 Bergamot 配置渲染在 Windows 上本来就不可达（`ensure_supported()`
    // 直接返回 CapabilityUnavailable，worker 的终止升级与 /proc 采样只在 Linux 验证过）。
    // 因此按平台语义把它们限定在 unix，Windows 侧记为"未验证"，而不是让它红着或悄悄跳过。
    #[cfg(unix)]
    #[test]
    fn 生成的配置把shortlist放在独立键而不是models段() {
        let yaml = render_bergamot_config(&spec("/snap/abc"), &BergamotOptions::default()).unwrap();
        assert!(
            yaml.contains("models:\n  - \"/snap/abc/model.bin\"\n\n"),
            "{yaml}"
        );
        assert!(
            yaml.contains(
                "shortlist:\n  - \"/snap/abc/lex.bin\"\n  - 50\n  - 50\n  - false\n  - true"
            ),
            "{yaml}"
        );
        // models 段里只能有一个条目
        let models_block = yaml.split("shortlist:").next().unwrap();
        assert_eq!(models_block.matches("  - \"").count(), 1, "{yaml}");
    }

    #[cfg(unix)]
    #[test]
    fn 解码参数来自类型化选项而不是字符串拼接() {
        let options = BergamotOptions {
            cpu_threads: 2,
            beam_size: 4,
            max_input_chars: 128,
            ..BergamotOptions::default()
        };
        let yaml = render_bergamot_config(&spec("/snap/abc"), &options).unwrap();
        assert!(yaml.contains("beam-size: 4"), "{yaml}");
        assert!(yaml.contains("max-length: 128"), "{yaml}");
        assert!(yaml.contains("mini-batch: 1"), "批处理必须固定为 1：{yaml}");
    }

    #[cfg(unix)]
    #[test]
    fn 没有短名单时就不生成shortlist键() {
        let mut s = spec("/snap/abc");
        s.lex_shortlist = None;
        let yaml = render_bergamot_config(&s, &BergamotOptions::default()).unwrap();
        assert!(!yaml.contains("shortlist:"), "{yaml}");
        assert!(yaml.contains("vocabs:"), "{yaml}");
    }

    #[cfg(unix)]
    #[test]
    fn ssplit前缀文件存在时才写键() {
        let mut s = spec("/snap/abc");
        s.split_prefix = Some("/snap/abc/nonbreaking_prefix.en".into());
        let yaml = render_bergamot_config(&s, &BergamotOptions::default()).unwrap();
        assert!(
            yaml.contains("ssplit-prefix-file: \"/snap/abc/nonbreaking_prefix.en\""),
            "{yaml}"
        );
    }

    #[test]
    fn 越界参数被拒绝() {
        for options in [
            BergamotOptions {
                cpu_threads: 0,
                ..Default::default()
            },
            BergamotOptions {
                beam_size: 0,
                ..Default::default()
            },
            BergamotOptions {
                max_input_chars: 4,
                ..Default::default()
            },
            BergamotOptions {
                target_language: "zh Hans".into(),
                ..Default::default()
            },
            BergamotOptions {
                source_language: "".into(),
                ..Default::default()
            },
        ] {
            let err = render_bergamot_config(&spec("/snap/abc"), &options).unwrap_err();
            assert_eq!(
                err.code(),
                translator_types::TranslationErrorCode::RuntimeInitFailed,
                "{err}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn 相对路径与含引号的路径被拒绝或被正确转义() {
        let mut s = spec("/snap/abc");
        s.model_path = "relative/model.bin".into();
        let err = render_bergamot_config(&s, &BergamotOptions::default()).unwrap_err();
        assert!(err.to_string().contains("绝对路径"), "{err}");

        let mut s = spec("/snap/abc");
        s.model_path = "/snap/abc/mo\"del.bin".into();
        let yaml = render_bergamot_config(&s, &BergamotOptions::default()).unwrap();
        assert!(yaml.contains("mo\\\"del.bin"), "引号必须转义：{yaml}");
    }
}
