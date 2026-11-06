//! P3.2 资源安全：反向依赖（LC-07）+ 安装关键点崩溃注入（LC-08）。
//!
//! LC-08 必须是**独立进程**：崩溃注入靠 `abort()` 跳过 `Drop`，同进程里做会带走整个测试。
//! 父测试用 `std::env::current_exe()` 重新执行本测试二进制，只跑 `child_install_with_crash`
//! 那一个 `#[ignore]` 子测试，用环境变量告诉它崩溃点。
//!
//! 运行：`cargo test -p translator-resource --test p32_lifecycle`

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use translator_resource::store::{sha256_file, ResourceStore};
use translator_resource::ResourceError;

const ROOT_ENV: &str = "P32_STORE_ROOT";
const PKG_ENV: &str = "P32_PACKAGE";
const CRASH_ENV: &str = "TRANSLATOR_TEST_CRASH_POINT";

// ---------------------------------------------------------------- 夹具

/// 造一个最小合法资源包；`deps` 是 `(id, 版本要求)`。
fn build_package(root: &Path, id: &str, version: &str, deps: &[(&str, Option<&str>)]) -> PathBuf {
    let dir = root.join(format!("pkg-{id}-{version}"));
    std::fs::create_dir_all(&dir).unwrap();

    let mut entries = Vec::new();
    for (name, content) in [
        ("index.fst", b"fake-fst-bytes".as_slice()),
        ("entries.sqlite", b"fake-sqlite-bytes".as_slice()),
    ] {
        std::fs::write(dir.join(name), content).unwrap();
        entries.push(format!(
            r#"{{ "path": "{name}", "sha256": "{}", "bytes": {} }}"#,
            sha256_file(dir.join(name)).unwrap(),
            content.len()
        ));
    }
    std::fs::write(dir.join("LICENSE.txt"), b"CC0").unwrap();

    let deps_json: Vec<String> = deps
        .iter()
        .map(|(dep_id, ver)| match ver {
            Some(v) => format!(r#"{{ "id": "{dep_id}", "version": "{v}" }}"#),
            None => format!(r#"{{ "id": "{dep_id}" }}"#),
        })
        .collect();

    let manifest = format!(
        r#"{{
            "manifest_schema_version": 2,
            "payload_schema_version": "dictionary.sqlite.v1",
            "id": "{id}",
            "version": "{version}",
            "kind": "dictionary",
            "language": "zh-Hans",
            "target_languages": ["en"],
            "entry_count": 1,
            "provenance": {{
                "name": "TEST", "source_url": "https://example.invalid",
                "revision": "r1", "license_spdx": "CC0-1.0",
                "license_file": "LICENSE.txt", "retrieved_at": "2026-11-05"
            }},
            "files": [{}],
            "dependencies": [{}],
            "platforms": ["*"]
        }}"#,
        entries.join(", "),
        deps_json.join(", ")
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    dir
}

fn store(root: &Path) -> ResourceStore {
    ResourceStore::open_with_timeout(root, Duration::from_millis(800)).unwrap()
}

// ---------------------------------------------------------------- LC-07

#[test]
fn lc07_被依赖的版本不可卸载() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let s = store(root);

    s.install_from_dir(build_package(root, "dictionary.base", "1.0.0", &[]))
        .unwrap();
    // dependent 点名要 base 1.0.0；安装时 base 1.0.0 是激活版本，兼容性检查通过
    s.install_from_dir(build_package(
        root,
        "dictionary.dependent",
        "1.0.0",
        &[("dictionary.base", Some("1.0.0"))],
    ))
    .unwrap();
    s.install_from_dir(build_package(root, "dictionary.base", "2.0.0", &[]))
        .unwrap();
    s.activate("dictionary.base", "2.0.0").unwrap();

    let err = s
        .uninstall("dictionary.base", "1.0.0")
        .expect_err("被依赖的版本必须拒绝卸载");
    match err {
        ResourceError::ReversedDependency {
            id,
            version,
            dependents,
        } => {
            assert_eq!(id, "dictionary.base");
            assert_eq!(version, "1.0.0");
            assert_eq!(dependents, vec!["dictionary.dependent@1.0.0".to_string()]);
        }
        other => panic!("期望 ReversedDependency，实际 {other:?}"),
    }
    // 目录必须还在（拒绝 = 什么都没动）
    assert!(s.version_dir("dictionary.base", "1.0.0").unwrap().exists());

    // 依赖方改成要 base 2.0.0，并把新版本激活；老版本成为可清理的历史
    s.install_from_dir(build_package(
        root,
        "dictionary.dependent",
        "2.0.0",
        &[("dictionary.base", Some("2.0.0"))],
    ))
    .unwrap();
    s.activate("dictionary.dependent", "2.0.0").unwrap();
    s.cleanup("dictionary.dependent", 0).unwrap();
    // 依赖解除之后允许回收
    s.uninstall("dictionary.base", "1.0.0").unwrap();
    assert!(!s.version_dir("dictionary.base", "1.0.0").unwrap().exists());
}

