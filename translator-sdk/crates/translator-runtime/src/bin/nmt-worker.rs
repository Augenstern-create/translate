//! `nmt-worker`：翻译引擎的**子进程侧**。
//!
//! 两种引擎：
//!
//! - `--mock`（默认）：纯 Rust 假引擎，用来验证进程边界、帧协议、超时与终止流程；
//! - `--bergamot`：**真实** Bergamot/Marian。P3.1-B 的交付（`docs/14` §3.1）。
//!
//! ## 真实引擎为什么是"worker 内部的 native 进程"
//!
//! `docs/14` §3.3 允许替代方案：如果 Bergamot 现有 API 无法稳定通过 C ABI 单句调用，
//! 可以把参考 CLI 的长驻翻译服务放进 worker 内部实现，只要帧协议与故障隔离契约不变。
//! P3.0 已经有打过行模式补丁的参考 CLI（`tools/p3/bergamot-cli-lines.patch`），
//! 它的输出是逐行 JSON；本轮就用它，把行协议**完全关在 worker 里**，不向上暴露。
//!
//! 代价与边界（写进 `docs/15` 的偏差清单）：
//! - 进程树是 宿主 → worker(Rust 监督者) → native(bergamot)；多一层进程与一次管道往返；
//! - native 死了 = 模型没了：worker 立刻以退出码 70 退出，让宿主按"worker 已退出"处理，
//!   从而复用 P3.1-A 已有的"不静默重放 + 下次请求才重启"语义；
//! - native 没有可靠的 unload：`Unload` = 结束 native 进程（`docs/14` §7 允许，但要如实记录冷启动代价）。
//!
//! 其它约定：stdin/stdout 只跑帧协议；所有诊断（含 native 的 stderr）只走 stderr。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use translator_runtime::config::{render_bergamot_config, BergamotOptions};
use translator_runtime::model::ModelSpec;
use translator_runtime::protocol::{
    read_frame, write_frame, Payload, Request, RequestEnvelope, Response, WorkerErrorCode,
    PROTOCOL_VERSION,
};

/// native 死掉后 worker 的退出码（宿主侧会把它呈现为 `WorkerExited{code:Some(70)}`）。
const EXIT_NATIVE_DIED: i32 = 70;

struct Args {
    engine: Engine,
    native: Option<PathBuf>,
    runtime_build: Option<String>,
    crash_on_load: bool,
    crash_on_translate: bool,
    crash_on_shutdown: bool,
    fail_on_translate: bool,
    garbage_on_translate: bool,
    dev_skip_recheck: bool,
    sleep_ms: u64,
    epoch_override: Option<u64>,
    noisy_stderr: bool,
}

#[derive(PartialEq, Clone, Copy)]
enum Engine {
    Mock,
    Bergamot,
}

impl Engine {
    fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Bergamot => "bergamot",
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        engine: Engine::Mock,
        native: std::env::var_os("NMT_BERGAMOT_BIN").map(PathBuf::from),
        runtime_build: std::env::var("NMT_BERGAMOT_BUILD").ok(),
        crash_on_load: false,
        crash_on_translate: false,
        crash_on_shutdown: false,
        fail_on_translate: false,
        garbage_on_translate: false,
        dev_skip_recheck: false,
        sleep_ms: 0,
        epoch_override: None,
        noisy_stderr: false,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--mock" => args.engine = Engine::Mock,
            "--bergamot" => args.engine = Engine::Bergamot,
            "--crash-on-load" => args.crash_on_load = true,
            "--crash-on-translate" => args.crash_on_translate = true,
            "--crash-on-shutdown" => args.crash_on_shutdown = true,
            "--fail-on-translate" => args.fail_on_translate = true,
            "--emit-garbage-on-translate" => args.garbage_on_translate = true,
            // 只给测试用：跳过 worker 侧摘要复核，让坏文件真的进 native，
            // 用来复现"真实 abort/SIGSEGV"（生产路径绝不允许打开它）
            "--dev-skip-recheck" => args.dev_skip_recheck = true,
            "--noisy-stderr" => args.noisy_stderr = true,
            "--native" => {
                args.native = Some(PathBuf::from(argv.next().ok_or("--native 需要参数")?));
            }
            "--runtime-build" => {
                args.runtime_build = Some(argv.next().ok_or("--runtime-build 需要参数")?);
            }
            "--sleep-ms" => {
                args.sleep_ms = argv
                    .next()
                    .ok_or("--sleep-ms 需要参数")?
                    .parse()
                    .map_err(|_| "--sleep-ms 需要整数")?;
            }
            "--epoch" => {
                args.epoch_override = Some(
                    argv.next()
                        .ok_or("--epoch 需要参数")?
                        .parse()
                        .map_err(|_| "--epoch 需要整数")?,
                );
            }
            other => return Err(format!("未知参数 {other}")),
        }
    }
    Ok(args)
}

