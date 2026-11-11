//! 用量与成本统计。
//!
//! # 为什么客户端统计**不能**当作计费依据（§12.2）
//!
//! 重试、取消、响应中断都可能导致"已经产生可计费输出、但客户端没有收到响应"。
//! 因此这里的数字只用于：
//!
//! - 用户界面上的用量提示；
//! - 单请求/单会话的成本估算与预算告警；
//! - 评测时比较不同后端的开销。
//!
//! 真正的硬性额度必须由转发站/服务端执行。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use translator_types::TokenUsage;

/// 线程安全的用量累计器。
#[derive(Debug, Default)]
pub struct UsageMeter {
    requests: AtomicU64,
    failures: AtomicU64,
    retries: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    cancelled: AtomicU64,
}

impl UsageMeter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cancelled(&self) {
        self.cancelled.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tokens(&self, usage: TokenUsage) {
        self.input_tokens
            .fetch_add(usage.input_tokens, Ordering::Relaxed);
        self.output_tokens
            .fetch_add(usage.output_tokens, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> UsageSnapshot {
        UsageSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            tokens: TokenUsage {
                input_tokens: self.input_tokens.load(Ordering::Relaxed),
                output_tokens: self.output_tokens.load(Ordering::Relaxed),
            },
        }
    }

    pub fn reset(&self) {
        for counter in [
            &self.requests,
            &self.failures,
            &self.retries,
            &self.input_tokens,
            &self.output_tokens,
            &self.cancelled,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }
}

/// 某一时刻的用量快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageSnapshot {
    /// 发起的翻译请求数（含失败与取消）。
    pub requests: u64,
    pub failures: u64,
    /// 传输层重试次数（不含首次）。
    pub retries: u64,
    pub cancelled: u64,
    pub tokens: TokenUsage,
}

impl UsageSnapshot {
    /// 按单价估算费用（美元）。单价缺失时返回 `None`：不要编一个默认价格。
    pub fn estimate_cost_usd(&self, pricing: Option<Pricing>) -> Option<f64> {
        let pricing = pricing?;
        let input = self.tokens.input_tokens as f64 / 1_000_000.0 * pricing.input_per_million;
        let output = self.tokens.output_tokens as f64 / 1_000_000.0 * pricing.output_per_million;
        Some(input + output)
    }
}

/// 单价（美元 / 百万 token）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

impl Pricing {
    /// 从配置的可选价格构造；任一缺失即视为"未配置价格"。
    pub fn from_option(input: Option<f64>, output: Option<f64>) -> Option<Self> {
        Some(Self {
            input_per_million: input?,
            output_per_million: output?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 累计器记录请求与令牌() {
        let meter = UsageMeter::new();
        meter.record_request();
        meter.record_request();
        meter.record_retry();
        meter.record_failure();
        meter.record_tokens(TokenUsage {
            input_tokens: 100,
            output_tokens: 40,
        });

        let snap = meter.snapshot();
        assert_eq!(snap.requests, 2);
        assert_eq!(snap.retries, 1);
        assert_eq!(snap.failures, 1);
        assert_eq!(snap.tokens.total(), 140);

        meter.reset();
        assert_eq!(meter.snapshot(), UsageSnapshot::default());
    }

    #[test]
    fn 未配置价格时不估算费用() {
        let snap = UsageSnapshot {
            tokens: TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            },
            ..UsageSnapshot::default()
        };
        assert_eq!(snap.estimate_cost_usd(None), None);

        let pricing = Pricing::from_option(Some(2.0), Some(8.0)).unwrap();
        let cost = snap.estimate_cost_usd(Some(pricing)).unwrap();
        assert!((cost - 10.0).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn 只有单边价格时视为未配置() {
        assert!(Pricing::from_option(Some(1.0), None).is_none());
        assert!(Pricing::from_option(None, Some(1.0)).is_none());
    }
}
