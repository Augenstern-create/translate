//! `RuntimeTransport`：宿主看到的"翻译后端"接口。
//!
//! 这一层是 **P3.1 之后所有后端的分界线**：Bergamot 走进程 IPC，将来 ncnn/ORT 可以是
//! 另一种实现，`translator-api` 完全不需要知道区别（`docs/12` §3.1）。
//!
//! 两个刻意的设计：
//!
//! 1. **模型实例独占**：内部只有一把锁、一个 worker、一次处理一个请求
//!    （默认并发 1 是 P3.0 实测的结论：2 线程不降延迟、内存翻倍）。
//! 2. **失败不静默重试**：请求失败就失败；只有"下一个**新**请求"才允许在预算内起新 worker。
//!    这条是为了不把一次崩溃变成 N 次不可见的重放。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use std::path::Path;

use crate::error::RuntimeError;
use crate::model::{
    process_rss_bytes, ModelInfo, ModelSpec, TranslationJob, TranslationReply, TransportMetrics,
    WorkerHealth,
};
use crate::protocol::{Payload, Request};
use crate::snapshot::ModelSnapshot;
use crate::worker::{WorkerConfig, WorkerProcess};
use translator_types::CancelToken;

/// 后端能力契约。实现方可以是进程、线程池、或未来的 FFI 边界。
pub trait RuntimeTransport: Send + Sync {
    /// 加载模型。已加载同一 `model_revision` 时可幂等返回。
    fn load(&self, spec: &ModelSpec) -> Result<ModelInfo, RuntimeError>;
    /// 翻译一句。**不得**在内部自动换后端（例如偷偷走远程）。
    fn translate(&self, job: &TranslationJob) -> Result<TranslationReply, RuntimeError>;
    /// 卸载模型，保留 worker 进程。
    fn unload(&self) -> Result<(), RuntimeError>;
    /// 心跳。
    fn health(&self) -> Result<WorkerHealth, RuntimeError>;
    /// 关闭并回收 worker。幂等。
    fn shutdown(&self) -> Result<(), RuntimeError>;
    /// 指标快照（宿主 / worker 分开）。
    fn metrics(&self) -> TransportMetrics;
}

struct State {
    worker: Option<WorkerProcess>,
    loaded: Option<ModelInfo>,
    /// 已加载模型对应的隔离快照。必须活到 unload/shutdown，
    /// 否则目录被删、native 的路径指向空（Linux 上已打开的 fd 还在，但语义已经不成立）。
    snapshot: Option<ModelSnapshot>,
    model_preparation_ms: Option<u64>,
    snapshot_bytes: Option<u64>,
    spawns: u32,
    respawns: u32,
    next_epoch: u64,
    /// 本次加载前清理/保留的崩溃残留快照数（`docs/17` §4.1.6）。
    stale_snapshots_removed: u32,
    stale_snapshots_kept: u32,
}

/// 基于子进程的传输实现。
pub struct ProcessTransport {
    config: WorkerConfig,
    restart_budget: u32,
    state: Mutex<State>,
    shutdown_requested: AtomicU32,
    /// single-flight 闸门：加载是一个"昂贵且不可并行"的动作，
    /// 只有拿到闸门的调用者才会真正加载，其余人在闸门外排队并在进入后复用结果。
    load_gate: Mutex<()>,
    /// 进行中的翻译请求数（租约的可观测部分；`docs/17` LC-02）。
    active_requests: AtomicU32,
    /// 残留回收只做一次（每个传输层一次，避免每次加载都扫目录）。
    swept: AtomicBool,
}

