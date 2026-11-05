//! 内存预算记账（P2.3）。
//!
//! 对应设计文档 §9.3「资源预算」与 `TranslationErrorCode::OutOfBudget`。
//!
//! # 这里**不测**真实 RSS
//!
//! 预算记账用的是**各组件自报的下界估计**，不是进程实际内存。原因有三：
//!
//! 1. 真正的 RSS 需要读 `/proc/self/statm`（Linux）或 `GetProcessMemoryInfo`
//!    （Windows），跨平台且不可移植；`translator-core` 不允许引入平台依赖。
//! 2. 分布式的内存归属本来就算不清：SQLite 的页缓存会随查询增长、mmap 的页
//!    在压力下才被换入、分配器的碎片不归任何人。
//! 3. 估低了比估高更危险，因此每个自报值都必须在自己的文档注释里写明
//!    「这是下界，实测见某处」——不允许把估计值当成保证值（§9.3 明确禁止
//!    把预算说成「已验证」）。
//!
//! 结论：预算的作用是**在装配期尽早失败**（`OutOfBudget`），而不是运行时限流。
//! 运行时峰值仍需宿主用系统手段监控，见 `docs/04` §6 与 `docs/07` §2.3。
//!
//! # 用法
//!
//! ```text
//! let budget = Arc::new(MemoryBudget::new(Some(32 * 1024 * 1024)));
//! let _held = budget.try_reserve("dict:zh-en", 8 * 1024 * 1024)?; // 超出则 Err
//! // _held 离开作用域时自动归还
//! ```

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use translator_types::{TranslationError, TranslationErrorCode};

/// 一次记账失败。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetError {
    /// 申请会让已用量超过上限。
    OutOfBudget {
        label: String,
        requested: u64,
        used: u64,
        limit: u64,
    },
    /// 申请量本身不合法（0 字节没有意义，多半是估计函数写错了）。
    ZeroRequest { label: String },
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBudget {
                label,
                requested,
                used,
                limit,
            } => write!(
                f,
                "内存预算不足：{label} 需要 {} MiB，已用 {} MiB / 上限 {} MiB",
                mib(*requested),
                mib(*used),
                mib(*limit)
            ),
            Self::ZeroRequest { label } => {
                write!(f, "内存预算申请为 0 字节：{label}")
            }
        }
    }
}

impl std::error::Error for BudgetError {}

/// 预算不足直接映射成公开错误码 `OutOfBudget`（设计文档 §4 已固定该码）。
///
/// 这条转换是"预算"与"对外 API"之间唯一的耦合点：`translator-core` 之上的
/// 组装器不需要自己判断错误码。
impl From<BudgetError> for TranslationError {
    fn from(e: BudgetError) -> Self {
        TranslationError::new(TranslationErrorCode::OutOfBudget, e.to_string())
    }
}

fn mib(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

/// 共享的内存预算池。
///
/// `Clone` 不实现：预算必须唯一——否则两个副本各自记账就等于没预算。
/// 需要共享时用 `Arc<MemoryBudget>`。
pub struct MemoryBudget {
    /// `None` 表示不限量：仍然记账，只是不拒绝。
    limit: Option<u64>,
    used: AtomicU64,
    /// 明细：label → 字节数。仅用于 `report()` 展示，不参与判定。
    ///
    /// 用 `Mutex` 而不是原子累加，是因为需要按键聚合（同一个 label 可能被
    /// 申请多次，例如同一种资源开了两份）。单个资源的预留是装配期动作，
    /// 不在热路径上，锁开销可以忽略。
    entries: Mutex<Vec<(String, u64)>>,
}

impl MemoryBudget {
    pub fn new(limit: Option<u64>) -> Self {
        Self {
            limit,
            used: AtomicU64::new(0),
            entries: Mutex::new(Vec::new()),
        }
    }

    /// 不限量预算：只记账、不拒绝。用于「宿主还没想好上限」的场景。
    pub fn unlimited() -> Self {
        Self::new(None)
    }

    pub fn limit(&self) -> Option<u64> {
        self.limit
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    /// 还能再申请多少字节。不限量时为 `None`。
    pub fn available(&self) -> Option<u64> {
        self.limit.map(|limit| limit.saturating_sub(self.used()))
    }

    /// 申请 `bytes` 字节。成功返回的 [`BudgetReservation`] 在 drop 时自动归还。
    ///
    /// 用 CAS 循环而不是 `fetch_add` + 事后检查：后者会短暂越过上限，
    /// 在多线程装配时可能让两个资源都以为自己拿到了预算。
    pub fn try_reserve(
        self: &Arc<Self>,
        label: impl Into<String>,
        bytes: u64,
    ) -> Result<BudgetReservation, BudgetError> {
        let label = label.into();
        if bytes == 0 {
            return Err(BudgetError::ZeroRequest { label });
        }

        let mut current = self.used.load(Ordering::Acquire);
        loop {
            if let Some(limit) = self.limit {
                let next = current.saturating_add(bytes);
                if next > limit {
                    return Err(BudgetError::OutOfBudget {
                        label,
                        requested: bytes,
                        used: current,
                        limit,
                    });
                }
            }
            match self.used.compare_exchange_weak(
                current,
                current.saturating_add(bytes),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }

        if let Ok(mut entries) = self.entries.lock() {
            entries.push((label.clone(), bytes));
        }

        Ok(BudgetReservation {
            budget: Arc::clone(self),
            bytes,
            label,
        })
    }

    /// 明细快照，按申请顺序。用于 `Translator::memory_report()`。
    pub fn report(&self) -> MemoryReport {
        let entries = self.entries.lock().map(|e| e.clone()).unwrap_or_default();
        MemoryReport {
            limit_bytes: self.limit,
            used_bytes: self.used(),
            entries,
        }
    }

    fn release(&self, label: &str, bytes: u64) {
        // 先还账再删明细：反过来的话 report() 可能看到"明细里没有、账上还在"的一瞬
        self.used.fetch_sub(bytes, Ordering::AcqRel);
        if let Ok(mut entries) = self.entries.lock() {
            if let Some(pos) = entries.iter().position(|(l, b)| l == label && *b == bytes) {
                entries.remove(pos);
            }
        }
    }
}

impl fmt::Debug for MemoryBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryBudget")
            .field("limit", &self.limit)
            .field("used", &self.used())
            .finish()
    }
}

/// 一次成功记账的凭据。drop 即归还。
///
/// 刻意不实现 `Clone`：预算凭据能被复制就意味着能被多算。
pub struct BudgetReservation {
    budget: Arc<MemoryBudget>,
    bytes: u64,
    label: String,
}

impl BudgetReservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// 显式提前归还（等价于 drop，但意图更清楚）。
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for BudgetReservation {
    fn drop(&mut self) {
        self.budget.release(&self.label, self.bytes);
    }
}

impl fmt::Debug for BudgetReservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BudgetReservation")
            .field("label", &self.label)
            .field("bytes", &self.bytes)
            .finish()
    }
}

