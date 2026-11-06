//! worker 进程的持有者：spawn、请求/响应配对、监控、终止。
//!
//! # 为什么是**进程**而不是线程
//!
//! P3.0 已经实测：Bergamot/Marian 在缺文件时 `abort()`、权重被截断时 SIGSEGV（`docs/11` §2.5）。
//! `catch_unwind` 拦不住这两种，线程隔离也拦不住——只有独立地址空间能把宿主保住
//! （`docs/12` §2.1 的 P0）。所以这一层的每个设计都围绕"**宿主绝不能跟着死**"：
//!
//! - 读帧在独立线程，主线程只等 channel，带超时；
//! - 任何写/读/超时异常都会把 worker 标记为 dead 并**回收进程**，绝不留僵尸；
//! - 终止分三级：礼貌 shutdown 帧 → SIGTERM → SIGKILL，每级都有等待上限。

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, ErrorKind};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::RuntimeError;
use crate::model::rss_bytes_of;
use crate::protocol::{
    self, Payload, Request, RequestEnvelope, Response, WorkerErrorCode, PROTOCOL_VERSION,
};

/// worker 启动参数。
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// worker 可执行文件
    pub binary: PathBuf,
    /// 额外参数（例如 `--mock`、`--crash-on load`）
    pub args: Vec<String>,
    /// 其它请求（Health/Unload/Shutdown）的等待上限
    pub request_timeout: Duration,
    /// `Load` 的等待上限。加载要读 44 MB 权重 + 建工作区，10 s 不够稳（`docs/14` §3.6）
    pub load_timeout: Duration,
    /// `Translate` 的等待上限。P3.0 单句热翻译 p50 13 ms、冷首句 415 ms
    pub translate_timeout: Duration,
    /// 礼貌关闭的等待上限
    pub shutdown_grace: Duration,
    /// SIGTERM 之后的等待上限（之后 SIGKILL）
    pub term_grace: Duration,
    /// stderr 保留的行数上限（诊断用，不能无限涨）
    pub stderr_lines: usize,
    /// 世代号：每次 respawn 必须不同，用来丢弃上一个 worker 的迟到响应
    pub epoch: u64,
}

impl WorkerConfig {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            args: Vec::new(),
            request_timeout: Duration::from_secs(10),
            load_timeout: Duration::from_secs(30),
            translate_timeout: Duration::from_secs(15),
            shutdown_grace: Duration::from_millis(500),
            term_grace: Duration::from_millis(500),
            stderr_lines: 64,
            epoch: 1,
        }
    }

    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn with_load_timeout(mut self, timeout: Duration) -> Self {
        self.load_timeout = timeout;
        self
    }

    pub fn with_translate_timeout(mut self, timeout: Duration) -> Self {
        self.translate_timeout = timeout;
        self
    }

    /// 按操作类型取截止时间：`docs/14` §3.6 要求加载与推理分开。
    pub fn timeout_for(&self, request: &Request) -> Duration {
        match request {
            Request::Load { .. } => self.load_timeout,
            Request::Translate { .. } => self.translate_timeout,
            _ => self.request_timeout,
        }
    }

    pub fn with_epoch(mut self, epoch: u64) -> Self {
        self.epoch = epoch;
        self
    }
}

/// 一个活着的 worker 进程。
pub struct WorkerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Response>>>>,
    dead: Arc<AtomicBool>,
    last_protocol_error: Arc<Mutex<Option<String>>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
    epoch: u64,
    next_id: u64,
    request_timeout: Duration,
    load_timeout: Duration,
    translate_timeout: Duration,
    shutdown_grace: Duration,
    /// SIGTERM 之后的宽限期。读写都在 `#[cfg(unix)]` 分支里（Windows 没有等价信号，
    /// 直接进 SIGKILL），所以非 unix 构建上它是只写字段——保留它是为了让 `WorkerConfig`
    /// 在两个平台上形状一致，不让平台差异漏进公开配置。
    #[cfg_attr(not(unix), allow(dead_code))]
    term_grace: Duration,
    exit: Option<ExitStatus>,
}

