//! `nmt-worker`：翻译引擎的**子进程侧**。
//!
//! P3.1-A 只实现 `--mock`。`--bergamot` 现在**故意**返回 `CapabilityUnavailable`：
//! 真实 Bergamot 接线是 P3.1-B，这里假装能跑比报错更危险（`docs/12` §3.3）。
//!
//! 约定：
//! - stdin/stdout 只跑帧协议，**不写人类可读日志**（否则协议流会被搅坏）；
//! - 诊断只走 stderr，宿主按行保留尾部；
//! - 故障注入用**真实崩溃**（`abort()`），不是返回错误码——只有真崩才能证明隔离有效。

use std::io::{BufReader, BufWriter, Write};
use std::process::exit;

use translator_runtime::model::ModelSpec;
use translator_runtime::protocol::{
    read_frame, write_frame, Payload, Request, RequestEnvelope, Response, WorkerErrorCode,
    PROTOCOL_VERSION,
};

struct Args {
    engine: Engine,
    crash_on_load: bool,
    crash_on_translate: bool,
    crash_on_shutdown: bool,
    fail_on_translate: bool,
    garbage_on_translate: bool,
    sleep_ms: u64,
    /// `--epoch N`：故意在响应里报一个**错的**世代号（用于测试宿主的丢弃逻辑）
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
        crash_on_load: false,
        crash_on_translate: false,
        crash_on_shutdown: false,
        fail_on_translate: false,
        garbage_on_translate: false,
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
            "--noisy-stderr" => args.noisy_stderr = true,
            "--sleep-ms" => {
                let value = argv.next().ok_or("--sleep-ms 需要参数")?;
                args.sleep_ms = value.parse().map_err(|_| "--sleep-ms 需要整数")?;
            }
            "--epoch" => {
                let value = argv.next().ok_or("--epoch 需要参数")?;
                args.epoch_override = Some(value.parse().map_err(|_| "--epoch 需要整数")?);
            }
            other => return Err(format!("未知参数 {other}")),
        }
    }
    Ok(args)
}

struct Session {
    loaded: Option<ModelSpec>,
    requests_served: u64,
    /// 非 None 时，响应里的 epoch 一律报这个值（故障注入）
    epoch_override: Option<u64>,
}

impl Session {
    /// 世代号归**父进程**所有：默认原样回带请求里的值，这样 respawn 之后
    /// 宿主能立刻发现"这是新一代 worker"。
    fn epoch_for(&self, incoming: u64) -> u64 {
        self.epoch_override.unwrap_or(incoming)
    }
}

/// mock 译文：明确标出目标语言前缀，让人一眼看出这不是真翻译
/// （与其它 mock 引擎保持同一种"看得见的假"）。
fn mock_translate(text: &str, target: &str) -> String {
    format!("[{target}] {text}")
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("nmt-worker: {message}");
            exit(2);
        }
    };

    if args.noisy_stderr {
        // 故意刷 stderr：验证宿主侧环形缓冲不会因为管线堵死而卡住 worker
        for i in 0..2000 {
            eprintln!("nmt-worker: noise {i} {}", "x".repeat(200));
        }
    }

    eprintln!(
        "nmt-worker: start pid={} engine={} epoch_override={:?}",
        std::process::id(),
        args.engine.as_str(),
        args.epoch_override
    );

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    let mut session = Session {
        loaded: None,
        requests_served: 0,
        epoch_override: args.epoch_override,
    };

    loop {
        let frame = match read_frame(&mut reader) {
            Ok(frame) => frame,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                eprintln!("nmt-worker: stdin 关闭，退出");
                exit(0);
            }
            Err(err) => {
                eprintln!("nmt-worker: 读帧失败（{err}），退出");
                exit(3);
            }
        };

        let envelope: RequestEnvelope = match serde_json::from_slice(&frame) {
            Ok(envelope) => envelope,
            Err(err) => {
                // 帧坏了也要回一条，让宿主能区分"帧坏"和"卡住"
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
            exit(4);
        }

        let id = envelope.id;
        let is_shutdown = matches!(envelope.request, Request::Shutdown);
        let response = handle(&args, &mut session, id, envelope.epoch, envelope.request);

        if let Some(response) = response {
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
                exit(3);
            }
        }

        if is_shutdown {
            eprintln!("nmt-worker: 收到 Shutdown，退出");
            exit(0);
        }
    }
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
) -> Option<Response> {
    let epoch = session.epoch_for(incoming_epoch);
    match request {
        Request::Load { spec } => {
            if args.crash_on_load {
                eprintln!("nmt-worker: 按 --crash-on-load 触发 abort()");
                std::process::abort();
            }
            if args.engine == Engine::Bergamot {
                return Some(Response::error(
                    id,
                    epoch,
                    WorkerErrorCode::CapabilityUnavailable,
                    "P3.1-A 尚未接线真实 Bergamot（计划 P3.1-B）".to_string(),
                ));
            }
            if args.sleep_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(args.sleep_ms));
            }
            session.loaded = Some(spec.clone());
            Some(Response::ok(
                id,
                epoch,
                Payload::Loaded {
                    engine_id: "worker.mock".to_string(),
                    model_revision: spec.model_revision.clone(),
                    threads: spec.threads,
                    beam_size: spec.beam_size,
                },
            ))
        }
        Request::Translate { text, target } => {
            if args.crash_on_translate {
                eprintln!("nmt-worker: 按 --crash-on-translate 触发 abort()");
                std::process::abort();
            }
            if args.fail_on_translate {
                return Some(Response::error(
                    id,
                    epoch,
                    WorkerErrorCode::BackendFailed,
                    "按 --fail-on-translate 注入后端失败".to_string(),
                ));
            }
            if args.sleep_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(args.sleep_ms));
            }
            if session.loaded.is_none() {
                return Some(Response::error(
                    id,
                    epoch,
                    WorkerErrorCode::InitFailed,
                    "未加载模型".to_string(),
                ));
            }
            session.requests_served += 1;
            Some(Response::ok(
                id,
                epoch,
                Payload::Translated {
                    text: mock_translate(&text, &target),
                    elapsed_ms: 1,
                },
            ))
        }
        Request::Unload => {
            session.loaded = None;
            Some(Response::ok(id, epoch, Payload::Unloaded))
        }
        Request::Health => Some(Response::ok(
            id,
            epoch,
            Payload::Health {
                alive: true,
                requests_served: session.requests_served,
            },
        )),
        Request::Shutdown => {
            if args.crash_on_shutdown {
                eprintln!("nmt-worker: 按 --crash-on-shutdown 触发 abort()");
                std::process::abort();
            }
            Some(Response::ok(id, epoch, Payload::Empty))
        }
    }
}
