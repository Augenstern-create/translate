//! 规则包离线迁移工具。
//!
//! 为什么是独立工具（而不是 SDK 里自动迁移）：
//!
//! - 旧语义（`pipeline_version: 1`）允许规则改写 URL/邮箱/代码，属于**不安全行为**，
//!   自动迁移等于把风险悄悄打开；
//! - 迁移必须在当前语义下**重跑金样本**，可能失败，需要人看输出再决定；
//! - 资源管理器要能"识别旧包但不执行"，这条工具就是它的配套动作。
//!
//! ```text
//! migrate-rules --inspect old.json              # 只看元数据，不执行规则
//! migrate-rules old.json --out new.json         # 迁移（金样本不过则失败，不写文件）
//! migrate-rules current.json --check            # 校验能否被执行
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use translator_rules::migrate::{inspect, migrate_json};

const USAGE: &str = "\
migrate-rules —— 规则包离线迁移 / 校验

用法:
  migrate-rules <规则文件> [--inspect | --check | --out <输出文件>]

选项:
  --inspect        只打印元数据（不执行规则，旧包也能看）
  --check          校验能否被当前 SDK 执行（不能则退出码 1）
  --out <文件>      写入迁移结果（默认: <输入>.migrated.json）
  --force          允许覆盖已存在的输出文件

退出码:
  0  成功 / 可执行
  1  迁移失败、金样本未通过或不可执行
  2  参数错误
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return if args.is_empty() {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        };
    }

    let input = PathBuf::from(&args[0]);
    let inspect_only = args.iter().any(|a| a == "--inspect");
    let check_only = args.iter().any(|a| a == "--check");
    let force = args.iter().any(|a| a == "--force");
    let out: Option<PathBuf> = args
        .iter()
        .position(|a| a == "--out")
        .map(|pos| PathBuf::from(args.get(pos + 1).cloned().unwrap_or_default()));

    if inspect_only || check_only {
        return match inspect(&input) {
            Ok(meta) => {
                println!("id                 {}", meta.id);
                println!("version            {}", meta.version);
                println!("pipeline_version   {}", meta.pipeline_version);
                println!("rules              {}", meta.rules);
                println!(
                    "license            {}",
                    meta.license.as_deref().unwrap_or("（未声明）")
                );
                println!(
                    "形态               {}",
                    if meta.binary { "二进制" } else { "JSON" }
                );
                println!(
                    "可执行             {}",
                    if meta.executable { "是" } else { "否" }
                );
                if let Some(reason) = meta.blocked_reason() {
                    println!("不可执行原因       {reason}");
                }
                if check_only && !meta.executable {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(err) => {
                eprintln!("读取失败：{err}");
                ExitCode::FAILURE
            }
        };
    }

    let text = match std::fs::read_to_string(&input) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("无法读取 {}：{err}", input.display());
            return ExitCode::FAILURE;
        }
    };

    let (migrated, report) = match migrate_json(&text) {
        Ok(result) => result,
        Err(err) => {
            // 关键：金样本不过就什么都不写，绝不产出半成品
            eprintln!("迁移失败：{err}");
            eprintln!("输出文件未写入（金样本必须在新语义下全部通过）");
            return ExitCode::FAILURE;
        }
    };

    let out_path = out.unwrap_or_else(|| {
        let mut p = input.clone();
        let name = format!(
            "{}.migrated.json",
            input
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "rules.json".to_string())
        );
        p.set_file_name(name);
        p
    });

    if out_path.exists() && !force {
        eprintln!(
            "输出文件已存在：{}（加 --force 覆盖，或换 --out）",
            out_path.display()
        );
        return ExitCode::from(2);
    }

    if let Err(err) = std::fs::write(&out_path, migrated) {
        eprintln!("写入 {} 失败：{err}", out_path.display());
        return ExitCode::FAILURE;
    }

    println!("迁移完成：{}@{}", report.id, report.version);
    println!(
        "pipeline_version   {} → {}",
        report.from_pipeline_version, report.to_pipeline_version
    );
    println!("规则数             {}", report.rules);
    println!("金样本             {} 条全部通过", report.samples_checked);
    println!(
        "内容有改动         {}",
        if report.changed {
            "是"
        } else {
            "否（已是当前语义）"
        }
    );
    println!("输出               {}", out_path.display());
    ExitCode::SUCCESS
}
