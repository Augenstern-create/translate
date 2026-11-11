//! 端到端流水线测试：保护 → 执行 → 还原 → 质量门禁 → 缓存。
//!
//! 这些测试用**脚本化引擎**精确控制"模型返回了什么"，因此能复现真实世界里
//! 最危险的三类故障：
//!
//! 1. 模型吞掉保护标记；
//! 2. 模型改动关键数字/编号；
//! 3. 模型返回空译文。
//!
//! 验收要求（§4.5 / §5.5）：这些情况**不得**静默返回成功，也不得写入缓存。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use translator_api::{
    EngineOutput, LanguageTag, LookupRequest, ProtectionPolicy, StrategyPreference, TemplateScope,
    TemplateTable, TermPolicy, TranslateRequest, TranslationEngine, TranslationError,
    TranslationErrorCode, TranslationJob, TranslationMode, Translator,
};
use translator_plugin_api::mocks::{MockDictionaryProvider, MockNmtEngine};

// ============================================================
// 脚本化引擎
// ============================================================

#[derive(Clone, Debug)]
struct Recorded {
    text: String,
    glossary: Vec<String>,
    placeholders: Vec<String>,
    retry_hint: Option<String>,
    mode: TranslationMode,
}

type Handler = dyn Fn(&TranslationJob<'_>, usize) -> Result<String, TranslationError> + Send + Sync;

struct ScriptedEngine {
    id: String,
    calls: Arc<AtomicUsize>,
    records: Arc<Mutex<Vec<Recorded>>>,
    handler: Arc<Handler>,
}

impl ScriptedEngine {
    fn new<F>(id: &str, handler: F) -> Arc<Self>
    where
        F: Fn(&TranslationJob<'_>, usize) -> Result<String, TranslationError>
            + Send
            + Sync
            + 'static,
    {
        Arc::new(Self {
            id: id.to_string(),
            calls: Arc::new(AtomicUsize::new(0)),
            records: Arc::new(Mutex::new(Vec::new())),
            handler: Arc::new(handler),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn records(&self) -> Vec<Recorded> {
        self.records.lock().unwrap().clone()
    }
}

impl TranslationEngine for ScriptedEngine {
    fn descriptor(&self) -> translator_api::EngineDescriptor {
        // 只声明 Nmt：脚本引擎代表"本地/自托管"后端，
        // 远程路径由 translator-http 的集成测试单独覆盖。
        let capabilities = vec![translator_api::Capability::Nmt];
        translator_api::EngineDescriptor::new(self.id.clone(), capabilities, 100, "MIT")
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> translator_plugin_api::BoxFuture<
        'a,
        Result<translator_api::TranslationResult, TranslationError>,
    > {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(translator_api::TranslationResult {
                text: req.text.clone(),
                source: req
                    .source
                    .clone()
                    .unwrap_or_else(|| LanguageTag::new("en").unwrap()),
                target: req.target.clone(),
                engine_id: self.id.clone(),
                model_id: Some("scripted".to_string()),
                elapsed_ms: 0,
                warnings: Vec::new(),
            })
        })
    }

    fn cache_identity(&self) -> Option<String> {
        Some(format!("{}|v1", self.id))
    }

    fn translate_job<'a>(
        &'a self,
        job: &'a TranslationJob<'a>,
    ) -> translator_plugin_api::BoxFuture<'a, Result<EngineOutput, TranslationError>> {
        Box::pin(async move {
            let attempt = self.calls.fetch_add(1, Ordering::SeqCst);
            self.records.lock().unwrap().push(Recorded {
                text: job.text.to_string(),
                glossary: job.glossary.iter().map(|g| g.source.clone()).collect(),
                placeholders: job.placeholders.to_vec(),
                retry_hint: job.retry_hint.map(str::to_string),
                mode: job.mode,
            });
            let text = (self.handler)(job, attempt)?;
            Ok(EngineOutput {
                text,
                model_id: Some("scripted".to_string()),
                model_revision: Some("rev-1".to_string()),
                usage: Some(translator_api::TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                }),
                warnings: Vec::new(),
            })
        })
    }
}

// ============================================================
// 辅助
// ============================================================

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}
fn zh() -> LanguageTag {
    LanguageTag::new("zh-CN").unwrap()
}

