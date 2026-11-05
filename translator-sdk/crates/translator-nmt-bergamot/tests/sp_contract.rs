//! SP-01～12 的**第一层**：确定性结构契约 + Mock 故障注入。
//!
//! 这一层不需要模型、不需要 worker 进程，属于默认门禁（`cargo test --workspace`）。
//! 真实 Bergamot 的第二层在 `sp_real.rs`（`#[ignore]`）。
//!
//! 判定口径（docs/17 §3.1/§6.2）：**字节级结构比较**，不用 `must_contain` 之类宽松断言；
//! 受保护片段丢失 / 重复 / 串换 / 未知标记一律要求错误，且错误码必须是 `IntegrityFailure`。

use translator_core::protection::{ProtectionError, ProtectionPlan};
use translator_types::{TranslationError, TranslationErrorCode};

/// 子串出现次数（用于"恰好一次"这类结构断言）。
fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// 模拟一个"结构完好"的模型：把每段原样加方括号，便于确认拼接顺序。
fn identity_model(plan: &ProtectionPlan) -> Vec<String> {
    plan.plain_texts()
        .iter()
        .map(|part| format!("[{part}]"))
        .collect()
}

// ---------------------------------------------------------------- SP-01～03

#[test]
fn sp01_单个模板变量完整保留() {
    let text = "Hello {name}!";
    let plan = ProtectionPlan::build(text);
    assert_eq!(plan.items().len(), 1);
    assert_eq!(plan.items()[0].kind_str(), "template_var");
    assert_eq!(plan.items()[0].original, "{name}");
    assert_eq!(plan.masked(), "Hello #P1#!");

    // 分段：模型只看到 "Hello " 与 "!"，受保护片段根本不会进模型
    assert_eq!(plan.plain_texts(), vec!["Hello ", "!"]);
    let out = plan
        .assemble(&["你好 ".to_string(), "！".to_string()])
        .unwrap();
    assert_eq!(out, "你好 {name}！");
    assert_eq!(count(&out, "{name}"), 1);

    // 标记：模型看到 #P1#，还原逐字节贴回
    let marker = ProtectionPlan::build_marker(text);
    let restored = marker.restore("你好 #P1#！").unwrap();
    assert_eq!(restored, "你好 {name}！");
    assert_eq!(count(&restored, "{name}"), 1);
}

#[test]
fn sp02_多种模板变量各自完整() {
    let text = "call {order_id} for {name}";
    let plan = ProtectionPlan::build(text);
    let originals: Vec<_> = plan.items().iter().map(|i| i.original.as_str()).collect();
    assert_eq!(originals, vec!["{order_id}", "{name}"]);
    assert_eq!(plan.masked(), "call #P1# for #P2#");

    let out = plan
        .assemble(&["打电话 ".to_string(), " 给 ".to_string(), "".to_string()])
        .unwrap();
    assert_eq!(out, "打电话 {order_id} 给 {name}");
    assert_eq!(count(&out, "{order_id}"), 1);
    assert_eq!(count(&out, "{name}"), 1);
    // 顺序也必须是原文顺序（{order_id} 在 {name} 之前）
    assert!(out.find("{order_id}") < out.find("{name}"));
}

#[test]
fn sp03_重复出现的同一模板变量按出现次数分别编号() {
    let text = "{name} and {name}";
    let plan = ProtectionPlan::build(text);
    assert_eq!(plan.items().len(), 2, "两次出现是两件事，不能合并");
    assert_eq!(plan.masked(), "#P1# and #P2#");
    let out = plan
        .assemble(&["甲乙".into(), " 和 ".into(), "丙丁".into()])
        .unwrap();
    assert_eq!(count(&out, "{name}"), 2);
    assert_eq!(out, "甲乙{name} 和 {name}丙丁");
}

// ---------------------------------------------------------------- SP-04～05