// ---------------------------------------------------------------------------
// native 进程：真实 Bergamot（行模式）
// ---------------------------------------------------------------------------

struct Native {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    lines: Receiver<String>,
    stderr_tail: VecDeque<String>,
    config_path: PathBuf,
}

impl Native {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// 读一行（带超时）。`None` = native 已死或超时。
    fn read_line(&mut self, budget: Duration) -> Option<String> {
        match self.lines.recv_timeout(budget) {
            Ok(line) => Some(line),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => None,
        }
    }

    /// 结束 native：关 stdin → 等 500 ms → SIGTERM → 等 500 ms → SIGKILL。
    fn terminate(&mut self) {
        self.stdin = None;
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            if !self.alive() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.alive() {
            #[cfg(unix)]
            {
                // SAFETY: 只读 pid + 常量信号；失败(ESRCH)说明进程已退出，下面 wait 收尸
                unsafe {
                    libc::kill(self.pid() as libc::pid_t, libc::SIGTERM);
                }
            }
            let deadline = std::time::Instant::now() + Duration::from_millis(500);
            while std::time::Instant::now() < deadline {
                if !self.alive() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.config_path);
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[derive(serde::Deserialize)]
struct NativeReply {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    ms: Option<f64>,
    #[serde(default)]
    load_ms: Option<f64>,
}

fn spawn_native(binary: &Path, config_path: &Path, threads: u16) -> Result<(Native, f64), String> {
    let mut command = Command::new(binary);
    command
        .arg("--model-config-paths")
        .arg(config_path)
        .arg("--cpu-threads")
        .arg(threads.to_string())
        // native 的日志必须走 stderr，且级别压低：stdout 只留结果行
        .arg("--log-level")
        .arg("critical")
        .env("BERGAMOT_STDIN_LINES", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // native 必须"跟着 worker 死"：宿主超时会直接 SIGKILL 整个 worker，
    // 若不做这一步，native（400+ MiB）会变成孤儿进程继续占着内存。
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        #[allow(unsafe_code)]
        // SAFETY: pre_exec 里只调用异步信号安全的 prctl(PR_SET_PDEATHSIG)
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动 native {}：{e}", binary.display()))?;

    let stdin = child.stdin.take();
    let stdout = child.stdout.take().ok_or("native stdout 未接管")?;
    let stderr = child.stderr.take().ok_or("native stderr 未接管")?;

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        // 通道在这里关闭：宿主侧会看到 Disconnected
    });

    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => eprintln!("native: {}", line.chars().take(400).collect::<String>()),
                Err(_) => break,
            }
        }
    });

    let mut native = Native {
        child,
        stdin,
        lines: rx,
        stderr_tail: VecDeque::new(),
        config_path: config_path.to_path_buf(),
    };

    // 第一行必须是 load 回执
    let line = native
        .read_line(Duration::from_secs(120))
        .ok_or_else(|| "native 在加载完成前退出（没有 load 回执）".to_string())?;
    let reply: NativeReply = serde_json::from_str(&line).map_err(|e| {
        format!(
            "native 的加载回执不是 JSON（{e}）：{}",
            line.chars().take(200).collect::<String>()
        )
    })?;
    let load_ms = reply
        .load_ms
        .ok_or_else(|| format!("native 加载回执缺少 load_ms：{line}"))?;
    native.stderr_tail.clear();
    Ok((native, load_ms))
}

// ---------------------------------------------------------------------------
// 会话
// ---------------------------------------------------------------------------

struct Session {
    engine: Engine,
    loaded: Option<ModelSpec>,
    requests_served: u64,
    epoch_override: Option<u64>,
    native: Option<Native>,
    workdir: Option<PathBuf>,
    runtime_build: Option<String>,
}

impl Session {
    fn epoch_for(&self, incoming: u64) -> u64 {
        self.epoch_override.unwrap_or(incoming)
    }