fn request(text: &str) -> TranslateRequest {
    TranslateRequest::new(text, zh()).with_source(en())
}

fn translator_with(engine: Arc<dyn TranslationEngine>) -> Translator {
    Translator::builder()
        .engine(engine)
        .expect("注册引擎失败")
        .cache_capacity(64)
        .build()
        .expect("构建 Translator 失败")
}

fn honest_engine() -> Arc<ScriptedEngine> {
    ScriptedEngine::new("scripted.honest", |job, _| Ok(job.text.to_string()))
}

/// 会吞掉保护标记的引擎（真实模型的常见故障）。
fn marker_eating_engine() -> Arc<ScriptedEngine> {
    ScriptedEngine::new("scripted.marker-eater", |job, _| {
        let mut out = job.text.to_string();
        while let Some(start) = out.find('\u{27E6}') {
            let Some(end_rel) = out[start..].find('\u{27E7}') else {
                break;
            };
            out.replace_range(start..start + end_rel + 3, "");
        }
        Ok(out)
    })
}

// ============================================================
// 基础链路
// ============================================================

#[test]
fn 纯文本翻译走配置的引擎() {
    let engine = honest_engine();
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let result = pollster::block_on(translator.translate(&request("Hello world."))).unwrap();
    assert_eq!(result.text, "Hello world.");
    assert_eq!(result.engine_id, "scripted.honest");
    assert_eq!(engine.calls(), 1);
}

#[test]
fn 受保护内容在诚实引擎下完整还原() {
    let engine = honest_engine();
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let source = "Visit https://example.com/x?id=3 and enter {code}.";
    let result = pollster::block_on(translator.translate(&request(source))).unwrap();
    assert!(
        result.text.contains("https://example.com/x?id=3"),
        "{}",
        result.text
    );
    assert!(result.text.contains("{code}"), "{}", result.text);

    let records = engine.records();
    assert_eq!(records.len(), 1);
    assert!(records[0].text.contains('\u{27E6}'), "{}", records[0].text);
    assert_eq!(records[0].placeholders.len(), 2);
}

#[test]
fn 吞掉标记会触发一次重试并回退分段() {
    let engine = marker_eating_engine();
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let source = "Visit https://example.com/x and enter {code}.";
    let output = pollster::block_on(translator.translate_with_diagnostics(&request(source)))
        .expect("分段回退后应成功");

    assert!(
        output.result.text.contains("https://example.com/x"),
        "{}",
        output.result.text
    );
    assert!(output.result.text.contains("{code}"));
    assert!(output.diagnostics.attempts >= 2);
    assert_eq!(output.diagnostics.protection_strategy, "segment");
    // 第二次尝试换成分段策略：一次标记调用 + 两段普通文本调用
    let records = engine.records();
    assert!(
        records.len() >= 2,
        "至少应有一次标记尝试 + 一次分段尝试：{records:?}"
    );
    assert!(records.len() <= 1 + 4, "分段数不应爆炸：{records:?}");
    assert!(!records[0].placeholders.is_empty(), "第一次应带占位符");
    assert!(records[1].retry_hint.is_some(), "重试应带纠正提示");
    assert!(
        records[1..].iter().all(|r| r.placeholders.is_empty()),
        "分段策略下不应再有占位符"
    );
}

#[test]
fn 保护彻底失败时报完整性错误且不写缓存() {
    // 模型对任何输入都返回空串：标记策略、分段策略都拿不到译文
    let engine = ScriptedEngine::new("scripted.destroyer", |_, _| Ok(String::new()));
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);

    let err = pollster::block_on(translator.translate(&request("Visit https://example.com.")))
        .unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::IntegrityFailure,
        "{}",
        err.message
    );
    assert_eq!(translator.cache_stats().len, 0, "失败的译文不得写缓存");
}

