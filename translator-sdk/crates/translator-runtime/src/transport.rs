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

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use crate::error::RuntimeError;
use crate::model::{
    process_rss_bytes, ModelInfo, ModelSpec, TranslationJob, TranslationReply, TransportMetrics,
    WorkerHealth,
};
use crate::protocol::{Payload, Request};
use crate::worker::{WorkerConfig, WorkerProcess};

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
    spawns: u32,
    respawns: u32,
    next_epoch: u64,
}

/// 基于子进程的传输实现。
pub struct ProcessTransport {
    config: WorkerConfig,
    restart_budget: u32,
    state: Mutex<State>,
    shutdown_requested: AtomicU32,
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
                spawns: 0,
                respawns: 0,
                next_epoch: 1,
            }),
            shutdown_requested: AtomicU32::new(0),
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 平台门禁。
    ///
    /// `std::process` 本身是跨平台的，但"能不能在某个平台安全地托管 native 推理"
    /// 目前只有 Linux 的证据（P3.0 实测 + 本轮的终止升级/RSS 采样都依赖 `/proc` 与信号）。
    /// 按 `docs/12` §3.1 的要求：非 Linux 明确报能力不可用，而不是给一个没测过的实现。
    fn ensure_platform() -> Result<(), RuntimeError> {
        #[cfg(target_os = "linux")]
        {
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(RuntimeError::CapabilityUnavailable(
                "进程隔离 worker 目前仅在 Linux 上验证过；该平台暂不可用（P3.1-E 逐个平台放开）"
                    .to_string(),
            ))
        }
    }

    /// 确保有一个活着的 worker；必要时在预算内重启。
    fn ensure_worker(&self, state: &mut State) -> Result<(), RuntimeError> {
        Self::ensure_platform()?;
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
        } = payload
        else {
            return Err(RuntimeError::Protocol("Load 返回了非 Loaded 载荷".into()));
        };
        let info = ModelInfo {
            engine_id,
            model_revision,
            threads,
            beam_size,
            worker_epoch: epoch,
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
        let payload = worker.request(Request::Translate {
            text: job.text.clone(),
            target: job.target.clone(),
        })?;
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
            return Ok(());
        };
        if worker.is_dead() {
            state.loaded = None;
            return Ok(());
        }
        match worker.request(Request::Unload)? {
            Payload::Unloaded => {
                state.loaded = None;
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
            threads: 1,
            beam_size: 1,
            model_revision: "test@1#deadbeef".into(),
        }
    }

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
