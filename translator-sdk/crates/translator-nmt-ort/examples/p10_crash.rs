//! P10.1-A4-4：**独立子进程**的进程级崩溃观测 PoC。
//!
//! 这不是“ORT 已具备进程隔离”的证明；它只验证父进程能观察到子进程异常退出、
//! 不把异常当成正常结果，也不影响父进程。生产 ORT 仍与宿主同进程。
//!
//! ```text
//! cargo run -p translator-nmt-ort --example p10_crash
//! ```
use std::process::Command;

#[cfg(windows)]
fn crash_child() -> ! {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn RaiseException(code: u32, flags: u32, nargs: u32, args: *const usize);
    }
    // 0xC0000005 = STATUS_ACCESS_VIOLATION，仅用于子进程实验。
    unsafe { RaiseException(0xC000_0005, 0, 0, std::ptr::null()) };
    std::process::abort();
}

#[cfg(not(windows))]
fn crash_child() -> ! {
    std::process::abort();
}

fn main() {
    if std::env::args().any(|a| a == "--child") {
        crash_child();
    }
    if std::env::args().any(|a| a == "--ok") {
        println!("child-ok");
        return;
    }

    let exe = std::env::current_exe().expect("current_exe");
    let crash = Command::new(&exe)
        .arg("--child")
        .output()
        .expect("spawn crash child");
    let normal = Command::new(&exe)
        .arg("--ok")
        .output()
        .expect("spawn normal child");
    let out = serde_json::json!({
        "schema": "p10.1-crash-poc/v1",
        "scope": "independent child process; NOT proof of ORT in-process isolation",
        "crash_child": {
            "status_ok": crash.status.success(),
            "status_debug": format!("{:?}", crash.status),
            "stdout": String::from_utf8_lossy(&crash.stdout).trim(),
        },
        "normal_child": {
            "status_ok": normal.status.success(),
            "status_debug": format!("{:?}", normal.status),
            "stdout": String::from_utf8_lossy(&normal.stdout).trim(),
        },
        "parent_survived": true,
        "orphan_possible": false,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
