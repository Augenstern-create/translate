//! 帧协议：4 字节大端长度 + UTF-8 JSON。
//!
//! 设计取舍（`docs/12` §2.1）：
//!
//! - **长度前缀**而不是换行分隔：译文里出现换行是常态，按行切会把一条消息劈成两半；
//! - **上限**（[`MAX_FRAME_BYTES`]）：没有上限的长度前缀等于把"分配多大内存"的决定权交给对端；
//! - **JSON** 而不是 bincode/protobuf：这一层是**进程边界**，可读性比几微秒的编码开销值钱得多；
//!   真要换编码，只改这一个文件；
//! - 业务数据只走 stdout 的帧，**stderr 只放诊断**——否则 native 的日志会把协议流搅坏。

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::model::ModelSpec;

/// 协议版本。改动消息形状必须加 1，否则新旧 worker 会"看起来能连"。
pub const PROTOCOL_VERSION: u32 = 1;

/// 单帧上限（8 MiB）。超限直接判协议错误，不尝试分配。
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// 请求。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// 加载模型（幂等：已加载同一 revision 时直接返回既有信息）
    Load { spec: ModelSpec },
    /// 翻译一句
    Translate { text: String, target: String },
    /// 卸载模型，保留 worker 进程
    Unload,
    /// 心跳 + 计数
    Health,
    /// 关闭进程（respond 后 worker 自己退出）
    Shutdown,
}

impl Request {
    pub fn op_name(&self) -> &'static str {
        match self {
            Self::Load { .. } => "load",
            Self::Translate { .. } => "translate",
            Self::Unload => "unload",
            Self::Health => "health",
            Self::Shutdown => "shutdown",
        }
    }
}

/// 请求信封。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub id: u64,
    /// 父进程在 spawn 时分配的世代号。worker 原样回带，用于丢弃"上一个 worker 的迟到响应"。
    pub epoch: u64,
    pub protocol: u32,
    #[serde(flatten)]
    pub request: Request,
}

impl RequestEnvelope {
    pub fn new(id: u64, epoch: u64, request: Request) -> Self {
        Self {
            id,
            epoch,
            protocol: PROTOCOL_VERSION,
            request,
        }
    }
}

/// 响应载荷。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Payload {
    Loaded {
        engine_id: String,
        model_revision: String,
        threads: u16,
        beam_size: u8,
        /// worker 内部真正跑推理的东西（`mock` / `bergamot-cli`）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime: Option<String>,
        /// 运行时版本（Bergamot commit 等）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_build: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platform: Option<String>,
        /// native 进程 pid（可核对身份，`docs/14` §3.7）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_pid: Option<u32>,
    },
    Translated {
        text: String,
        elapsed_ms: u64,
    },
    Unloaded,
    Health {
        alive: bool,
        requests_served: u64,
    },
    /// 没有附加数据的成功（例如 Shutdown 的回执）
    Empty,
}

/// worker 侧错误类别：**只允许这几个**，避免把 native 的任意字符串当控制流。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerErrorCode {
    NotInstalled,
    IntegrityFailure,
    CapabilityUnavailable,
    InitFailed,
    BackendFailed,
    InvalidRequest,
}

/// 响应信封（扁平结构：id/epoch/ok/code/message/payload 都在一层，
/// 免得 serde 的内外标签组合在两端解析出不一致的形状）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub epoch: u64,
    pub protocol: u32,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<WorkerErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Payload>,
}

impl Response {
    pub fn ok(id: u64, epoch: u64, payload: Payload) -> Self {
        Self {
            id,
            epoch,
            protocol: PROTOCOL_VERSION,
            ok: true,
            code: None,
            message: None,
            payload: Some(payload),
        }
    }

    pub fn error(id: u64, epoch: u64, code: WorkerErrorCode, message: impl Into<String>) -> Self {
        Self {
            id,
            epoch,
            protocol: PROTOCOL_VERSION,
            ok: false,
            code: Some(code),
            message: Some(message.into()),
            payload: None,
        }
    }
}

/// 写一帧。长度超过 [`MAX_FRAME_BYTES`] 直接拒绝（不截断、不重试）。
pub fn write_frame<W: Write>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("帧过大：{} > {}", body.len(), MAX_FRAME_BYTES),
        ));
    }
    writer.write_all(&(body.len() as u32).to_be_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

/// 读一帧。对端声称的长度超限 → `InvalidData`，**不先分配**。
pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    reader.read_exact(&mut len)?;
    let declared = u32::from_be_bytes(len) as usize;
    if declared > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("帧长度超限：{declared} > {MAX_FRAME_BYTES}"),
        ));
    }
    let mut body = vec![0u8; declared];
    reader.read_exact(&mut body)?;
    Ok(body)
}

/// 编码请求（含长度前缀由 [`write_frame`] 负责，这里只产生 JSON）。
pub fn encode_request(envelope: &RequestEnvelope) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(envelope)
}

/// 解码响应。
pub fn decode_response(body: &[u8]) -> Result<Response, serde_json::Error> {
    serde_json::from_slice(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 帧可以往返且长度前缀正确() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"{\"a\":1}").unwrap();
        assert_eq!(&buf[..4], &(7u32).to_be_bytes());
        let mut cursor = std::io::Cursor::new(buf);
        let body = read_frame(&mut cursor).unwrap();
        assert_eq!(body, b"{\"a\":1}");
    }

    #[test]
    fn 超限的帧在分配前被拒绝() {
        // 声称 9 MiB：必须立刻报错，而不是先 vec![0; 9MiB]
        let mut cursor = std::io::Cursor::new((9u32 * 1024 * 1024).to_be_bytes().to_vec());
        let err = read_frame(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("超限"), "{err}");
    }

    #[test]
    fn 写超限帧被拒绝() {
        let mut buf = Vec::new();
        let err = write_frame(&mut buf, &vec![0u8; MAX_FRAME_BYTES + 1]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(buf.is_empty(), "拒绝时不该写出半帧");
    }

    #[test]
    fn 请求信封带操作名与协议版本() {
        let env = RequestEnvelope::new(7, 3, Request::Health);
        let json: serde_json::Value = serde_json::to_value(&env).unwrap();
        assert_eq!(json["op"], "health");
        assert_eq!(json["id"], 7);
        assert_eq!(json["epoch"], 3);
        assert_eq!(json["protocol"], PROTOCOL_VERSION);

        let back: RequestEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(back, env);
    }

    #[test]
    fn 响应缺失可选字段也能解析() {
        // 最小响应（模拟旧/简陋 worker）：其余字段必须能缺省
        let raw = r#"{"id":1,"epoch":1,"protocol":1,"ok":true}"#;
        let resp: Response = serde_json::from_str(raw).unwrap();
        assert!(resp.ok);
        assert!(resp.payload.is_none());
    }
}