impl ProcessTransport {
    /// 创建一个传输层（**不**立即 spawn：第一次 `load` 时才起进程）。
    pub fn new(config: WorkerConfig, restart_budget: u32) -> Self {
        Self {
            config,
            restart_budget,
            state: Mutex::new(State {
                worker: None,
                loaded: None,
                snapshot: None,
                model_preparation_ms: None,
                snapshot_bytes: None,
                spawns: 0,
                respawns: 0,
                next_epoch: 1,
                stale_snapshots_removed: 0,
                stale_snapshots_kept: 0,
            }),
            shutdown_requested: AtomicU32::new(0),
            load_gate: Mutex::new(()),
            active_requests: AtomicU32::new(0),
            swept: AtomicBool::new(false),
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 带截止时间地拿状态锁：**排队不能无限等**（`docs/17` LC-04）。
    fn guard_until(
        &self,
        deadline: Option<Instant>,
        cancel: Option<&CancelToken>,
    ) -> Result<std::sync::MutexGuard<'_, State>, RuntimeError> {
        let started = Instant::now();
        // 退避从 20 µs 起翻倍到 500 µs 封顶：
        // 固定 1 ms 轮询在 10 个并发客户端时会把吞吐拖掉一半以上（`docs/20` §2.6 实测），
        // 而纯 `yield_now` 自旋又会烧 CPU。指数退避是这两者之间的折中，且不变更
        // "排队到期返回 Timeout"的语义。
        let mut backoff = Duration::from_micros(20);
        loop {
            // 排队中取消：**不 spawn、不 kill**，直接返回；锁从未拿到。
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(RuntimeError::Cancelled);
            }
            match self.state.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    return Ok(poisoned.into_inner());
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    if let Some(deadline) = deadline {
                        if Instant::now() >= deadline {
                            return Err(RuntimeError::Timeout {
                                operation: "queue",
                                waited_ms: started.elapsed().as_millis() as u64,
                            });
                        }
                    }
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_micros(500));
                }
            }
        }
    }

    /// 当前是否有请求正在执行（租约观测，`docs/17` LC-02）。
    pub fn active_requests(&self) -> u32 {
        self.active_requests.load(Ordering::SeqCst)
    }

    /// 确保有一个活着的 worker；必要时在预算内重启。
    fn ensure_worker(&self, state: &mut State) -> Result<(), RuntimeError> {
        // 平台门禁见 `platform` 模块：判定表可以单测，不再依赖 cfg 分支（P3.4）
        crate::platform::ensure_supported()?;
        if self.shutdown_requested.load(Ordering::SeqCst) != 0 {
            return Err(RuntimeError::CapabilityUnavailable(
                "传输层已 shutdown，不能再用".into(),
            ));
        }
        if let Some(worker) = state.worker.as_ref() {
            if !worker.is_dead() {
                return Ok(());
            }
        }
        if state.worker.is_some() {
            // 死掉的 worker：先回收（Drop 会走终止流程），不留僵尸
            state.worker = None;
            state.loaded = None;
            state.snapshot = None;
            if state.respawns >= self.restart_budget {
                return Err(RuntimeError::CapabilityUnavailable(format!(
                    "worker 重启预算耗尽（{} 次），不再自动重启",
                    self.restart_budget
                )));
            }
            state.respawns += 1;
        }
        let epoch = state.next_epoch;
        state.next_epoch += 1;
        let config = WorkerConfig {
            epoch,
            ..self.config.clone()
        };
        let worker = WorkerProcess::spawn(config)?;
        state.worker = Some(worker);
        state.spawns += 1;
        Ok(())
    }

    /// 当前**活着的** worker 的 pid（没有或已死则为 None）。
    ///
    /// 已经死掉的进程不算"有 pid"：句柄还在只是为了下一次 `ensure_worker` 能把它回收掉。
    pub fn worker_pid(&self) -> Option<u32> {
        self.guard()
            .worker
            .as_ref()
            .filter(|w| !w.is_dead())
            .map(|w| w.pid())
    }

    /// worker 的 stderr 尾部（诊断用）。
    pub fn worker_stderr(&self) -> Vec<String> {
        self.guard()
            .worker
            .as_ref()
            .map(|w| w.stderr_tail())
            .unwrap_or_default()
    }
}

