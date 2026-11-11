//! 并发闸门（全局 + 每后端），带截止时间与取消感知。
//!
//! 旧实现每个后端一个 `Limiter`，配置里的"全局上限"其实只是"每个后端各自的上限"。
//! 现在同一 `Translator` 内共享一个 [`Limiter`] 作为全局闸门，每个后端各有一个，
//! 一次请求必须**同时**拿到两者。
//!
//! # 为什么不用 async 信号量
//!
//! SDK 没有 async runtime：等待发生在 worker 线程里，阻塞是允许的。
//! 但等待必须：可取消、受截止时间约束、不泄漏配额。

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use translator_types::CancelToken;

/// 等待配额失败的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcquireError {
    /// 等待期间被取消。
    Cancelled,
    /// 等待期间到达截止时间。
    Timeout,
}

/// 闸门：最多 `limit` 个并发。
#[derive(Debug)]
pub struct Limiter {
    limit: usize,
    active: Mutex<usize>,
    released: Condvar,
}

impl Limiter {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit: limit.max(1),
            active: Mutex::new(0),
            released: Condvar::new(),
        })
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn active(&self) -> usize {
        *self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 获取一个名额。
    ///
    /// 等待策略：每 25ms 醒一次检查取消与截止时间。取消令牌没有唤醒机制
    /// （它是纯原子标志），因此只能用有界等待；这在 worker 线程里是可接受的。
    pub fn acquire(
        self: &Arc<Self>,
        deadline: Option<Instant>,
        cancel: Option<&CancelToken>,
    ) -> Result<Permit, AcquireError> {
        if Self::cancelled(cancel) {
            return Err(AcquireError::Cancelled);
        }
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if *active < self.limit {
                *active += 1;
                // 拿到配额后还要再确认一次：等待期间可能已经超时/取消
                drop(active);
                if Self::cancelled(cancel) {
                    self.release();
                    return Err(AcquireError::Cancelled);
                }
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    self.release();
                    return Err(AcquireError::Timeout);
                }
                return Ok(Permit {
                    limiter: Arc::clone(self),
                });
            }

            let wait = Self::next_wait(deadline);
            active = self
                .released
                .wait_timeout(active, wait)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|e| e.into_inner().0);

            if Self::cancelled(cancel) {
                return Err(AcquireError::Cancelled);
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(AcquireError::Timeout);
            }
        }
    }

    fn cancelled(cancel: Option<&CancelToken>) -> bool {
        cancel.is_some_and(CancelToken::is_cancelled)
    }

    /// 下一次等待时长：默认 25ms，且不超过剩余时间。
    fn next_wait(deadline: Option<Instant>) -> Duration {
        let slice = Duration::from_millis(25);
        match deadline {
            None => slice,
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    // 已到期：立即返回，让调用方转成 Timeout
                    Duration::from_millis(1)
                } else {
                    remaining.min(slice)
                }
            }
        }
    }

    fn release(&self) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        *active = active.saturating_sub(1);
        self.released.notify_one();
    }
}

/// 配额凭证；`Drop` 时自动归还，任何返回路径都不会泄漏。
pub struct Permit {
    limiter: Arc<Limiter>,
}

impl Permit {
    pub fn limiter(&self) -> &Arc<Limiter> {
        &self.limiter
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.limiter.release();
    }
}

impl std::fmt::Debug for Permit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Permit")
            .field("limit", &self.limiter.limit())
            .finish()
    }
}

/// 同时持有全局与后端配额。
#[derive(Debug)]
pub struct DualPermit {
    _global: Permit,
    _backend: Permit,
}

/// 按固定顺序（先全局、后后端）获取两个配额。
///
/// 顺序固定 ⇒ 不会出现"两个线程各持一个、互相等待"的死锁。
/// 等待后端配额时会暂时占用全局配额，但等待本身受同一个截止时间约束。
pub fn acquire_dual(
    global: &Arc<Limiter>,
    backend: &Arc<Limiter>,
    deadline: Option<Instant>,
    cancel: Option<&CancelToken>,
) -> Result<DualPermit, AcquireError> {
    let global_permit = global.acquire(deadline, cancel)?;
    match backend.acquire(deadline, cancel) {
        Ok(backend_permit) => Ok(DualPermit {
            _global: global_permit,
            _backend: backend_permit,
        }),
        Err(err) => Err(err), // global_permit 在这里被释放
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn 并发不超过上限() {
        let limiter = Limiter::new(2);
        let peak = Arc::new(AtomicUsize::new(0));
        let current = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let limiter = Arc::clone(&limiter);
            let peak = Arc::clone(&peak);
            let current = Arc::clone(&current);
            handles.push(std::thread::spawn(move || {
                let _permit = limiter.acquire(None, None).expect("应拿到配额");
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                current.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2, "并发必须被压到上限");
        assert_eq!(limiter.active(), 0, "配额必须全部归还");
    }

    #[test]
    fn 取消可以打断等待() {
        let limiter = Limiter::new(1);
        let held = limiter.acquire(None, None).expect("先占用唯一配额");

        let token = CancelToken::new();
        let token_for_thread = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            token_for_thread.cancel();
        });

        let started = Instant::now();
        let err = limiter.acquire(None, Some(&token)).expect_err("应被取消");
        assert_eq!(err, AcquireError::Cancelled);
        assert!(started.elapsed() < Duration::from_millis(800));
        drop(held);
        assert_eq!(limiter.active(), 0);
    }

    #[test]
    fn 截止时间可以打断等待() {
        let limiter = Limiter::new(1);
        let _held = limiter.acquire(None, None).expect("占用配额");

        let deadline = Instant::now() + Duration::from_millis(120);
        let started = Instant::now();
        let err = limiter.acquire(Some(deadline), None).expect_err("应超时");
        assert_eq!(err, AcquireError::Timeout);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn 双配额获取失败不会泄漏全局配额() {
        let global = Limiter::new(4);
        let backend = Limiter::new(1);
        let _held_backend = backend.acquire(None, None).unwrap();

        let deadline = Instant::now() + Duration::from_millis(80);
        let err = acquire_dual(&global, &backend, Some(deadline), None).unwrap_err();
        assert_eq!(err, AcquireError::Timeout);
        assert_eq!(global.active(), 0, "失败路径必须归还全局配额");
        assert_eq!(backend.active(), 1, "已占用的后端配额不受影响");
    }
}