#[test]
fn lc07_清理保留被依赖的版本而不是删掉() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let s = store(root);

    s.install_from_dir(build_package(root, "dictionary.base", "1.0.0", &[]))
        .unwrap();
    s.install_from_dir(build_package(
        root,
        "dictionary.dependent",
        "1.0.0",
        &[("dictionary.base", Some("1.0.0"))],
    ))
    .unwrap();
    s.install_from_dir(build_package(root, "dictionary.base", "2.0.0", &[]))
        .unwrap();
    s.activate("dictionary.base", "2.0.0").unwrap();

    let report = s.cleanup("dictionary.base", 0).unwrap();
    assert!(report.removed.is_empty(), "不该删任何版本：{report:?}");
    assert_eq!(
        report.kept_dependents,
        vec![(
            "1.0.0".to_string(),
            vec!["dictionary.dependent@1.0.0".to_string()]
        )]
    );
    assert!(s.version_dir("dictionary.base", "1.0.0").unwrap().exists());
}

#[test]
fn lc07_依赖缺口会在对账里报出来() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let s = store(root);

    s.install_from_dir(build_package(root, "dictionary.base", "1.0.0", &[]))
        .unwrap();
    s.install_from_dir(build_package(
        root,
        "dictionary.dependent",
        "1.0.0",
        &[("dictionary.base", Some("1.0.0"))],
    ))
    .unwrap();
    assert!(s.audit().unwrap().broken_dependencies.is_empty());

    // 绕过 SDK 直接删目录（模拟外部误删）：对账必须指出依赖缺口
    std::fs::remove_dir_all(s.version_dir("dictionary.base", "1.0.0").unwrap()).unwrap();
    let audit = s.audit().unwrap();
    assert!(
        audit
            .broken_dependencies
            .iter()
            .any(|line| line.contains("dictionary.dependent@1.0.0")),
        "{:?}",
        audit.broken_dependencies
    );
    assert!(audit.needs_attention());
}

// ---------------------------------------------------------------- LC-08

/// 子进程角色：按环境变量指定的崩溃点装一个包，然后按注入点死掉。
#[test]
#[ignore = "由父测试重新执行本二进制时使用"]
fn child_install_with_crash() {
    // 只有在被父测试点名（环境变量齐全）时才真的干活；
    // 否则（例如 `cargo test -- --ignored` 全量跑）直接跳过，不让辅助角色污染结果。
    let (Ok(root), Ok(pkg)) = (std::env::var(ROOT_ENV), std::env::var(PKG_ENV)) else {
        return;
    };
    let s = store(Path::new(&root));
    // 崩溃点由 TRANSLATOR_TEST_CRASH_POINT 决定；正常情况下这里会返回 Ok
    let outcome = s.install_from_dir(&pkg);
    println!("CHILD_RESULT={outcome:?}");
}