impl WorkerProcess {
    /// 启动 worker 并等它"活着"（进程起来即可，不代表模型已加载）。
    pub fn spawn(config: WorkerConfig) -> Result<Self, RuntimeError> {
        let mut command = Command::new(&config.binary);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| {
            RuntimeError::InitFailed(format!("无法启动 worker {}：{e}", config.binary.display()))
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::InitFailed("worker stdout 未接管".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeError::InitFailed("worker stderr 未接管".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::InitFailed("worker stdin 未接管".into()))?;

        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let dead = Arc::new(AtomicBool::new(false));
        let last_protocol_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

        // 读帧线程：任何退出路径都要把 dead 置位并清空 pending（让等待者拿到 Disconnected）
        let reader = {
            let pending = Arc::clone(&pending);
            let dead = Arc::clone(&dead);
            let last = Arc::clone(&last_protocol_error);
            let mut stdout = stdout;
            std::thread::spawn(move || {
                loop {
                    match protocol::read_frame(&mut stdout) {
                        Ok(body) => match protocol::decode_response(&body) {
                            Ok(response) => {
                                let sender = pending.lock().unwrap().remove(&response.id);
                                if let Some(sender) = sender {
                                    let _ = sender.send(response);
                                }
                                // 找不到 id：迟到响应或已超时，直接丢弃
                            }
                            Err(e) => {
                                *last.lock().unwrap() = Some(format!("响应不是合法 JSON：{e}"));
                                break;
                            }
                        },
                        Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                        Err(e) => {
                            *last.lock().unwrap() = Some(format!("读帧失败：{e}"));
                            break;
                        }
                    }
                }
                dead.store(true, Ordering::SeqCst);
                pending.lock().unwrap().clear();
            })
        };

        // stderr 收集线程：只保留尾部 N 行，避免 worker 疯狂输出把宿主内存吃掉
        let stderr_reader = {
            let tail = Arc::clone(&stderr_tail);
            let limit = config.stderr_lines.max(1);
            let reader = BufReader::new(stderr);
            std::thread::spawn(move || {
                for line in reader.lines() {
                    match line {
                        Ok(line) => {
                            let mut tail = tail.lock().unwrap();
                            if tail.len() == limit {
                                tail.pop_front();
                            }
                            let mut line = line;
                            if line.len() > 512 {
                                line.truncate(512);
                            }
                            tail.push_back(line);
                        }
                        Err(_) => break,
                    }
                }
            })
        };

        Ok(Self {
            child,
            stdin: Some(stdin),
            pending,
            dead,
            last_protocol_error,
            stderr_tail,
            reader: Some(reader),
            stderr_reader: Some(stderr_reader),
            epoch: config.epoch,
            next_id: 0,
            request_timeout: config.request_timeout,
            load_timeout: config.load_timeout,
            translate_timeout: config.translate_timeout,
            shutdown_grace: config.shutdown_grace,
            term_grace: config.term_grace,
            exit: None,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// 按操作类型取截止时间（实例侧；与 `WorkerConfig::timeout_for` 对应）。
    fn timeout_for(&self, request: &Request) -> Duration {
        match request {
            Request::Load { .. } => self.load_timeout,
            Request::Translate { .. } => self.translate_timeout,
            _ => self.request_timeout,
        }
    }

    /// 本进程使用的三段截止时间（诊断用）。
    pub fn timeouts(&self) -> (Duration, Duration, Duration) {
        (
            self.request_timeout,
            self.load_timeout,
            self.translate_timeout,
        )
    }

    /// worker 是否已经不可用（进程退出 / 协议错乱 / 被我们终止）。
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst) || self.exit.is_some()
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.exit
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.lock().unwrap().iter().cloned().collect()
    }

    pub fn resident_bytes(&self) -> Option<u64> {
        if self.is_dead() {
            None
        } else {
            rss_bytes_of(self.pid())
        }
    }

    /// 发一个请求并等它的响应（用该操作类型的默认截止时间）。
    pub fn request(&mut self, request: Request) -> Result<Payload, RuntimeError> {
        let timeout = self.timeout_for(&request);
        self.request_with_timeout(request, timeout)
    }

    /// 发一个请求，**由调用方**给截止时间。
    ///
    /// 排队 deadline 与执行 deadline 必须能合成同一个预算（`docs/17` LC-04）：
    /// 调用方算出"还剩多少时间"，这里只负责不超过它。
    pub fn request_with_timeout(
        &mut self,
        request: Request,
        timeout: Duration,
    ) -> Result<Payload, RuntimeError> {
        if self.is_dead() {
            return Err(self.exit_error("请求发出前 worker 已退出"));
        }
        self.next_id += 1;
        let id = self.next_id;
        let (sender, receiver) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, sender);

        let envelope = RequestEnvelope::new(id, self.epoch, request);
        let bytes = match protocol::encode_request(&envelope) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.pending.lock().unwrap().remove(&id);
                return Err(RuntimeError::Protocol(format!("请求无法编码：{e}")));
            }
        };
        let write_result = match self.stdin.as_mut() {
            Some(stdin) => protocol::write_frame(stdin, &bytes),
            None => Err(std::io::Error::new(ErrorKind::BrokenPipe, "stdin 已关闭")),
        };
        if let Err(e) = write_result {
            self.pending.lock().unwrap().remove(&id);
            self.terminate();
            return Err(self.exit_error(&format!("写入请求失败：{e}")));
        }

        match receiver.recv_timeout(timeout) {
            Ok(response) => {
                // 世代/版本不符说明这个 worker 已经不可信（可能是别人的迟到响应，
                // 也可能是协议实现坏了）。不做猜测：标记不可用，让上层决定要不要重来。
                if response.epoch != self.epoch {
                    self.terminate();
                    return Err(RuntimeError::Protocol(format!(
                        "响应世代不符：期望 {}，收到 {}",
                        self.epoch, response.epoch
                    )));
                }
                if response.protocol != PROTOCOL_VERSION {
                    self.terminate();
                    return Err(RuntimeError::Protocol(format!(
                        "协议版本不符：期望 {PROTOCOL_VERSION}，收到 {}",
                        response.protocol
                    )));
                }
                self.payload_of(response)
            }
            Err(RecvTimeoutError::Timeout) => {
                // native 推理可能不可中断：不能假装"停下来了"，只能杀掉独占 worker
                let waited = timeout.as_millis() as u64;
                self.terminate();
                Err(RuntimeError::Timeout {
                    operation: envelope.request.op_name(),
                    waited_ms: waited,
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                let detail = self
                    .last_protocol_error
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "worker 在响应前关闭了输出".to_string());
                self.terminate();
                Err(self.exit_error(&detail))
            }
        }
    }

    fn payload_of(&self, response: Response) -> Result<Payload, RuntimeError> {
        if response.ok {
            return response
                .payload
                .ok_or_else(|| RuntimeError::Protocol("成功的响应缺少 payload".into()));
        }
        let code = response.code.unwrap_or(WorkerErrorCode::BackendFailed);
        let message = response.message.unwrap_or_default();
        Err(match code {
            WorkerErrorCode::NotInstalled => RuntimeError::NotInstalled(message),
            WorkerErrorCode::IntegrityFailure => RuntimeError::IntegrityFailure(message),
            WorkerErrorCode::CapabilityUnavailable => RuntimeError::CapabilityUnavailable(message),
            WorkerErrorCode::InitFailed => RuntimeError::InitFailed(message),
            WorkerErrorCode::BackendFailed => RuntimeError::BackendFailed(message),
            WorkerErrorCode::InvalidRequest => RuntimeError::Protocol(message),
        })
    }

    fn exit_error(&self, detail: &str) -> RuntimeError {
        RuntimeError::WorkerExited {
            code: self.exit.and_then(|s| s.code()),
            signal: self.exit.and_then(exit_signal),
            detail: detail.to_string(),
        }
    }

    /// 三级终止：礼貌 shutdown → SIGTERM → SIGKILL。幂等。
    #[allow(unsafe_code)] // 仅此处：向子进程发 SIGTERM（libc::kill）
    pub fn terminate(&mut self) {
        if self.exit.is_some() {
            self.reap_children();
            return;
        }
        // 1) 礼貌：还能写就发一个 shutdown 帧
        if let Some(stdin) = self.stdin.as_mut() {
            self.next_id += 1;
            let envelope = RequestEnvelope::new(self.next_id, self.epoch, Request::Shutdown);
            if let Ok(bytes) = protocol::encode_request(&envelope) {
                let _ = protocol::write_frame(stdin, &bytes);
            }
        }
        if self.wait_for_exit(self.shutdown_grace) {
            self.reap_children();
            return;
        }
        // 2) SIGTERM（Unix；Windows 没有等价的"礼貌信号"，直接进 3）
        #[cfg(unix)]
        {
            // SAFETY: 只读 pid，信号编号是常量；失败(ESRCH)说明进程已没了，下面 wait 会收尸
            unsafe {
                libc::kill(self.pid() as libc::pid_t, libc::SIGTERM);
            }
            if self.wait_for_exit(self.term_grace) {
                self.reap_children();
                return;
            }
        }
        // 3) SIGKILL
        let _ = self.child.kill();
        self.exit = self.child.wait().ok();
        self.reap_children();
    }

    fn wait_for_exit(&mut self, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.exit = Some(status);
                    return true;
                }
                Ok(None) => {}
                Err(_) => return false,
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// 关掉 stdin 并回收两个辅助线程；线程若不是立即结束就**丢弃句柄**（detach），
    /// 绝不让宿主卡在 join 上。
    fn reap_children(&mut self) {
        self.stdin = None;
        for handle in [self.reader.take(), self.stderr_reader.take()]
            .into_iter()
            .flatten()
        {
            let deadline = Instant::now() + Duration::from_millis(200);
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

/// 进程是被哪个信号杀掉的（Unix）。Windows 没有这个概念。
fn exit_signal(status: ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 配置默认值足够保守() {
        let config = WorkerConfig::new("/bin/true");
        assert_eq!(config.epoch, 1);
        assert!(config.request_timeout >= Duration::from_secs(1));
        assert!(config.shutdown_grace <= Duration::from_secs(1));
        assert!(config.stderr_lines >= 8);
    }

    #[test]
    fn 启动不存在的程序报初始化失败() {
        let config = WorkerConfig::new("/nonexistent/worker-binary");
        let err = match WorkerProcess::spawn(config) {
            Ok(_) => panic!("不存在的程序不该启动成功"),
            Err(err) => err,
        };
        assert_eq!(
            err.code(),
            translator_types::TranslationErrorCode::RuntimeInitFailed
        );
    }
}