    fn cleanup(&mut self) {
        if let Some(mut native) = self.native.take() {
            native.terminate();
        }
        if let Some(dir) = self.workdir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.loaded = None;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn mock_translate(text: &str, target: &str) -> String {
    format!("[{target}] {text}")
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("nmt-worker: {message}");
            std::process::exit(2);
        }
    };

    if args.noisy_stderr {
        for i in 0..2000 {
            eprintln!("nmt-worker: noise {i} {}", "x".repeat(200));
        }
    }

    eprintln!(
        "nmt-worker: start pid={} engine={} native={:?}",
        std::process::id(),
        args.engine.as_str(),
        args.native
    );

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    let mut session = Session {
        engine: args.engine,
        loaded: None,
        requests_served: 0,
        epoch_override: args.epoch_override,
        native: None,
        workdir: None,
        runtime_build: args.runtime_build.clone(),
    };

    loop {
        let frame = match read_frame(&mut reader) {
            Ok(frame) => frame,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                eprintln!("nmt-worker: stdin 关闭，退出");
                session.cleanup();
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("nmt-worker: 读帧失败（{err}），退出");
                session.cleanup();
                std::process::exit(3);
            }
        };

        let envelope: RequestEnvelope = match serde_json::from_slice(&frame) {
            Ok(envelope) => envelope,
            Err(err) => {
                let response = Response::error(
                    0,
                    session.epoch_for(0),
                    WorkerErrorCode::InvalidRequest,
                    format!("帧不是合法 JSON：{err}"),
                );
                let _ = reply(&mut writer, &response);
                continue;
            }
        };

        if envelope.protocol != PROTOCOL_VERSION {
            let response = Response::error(
                envelope.id,
                session.epoch_for(envelope.epoch),
                WorkerErrorCode::InvalidRequest,
                format!(
                    "协议版本不匹配：收到 {}，本 worker 只支持 {PROTOCOL_VERSION}",
                    envelope.protocol
                ),
            );
            let _ = reply(&mut writer, &response);
            eprintln!("nmt-worker: 协议版本不匹配，退出");
            std::process::exit(4);
        }

        let id = envelope.id;
        let is_shutdown = matches!(envelope.request, Request::Shutdown);
        let outcome = handle(&args, &mut session, id, envelope.epoch, envelope.request);

        match outcome {
            Outcome::Reply(response) => {
                if args.garbage_on_translate
                    && matches!(response.payload, Some(Payload::Translated { .. }))
                {
                    eprintln!("nmt-worker: 按 --emit-garbage-on-translate 写入坏帧");
                    let mut raw = (9u32).to_be_bytes().to_vec();
                    raw.extend_from_slice(b"not json!");
                    let _ = writer.write_all(&raw);
                    let _ = writer.flush();
                    continue;
                }
                if reply(&mut writer, &response).is_err() {
                    std::process::exit(3);
                }
            }
            Outcome::ReplyThenExit(response, code) => {
                let _ = reply(&mut writer, &response);
                session.cleanup();
                std::process::exit(code);
            }
        }

        if is_shutdown {
            eprintln!("nmt-worker: 收到 Shutdown，退出");
            session.cleanup();
            std::process::exit(0);
        }
    }
}

enum Outcome {
    Reply(Response),
    ReplyThenExit(Response, i32),
}