#[test]
fn sp04_url_email_模板混合精确保留() {
    let text = "see https://x.example/a mail a.b+c@ex.example about {name}";
    let plan = ProtectionPlan::build(text);
    let kinds: Vec<_> = plan.items().iter().map(|i| i.kind_str()).collect();
    assert_eq!(kinds, vec!["url", "email", "template_var"]);
    let out = plan
        .assemble(&["看 ".into(), " 写信给 ".into(), " 关于 ".into(), "".into()])
        .unwrap();
    assert_eq!(
        out,
        "看 https://x.example/a 写信给 a.b+c@ex.example 关于 {name}"
    );
    for item in plan.items() {
        assert_eq!(
            count(&out, &item.original),
            1,
            "{} 必须恰好一次",
            item.kind_str()
        );
    }
    // 字节级：受保护片段来自原文切片，长度必须一致
    let protected_bytes: usize = plan.items().iter().map(|i| i.original.len()).sum();
    assert_eq!(
        protected_bytes,
        "https://x.example/a".len() + "a.b+c@ex.example".len() + "{name}".len()
    );
}

#[test]
fn sp05_带查询参数的url不被ampersand截断() {
    let text = "go https://x.example/a?b=1&c=2&d=%E4%B8%AD now";
    let plan = ProtectionPlan::build(text);
    assert_eq!(plan.items().len(), 1);
    assert_eq!(
        plan.items()[0].original,
        "https://x.example/a?b=1&c=2&d=%E4%B8%AD"
    );
    // 查询串里的 = & % 都属于 URL，不能被当成纯文本送进模型
    let plain = plan.plain_texts().join("");
    assert!(!plain.contains('&'), "查询串泄漏到纯文本：{plain:?}");
    assert!(!plain.contains("b=1"), "查询串泄漏到纯文本：{plain:?}");
}

// ---------------------------------------------------------------- SP-06～07

#[test]
fn sp06_反引号代码整体保留含反引号本身() {
    let text = "use `cargo build --release` now";
    let plan = ProtectionPlan::build(text);
    assert_eq!(plan.items().len(), 1);
    assert_eq!(plan.items()[0].kind_str(), "code");
    assert_eq!(plan.items()[0].original, "`cargo build --release`");
    let out = plan.assemble(&["用 ".into(), " 现在".into()]).unwrap();
    assert_eq!(count(&out, "`cargo build --release`"), 1);
    assert_eq!(count(&out, "`"), 2, "反引号必须成对保留，不能丢");
}

#[test]
fn sp07_代码内嵌url不重复标记() {
    let text = "`curl https://x.example/a`";
    let plan = ProtectionPlan::build(text);
    assert_eq!(plan.items().len(), 1, "整段代码一个片段，URL 不另立片段");
    assert_eq!(plan.items()[0].kind_str(), "code");
    assert_eq!(plan.items()[0].original, "`curl https://x.example/a`");
    assert_eq!(plan.masked(), "#P1#");
}

// ---------------------------------------------------------------- SP-08～09

#[test]
fn sp08_伪造标记必须报错() {
    let plan = ProtectionPlan::build_marker("hello {name}");
    assert_eq!(plan.items().len(), 1);

    // 多出来的 #P2#：模型凭空造了一个不存在的片段
    let err = plan.restore("你好 #P1# #P2#").unwrap_err();
    assert_eq!(err, ProtectionError::UnknownMarker("#P2#".into()));

    // #P0#：编号从 1 开始
    let err = plan.restore("你好 #P0# #P1#").unwrap_err();
    assert_eq!(err, ProtectionError::UnknownMarker("#P0#".into()));

    // 标记残缺：模型把 #P1# 拆坏了
    let err = plan.restore("你好 #P").unwrap_err();
    assert!(matches!(err, ProtectionError::UnknownMarker(_)), "{err:?}");
}

#[test]
fn sp09_丢失复制篡改必须报错且错误码是完整性失败() {
    let plan = ProtectionPlan::build_marker("hello {name} and {order_id}");

    let missing = plan.restore("你好 #P1#").unwrap_err();
    assert_eq!(
        missing,
        ProtectionError::Missing {
            id: 2,
            kind: "template_var"
        }
    );
    let duplicated = plan.restore("你好 #P1# #P1# #P2#").unwrap_err();
    assert_eq!(duplicated, ProtectionError::Duplicated { id: 1, count: 2 });

    // 大小写被改：扫描不到，等价于丢失（不能"看起来正常"地返回）
    let case = plan.restore("你好 #p1# 和 #P2#").unwrap_err();
    assert!(
        matches!(case, ProtectionError::Missing { id: 1, .. }),
        "{case:?}"
    );

    for err in [missing, duplicated, case] {
        let translated: TranslationError = err.into();
        assert_eq!(translated.code, TranslationErrorCode::IntegrityFailure);
    }
}

