//! 临时诊断：打印 ORT 会话的真实输入/输出名，核对 `initial_past()` 的名字与形状。
//! 只在手动排障时跑：
//! `P4_ORT_MODEL_DIR=... cargo run -p translator-nmt-ort --features ort-runtime --example dump_io`

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var("P4_ORT_MODEL_DIR").expect("P4_ORT_MODEL_DIR"));
    let env = ort::init().with_name("dump").build().unwrap();
    for (label, file) in [
        ("encoder", "encoder_model_quantized.onnx"),
        ("decoder", "decoder_model_merged_quantized.onnx"),
    ] {
        let mut b = ort::session::Session::builder(&env).unwrap();
        b = b.with_intra_threads(1).unwrap();
        let s = b.commit_from_file(dir.join(file)).unwrap();
        println!("=== {label}: inputs ===");
        for i in s.inputs() {
            println!("  in  {}", i.name());
        }
        println!("=== {label}: outputs ===");
        for o in s.outputs() {
            println!("  out {}", o.name());
        }
    }
}
