//! 学习链与翻译门面的桥（P17-A / P20-K）。
//!
//! 设计要点：
//! - 默认不启用；`TranslatorBuilder::learning()` 才会创建线程与数据库连接；
//! - 记录在**后台线程**完成，主翻译链只做一次 `try_send`；
//! - 队列满、数据库错误、隐私拒绝都不会让翻译失败；
//! - 学习授权与远程翻译授权相互独立。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub use translator_learning::{
    CandidateKind, KnowledgeCandidate, LearningConsent, LearningError, LearningInput,
    LearningStats, LearningStore, PrivacyLevel, RecordOutcome, ReleaseInfo, ReleaseStatus,
    ReviewStatus, SkipReason,
};

use crate::{TranslateOutput, TranslateRequest};

/// 学习运行时统计快照。与 `RouteStats` 分开，避免把“翻译单元”与“候选”混为一谈。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LearningRuntimeStats {
    pub submitted: u64,
    pub recorded: u64,
    pub skipped: u64,
    pub errors: u64,
    pub queue_full: u64,
    pub privacy_disabled: u64,
}

#[derive(Default)]
struct Counters {
    submitted: AtomicU64,
    recorded: AtomicU64,
    skipped: AtomicU64,
    errors: AtomicU64,
    queue_full: AtomicU64,
    privacy_disabled: AtomicU64,
}

enum Job {
    Record {
        input: LearningInput,
        consent: LearningConsent,
        privacy_disabled: bool,
    },
    Flush(SyncSender<()>),
}

/// 后台学习记录器。`Translator` 持有它；丢弃时发送线程结束。
pub struct LearningRuntime {
    sender: SyncSender<Job>,
    counters: Arc<Counters>,
    worker: Mutex<Option<JoinHandle<()>>>,
    store: Arc<LearningStore>,
}

impl LearningRuntime {
    pub fn new(store: Arc<LearningStore>, consent: LearningConsent) -> Result<Self, LearningError> {
        store.set_retention_days(consent.retention_days)?;
        let (sender, receiver) = sync_channel::<Job>(1024);
        let counters = Arc::new(Counters::default());
        let worker_store = store.clone();
        let worker_counters = counters.clone();
        let worker = std::thread::Builder::new()
            .name("translator-learning".to_string())
            .spawn(move || worker_loop(receiver, worker_store, worker_counters))?;
        Ok(Self {
            sender,
            counters,
            worker: Mutex::new(Some(worker)),
            store,
        })
    }

    pub fn store(&self) -> &Arc<LearningStore> {
        &self.store
    }