#[test]
fn 缓存命中返回与首次一致的终检文本() {
    // 候选会被格式修复：最终文本必须是"修复后且校验通过"的那一份
    let engine = ScriptedEngine::new("scripted.repair", |_, _| Ok("温度在以上20度。".to_string()));
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);

    let first = pollster::block_on(
        translator.translate_with_diagnostics(&request("The temperature is above 20 degrees.")),
    )
    .expect("应通过终检");
    assert_eq!(first.result.text, "温度在20度以上。");
    assert_eq!(first.diagnostics.repair, "applied");

    let second = pollster::block_on(
        translator.translate_with_diagnostics(&request("The temperature is above 20 degrees.")),
    )
    .expect("缓存命中");
    assert_eq!(
        second.result.text, first.result.text,
        "缓存文本必须与首次一致"
    );
    assert!(second.diagnostics.cache_hit);
    assert_eq!(second.diagnostics.repair, "applied", "诊断也要一起缓存");
    assert_eq!(engine.calls(), 1);
}

#[test]
fn 修复回退后的文本同样进入缓存且稳定() {
    // 关闭数字保护，让"修复会拆坏版本号"成为唯一的变化来源：
    // 修复把 "以上2.1.3" 拆成 "2.1以上.3"，关键数字变了 → 必须退回原候选
    let engine = ScriptedEngine::new("scripted.repair2", |_, _| Ok("版本 以上2.1.3".to_string()));
    let translator =
        translator_without_number_protection(Arc::clone(&engine) as Arc<dyn TranslationEngine>);

    let first = pollster::block_on(
        translator.translate_with_diagnostics(&request("Version 2.1.3 or above is supported.")),
    )
    .expect("退回原候选后应成功");
    assert_eq!(first.result.text, "版本 以上2.1.3");
    assert_eq!(first.diagnostics.repair, "rejected");

    let second = pollster::block_on(
        translator.translate_with_diagnostics(&request("Version 2.1.3 or above is supported.")),
    )
    .expect("缓存命中");
    assert_eq!(second.result.text, first.result.text);
    assert_eq!(second.diagnostics.repair, "rejected");
    assert_eq!(engine.calls(), 1);
}

// ============================================================
// 数字与完整性
// ============================================================

/// 关闭"数字保护"、只保留质量门禁的装配：用来单独验证数字检查本身。
fn translator_without_number_protection(engine: Arc<dyn TranslationEngine>) -> Translator {
    Translator::builder()
        .engine(engine)
        .expect("注册引擎失败")
        .protection_policy(ProtectionPolicy {
            preference: StrategyPreference::Auto,
            spans: translator_api::SpanPolicy::minimal(),
        })
        .build()
        .expect("构建 Translator 失败")
}

#[test]
fn 默认保护下编号即使被模型改写也不会丢() {
    // 保护优先：模型把编号写错也没关系，还原时会被原样贴回
    let engine = ScriptedEngine::new("scripted.rename", |_, _| {
        Ok("订单 ⟦P1⟧ 已发货。".to_string())
    });
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let result = pollster::block_on(translator.translate(&request("Order #10248 has shipped.")))
        .expect("保护机制应把编号贴回");
    assert!(result.text.contains("#10248"), "{}", result.text);
}

#[test]
fn 关闭数字保护后编号被改会致命() {
    let engine = ScriptedEngine::new("scripted.liar", |_, _| {
        Ok("订单 #99999 已发货。".to_string())
    });
    let translator =
        translator_without_number_protection(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let err = pollster::block_on(translator.translate(&request("Order #10248 has shipped.")))
        .unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::IntegrityFailure,
        "{}",
        err.message
    );
}

#[test]
fn 关闭数字保护后百分比被改会致命() {
    let engine = ScriptedEngine::new("scripted.percent", |_, _| Ok("节省 50%。".to_string()));
    let translator =
        translator_without_number_protection(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let err = pollster::block_on(translator.translate(&request("Save 15% today."))).unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::IntegrityFailure,
        "{}",
        err.message
    );
}

#[test]
fn 金额币种丢失不会被忽略() {
    let engine = ScriptedEngine::new("scripted.price", |_, _| Ok("价格是 39.99。".to_string()));
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let output =
        pollster::block_on(translator.translate_with_diagnostics(&request("It costs $39.99.")))
            .expect("金额丢失属于 Major 诊断，不拒绝返回");
    let quality = output.diagnostics.quality.expect("应有质量报告");
    assert!(
        quality.issues.iter().any(|i| i.detail.contains("币种")),
        "{:?}",
        quality.issues
    );
}

