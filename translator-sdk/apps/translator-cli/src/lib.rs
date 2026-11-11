//! 命令行客户端的内核部分。
//!
//! 目前只有本地 Web 界面（`serve`）需要被集成测试直接驱动，
//! 因此单独抽出 lib target；CLI 的参数解析仍在 `main.rs`。

pub mod serve;