/// 子进程角色：到达指定崩溃点后写 marker，等父进程从**外部** SIGKILL（G08）。
///
/// 返回子进程的退出状态；Unix 上额外由调用方断言 `signal == SIGKILL`。
fn run_child(root: &Path, pkg: &Path, crash: &str, marker: &Path) -> std::process::ExitStatus {
    let _ = std::fs::remove_file(marker);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--ignored")
        .arg("--exact")
        .arg("child_install_with_crash")
        .arg("--test-threads=1")
        .env(ROOT_ENV, root)
        .env(PKG_ENV, pkg)
        .env(CRASH_ENV, crash)
        .env("TRANSLATOR_TEST_CRASH_MARKER", marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("无法启动子进程");

    // 等子进程真正到达崩溃点（marker 出现）；最多 10 s。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if marker.exists() {
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(marker.exists(), "{crash}：子进程没有到达崩溃点");

    // 外部 kill -9：父进程发 SIGKILL（Windows 用 TerminateProcess 等价）。
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    child.wait().expect("等待子进程失败")
}

// 两个平台都跑：崩溃子进程留下的 `state.lock` 必须被判死并回收。
// （Windows 侧靠 `OpenProcess` + `GetExitCodeProcess` + `GetProcessTimes` 创建时间，
//  见 `leases.rs` 的 `win_probe`；P5.4 之前这里是 ignore，因为那时 probe() 恒为 Unknown。）
#[test]
fn lc08_安装关键点被外部sigkill杀死后仓库仍可修复且旧版本不损坏() {
    let points = [
        "after-staging",
        "before-rename",
        "after-rename",
        "before-state",
        "after-state",
    ];
    for point in points {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // 先装好并激活 v1：崩溃安装 v2 时，v1 必须原样可用。
        let pkg_v1 = build_package(root, "dictionary.crash", "1.0.0", &[]);
        let pkg_v2 = build_package(root, "dictionary.crash", "2.0.0", &[]);
        let store0 = store(root);
        store0.install_from_dir(&pkg_v1).unwrap();
        let v1_manifest = translator_resource::store::read_manifest_from(
            store0.version_dir("dictionary.crash", "1.0.0").unwrap(),
        )
        .unwrap();
        let v1_dir = store0.version_dir("dictionary.crash", "1.0.0").unwrap();

        let marker = root.join(format!("crash-{point}.marker"));
        let status = run_child(root, &pkg_v2, point, &marker);
        assert!(!status.success(), "{point}：子进程必须被外部杀死");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(
                status.signal(),
                Some(libc::SIGKILL),
                "{point}：必须是外部 SIGKILL（signal=9），实际 {status:?}"
            );
        }

        let s = store(root);
        let audit_before = s.audit().unwrap();
        let recovered = s.recover().unwrap();
        let audit_after = s.audit().unwrap();
        println!(
            "{point}: 崩溃后 {}；recover={:?}；修复后 {}",
            audit_before.summary(),
            recovered,
            audit_after.summary()
        );
        assert!(
            audit_after.is_clean(),
            "{point}：recover 之后必须干净，实际 {}",
            audit_after.summary()
        );

        // 旧版本：仍然是激活版本、目录还在、逐文件摘要仍然正确。
        let inspect = s.inspect("dictionary.crash").unwrap();
        assert_eq!(
            inspect.active.as_deref(),
            Some("1.0.0"),
            "{point}：激活版本被改坏"
        );
        assert!(v1_dir.exists(), "{point}：旧版本目录被删");
        translator_resource::store::verify_files(&v1_dir, &v1_manifest)
            .unwrap_or_else(|e| panic!("{point}：旧版本内容损坏：{e}"));

        // 修复后必须能重新安装 v2（after-state 时 v2 已落账，重装必须报已安装）。
        match s.install_from_dir(&pkg_v2) {
            Ok(_) => assert_ne!(
                point, "after-state",
                "after-state 时状态已落盘，重装必须报已安装"
            ),
            Err(ResourceError::AlreadyInstalled { .. }) => {
                assert_eq!(point, "after-state", "{point}：不该报已安装");
            }
            Err(other) => panic!("{point}：重装失败 {other:?}"),
        }
        assert!(s.audit().unwrap().is_clean(), "{point}：重装后仓库必须一致");
        // v2 安装（或已安装）不应改变激活版本。
        assert_eq!(
            s.inspect("dictionary.crash").unwrap().active.as_deref(),
            Some("1.0.0"),
            "{point}：安装新版本不得隐式激活"
        );
    }
}