#[test]
fn 区间上下限被改会致命() {
    let engine = ScriptedEngine::new("scripted.range", |_, _| {
        Ok("需要 20 至 30 分钟。".to_string())
    });
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let err =
        pollster::block_on(translator.translate(&request("It takes between 10 and 20 minutes.")))
            .unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::IntegrityFailure,
        "{}",
        err.message
    );
}

#[test]
fn 数量词用中文写法是允许的() {
    let engine = ScriptedEngine::new("scripted.quantity", |_, _| Ok("我有三本书。".to_string()));
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let result = pollster::block_on(translator.translate(&request("I have three books.")))
        .expect("数量词允许中英互转");
    assert_eq!(result.text, "我有三本书。");
}

#[test]
fn 空译文不会静默成功() {
    let engine = ScriptedEngine::new("scripted.empty", |_, _| Ok("   ".to_string()));
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let err = pollster::block_on(translator.translate(&request("Hello."))).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::IntegrityFailure);
}

#[test]
fn 输入为空是无效请求() {
    let engine = honest_engine();
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let err = pollster::block_on(translator.translate(&request("   "))).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::InvalidRequest);
    assert_eq!(engine.calls(), 0);
}

#[test]
fn 超长输入被拒绝而不是发出去花钱() {
    let engine = honest_engine();
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册引擎失败")
        .max_input_chars(10)
        .build()
        .expect("构建失败");
    let err = pollster::block_on(translator.translate(&request(
        "This sentence is definitely longer than ten characters.",
    )))
    .unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::InvalidRequest);
    assert_eq!(engine.calls(), 0);
}

// ============================================================
// 缓存
// ============================================================

#[test]
fn 第二次相同请求命中缓存且不再调用模型() {
    let engine = honest_engine();
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let first = pollster::block_on(translator.translate(&request("Hello."))).unwrap();
    let second =
        pollster::block_on(translator.translate_with_diagnostics(&request("Hello."))).unwrap();

    assert_eq!(first.text, second.result.text);
    assert_eq!(engine.calls(), 1, "第二次必须命中缓存");
    assert!(second.diagnostics.cache_hit);
    assert!(second
        .result
        .warnings
        .iter()
        .any(|w| matches!(w, translator_api::TranslationWarning::CacheHit)));
}

#[test]
fn 模式变化不会复用缓存() {
    let engine = honest_engine();
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let mut fast = translator.default_options().clone();
    fast.mode = TranslationMode::Fast;

    pollster::block_on(translator.translate(&request("Hello."))).unwrap();
    pollster::block_on(translator.translate_with_options(&request("Hello."), &fast)).unwrap();
    assert_eq!(engine.calls(), 2, "Precise 与 Fast 不得共享缓存");
    assert_eq!(engine.records()[1].mode, TranslationMode::Fast);
}

#[test]
fn 术语表版本变化不会复用缓存() {
    let engine = honest_engine();
    let glossary_v1 = Arc::new(
        translator_api::Glossary::from_json(
            r#"{"schema_version":1,"id":"g","version":1,"entries":[
                {"id":"a","source":"model","target":"模型"}]}"#,
        )
        .unwrap(),
    );
    let glossary_v2 = Arc::new(
        translator_api::Glossary::from_json(
            r#"{"schema_version":1,"id":"g","version":2,"entries":[
                {"id":"a","source":"model","target":"模型"}]}"#,
        )
        .unwrap(),
    );

    let t1 = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .glossary(Arc::clone(&glossary_v1))
        .build()
        .unwrap();
    pollster::block_on(t1.translate(&request("The model is ready."))).unwrap();

    let t2 = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .glossary(glossary_v2)
        .build()
        .unwrap();
    pollster::block_on(t2.translate(&request("The model is ready."))).unwrap();
    assert_eq!(engine.calls(), 2, "术语表版本变化必须让缓存失效");
}

// ============================================================
// 模板
// ============================================================

fn template_table() -> Arc<dyn translator_api::SentenceTemplates> {
    TemplateTable::from_json(
        r#"{"schema_version":2,"id":"templates.ui","version":1,
            "source":"en","target":"zh-CN",
            "entries":[
              {"source":"Save","target":"保存"},
              {"source":"File not found","target":"找不到文件"},
              {"source":"Settings","target":"选项","app":"com.acme.editor"}
            ]}"#,
    )
    .unwrap()
    .as_arc()
}