impl ProcessTransport {
    /// 从**清单**加载：先生成隔离快照并校验，再把快照路径交给 worker。
    ///
    /// 这是 P3.1-B 的真实入口（`docs/14` §3.5）。`RuntimeTransport::load` 仍然可用，
    /// 但它只做路径级检查、假定调用方已经准备好了可信路径——那是给 mock 与测试用的。
    ///
    /// `snapshot_root` 需要是可写目录；快照在 `unload`/`shutdown`/传输层 Drop 时删除，
    /// 被 SIGKILL 的残留由 [`crate::snapshot::prune_stale_snapshots`] 按 pid 清理。
    pub fn load_from_manifest(
        &self,
        manifest: &crate::model::ModelManifest,
        snapshot_root: &Path,
        threads: u16,
        beam_size: u8,
    ) -> Result<ModelInfo, RuntimeError> {
        // single-flight：同一个传输层上并发调用只会真正加载一次（`docs/17` LC-01）。
        // 闸门只在"加载"这一件事上互斥；翻译请求不会拿到它。
        let _gate = self.load_gate.lock().unwrap_or_else(|e| e.into_inner());
        let expected_revision = manifest.content_digest();
        // 注意：scrutinee 里的 `guard()` 临时量会活到整个 if-let 结束，
        // 在体内再取锁就是自死锁（std::sync::Mutex 不可重入）。
        let cached = self.guard().loaded.clone();
        if let Some(info) = cached {
            let worker_alive = self.guard().worker.as_ref().is_some_and(|w| !w.is_dead());
            if info.model_revision == expected_revision && worker_alive {
                return Ok(info);
            }
        }

        let started = Instant::now();
        // 崩溃残留兜底：先清掉"持有者已死"的快照目录再生成新的。
        // 每个传输层只做一次（正常路径上目录数是 0，代价可以忽略）。
        if !self.swept.swap(true, Ordering::Relaxed) {
            if let Ok(report) = crate::reclaim::sweep_snapshots(snapshot_root) {
                let mut state = self.guard();
                state.stale_snapshots_removed = report.removed as u32;
                state.stale_snapshots_kept = (report.kept_alive + report.unverifiable) as u32;
            }
        }
        let snapshot = ModelSnapshot::materialize(manifest, snapshot_root, threads, beam_size)?;
        let prepare_ms = started.elapsed().as_millis() as u64;
        let bytes = snapshot.bytes();
        let spec = snapshot.spec().clone();

        {
            let mut state = self.guard();
            state.snapshot = Some(snapshot);
            state.model_preparation_ms = Some(prepare_ms);
            state.snapshot_bytes = Some(bytes);
        }
        self.load(&spec)
    }

    /// 当前是否持有一个模型快照（诊断用）。
    pub fn has_snapshot(&self) -> bool {
        self.guard().snapshot.is_some()
    }

    /// 当前快照对应的 `ModelSpec`（含每个文件的期望摘要）。诊断/取证与测试用。
    pub fn snapshot_spec(&self) -> Option<ModelSpec> {
        self.guard()
            .snapshot
            .as_ref()
            .map(|snap| snap.spec().clone())
    }

    /// 当前快照里**权重**的路径（诊断/取证用；不对外暴露给 `translator-api`）。
    pub fn snapshot_model_path(&self) -> Option<std::path::PathBuf> {
        self.guard()
            .snapshot
            .as_ref()
            .and_then(|snap| snap.spec().path_of(crate::model::FileRole::Model))
            .map(std::path::PathBuf::from)
    }
}

impl ProcessTransport {
    /// 带**绝对截止时间**的翻译：排队等待与执行共用同一个预算。
    ///
    /// 与 [`RuntimeTransport::translate`] 的区别只有一处：拿不到锁时会按时返回
    /// `Timeout{operation:"queue"}`，而不是无限等（`docs/17` LC-04）。
    pub fn translate_with_deadline(
        &self,
        job: &TranslationJob,
        deadline: Instant,
    ) -> Result<TranslationReply, RuntimeError> {
        self.translate_with_deadline_and_cancel(job, deadline, None)
    }

    /// 可取消版本：排队阶段观察令牌；拿到 worker 后把令牌交给请求等待循环，
    /// 到点终止独占 worker（含进程组），返回 `Cancelled`。
    pub fn translate_with_deadline_and_cancel(
        &self,
        job: &TranslationJob,
        deadline: Instant,
        cancel: Option<&CancelToken>,
    ) -> Result<TranslationReply, RuntimeError> {
        let mut state = self.guard_until(Some(deadline), cancel)?;
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(RuntimeError::Cancelled);
        }
        self.ensure_worker(&mut state)?;
        let Some(loaded) = state.loaded.clone() else {
            return Err(RuntimeError::InitFailed(
                "尚未加载模型（worker 可能已重启，需要重新 load）".into(),
            ));
        };
        let worker = state
            .worker
            .as_mut()
            .ok_or_else(|| RuntimeError::InitFailed("worker 未就绪".into()))?;
        if Instant::now() >= deadline {
            return Err(RuntimeError::Timeout {
                operation: "queue",
                waited_ms: 0,
            });
        }
        self.active_requests.fetch_add(1, Ordering::SeqCst);
        let remaining = deadline.saturating_duration_since(Instant::now());
        let outcome = worker.request_with_timeout_and_cancel(
            Request::Translate {
                text: job.text.clone(),
                target: job.target.clone(),
            },
            remaining,
            cancel,
        );
        self.active_requests.fetch_sub(1, Ordering::SeqCst);
        let payload = outcome?;
        let Payload::Translated { text, elapsed_ms } = payload else {
            return Err(RuntimeError::Protocol(
                "Translate 返回了非 Translated 载荷".into(),
            ));
        };
        Ok(TranslationReply {
            text,
            worker_elapsed_ms: elapsed_ms,
            worker_epoch: worker.epoch(),
            model_revision: loaded.model_revision,
        })
    }
}