// ---------------------------------------------------------------- SP-10～12

#[test]
fn sp10_utf8与emoji不破坏字节边界() {
    let text = "😀 see https://a.example/😀 与 {名字} ok";
    let plan = ProtectionPlan::build(text);
    // 每个片段的原始字节必须与原文切片完全一致（不是"字符数"一致）
    for item in plan.items() {
        assert_eq!(&text[item.start..item.end], item.original);
    }
    let out = plan
        .assemble(&["😀 看 ".into(), " 和 ".into(), " 好的".into()])
        .unwrap();
    assert_eq!(count(&out, "😀 see https://a.example/😀"), 0);
    assert_eq!(count(&out, "https://a.example/😀"), 1);
    assert_eq!(count(&out, "😀"), 2, "emoji 不能在拼接时被拆坏");
    assert_eq!(count(&out, "{名字}"), 1);
}

#[test]
fn sp11_多行文本换行保留且不送进模型() {
    let text = "line1\nline2 {x}\nline3";
    let plan = ProtectionPlan::build(text);
    // 换行是 Piece::Literal，不属于 plain_texts（模型会把 \n 变成空格）
    assert_eq!(plan.plain_texts(), vec!["line1", "line2 ", "", "line3"]);
    for part in plan.plain_texts() {
        assert!(!part.contains('\n'), "换行泄漏进 plain：{part:?}");
    }
    let out = plan
        .assemble(&["行一".into(), "行二 ".into(), "".into(), "行三".into()])
        .unwrap();
    assert_eq!(out, "行一\n行二 {x}\n行三");
    assert_eq!(count(&out, "\n"), 2);
}

#[test]
fn sp12_空白与全保护输入不崩溃() {
    // 纯空白：没有受保护片段，纯文本原样回填（引擎层对空白段直接透传，不调用模型）
    let blank = ProtectionPlan::build("   ");
    assert!(blank.items().is_empty());
    assert_eq!(blank.plain_texts(), vec!["   "]);
    assert_eq!(blank.assemble(&["   ".into()]).unwrap(), "   ");

    // 空串
    let empty = ProtectionPlan::build("");
    assert!(empty.items().is_empty());
    assert_eq!(empty.assemble(&["".into()]).unwrap(), "");

    // 全保护：没有任何纯文本需要翻译，模型一次都不该被调用
    let full = ProtectionPlan::build("https://only.example/path");
    assert!(full.plain_texts().iter().all(|p| p.is_empty()));
    assert_eq!(
        full.assemble(&["".into(), "".into()]).unwrap(),
        "https://only.example/path"
    );
    assert_eq!(full.masked(), "#P1#");
    assert_eq!(full.restore("#P1#").unwrap(), "https://only.example/path");
}

// ------------------------------------------------------- Mock 故障注入（第一层）

#[test]
fn 模型丢段必须报错而不是返回半截译文() {
    let plan = ProtectionPlan::build("Hello {name}!");
    assert_eq!(plan.plain_texts().len(), 2);
    let err = plan.assemble(&["你好 ".to_string()]).unwrap_err();
    assert_eq!(
        err,
        ProtectionError::SegmentCount {
            expected: 2,
            got: 1
        }
    );
    assert_eq!(
        TranslationError::from(err).code,
        TranslationErrorCode::IntegrityFailure
    );
}

#[test]
fn 模型多段必须报错() {
    let plan = ProtectionPlan::build("Hello {name}!");
    let err = plan
        .assemble(&["a".into(), "b".into(), "c".into()])
        .unwrap_err();
    assert_eq!(
        err,
        ProtectionError::SegmentCount {
            expected: 2,
            got: 3
        }
    );
}

#[test]
fn 结构完好时不受影响() {
    let plan = ProtectionPlan::build("Hello {name}!");
    let out = plan.assemble(&identity_model(&plan)).unwrap();
    assert_eq!(out, "[Hello ]{name}[!]");
    assert_eq!(count(&out, "{name}"), 1);
}