#[test]
fn 模板命中不调用模型() {
    let engine = honest_engine();
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .templates(template_table())
        .build()
        .unwrap();

    let result = pollster::block_on(translator.translate(&request("Save"))).unwrap();
    assert_eq!(result.text, "保存");
    assert!(result.engine_id.starts_with("template:"));
    assert_eq!(engine.calls(), 0, "模板命中不得调用模型");
}

#[test]
fn 模板作用域生效() {
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册失败")
        .templates(template_table())
        .template_scope(TemplateScope::app("com.acme.editor"))
        .build()
        .unwrap();
    assert_eq!(
        pollster::block_on(translator.translate(&request("Settings")))
            .unwrap()
            .text,
        "选项"
    );
}

#[test]
fn 模板也可以被显式关闭() {
    let engine = honest_engine();
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .build()
        .unwrap();
    let result = pollster::block_on(translator.translate(&request("Save"))).unwrap();
    assert_eq!(result.text, "Save");
    assert_eq!(engine.calls(), 1);
}

// ============================================================
// 术语
// ============================================================

fn glossary_json(strict: bool) -> String {
    format!(
        r#"{{"schema_version":1,"id":"glossary.software","version":3,"entries":[
            {{"id":"t1","source":"large language model","target":"大语言模型","strict":{strict}}}]}}"#
    )
}

#[test]
fn 术语约束会送进执行任务() {
    let engine = honest_engine();
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .glossary(Arc::new(
            translator_api::Glossary::from_json(&glossary_json(false)).unwrap(),
        ))
        .build()
        .unwrap();

    let _ =
        pollster::block_on(translator.translate(&request("This large language model is fast.")));
    let records = engine.records();
    assert!(
        records[0]
            .glossary
            .contains(&"large language model".to_string()),
        "{:?}",
        records[0].glossary
    );
}

#[test]
fn warn_策略下术语未满足只报告不失败() {
    let engine = ScriptedEngine::new("scripted.term", |_, _| Ok("这个 LLM 很快。".to_string()));
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .glossary(Arc::new(
            translator_api::Glossary::from_json(&glossary_json(true)).unwrap(),
        ))
        .term_policy(TermPolicy::Warn)
        .build()
        .unwrap();

    let output = pollster::block_on(
        translator.translate_with_diagnostics(&request("This large language model is fast.")),
    )
    .expect("Warn 策略不得让翻译失败");

    assert!(output
        .result
        .warnings
        .iter()
        .any(|w| matches!(w, translator_api::TranslationWarning::TermMismatch(_))));
    let trace = output.diagnostics.glossary.expect("应有术语诊断");
    assert_eq!(trace.hits, 1);
    assert!(!trace.violations.is_empty());
}

#[test]
fn enforce_策略下硬约束未满足则失败() {
    let engine = ScriptedEngine::new("scripted.term2", |_, _| Ok("这个 LLM 很快。".to_string()));
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .glossary(Arc::new(
            translator_api::Glossary::from_json(&glossary_json(true)).unwrap(),
        ))
        .term_policy(TermPolicy::Enforce)
        .build()
        .unwrap();

    let err =
        pollster::block_on(translator.translate(&request("This large language model is fast.")))
            .unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::IntegrityFailure);
    assert!(err.message.contains("术语"), "{}", err.message);
}

#[test]
fn 术语表冲突在装配期报错() {
    let err = translator_api::Glossary::from_json(
        r#"{"schema_version":1,"id":"g","version":1,"entries":[
            {"id":"a","source":"model","target":"模型"},
            {"id":"b","source":"model","target":"型号"}]}"#,
    )
    .unwrap_err();
    assert!(matches!(err, translator_api::GlossaryError::Conflict(_)));
}

// ============================================================
// S1-01：统一终检（模板与模型走同一套规则）
// ============================================================