/// 预算快照。
#[derive(Clone, Debug, Default)]
pub struct MemoryReport {
    pub limit_bytes: Option<u64>,
    pub used_bytes: u64,
    /// `(label, bytes)`，按申请顺序。
    pub entries: Vec<(String, u64)>,
}

impl MemoryReport {
    pub fn available_bytes(&self) -> Option<u64> {
        self.limit_bytes
            .map(|limit| limit.saturating_sub(self.used_bytes))
    }

    /// 人类可读摘要，CLI 与日志直接用。
    pub fn summary(&self) -> String {
        let mut out = match self.limit_bytes {
            Some(limit) => format!(
                "已用 {} MiB / 上限 {} MiB",
                mib(self.used_bytes),
                mib(limit)
            ),
            None => format!("已用 {} MiB（无上限）", mib(self.used_bytes)),
        };
        if self.entries.is_empty() {
            out.push_str("；无占用项");
        } else {
            out.push('；');
            let items: Vec<String> = self
                .entries
                .iter()
                .map(|(label, bytes)| format!("{label}={} MiB", mib(*bytes)))
                .collect();
            out.push_str(&items.join(", "));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn 预留与释放在账面上平衡() {
        let budget = Arc::new(MemoryBudget::new(Some(16 * MIB)));
        {
            let a = budget.try_reserve("a", 4 * MIB).unwrap();
            assert_eq!(a.bytes(), 4 * MIB);
            assert_eq!(budget.used(), 4 * MIB);
        }
        assert_eq!(budget.used(), 0, "drop 后必须归还");
        assert!(budget.report().entries.is_empty());
    }

    #[test]
    fn 超出上限时拒绝且不记账() {
        let budget = Arc::new(MemoryBudget::new(Some(8 * MIB)));
        let _held = budget.try_reserve("a", 6 * MIB).unwrap();
        let err = budget.try_reserve("b", 4 * MIB).unwrap_err();
        match err {
            BudgetError::OutOfBudget {
                requested,
                used,
                limit,
                ..
            } => {
                assert_eq!(requested, 4 * MIB);
                assert_eq!(used, 6 * MIB);
                assert_eq!(limit, 8 * MIB);
            }
            other => panic!("期望 OutOfBudget，实际 {other:?}"),
        }
        assert_eq!(budget.used(), 6 * MIB, "失败不得留下半笔记账");
    }

    #[test]
    fn 不限量时只记账不拒绝() {
        let budget = Arc::new(MemoryBudget::unlimited());
        let _a = budget.try_reserve("a", 100 * MIB).unwrap();
        let _b = budget.try_reserve("b", 100 * MIB).unwrap();
        assert_eq!(budget.used(), 200 * MIB);
        assert_eq!(budget.available(), None);
        assert!(budget.report().summary().contains("无上限"));
    }

    #[test]
    fn 零字节申请被拒绝() {
        let budget = Arc::new(MemoryBudget::new(Some(MIB)));
        assert!(matches!(
            budget.try_reserve("empty", 0),
            Err(BudgetError::ZeroRequest { .. })
        ));
    }

    #[test]
    fn 并发申请不会突破上限() {
        use std::sync::atomic::AtomicUsize;

        let budget = Arc::new(MemoryBudget::new(Some(10 * MIB)));
        let ok = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for i in 0..16 {
            let budget = Arc::clone(&budget);
            let ok = Arc::clone(&ok);
            handles.push(std::thread::spawn(move || {
                if let Ok(held) = budget.try_reserve(format!("t{i}"), MIB) {
                    ok.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    drop(held);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 上限 10 MiB / 每次 1 MiB：成功的必然不超过 10 个
        assert!(
            ok.load(Ordering::SeqCst) <= 10,
            "成功 {} 次，超过上限允许的 10 次",
            ok.load(Ordering::SeqCst)
        );
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn 同名预留各自独立归还() {
        let budget = Arc::new(MemoryBudget::new(Some(64 * MIB)));
        let a = budget.try_reserve("dict", 4 * MIB).unwrap();
        let b = budget.try_reserve("dict", 8 * MIB).unwrap();
        assert_eq!(budget.used(), 12 * MIB);
        drop(a);
        assert_eq!(budget.used(), 8 * MIB, "只归还自己那一笔");
        drop(b);
        assert_eq!(budget.used(), 0);
    }
}
