//! `pack-dictionary` 命令行入口。
//!
//! ```bash
//! pack-dictionary cedict_1_0_ts_utf-8_mdbg.txt out/zh-en \
//!     --id dictionary.cc-cedict.zh-en \
//!     --version 0.1.0 \
//!     --language zh-Hans \
//!     --target en
//! ```

use std::process::ExitCode;

use pack_dictionary::{pack, parse_cedict, parse_jsonl, PackOptions};
use translator_dictionary::{PayloadKind, Provenance};

const USAGE: &str = "\
pack-dictionary —— 把 CC-CEDICT 或规范化 JSONL 词表编译为词典包

用法:
  pack-dictionary <输入词表> <输出目录> [选项]

选项:
  --format <F>          输入格式：cedict | jsonl（默认 cedict）
  --id <ID>             包 id（默认 dictionary.cc-cedict.zh-en）
  --version <VER>       包版本（默认 0.1.0）
  --language <LANG>     词条语言（默认 zh-Hans）
  --target <LANG>       释义语言，可重复（默认 en）
  --limit <N>           只取前 N 条，用于生成子集
  --name <NAME>         来源名称（默认 CC-CEDICT）
  --url <URL>           来源地址
  --license <SPDX>      许可证（默认 CC-BY-SA-4.0）
  --license-file <PATH> LICENSE.txt 的原文（JSONL 输入建议显式提供）
  --revision <REV>      数据 revision（默认取源文件 #! version=，否则用当天日期）
  --payload-schema <S>  释义库格式：dictionary.sqlite.v1 | v2 | v3（默认 v2；v3 支持多值索引）
  -h, --help            显示本帮助

输出:
  <输出目录>/manifest.json   index.fst   entries.sqlite   LICENSE.txt
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let mut positional: Vec<String> = Vec::new();
    let mut id = "dictionary.cc-cedict.zh-en".to_string();
    let mut version = "0.1.0".to_string();
    let mut language = "zh-Hans".to_string();
    let mut targets: Vec<String> = Vec::new();
    let mut limit: Option<usize> = None;
    let mut name = "CC-CEDICT".to_string();
    let mut url = "https://www.mdbg.net/chinese/dictionary?page=cc-cedict".to_string();
    let mut license_spdx = "CC-BY-SA-4.0".to_string();
    let mut revision: Option<String> = None;
    let mut payload = PayloadKind::V2;
    let mut format = "cedict".to_string();
    let mut license_file: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => {
                i += 1;
                id = args.get(i).cloned().unwrap_or_default();
            }
            "--version" => {
                i += 1;
                version = args.get(i).cloned().unwrap_or_default();
            }
            "--language" => {
                i += 1;
                language = args.get(i).cloned().unwrap_or_default();
            }
            "--target" => {
                i += 1;
                if let Some(t) = args.get(i) {
                    targets.push(t.clone());
                }
            }
            "--format" => {
                i += 1;
                format = args.get(i).cloned().unwrap_or_default();
                if format != "cedict" && format != "jsonl" {
                    eprintln!("--format 只能是 cedict | jsonl：{format}");
                    return ExitCode::from(2);
                }
            }
            "--license-file" => {
                i += 1;
                license_file = args.get(i).cloned();
            }
            "--payload-schema" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("dictionary.sqlite.v1") => payload = PayloadKind::V1,
                    Some("dictionary.sqlite.v2") => payload = PayloadKind::V2,
                    Some("dictionary.sqlite.v3") => payload = PayloadKind::V3,
                    other => {
                        eprintln!(
                            "--payload-schema 只能是 dictionary.sqlite.v1 | v2 | v3：{}",
                            other.unwrap_or("（缺失）")
                        );
                        return ExitCode::from(2);
                    }
                }
            }
            "--limit" => {
                i += 1;
                limit = args.get(i).and_then(|v| v.parse().ok());
            }
            "--name" => {
                i += 1;
                name = args.get(i).cloned().unwrap_or_default();
            }
            "--url" => {
                i += 1;
                url = args.get(i).cloned().unwrap_or_default();
            }
            "--license" => {
                i += 1;
                license_spdx = args.get(i).cloned().unwrap_or_default();
            }
            "--revision" => {
                i += 1;
                revision = args.get(i).cloned();
            }
            flag if flag.starts_with("--") => {
                eprintln!("未知选项：{flag}\n");
                print!("{USAGE}");
                return ExitCode::from(2);
            }
            positional_arg => positional.push(positional_arg.to_string()),
        }
        i += 1;
    }

    if positional.len() != 2 {
        eprintln!("需要 2 个位置参数：<输入词表> <输出目录>\n");
        print!("{USAGE}");
        return ExitCode::from(2);
    }

    if targets.is_empty() {
        targets.push("en".to_string());
    }

    let input_path = &positional[0];
    let out_dir = &positional[1];

    let file = match std::fs::File::open(input_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("无法打开 {input_path}：{e}");
            return ExitCode::FAILURE;
        }
    };

    println!("解析 {input_path}（format={format}）...");
    let (entries, header, skipped, source_version) = if format == "jsonl" {
        let entries = match parse_jsonl(std::io::BufReader::new(file), limit) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("解析失败：{e}");
                return ExitCode::FAILURE;
            }
        };
        (entries, String::new(), 0, None)
    } else {
        let parsed = match parse_cedict(std::io::BufReader::new(file), limit) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("解析失败：{e}");
                return ExitCode::FAILURE;
            }
        };
        (
            parsed.entries,
            parsed.header,
            parsed.skipped_lines,
            parsed.source_version,
        )
    };

    println!("  词条 {} 条，跳过畸形行 {} 条", entries.len(), skipped);
    let license_text = match license_file {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("无法读取 --license-file {path}：{e}");
                return ExitCode::FAILURE;
            }
        },
        None => {
            if header.is_empty() {
                eprintln!(
                    "警告：没有 --license-file，且输入没有头部注释块；LICENSE.txt 只是占位说明"
                );
                format!("# 许可证见 {}\n", positional[0])
            } else {
                header.clone()
            }
        }
    };

    // 注意：CC-CEDICT 的 `#! version=1` 是**文件格式版本**，不是数据版本。
    // 把它当 revision 会误导读者，因此默认用抓取日期作为 revision。
    if let Some(v) = &source_version {
        println!("  源文件格式版本：{v}");
    }
    let revision = revision.unwrap_or_else(|| format!("snapshot-{}", today()));

    let opts = PackOptions {
        id,
        version,
        payload,
        language,
        target_languages: targets,
        provenance: Provenance {
            name,
            source_url: url,
            revision,
            license_spdx,
            license_file: "LICENSE.txt".to_string(),
            retrieved_at: today(),
        },
        license_text,
    };

    println!("打包到 {out_dir} ...");
    match pack(entries, out_dir, &opts) {
        Ok(stats) => {
            println!("  payload {} ", payload.as_str());
            println!("  记录 {} 条 / 索引键 {} 个", stats.records, stats.keys);
            println!(
                "  index.fst {:.1} KB / entries.sqlite {:.1} MB",
                stats.index_bytes as f64 / 1024.0,
                stats.db_bytes as f64 / 1_048_576.0
            );
            println!("完成。");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("打包失败：{e}");
            ExitCode::FAILURE
        }
    }
}

fn today() -> String {
    // 不引入 chrono，用简单算法从 UNIX 时间换算 UTC 日期
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;

    // Howard Hinnant 的 civil_from_days 算法
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}")
}