#[test]
fn 模板译文也必须通过术语终检() {
    // 模板数据可能与术语表冲突：模板命中不是"免检"的理由。
    let templates = TemplateTable::from_json(
        r#"{"schema_version":2,"id":"templates.conflict","version":1,
            "source":"en","target":"zh-CN",
            "entries":[{"source":"A large language model",
                        "target":"大型语言模型。"}]}"#,
    )
    .unwrap()
    .as_arc();

    let engine = ScriptedEngine::new("scripted.unused", |job, _| Ok(job.text.to_string()));
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .templates(templates)
        .glossary(Arc::new(
            translator_api::Glossary::from_json(&glossary_json(true)).unwrap(),
        ))
        .term_policy(TermPolicy::Enforce)
        .build()
        .unwrap();

    let err = pollster::block_on(translator.translate(&request("A large language model")))
        .expect_err("模板译文违反 strict 术语时不得返回成功");
    assert_eq!(
        err.code,
        TranslationErrorCode::IntegrityFailure,
        "{}",
        err.message
    );
    assert!(err.message.contains("术语"), "{}", err.message);
    assert_eq!(engine.calls(), 0, "模板命中不应调用模型");
}

#[test]
fn 模板路径与模型路径的终检规则一致() {
    // 同一条源文：模板命中与模型产出都必须过完整性校验
    let templates = TemplateTable::from_json(
        r#"{"schema_version":2,"id":"templates.bad","version":1,
            "source":"en","target":"zh-CN",
            "entries":[{"source":"Open {file}","target":"打开"}]}"#,
    )
    .unwrap()
    .as_arc();
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册失败")
        .templates(templates)
        .build()
        .unwrap();
    let err = pollster::block_on(translator.translate(&request("Open {file}")))
        .expect_err("模板丢失占位符必须被拒绝");
    assert_eq!(
        err.code,
        TranslationErrorCode::IntegrityFailure,
        "{}",
        err.message
    );
}

// ============================================================
// S2：预算与调用计数（协调器侧）
// ============================================================

/// 会"消费 HTTP 预算"的测试引擎：模拟真实后端的扣费行为。
struct BudgetEatingEngine {
    calls: Arc<AtomicUsize>,
    /// 返回的文本（不含标记），用于触发完整性失败与分段回退。
    reply: String,
}

impl BudgetEatingEngine {
    fn new(reply: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::new(AtomicUsize::new(0)),
            reply: reply.to_string(),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl TranslationEngine for BudgetEatingEngine {
    fn descriptor(&self) -> translator_api::EngineDescriptor {
        translator_api::EngineDescriptor::new(
            "mock.budget",
            [translator_api::Capability::Nmt],
            100,
            "MIT",
        )
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> translator_plugin_api::BoxFuture<
        'a,
        Result<translator_api::TranslationResult, TranslationError>,
    > {
        Box::pin(async move {
            Ok(translator_api::TranslationResult {
                text: self.reply.clone(),
                source: req
                    .source
                    .clone()
                    .unwrap_or_else(|| LanguageTag::new("en").unwrap()),
                target: req.target.clone(),
                engine_id: "mock.budget".to_string(),
                model_id: None,
                elapsed_ms: 0,
                warnings: Vec::new(),
            })
        })
    }

    fn translate_job<'a>(
        &'a self,
        job: &'a TranslationJob<'a>,
    ) -> translator_plugin_api::BoxFuture<'a, Result<EngineOutput, TranslationError>> {
        Box::pin(async move {
            // 真实后端在发请求前扣预算；这里复现同一契约
            if let Some(budget) = &job.budget {
                if !budget.try_start_call() {
                    return Err(TranslationError::new(
                        TranslationErrorCode::BudgetExceeded,
                        "预算耗尽",
                    ));
                }
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            // 模拟"吞掉标记"的模型：永远只返回固定文本，任何占位符都丢失
            let _ = job.text;
            Ok(EngineOutput::new(self.reply.clone()))
        })
    }
}

#[test]
fn 预算按逻辑请求创建并被协调器计入诊断() {
    let engine = BudgetEatingEngine::new("你好。");
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .max_http_calls_per_request(4)
        .build()
        .unwrap();

    let output = pollster::block_on(translator.translate_with_diagnostics(&request("Hello.")))
        .expect("应成功");
    assert_eq!(engine.calls(), 1);
    assert_eq!(output.diagnostics.http_calls, 1, "诊断应报告实际调用数");
}

#[test]
fn 分段回退与内容重试共享同一预算() {
    // 预算 2：第一次（标记）消耗 1 → 完整性失败 → 分段回退需要 3 段，
    // 第二段的第二次调用就会撞上预算上限。
    let engine = BudgetEatingEngine::new("无标记的译文。");
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .max_http_calls_per_request(2)
        .content_retry(1)
        .build()
        .unwrap();

    let err = pollster::block_on(
        translator.translate(&request("Visit https://example.com/a and {file} now.")),
    )
    .unwrap_err();
    assert_eq!(
        err.code,
        TranslationErrorCode::BudgetExceeded,
        "{}",
        err.message
    );
    assert_eq!(engine.calls(), 2, "预算只允许两次真实调用");
}

#[test]
fn 预算耗尽后不再重试() {
    let engine = BudgetEatingEngine::new("无标记的译文。");
    let translator = Translator::builder()
        .engine(Arc::clone(&engine) as Arc<dyn TranslationEngine>)
        .expect("注册失败")
        .max_http_calls_per_request(1)
        .content_retry(3)
        .build()
        .unwrap();

    let err = pollster::block_on(
        translator.translate(&request("Visit https://example.com/a and {file} now.")),
    )
    .unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::BudgetExceeded);
    assert_eq!(engine.calls(), 1, "只有一次调用额度");
}

// ============================================================
// 取消 / 查词 / 路由
// ============================================================

#[test]
fn 预取消的请求不调用模型() {
    let engine = honest_engine();
    let translator = translator_with(Arc::clone(&engine) as Arc<dyn TranslationEngine>);
    let token = translator_api::CancelToken::new();
    token.cancel();
    let err = pollster::block_on(translator.translate_cancellable(&request("Hello."), &token))
        .unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::Cancelled);
    assert_eq!(engine.calls(), 0);
}

