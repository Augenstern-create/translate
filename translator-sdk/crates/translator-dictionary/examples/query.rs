//! 查询词典包的示例工具。
//!
//! ```bash
//! cargo run -p translator-dictionary --example query -- ../../resources/zh-en 猫 你好 世界
//! ```

use translator_dictionary::DictionaryPackage;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("用法: query <包目录> <词条> [更多词条...]");
        std::process::exit(2);
    }

    let root = &args[0];
    let package = match DictionaryPackage::open(root, false) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("打开词典包失败：{e}");
            std::process::exit(1);
        }
    };

    println!(
        "包 {}  v{}  语言 {} → {:?}  词条 {} 条  索引 {:.1} KB",
        package.manifest().id,
        package.manifest().version,
        package.language(),
        package.manifest().target_languages,
        package.entry_count(),
        package.index_bytes() as f64 / 1024.0,
    );
    println!(
        "许可 {}（{}）\n",
        package.manifest().provenance.license_spdx,
        package.manifest().provenance.revision
    );

    for term in &args[1..] {
        match package.lookup(term, 3) {
            Ok(entries) if entries.is_empty() => println!("「{term}」未找到"),
            Ok(entries) => {
                for e in entries {
                    let pinyin = e.readings.first().filter(|r| !r.starts_with("繁体"));
                    println!(
                        "「{}」{}",
                        e.headword,
                        pinyin.map(|p| format!("  [{p}]")).unwrap_or_default()
                    );
                    for s in e.senses.iter().take(3) {
                        println!("     - {}", s.definition);
                    }
                }
            }
            Err(e) => println!("「{term}」查询失败：{e}"),
        }
        println!();
    }
}
