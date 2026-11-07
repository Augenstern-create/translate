//! 可选 fastText LID（`lid.176.ftz`）实现。
//!
//! 依赖 `fasttext-pure-rs`（MIT，纯 Rust，无 native 构建脚本）；模型由宿主显式提供，
//! 不自动下载、不写缓存。模型标签是 ISO 639-3（`__label__eng` 等），这里映射到
//! SDK 的 BCP-47 风格标签；未知标签保留为三字母代码，不伪造。
#![cfg(feature = "fasttext")]

use std::path::Path;

use fasttext_pure_rs::FastText;
use translator_types::{LanguageGuess, LanguageTag};

/// fastText 官方 `lid.176.ftz` 的识别器。
pub struct FastTextIdentifier {
    model: FastText,
}

impl std::fmt::Debug for FastTextIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastTextIdentifier").finish_non_exhaustive()
    }
}

impl FastTextIdentifier {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        let model = FastText::load_from_reader(std::io::Cursor::new(bytes))
            .map_err(|e| format!("fastText 模型加载失败：{e}"))?;
        Ok(Self { model })
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let bytes = std::fs::read(path.as_ref())
            .map_err(|e| format!("读取 {} 失败：{e}", path.as_ref().display()))?;
        Self::from_bytes(bytes)
    }

    /// 卸载模型（drop 后由 allocator 决定 RSS 回收；本方法只表达生命周期终点）。
    pub fn unload(self) {}
}

impl super::LanguageIdentifier for FastTextIdentifier {
    fn identify(&self, text: &str, limit: usize) -> Vec<LanguageGuess> {
        if limit == 0 || text.trim().is_empty() {
            return Vec::new();
        }
        let Ok(preds) = self.model.predict(text, limit.max(1), 0.0) else {
            return Vec::new();
        };
        preds
            .into_iter()
            .filter_map(|p| {
                let raw = p.label.strip_prefix("__label__").unwrap_or(&p.label);
                let tag = map_iso3(raw)?;
                LanguageTag::new(tag).ok().map(|language| LanguageGuess {
                    language,
                    confidence: p.probability,
                })
            })
            .take(limit)
            .collect()
    }

    fn name(&self) -> &'static str {
        "fasttext-lid176-ftz"
    }
}

/// fastText LID 使用 ISO 639-3；映射到项目现在使用的短标签。
fn map_iso3(raw: &str) -> Option<&str> {
    Some(match raw {
        "en" | "eng" => "en",
        "zh" | "cmn" | "zho" | "wuu" | "yue" | "nan" | "hak" | "lzh" => "zh-Hans",
        "ja" | "jpn" => "ja",
        "ko" | "kor" => "ko",
        "fra" | "fre" => "fr",
        "deu" | "ger" => "de",
        "spa" => "es",
        "rus" => "ru",
        "ukr" => "uk",
        "ara" => "ar",
        "fas" | "per" => "fa",
        "por" => "pt",
        "ita" => "it",
        "nld" | "dut" => "nl",
        "tur" => "tr",
        "vie" => "vi",
        "tha" => "th",
        "ind" => "id",
        "msa" | "may" => "ms",
        other if other.len() == 2 || other.len() == 3 => other,
        _ => return None,
    })
}
