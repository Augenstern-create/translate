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
    logical_requests: AtomicU64,
    http_calls_started: AtomicU64,
    http_calls_completed: AtomicU64,
    transport_retries: AtomicU64,
    content_retries: AtomicU64,
    failed_calls: AtomicU64,
    cancelled_requests: AtomicU64,
    unknown_billable_calls: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
}

impl UsageMeter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 一次**用户可见**的逻辑翻译请求。
    pub fn record_logical_request(&self) {
        self.logical_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// 一次物理 HTTP 调用即将发出（预算已扣除、配额已取得）。
    pub fn record_call_started(&self) {
        self.http_calls_started.fetch_add(1, Ordering::Relaxed);
    }

    /// 收到了 HTTP 响应（任何状态码）。
    pub fn record_call_completed(&self) {
        self.http_calls_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_transport_retry(&self) {
        self.transport_retries.fetch_add(1, Ordering::Relaxed);
    }

    /// 协调器因内容完整性失败发起的一次重试。
    pub fn record_content_retry(&self) {
        self.content_retries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failed_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cancelled(&self) {
        self.cancelled_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// 请求已经发出但没有拿到响应：**可能仍会产生费用**，不能当作零成本。
    pub fn record_unknown_billable(&self) {
        self.unknown_billable_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tokens(&self, usage: TokenUsage) {
        self.input_tokens
            .fetch_add(usage.input_tokens, Ordering::Relaxed);
        self.output_tokens
            .fetch_add(usage.output_tokens, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> UsageSnapshot {
        UsageSnapshot {
            logical_requests: self.logical_requests.load(Ordering::Relaxed),
            http_calls_started: self.http_calls_started.load(Ordering::Relaxed),
            http_calls_completed: self.http_calls_completed.load(Ordering::Relaxed),
            transport_retries: self.transport_retries.load(Ordering::Relaxed),
            content_retries: self.content_retries.load(Ordering::Relaxed),
            failed_calls: self.failed_calls.load(Ordering::Relaxed),
            cancelled_requests: self.cancelled_requests.load(Ordering::Relaxed),
            potentially_billable_unknown_calls: self.unknown_billable_calls.load(Ordering::Relaxed),
            tokens: TokenUsage {
                input_tokens: self.input_tokens.load(Ordering::Relaxed),
                output_tokens: self.output_tokens.load(Ordering::Relaxed),
            },
        }
    }

    pub fn reset(&self) {
        for counter in [
            &self.logical_requests,
            &self.http_calls_started,
            &self.http_calls_completed,
            &self.transport_retries,
            &self.content_retries,
            &self.failed_calls,
            &self.cancelled_requests,
            &self.unknown_billable_calls,
            &self.input_tokens,
            &self.output_tokens,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }
}

/// 某一时刻的用量快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageSnapshot {
    /// 逻辑翻译请求数（用户发起次数）。
    pub logical_requests: u64,
    /// 实际发出的物理 HTTP 调用数（含重试）。
    pub http_calls_started: u64,
    /// 收到响应的调用数（任何状态码）。
    pub http_calls_completed: u64,
    /// 传输层重试次数。
    pub transport_retries: u64,
    /// 内容完整性重试次数（由协调器上报）。
    pub content_retries: u64,
    /// 未拿到响应的调用数。
    pub failed_calls: u64,
    /// 被取消的请求数。
    pub cancelled_requests: u64,
    /// 已发出但没有可靠响应的调用数——**可能仍然计费**，不能算作零成本。
    pub potentially_billable_unknown_calls: u64,
    pub tokens: TokenUsage,
}

impl UsageSnapshot {
    /// 按单价估算费用（美元）。单价缺失时返回 `None`：不要编一个默认价格。
    ///
    /// 注意：这是**估算**。服务端可能有缓存命中价、分时价或其它计费规则，
    /// 最终账单以转发站为准。
    pub fn estimate_cost_usd(&self, pricing: Option<Pricing>) -> Option<f64> {
        let pricing = pricing?;
        let input = self.tokens.input_tokens as f64 / 1_000_000.0 * pricing.input_per_million;
        let output = self.tokens.output_tokens as f64 / 1_000_000.0 * pricing.output_per_million;
        Some(input + output)
    }

    /// 一行摘要（不含文本）。
    pub fn summary(&self) -> String {
        format!(
            "请求 {}（HTTP {}，完成 {}，失败 {}，传输重试 {}，内容重试 {}，取消 {}，疑似计费未知 {}）；token {}/{}",
            self.logical_requests,
            self.http_calls_started,
            self.http_calls_completed,
            self.failed_calls,
            self.transport_retries,
            self.content_retries,
            self.cancelled_requests,
            self.potentially_billable_unknown_calls,
            self.tokens.input_tokens,
            self.tokens.output_tokens
        )
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
    fn 累计器区分逻辑请求与物理调用() {
        let meter = UsageMeter::new();
        meter.record_logical_request();
        meter.record_call_started();
        meter.record_call_completed();
        meter.record_transport_retry();
        meter.record_content_retry();
        meter.record_unknown_billable();
        meter.record_tokens(TokenUsage {
            input_tokens: 100,
            output_tokens: 40,
        });

        let snapshot = meter.snapshot();
        assert_eq!(snapshot.logical_requests, 1);
        assert_eq!(snapshot.http_calls_started, 1);
        assert_eq!(snapshot.http_calls_completed, 1);
        assert_eq!(snapshot.transport_retries, 1);
        assert_eq!(snapshot.content_retries, 1);
        assert_eq!(snapshot.potentially_billable_unknown_calls, 1);
        assert_eq!(snapshot.tokens.total(), 140);
        assert!(snapshot.summary().contains("疑似计费未知 1"));

        meter.reset();
        assert_eq!(meter.snapshot(), UsageSnapshot::default());
    }

    #[test]
    fn 未配置价格时不估算费用() {
        let snapshot = UsageSnapshot {
            tokens: TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            },
            ..UsageSnapshot::default()
        };
        assert_eq!(snapshot.estimate_cost_usd(None), None);

        let pricing = Pricing::from_option(Some(2.0), Some(8.0)).unwrap();
        let cost = snapshot.estimate_cost_usd(Some(pricing)).unwrap();
        assert!((cost - 10.0).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn 只有单边价格时视为未配置() {
        assert!(Pricing::from_option(Some(1.0), None).is_none());
        assert!(Pricing::from_option(None, Some(1.0)).is_none());
    }
}
