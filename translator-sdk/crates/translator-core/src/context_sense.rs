//! P16-R / R-03：带上下文的高置信度词义选择。
//!
//! 只覆盖少量**明确搭配**；没有命中线索时保留多义候选，不猜。
//! 这不是通用词义消歧器，也不调用模型。

use translator_types::{DictionaryEntry, LanguageTag};

/// 一条上下文线索：源文出现 `cue` 时，优先选择包含 `expected` 的义项。
struct Cue {
    word: &'static str,
    cue: &'static str,
    expected: &'static str,
}

const CUES: &[Cue] = &[
    Cue {
        word: "take off",
        cue: "plane",
        expected: "起飞",
    },
    Cue {
        word: "take off",
        cue: "flight",
        expected: "起飞",
    },
    Cue {
        word: "take off",
        cue: "airport",
        expected: "起飞",
    },
    Cue {
        word: "take off",
        cue: "shoes",
        expected: "脱下",
    },
    Cue {
        word: "take off",
        cue: "coat",
        expected: "脱下",
    },
    Cue {
        word: "bank",
        cue: "river",
        expected: "岸",
    },
    Cue {
        word: "bank",
        cue: "water",
        expected: "岸",
    },
    Cue {
        word: "bank",
        cue: "money",
        expected: "银行",
    },
    Cue {
        word: "bank",
        cue: "account",
        expected: "银行",
    },
    Cue {
        word: "bank",
        cue: "loan",
        expected: "银行",
    },
    Cue {
        word: "spring",
        cue: "mattress",
        expected: "弹簧",
    },
    Cue {
        word: "spring",
        cue: "metal",
        expected: "弹簧",
    },
    Cue {
        word: "spring",
        cue: "season",
        expected: "春天",
    },
    Cue {
        word: "spring",
        cue: "flower",
        expected: "春天",
    },
    Cue {
        word: "spring",
        cue: "hot",
        expected: "泉",
    },
    Cue {
        word: "charge",
        cue: "battery",
        expected: "充电",
    },
    Cue {
        word: "charge",
        cue: "phone",
        expected: "充电",
    },
    Cue {
        word: "charge",
        cue: "hotel",
        expected: "收费",
    },
    Cue {
        word: "charge",
        cue: "price",
        expected: "收费",
    },
    Cue {
        word: "charge",
        cue: "police",
        expected: "指控",
    },
    Cue {
        word: "charge",
        cue: "court",
        expected: "指控",
    },
    Cue {
        word: "light",
        cue: "room",
        expected: "光",
    },
    Cue {
        word: "light",
        cue: "lamp",
        expected: "灯",
    },
    Cue {
        word: "light",
        cue: "bag",
        expected: "轻",
    },
    Cue {
        word: "light",
        cue: "weight",
        expected: "轻",
    },
    Cue {
        word: "light",
        cue: "candle",
        expected: "点燃",
    },
    Cue {
        word: "book",
        cue: "read",
        expected: "书",
    },
    Cue {
        word: "book",
        cue: "library",
        expected: "书",
    },
    Cue {
        word: "book",
        cue: "room",
        expected: "预订",
    },
    Cue {
        word: "book",
        cue: "hotel",
        expected: "预订",
    },
    Cue {
        word: "right",
        cue: "turn",
        expected: "右",
    },
    Cue {
        word: "right",
        cue: "left",
        expected: "右",
    },
    Cue {
        word: "right",
        cue: "silent",
        expected: "权利",
    },
    Cue {
        word: "right",
        cue: "law",
        expected: "权利",
    },
    Cue {
        word: "right",
        cue: "answer",
        expected: "正确",
    },
    Cue {
        word: "present",
        cue: "gift",
        expected: "礼物",
    },
    Cue {
        word: "present",
        cue: "birthday",
        expected: "礼物",
    },
    Cue {
        word: "present",
        cue: "meeting",
        expected: "出席",
    },
    Cue {
        word: "present",
        cue: "attend",
        expected: "出席",
    },
    Cue {
        word: "close",
        cue: "door",
        expected: "关",
    },
    Cue {
        word: "close",
        cue: "window",
        expected: "关",
    },
    Cue {
        word: "close",
        cue: "friend",
        expected: "亲密",
    },
    Cue {
        word: "close",
        cue: "family",
        expected: "亲密",
    },
    Cue {
        word: "close",
        cue: "station",
        expected: "靠近",
    },
    Cue {
        word: "run",
        cue: "morning",
        expected: "跑",
    },
    Cue {
        word: "run",
        cue: "jog",
        expected: "跑",
    },
    Cue {
        word: "run",
        cue: "program",
        expected: "运行",
    },
    Cue {
        word: "run",
        cue: "software",
        expected: "运行",
    },
    Cue {
        word: "run",
        cue: "company",
        expected: "经营",
    },
    Cue {
        word: "run",
        cue: "business",
        expected: "经营",
    },
    Cue {
        word: "match",
        cue: "football",
        expected: "比赛",
    },
    Cue {
        word: "match",
        cue: "game",
        expected: "比赛",
    },
    Cue {
        word: "match",
        cue: "color",
        expected: "相配",
    },
    Cue {
        word: "match",
        cue: "design",
        expected: "相配",
    },
    Cue {
        word: "match",
        cue: "fire",
        expected: "火柴",
    },
];