    /// 提交一次翻译结果。**绝不阻塞、绝不返回错误。**
    pub fn submit(&self, req: &TranslateRequest, out: &TranslateOutput, consent: LearningConsent) {
        if !consent.enabled() {
            self.counters
                .privacy_disabled
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        for input in learning_inputs(req, out, consent) {
            self.counters.submitted.fetch_add(1, Ordering::Relaxed);
            match self.sender.try_send(Job::Record {
                input,
                consent,
                privacy_disabled: false,
            }) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.counters.queue_full.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.counters.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// 等待队列处理完（测试/关闭时使用）。生产路径不需要调用。
    pub fn flush(&self) {
        let (tx, rx) = sync_channel(1);
        if self.sender.send(Job::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    pub fn stats(&self) -> LearningRuntimeStats {
        LearningRuntimeStats {
            submitted: self.counters.submitted.load(Ordering::Relaxed),
            recorded: self.counters.recorded.load(Ordering::Relaxed),
            skipped: self.counters.skipped.load(Ordering::Relaxed),
            errors: self.counters.errors.load(Ordering::Relaxed),
            queue_full: self.counters.queue_full.load(Ordering::Relaxed),
            privacy_disabled: self.counters.privacy_disabled.load(Ordering::Relaxed),
        }
    }

    /// 显式清理已存学习数据。只影响本地学习库，不影响正式词典/规则。
    pub fn clear(&self) -> Result<(), LearningError> {
        self.store.clear_all()
    }
}

impl Drop for LearningRuntime {
    fn drop(&mut self) {
        // 先丢弃 sender 让 worker 看到 Disconnected；再等待线程退出。
        // 不能移动字段，故用一个替换技巧。
        let (tx, _rx) = sync_channel(1);
        let old = std::mem::replace(&mut self.sender, tx);
        drop(old);
        if let Some(worker) = self.worker.lock().expect("学习线程锁").take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(receiver: Receiver<Job>, store: Arc<LearningStore>, counters: Arc<Counters>) {
    let mut processed = 0u64;
    while let Ok(job) = receiver.recv() {
        match job {
            Job::Record {
                input,
                consent,
                privacy_disabled,
            } => {
                processed = processed.saturating_add(1);
                if processed % 64 == 1 {
                    let _ = store.purge_expired_now();
                }
                let result = store.record(&input, &consent);
                match result {
                    Ok(RecordOutcome::Recorded { .. }) => {
                        counters.recorded.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(RecordOutcome::Skipped(reason)) => {
                        if matches!(reason, translator_learning::SkipReason::PrivacyOff)
                            || privacy_disabled
                        {
                            counters.privacy_disabled.fetch_add(1, Ordering::Relaxed);
                        } else {
                            counters.skipped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        counters.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Job::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

/// 从一次翻译输出提取学习记录。
///
/// 只记录模型/兜底单元；词典与规则命中不是缺口。聚合诊断没有逐单元原因时
/// 不记录，避免把整篇文章错误地当成一个词库缺口。
fn learning_inputs(
    req: &TranslateRequest,
    out: &TranslateOutput,
    consent: LearningConsent,
) -> Vec<LearningInput> {
    if !consent.enabled() {
        return Vec::new();
    }
    let mut inputs = Vec::new();
    let knowledge_version = if out.diagnostics.resource_fingerprint.is_empty() {
        "unversioned".to_string()
    } else {
        out.diagnostics.resource_fingerprint.clone()
    };

    if out.diagnostics.units.len() <= 1 {
        let Some(reason) = out.diagnostics.fallback_reason else {
            return inputs;
        };
        if !matches!(
            out.diagnostics.route,
            crate::TranslationRoute::LocalNmt | crate::TranslationRoute::Remote
        ) {
            return inputs;
        }
        inputs.push(LearningInput {
            source_language: out.result.source.as_str().to_string(),
            target_language: out.result.target.as_str().to_string(),
            source_text: req.text.clone(),
            translated_text: if consent.level.stores_translation() || consent.store_model_output {
                Some(out.result.text.clone())
            } else {
                None
            },
            fallback_reason: reason,
            translation_route: out.diagnostics.route,
            model_identity: out.result.model_id.clone(),
            knowledge_version,
            context_key: out.diagnostics.context_id.clone(),
        });
        return inputs;
    }

    for unit in &out.diagnostics.units {
        let Some(reason) = unit.fallback_reason else {
            continue;
        };
        if !matches!(
            unit.route,
            crate::TranslationRoute::LocalNmt | crate::TranslationRoute::Remote
        ) {
            continue;
        }
        let Some(source) = unit.span.slice(&req.text) else {
            continue;
        };
        if source.trim().is_empty() {
            continue;
        }
        inputs.push(LearningInput {
            source_language: out.result.source.as_str().to_string(),
            target_language: out.result.target.as_str().to_string(),
            source_text: source.to_string(),
            translated_text: None,
            fallback_reason: reason,
            translation_route: unit.route,
            model_identity: None,
            knowledge_version: knowledge_version.clone(),
            context_key: out.diagnostics.context_id.clone(),
        });
    }
    inputs
}

#[cfg(test)]
mod tests {
    use super::*;
    use translator_types::{Capability, FallbackReason, TextKind, TranslationRoute};

    fn out() -> TranslateOutput {
        TranslateOutput {
            result: crate::TranslationResult {
                text: "内存占用".into(),
                source: crate::LanguageTag::new("en").unwrap(),
                target: crate::LanguageTag::new("zh-Hans").unwrap(),
                engine_id: "mock.nmt".into(),
                model_id: Some("m".into()),
                elapsed_ms: 1,
                warnings: Vec::new(),
            },
            diagnostics: crate::TranslationDiagnostics {
                engine_id: "mock.nmt".into(),
                capability: Capability::Nmt,
                route_reason: "test".into(),
                cache_hit: false,
                source_assumed: false,
                rules_version: 1,
                resource_fingerprint: "dict@1".into(),
                mode: crate::TranslationMode::Precise,
                text_kind: TextKind::Sentence,
                route: TranslationRoute::LocalNmt,
                model_invoked: true,
                dictionary_hit: false,
                rule_matched: false,
                fallback_reason: Some(FallbackReason::RuleNotCovered),
                evidence: None,
                units: Vec::new(),
                strategy: "sentence".into(),
                context_id: None,
                quality: None,
                rules: None,
                protected_spans: Vec::new(),
                restore_missing: Vec::new(),
                term: None,
            },
        }
    }

    #[test]
    fn 隐私关闭时不生成学习输入() {
        let req = TranslateRequest::new(
            "memory footprint",
            crate::LanguageTag::new("zh-Hans").unwrap(),
        );
        let inputs = learning_inputs(&req, &out(), LearningConsent::default());
        assert!(inputs.is_empty());
    }

    #[test]
    fn 本地授权只保存源文默认不保存模型译文() {
        let req = TranslateRequest::new(
            "memory footprint",
            crate::LanguageTag::new("zh-Hans").unwrap(),
        );
        let inputs = learning_inputs(&req, &out(), LearningConsent::local());
        assert_eq!(inputs.len(), 1);
        assert!(inputs[0].translated_text.is_none());
    }

    #[test]
    fn 授权语料保存候选译文但仍标记为原始数据() {
        let req = TranslateRequest::new(
            "memory footprint",
            crate::LanguageTag::new("zh-Hans").unwrap(),
        );
        let inputs = learning_inputs(&req, &out(), LearningConsent::authorized_corpus());
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].translated_text.as_deref(), Some("内存占用"));
        assert_eq!(inputs[0].fallback_reason, FallbackReason::RuleNotCovered);
    }
}
