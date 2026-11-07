//! 资源指纹与缓存失效（P2-S / S-04）。
//!
//! # 这条约束为什么重要
//!
//! 翻译结果的正确性依赖"用哪份资源"。同一个 `Translator` 如果换了词典版本、规则包
//! 或术语表，而缓存键没变，用户会拿到**旧资源算出来的译文**——而且是"看起来正常"的
//! 错误结果，比报错难查得多。
//!
//! 设计文档 ADR-011 要求资源版本变化必须让旧缓存失效。本文件验证三件事：
//!
//! 1. 指纹变化后**真的重新执行**（不是只多打一条警告）；
//! 2. 指纹进入缓存键（诊断能把用的指纹报出来）；
//! 3. 空指纹不会把已有指纹抹掉（避免"忘了填"变成"静默共享缓存"）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use translator_core::{Coordinator, CoordinatorConfig};
use translator_plugin_api::{BoxFuture, PluginRegistry, TranslationEngine};
use translator_types::{
    Capability, EngineDescriptor, LanguageTag, PluginApiVersion, TranslateRequest,
    TranslationError, TranslationResult, TranslationWarning,
};

/// 每次调用都换一个标记的 NMT 引擎：能直接看出"到底执行了几次"。
struct CountingNmt {
    calls: Arc<AtomicUsize>,
}

impl TranslationEngine for CountingNmt {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: "test.counting",
            capabilities: &[Capability::Nmt],
            priority: 50,
            plugin_api: PluginApiVersion::CURRENT,
            license: "MIT",
        }
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(TranslationResult {
                text: format!("[call{n}] {}", req.text),
                source: req
                    .source
                    .clone()
                    .unwrap_or_else(|| LanguageTag::new("en").expect("静态标签合法")),
                target: req.target.clone(),
                engine_id: "test.counting".to_string(),
                model_id: None,
                elapsed_ms: 0,
                warnings: Vec::new(),
            })
        })
    }
}

/// 可切换执行身份的引擎：模拟“同 engine_id、模型内容/解码身份变化”。
struct IdentityNmt {
    calls: Arc<AtomicUsize>,
    identity: Arc<Mutex<String>>,
}

impl TranslationEngine for IdentityNmt {
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: "test.identity",
            capabilities: &[Capability::Nmt],
            priority: 50,
            plugin_api: PluginApiVersion::CURRENT,
            license: "MIT",
        }
    }

    fn supports(&self, _source: &LanguageTag, _target: &LanguageTag) -> bool {
        true
    }

    fn translate<'a>(
        &'a self,
        req: &'a TranslateRequest,
    ) -> BoxFuture<'a, Result<TranslationResult, TranslationError>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(TranslationResult {
                text: format!("[identity{n}] {}", req.text),
                source: req
                    .source
                    .clone()
                    .unwrap_or_else(|| LanguageTag::new("en").expect("静态标签合法")),
                target: req.target.clone(),
                engine_id: "test.identity".to_string(),
                model_id: None,
                elapsed_ms: 0,
                warnings: Vec::new(),
            })
        })
    }

    fn cache_identity(&self) -> Option<String> {
        Some(self.identity.lock().unwrap().clone())
    }
}

fn setup_identity() -> (Coordinator, Arc<AtomicUsize>, Arc<Mutex<String>>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let identity = Arc::new(Mutex::new("plan-v1".to_string()));
    let mut registry = PluginRegistry::new();
    registry
        .register_engine(Arc::new(IdentityNmt {
            calls: calls.clone(),
            identity: identity.clone(),
        }))
        .expect("注册身份引擎");
    let coordinator = Coordinator::new(Arc::new(registry), CoordinatorConfig::default());
    (coordinator, calls, identity)
}

fn setup() -> (Coordinator, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = PluginRegistry::new();
    registry
        .register_engine(Arc::new(CountingNmt {
            calls: calls.clone(),
        }))
        .expect("注册计数引擎");
    let coordinator = Coordinator::new(Arc::new(registry), CoordinatorConfig::default());
    (coordinator, calls)
}

fn zh() -> LanguageTag {
    LanguageTag::new("zh-Hans").unwrap()
}

fn en() -> LanguageTag {
    LanguageTag::new("en").unwrap()
}

/// 整句 → 走 NMT（单词会被路由到词典，这里刻意用句子）。
fn request() -> TranslateRequest {
    TranslateRequest::new("good morning my friend", zh()).with_source(en())
}

fn hit(warnings: &[TranslationWarning]) -> bool {
    warnings
        .iter()
        .any(|w| matches!(w, TranslationWarning::CacheHit))
}

#[test]
fn 同一次绑定内重复请求命中缓存() {
    let (mut coord, calls) = setup();
    coord.bind_resource_fingerprint("dictionary.test@1.0.0");

    let first = pollster::block_on(coord.translate(&request())).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!hit(&first.warnings));

    let second = pollster::block_on(coord.translate(&request())).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1, "第二次不该再执行");
    assert!(hit(&second.warnings));
    assert_eq!(second.text, first.text);
}

#[test]
fn 执行身份变化必须重新执行() {
    let (coord, calls, identity) = setup_identity();
    let first = pollster::block_on(coord.translate(&request())).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 同 engine_id，但后端执行身份（模型/词表/解码/保护）变了：不得命中旧缓存。
    *identity.lock().unwrap() = "plan-v2".to_string();
    let second = pollster::block_on(coord.translate(&request())).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "cache_identity 变化后必须重新执行（EP-01/EP-03）"
    );
    assert_ne!(second.text, first.text);
}

#[test]
fn 资源指纹变化必须重新执行() {
    let (mut coord, calls) = setup();
    coord.bind_resource_fingerprint("dictionary.test@1.0.0");

    let first = pollster::block_on(coord.translate(&request())).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 换资源版本：旧译文必须失效
    coord.bind_resource_fingerprint("dictionary.test@2.0.0");
    let after = pollster::block_on(coord.translate(&request())).unwrap();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "指纹变化后必须真的重新执行，而不是复用旧结果"
    );
    assert!(!hit(&after.warnings), "新指纹下首次请求不该被当成缓存命中");
    assert_ne!(after.text, first.text, "引擎被调用了就会换标记");
}

#[test]
fn 诊断里能读到本次使用的资源指纹() {
    let (mut coord, _calls) = setup();
    coord.bind_resource_fingerprint("dictionary.test@2.0.0;rules.test@3");
    assert_eq!(
        coord.resource_fingerprint(),
        "dictionary.test@2.0.0;rules.test@3"
    );

    let out = pollster::block_on(coord.translate_with_diagnostics(&request())).unwrap();
    assert_eq!(
        out.diagnostics.resource_fingerprint, "dictionary.test@2.0.0;rules.test@3",
        "诊断必须报出真正进键的指纹，否则用户无法排查缓存来源"
    );
}

#[test]
fn 空指纹不会抹掉已绑定的指纹() {
    let (mut coord, _calls) = setup();
    coord.bind_resource_fingerprint("dictionary.test@1.0.0");
    coord.bind_resource_fingerprint("");
    assert_eq!(
        coord.resource_fingerprint(),
        "dictionary.test@1.0.0",
        "空串是「没绑定」，不是「绑定为空」"
    );
}

#[test]
fn 未绑定指纹时是空串而不是占位符() {
    let (coord, _calls) = setup();
    assert_eq!(coord.resource_fingerprint(), "");
    let out = pollster::block_on(coord.translate_with_diagnostics(&request())).unwrap();
    assert_eq!(out.diagnostics.resource_fingerprint, "");
}