/// 用上下文线索从多义项中优先选一个；没有可靠线索返回 `None`。
pub fn select_by_context(
    word: &str,
    context: &str,
    entries: &[DictionaryEntry],
    _target: &LanguageTag,
) -> Option<String> {
    let word = word.trim().to_lowercase();
    let context = context.to_lowercase();
    let mut best: Option<(&str, usize)> = None;
    for cue in CUES {
        if cue.word != word || !context.contains(cue.cue) {
            continue;
        }
        for entry in entries {
            for (idx, sense) in entry.senses.iter().enumerate() {
                if sense.definition.contains(cue.expected) {
                    let score = cue.expected.len();
                    if best.map(|(_, s)| score > s).unwrap_or(true) {
                        best = Some((sense.definition.as_str(), idx));
                    }
                }
            }
        }
    }
    best.map(|(text, _)| text.to_string())
}

/// 没有上下文时的安全展示：前 3 个不同义项，分号连接。
pub fn compose_senses(entries: &[DictionaryEntry], limit: usize) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for entry in entries {
        for sense in &entry.senses {
            let text = sense.definition.trim();
            if !text.is_empty() && !out.contains(&text) {
                out.push(text);
                if out.len() >= limit {
                    return Some(out.join("；"));
                }
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("；"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_types::Sense;

    fn entry(senses: &[&str]) -> DictionaryEntry {
        DictionaryEntry {
            headword: "bank".to_string(),
            normalized: "bank".to_string(),
            language: LanguageTag::new("en").unwrap(),
            readings: Vec::new(),
            parts_of_speech: vec!["noun".to_string()],
            senses: senses
                .iter()
                .map(|s| Sense {
                    definition: s.to_string(),
                    source_id: "test".to_string(),
                })
                .collect(),
            source_id: "test".to_string(),
            source_license: "MIT".to_string(),
            confidence: 1.0,
        }
    }

    #[test]
    fn 上下文线索选择义项() {
        let e = entry(&["银行, 堤, 岸", "[医] 库"]);
        let zh = LanguageTag::new("zh-Hans").unwrap();
        assert_eq!(
            select_by_context(
                "bank",
                "We sat on the river bank.",
                std::slice::from_ref(&e),
                &zh
            )
            .as_deref(),
            Some("银行, 堤, 岸")
        );
        // 没有线索不猜
        assert_eq!(select_by_context("bank", "It is fine.", &[e], &zh), None);
    }

    #[test]
    fn 无上下文时多义项拼接() {
        let e = entry(&["银行", "岸"]);
        assert_eq!(compose_senses(&[e], 3).as_deref(), Some("银行；岸"));
    }
}