impl RuntimeTransport for ProcessTransport {
    fn load(&self, spec: &ModelSpec) -> Result<ModelInfo, RuntimeError> {
        // 顺序很重要：**先校验，后 spawn**。P3.0 的教训是缺文件会让 native abort()。
        spec.verify_paths()?;

        let mut state = self.guard();
        // 先确保 worker 活着：worker 死过一次之后，缓存的 ModelInfo 已经作废
        // （它描述的是上一个进程里的模型），必须先让 ensure_worker 清掉缓存再判断。
        self.ensure_worker(&mut state)?;
        if let Some(loaded) = state.loaded.as_ref() {
            if loaded.worker_epoch == state.worker.as_ref().map(|w| w.epoch()).unwrap_or(0)
                && loaded.model_revision == spec.model_revision
            {
                return Ok(loaded.clone());
            }
        }
        let worker = state
            .worker
            .as_mut()
            .ok_or_else(|| RuntimeError::InitFailed("worker 未就绪".into()))?;
        let epoch = worker.epoch();
        let payload = worker.request(Request::Load { spec: spec.clone() })?;
        let Payload::Loaded {
            engine_id,
            model_revision,
            threads,
            beam_size,
            runtime,
            runtime_build,
            platform,
            native_pid,
        } = payload
        else {
            return Err(RuntimeError::Protocol("Load 返回了非 Loaded 载荷".into()));
        };
        // revision 必须与我们要求加载的那个一致：这是 `docs/14` §1.1-3 的"回复一致性"
        if model_revision != spec.model_revision {
            return Err(RuntimeError::Protocol(format!(
                "worker 报回 model_revision={model_revision}，与请求的 {} 不符",
                spec.model_revision
            )));
        }
        let info = ModelInfo {
            engine_id,
            model_revision,
            threads,
            beam_size,
            worker_epoch: epoch,
            runtime,
            runtime_build,
            platform,
            native_pid,
        };
        state.loaded = Some(info.clone());
        Ok(info)
    }

    fn translate(&self, job: &TranslationJob) -> Result<TranslationReply, RuntimeError> {
        let mut state = self.guard();
        self.ensure_worker(&mut state)?;
        let Some(loaded) = state.loaded.clone() else {
            return Err(RuntimeError::InitFailed(
                "尚未加载模型（worker 可能已重启，需要重新 load）".into(),
            ));
        };
        let worker = state
            .worker
            .as_mut()
            .ok_or_else(|| RuntimeError::InitFailed("worker 未就绪".into()))?;
        self.active_requests.fetch_add(1, Ordering::SeqCst);
        let payload = worker.request(Request::Translate {
            text: job.text.clone(),
            target: job.target.clone(),
        });
        self.active_requests.fetch_sub(1, Ordering::SeqCst);
        let payload = payload?;
        let Payload::Translated { text, elapsed_ms } = payload else {
            return Err(RuntimeError::Protocol(
                "Translate 返回了非 Translated 载荷".into(),
            ));
        };
        Ok(TranslationReply {
            text,
            worker_elapsed_ms: elapsed_ms,
            worker_epoch: worker.epoch(),
            model_revision: loaded.model_revision,
        })
    }

    fn unload(&self) -> Result<(), RuntimeError> {
        let mut state = self.guard();
        let Some(worker) = state.worker.as_mut() else {
            state.loaded = None;
            state.snapshot = None;
            return Ok(());
        };
        if worker.is_dead() {
            state.loaded = None;
            state.snapshot = None;
            return Ok(());
        }
        match worker.request(Request::Unload)? {
            Payload::Unloaded => {
                state.loaded = None;
                state.snapshot = None;
                Ok(())
            }
            other => Err(RuntimeError::Protocol(format!(
                "Unload 返回了意外载荷：{other:?}"
            ))),
        }
    }

