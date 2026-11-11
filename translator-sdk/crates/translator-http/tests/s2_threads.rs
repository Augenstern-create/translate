//! S2 审查修复：线程生命周期。
//!
//! 放在**独立测试二进制**里：只有这一个测试，线程计数不会被同进程的其它测试干扰。

use std::time::{Duration, Instant};

use translator_http::client::HttpRuntime;
use translator_http::usage::UsageMeter;
use translator_http::{live_watcher_threads, live_worker_threads};

fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    condition()
}

#[test]
fn 反复创建销毁运行时不会累积线程() {
    let baseline_watchers = live_watcher_threads();
    let baseline_workers = live_worker_threads();

    for round in 0..10 {
        let runtime = HttpRuntime::new(2, 4, UsageMeter::new());
        assert_eq!(runtime.workers(), 2);

        // 线程真正启动（异步 spawn，等一下计数器）
        assert!(
            wait_until(
                || live_worker_threads() >= baseline_workers + 2,
                Duration::from_secs(2)
            ),
            "第 {round} 轮 worker 未启动：baseline={baseline_workers} now={}",
            live_worker_threads()
        );
        assert!(
            wait_until(
                || live_watcher_threads() > baseline_watchers,
                Duration::from_secs(2)
            ),
            "第 {round} 轮观察线程未启动"
        );

        drop(runtime);

        // 运行时销毁后，worker 与观察线程都必须退出
        assert!(
            wait_until(
                || live_worker_threads() <= baseline_workers
                    && live_watcher_threads() <= baseline_watchers,
                Duration::from_secs(3)
            ),
            "第 {round} 轮线程未退出：workers {}/{} watchers {}/{}",
            live_worker_threads(),
            baseline_workers,
            live_watcher_threads(),
            baseline_watchers
        );
    }

    assert_eq!(
        live_worker_threads(),
        baseline_workers,
        "10 轮之后 worker 线程数不得增长"
    );
    assert_eq!(
        live_watcher_threads(),
        baseline_watchers,
        "10 轮之后观察线程数不得增长（不得自持 Arc）"
    );
}