fn reply<W: Write>(writer: &mut W, response: &Response) -> std::io::Result<()> {
    let body = serde_json::to_vec(response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    write_frame(writer, &body)
}

fn handle(
    args: &Args,
    session: &mut Session,
    id: u64,
    incoming_epoch: u64,
    request: Request,
) -> Outcome {
    let epoch = session.epoch_for(incoming_epoch);
    match request {
        Request::Load { spec } => Outcome::Reply(load(args, session, id, epoch, spec)),
        Request::Translate { text, target } => translate(args, session, id, epoch, text, target),
        Request::Unload => {
            if let Some(mut native) = session.native.take() {
                native.terminate();
            }
            if let Some(dir) = session.workdir.take() {
                let _ = std::fs::remove_dir_all(dir);
            }
            session.loaded = None;
            Outcome::Reply(Response::ok(id, epoch, Payload::Unloaded))
        }
        Request::Health => {
            let native_alive = session
                .native
                .as_mut()
                .map(|native| native.alive())
                .unwrap_or(true);
            Outcome::Reply(Response::ok(
                id,
                epoch,
                Payload::Health {
                    alive: native_alive,
                    requests_served: session.requests_served,
                },
            ))
        }
        Request::Shutdown => {
            if args.crash_on_shutdown {
                eprintln!("nmt-worker: 按 --crash-on-shutdown 触发 abort()");
                std::process::abort();
            }
            Outcome::Reply(Response::ok(id, epoch, Payload::Empty))
        }
    }
}

fn load(args: &Args, session: &mut Session, id: u64, epoch: u64, spec: ModelSpec) -> Response {
    if args.crash_on_load {
        eprintln!("nmt-worker: 按 --crash-on-load 触发 abort()");
        std::process::abort();
    }
    if args.sleep_ms > 0 {
        std::thread::sleep(Duration::from_millis(args.sleep_ms));
    }

    if session.engine == Engine::Mock {
        session.loaded = Some(spec.clone());
        return Response::ok(
            id,
            epoch,
            Payload::Loaded {
                engine_id: "worker.mock".to_string(),
                model_revision: spec.model_revision.clone(),
                threads: spec.threads,
                beam_size: spec.beam_size,
                runtime: Some("mock".to_string()),
                runtime_build: None,
                platform: Some(std::env::consts::OS.to_string()),
                native_pid: None,
            },
        );
    }

    // ---- 真实路径 ----
    let Some(native_bin) = args.native.clone() else {
        return Response::error(
            id,
            epoch,
            WorkerErrorCode::CapabilityUnavailable,
            "未指定 native 可执行文件（--native 或 NMT_BERGAMOT_BIN）".to_string(),
        );
    };
    if !native_bin.exists() {
        return Response::error(
            id,
            epoch,
            WorkerErrorCode::CapabilityUnavailable,
            format!("native 可执行文件不存在：{}", native_bin.display()),
        );
    }

    // worker 侧**再核一遍**摘要：宿主校验过不代表 native 打开时还是同一份字节
    if !args.dev_skip_recheck {
        if let Err(err) = spec.verify_digests() {
            return Response::error(
                id,
                epoch,
                WorkerErrorCode::IntegrityFailure,
                err.to_string(),
            );
        }
    }

    let options = BergamotOptions {
        cpu_threads: spec.threads.max(1),
        beam_size: spec.beam_size.max(1),
        ..BergamotOptions::default()
    };
    let yaml = match render_bergamot_config(&spec, &options) {
        Ok(yaml) => yaml,
        Err(err) => {
            return Response::error(id, epoch, WorkerErrorCode::InitFailed, err.to_string());
        }
    };

    // 崩溃残留兜底：被 SIGKILL 的 worker 会留下 `nmt-worker-<pid>-<epoch>/`。
    // 判定按 pid + 进程启动标识（PID 复用不会被误当成"还活着"）。
    if let Ok(report) = translator_runtime::reclaim::sweep_worker_dirs(
        &translator_runtime::reclaim::worker_dir_root(),
    ) {
        if report.removed > 0 {
            eprintln!("nmt-worker: 清理 {} 个残留工作目录", report.removed);
        }
    }

    // 配置写在 worker 私有临时目录里（快照目录是只读语义，不往里写东西）
    let workdir = std::env::temp_dir().join(format!("nmt-worker-{}-{}", std::process::id(), epoch));
    if let Err(e) = std::fs::create_dir_all(&workdir) {
        return Response::error(
            id,
            epoch,
            WorkerErrorCode::InitFailed,
            format!("无法创建 worker 工作目录 {}：{e}", workdir.display()),
        );
    }
    let config_path = workdir.join("bergamot.yml");
    if let Err(e) = std::fs::write(&config_path, yaml) {
        return Response::error(
            id,
            epoch,
            WorkerErrorCode::InitFailed,
            format!("无法写入配置 {}：{e}", config_path.display()),
        );
    }
    session.workdir = Some(workdir);

    match spawn_native(&native_bin, &config_path, options.cpu_threads) {
        Ok((native, load_ms)) => {
            eprintln!(
                "nmt-worker: native pid={} load_ms={load_ms:.1}",
                native.pid()
            );
            session.native = Some(native);
            session.loaded = Some(spec.clone());
            Response::ok(
                id,
                epoch,
                Payload::Loaded {
                    engine_id: "nmt.bergamot".to_string(),
                    model_revision: spec.model_revision.clone(),
                    threads: spec.threads,
                    beam_size: spec.beam_size,
                    runtime: Some("bergamot-cli".to_string()),
                    runtime_build: session.runtime_build.clone(),
                    platform: Some(format!(
                        "{}-{}",
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    )),
                    native_pid: session.native.as_ref().map(|n| n.pid()),
                },
            )
        }
        Err(message) => Response::error(id, epoch, WorkerErrorCode::InitFailed, message),
    }
}

fn translate(
    args: &Args,
    session: &mut Session,
    id: u64,
    epoch: u64,
    text: String,
    target: String,
) -> Outcome {
    if args.crash_on_translate {
        eprintln!("nmt-worker: 按 --crash-on-translate 触发 abort()");
        std::process::abort();
    }
    if args.fail_on_translate {
        return Outcome::Reply(Response::error(
            id,
            epoch,
            WorkerErrorCode::BackendFailed,
            "按 --fail-on-translate 注入后端失败".to_string(),
        ));
    }
    if args.sleep_ms > 0 {
        std::thread::sleep(Duration::from_millis(args.sleep_ms));
    }
    if session.loaded.is_none() {
        return Outcome::Reply(Response::error(
            id,
            epoch,
            WorkerErrorCode::InitFailed,
            "未加载模型".to_string(),
        ));
    }

    if session.engine == Engine::Mock {
        session.requests_served += 1;
        return Outcome::Reply(Response::ok(
            id,
            epoch,
            Payload::Translated {
                text: mock_translate(&text, &target),
                elapsed_ms: 1,
            },
        ));
    }

    let Some(native) = session.native.as_mut() else {
        return Outcome::Reply(Response::error(
            id,
            epoch,
            WorkerErrorCode::InitFailed,
            "native 未就绪".to_string(),
        ));
    };

    // 行协议：一条请求必须是一行（换行会把它劈成两条请求，直接错位）
    let single_line: String = text.replace(['\n', '\r'], " ");
    let wrote = match native.stdin.as_mut() {
        Some(stdin) => {
            let ok = stdin.write_all(single_line.as_bytes()).is_ok()
                && stdin.write_all(b"\n").is_ok()
                && stdin.flush().is_ok();
            ok
        }
        None => false,
    };
    if !wrote {
        let message = "写入 native stdin 失败（native 可能已退出）".to_string();
        return Outcome::ReplyThenExit(
            Response::error(id, epoch, WorkerErrorCode::BackendFailed, message),
            EXIT_NATIVE_DIED,
        );
    }

    match native.read_line(Duration::from_secs(60)) {
        Some(line) => match serde_json::from_str::<NativeReply>(&line) {
            Ok(reply) => {
                let Some(translated) = reply.text else {
                    return Outcome::Reply(Response::error(
                        id,
                        epoch,
                        WorkerErrorCode::BackendFailed,
                        format!(
                            "native 回执缺少 text：{}",
                            line.chars().take(200).collect::<String>()
                        ),
                    ));
                };
                session.requests_served += 1;
                Outcome::Reply(Response::ok(
                    id,
                    epoch,
                    Payload::Translated {
                        text: translated,
                        elapsed_ms: reply.ms.unwrap_or(0.0).round() as u64,
                    },
                ))
            }
            Err(e) => Outcome::Reply(Response::error(
                id,
                epoch,
                WorkerErrorCode::BackendFailed,
                format!(
                    "native 回执不是 JSON（{e}）：{}",
                    line.chars().take(200).collect::<String>()
                ),
            )),
        },
        None => {
            // native 没了：把这个请求如实报错，然后 worker 自己也退出，
            // 让宿主按"worker 已退出"走既有的回收/重启路径（不静默重放）
            let detail = if native.alive() {
                "native 在超时时间内没有回执".to_string()
            } else {
                "native 进程已退出".to_string()
            };
            Outcome::ReplyThenExit(
                Response::error(id, epoch, WorkerErrorCode::BackendFailed, detail),
                EXIT_NATIVE_DIED,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn 行协议把换行替换成空格() {
        // 与 translate() 中的清洗保持一致：一条帧 = 一行 stdin
        let text = "a\nb\r\nc";
        let single_line: String = text.replace(['\n', '\r'], " ");
        assert_eq!(single_line, "a b  c");
        assert!(!single_line.contains('\n'));
    }
}