    fn health(&self) -> Result<WorkerHealth, RuntimeError> {
        let mut state = self.guard();
        self.ensure_worker(&mut state)?;
        let worker = state
            .worker
            .as_mut()
            .ok_or_else(|| RuntimeError::InitFailed("worker 未就绪".into()))?;
        let pid = worker.pid();
        let payload = worker.request(Request::Health)?;
        let Payload::Health {
            alive,
            requests_served,
        } = payload
        else {
            return Err(RuntimeError::Protocol("Health 返回了非 Health 载荷".into()));
        };
        Ok(WorkerHealth {
            alive,
            requests_served,
            worker_rss_bytes: worker.resident_bytes(),
            worker_pid: pid,
            worker_epoch: worker.epoch(),
        })
    }

    fn shutdown(&self) -> Result<(), RuntimeError> {
        self.shutdown_requested.store(1, Ordering::SeqCst);
        let mut state = self.guard();
        if let Some(mut worker) = state.worker.take() {
            worker.terminate();
            // 进程句柄在这里释放；WorkerProcess::drop 是幂等的
        }
        state.loaded = None;
        state.snapshot = None;
        Ok(())
    }

    fn metrics(&self) -> TransportMetrics {
        let state = self.guard();
        // 指标只描述活着的 worker：死进程的 pid 对调用方没有意义
        let worker = state.worker.as_ref().filter(|w| !w.is_dead());
        TransportMetrics {
            spawns: state.spawns,
            respawns: state.respawns,
            epoch: state.next_epoch.saturating_sub(1),
            host_rss_bytes: process_rss_bytes(),
            worker_rss_bytes: worker.and_then(|w| w.resident_bytes()),
            worker_pid: worker.map(|w| w.pid()),
            model_preparation_ms: state.model_preparation_ms,
            snapshot_bytes: state.snapshot_bytes,
            stale_snapshots_removed: state.stale_snapshots_removed,
            stale_snapshots_kept: state.stale_snapshots_kept,
        }
    }
}

impl Drop for ProcessTransport {
    fn drop(&mut self) {
        // 不依赖调用方记得 shutdown：drop 时也要把子进程收掉，否则测试会漏僵尸
        if let Ok(mut state) = self.state.lock() {
            if let Some(mut worker) = state.worker.take() {
                worker.terminate();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelSpec;

    fn spec() -> ModelSpec {
        ModelSpec {
            package_id: "test".into(),
            version: "1".into(),
            model_path: "/tmp/x".into(),
            src_vocab: "/tmp/y".into(),
            trg_vocab: "/tmp/z".into(),
            lex_shortlist: None,
            split_prefix: None,
            threads: 1,
            beam_size: 1,
            model_revision: "test@1#deadbeef".into(),
            digests: Default::default(),
        }
    }

    // 这条依赖进程隔离 worker（Linux 语义：SIGTERM 升级、/proc 采样、kill -9 恢复）与
    // 宿主 RSS 观测；Windows 上平台门禁直接给 CapabilityUnavailable，故限定在 Linux。
    #[cfg(target_os = "linux")]
    #[test]
    fn 未加载就翻译给出明确的初始化错误() {
        let transport = ProcessTransport::new(WorkerConfig::new("/bin/true"), 1);
        let err = transport
            .translate(&TranslationJob {
                text: "hi".into(),
                target: "zh-Hans".into(),
            })
            .unwrap_err();
        // 还没 spawn：先把 /bin/true 起起来，然后 Load 会失败
        assert!(
            matches!(
                err,
                RuntimeError::InitFailed(_) | RuntimeError::WorkerExited { .. }
            ),
            "{err}"
        );
    }

    #[test]
    fn 缺文件在spawn之前就被拒绝() {
        let transport = ProcessTransport::new(WorkerConfig::new("/bin/true"), 1);
        let err = transport.load(&spec()).unwrap_err();
        assert_eq!(
            err.code(),
            translator_types::TranslationErrorCode::ResourceNotInstalled
        );
        // 没有 spawn 过任何 worker
        assert_eq!(transport.metrics().spawns, 0);
    }

    // 这条依赖进程隔离 worker（Linux 语义：SIGTERM 升级、/proc 采样、kill -9 恢复）与
    // 宿主 RSS 观测；Windows 上平台门禁直接给 CapabilityUnavailable，故限定在 Linux。
    #[cfg(target_os = "linux")]
    #[test]
    fn 指标包含宿主rss() {
        let transport = ProcessTransport::new(WorkerConfig::new("/bin/true"), 1);
        let metrics = transport.metrics();
        assert!(metrics.spawns == 0);
        if cfg!(target_os = "linux") {
            assert!(metrics.host_rss_bytes.unwrap_or(0) > 1024 * 1024);
        }
        assert!(metrics.combined_rss_bytes().unwrap_or(0) > 0);
    }
}