#[test]
fn 查词与翻译是两条独立路径() {
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .provider(MockDictionaryProvider::new())
        .expect("注册词典失败")
        .build()
        .unwrap();

    let entries = pollster::block_on(translator.lookup(&LookupRequest::new("hello"))).unwrap();
    assert!(!entries.is_empty());

    // 句子翻译不会因为"单词很短"就自动走词典
    let result = pollster::block_on(translator.translate(&request("hello"))).unwrap();
    assert!(result.engine_id.starts_with("mock.nmt"));
}

#[test]
fn 没有词典时报词典不可用() {
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .build()
        .unwrap();
    let err = pollster::block_on(translator.lookup(&LookupRequest::new("hello"))).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::DictionaryUnavailable);
}

#[test]
fn 没有后端时装配失败而不是静默返回原文() {
    let err = Translator::builder().build().unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::ConfigurationError);
}

#[test]
fn 可以解释路由() {
    let translator = Translator::builder()
        .engine(MockNmtEngine::new())
        .expect("注册引擎失败")
        .build()
        .unwrap();
    let decision = translator.explain_route(&request("Hello.")).unwrap();
    assert_eq!(decision.engine_id, "mock.nmt");
    assert!(!decision.reason.is_empty());
}

// ============================================================
// HTTP 后端装配（不发真实请求）
// ============================================================

#[test]
fn http_配置缺少密钥时在调用阶段报鉴权错误() {
    let toml = r#"
[backend.remote]
base_url = "http://127.0.0.1:9/v1"
model = "m"
api_key_env = "TRANSLATOR_TEST_UNSET_KEY_54321"
max_retries = 0
timeout_ms = 300
"#;
    let translator = Translator::builder()
        .http_config_toml(toml)
        .expect("配置应可解析")
        // 远程后端必须被显式选中：默认不把原文上传到第三方
        .preferred_backend("http.remote")
        .build()
        .expect("应可构造");
    let err = pollster::block_on(translator.translate(&request("Hello."))).unwrap_err();
    assert_eq!(err.code, TranslationErrorCode::AuthenticationFailed);
}

#[test]
fn http_配置非法时装配失败() {
    let result = Translator::builder()
        .http_config_toml("[backend.remote]\nbase_url=\"ftp://x\"\nmodel=\"m\"\n");
    match result {
        Ok(_) => panic!("ftp 地址应被拒绝"),
        Err(err) => assert_eq!(err.code, TranslationErrorCode::ConfigurationError),
    }
}
