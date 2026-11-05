//! P2.1 + P2.2 集成测试：清单协议、兼容性、安装/激活/回滚/清理。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use translator_resource::compat::{check, CompatibilityIssue, HostSupport};
use translator_resource::manifest::{PayloadSchema, ResourceManifest};
use translator_resource::store::{sha256_file, ResourceStore};
use translator_resource::{ResourceError, ResourceKind};

// ---------------- 构造测试包 ----------------

struct PackageSpec<'a> {
    id: &'a str,
    version: &'a str,
    /// 1 或 2
    manifest_schema: u32,
    payload: &'a str,
    kind: &'a str,
    /// 直接写进 manifest 的额外字段（用于构造非法包）
    extra_fields: &'a str,
    /// 篡改 payload 内容但不改 manifest（模拟损坏）
    tamper: bool,
    /// 不写某个文件（模拟缺文件）
    omit_file: Option<&'a str>,
}

impl<'a> PackageSpec<'a> {
    fn v2(id: &'a str, version: &'a str) -> Self {
        Self {
            id,
            version,
            manifest_schema: 2,
            payload: "dictionary.sqlite.v1",
            kind: "dictionary",
            extra_fields: "\"dependencies\": [], \"platforms\": [\"*\"]",
            tamper: false,
            omit_file: None,
        }
    }

    fn v1(id: &'a str, version: &'a str) -> Self {
        Self {
            manifest_schema: 1,
            ..Self::v2(id, version)
        }
    }
}

fn build_package(root: &Path, spec: PackageSpec<'_>) -> PathBuf {
    let dir = root.join(format!("pkg-{}-{}", spec.id, spec.version));
    std::fs::create_dir_all(&dir).unwrap();

    // payload 决定文件构成：词典是 fst+sqlite，规则是 json
    let files: [(&str, &[u8]); 2] = if spec.payload.starts_with("rules") {
        [("rules.json", b"fake-rules-json"), ("LICENSE.txt", b"CC0")]
    } else {
        [
            ("index.fst", b"fake-fst-bytes"),
            ("entries.sqlite", b"fake-sqlite-bytes"),
        ]
    };

    let mut entries = Vec::new();
    for (name, content) in files {
        let bytes: Vec<u8> = if spec.tamper {
            let mut v = content.to_vec();
            v.push(b'!');
            v
        } else {
            content.to_vec()
        };
        if Some(name) == spec.omit_file {
            // 清单里声明了，但磁盘上没有：用来验证"缺文件"必须报 IntegrityFailure
            entries.push(format!(
                r#"{{ "path": "{name}", "sha256": "{}", "bytes": {} }}"#,
                "0".repeat(64),
                content.len()
            ));
            continue;
        }
        std::fs::write(dir.join(name), &bytes).unwrap();
        entries.push(format!(
            r#"{{ "path": "{name}", "sha256": "{}", "bytes": {} }}"#,
            sha256_file(dir.join(name)).unwrap(),
            content.len()
        ));
    }

    let head = if spec.manifest_schema == 1 {
        "\"schema_version\": 1".to_string()
    } else {
        format!(
            "\"manifest_schema_version\": 2, \"payload_schema_version\": \"{}\"",
            spec.payload
        )
    };
    let extra = if spec.manifest_schema == 1 {
        String::new()
    } else {
        format!(", {}", spec.extra_fields)
    };

    let manifest = format!(
        r#"{{
            {head},
            "id": "{}",
            "version": "{}",
            "kind": "{}",
            "language": "zh-Hans",
            "target_languages": ["en"],
            "entry_count": 1,
            "provenance": {{
                "name": "TEST", "source_url": "https://example.invalid",
                "revision": "r1", "license_spdx": "CC0-1.0",
                "license_file": "LICENSE.txt", "retrieved_at": "2026-11-05"
            }},
            "files": [{}]{extra}
        }}"#,
        spec.id,
        spec.version,
        spec.kind,
        entries.join(", ")
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    std::fs::write(dir.join("LICENSE.txt"), b"CC0").unwrap();
    dir
}

fn store() -> (tempfile::TempDir, ResourceStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ResourceStore::open(dir.path()).unwrap();
    (dir, store)
}

// ---------------- 安装与激活 ----------------

#[test]
fn 安装后成为激活版本() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));

    let outcome = store.install_from_dir(&pkg).unwrap();
    assert!(outcome.activated, "首次安装应自动激活");
    assert_eq!(outcome.resource.version, "1.0.0");
    assert!(outcome.resource.path.exists());

    let status = store.inspect("dictionary.test").unwrap();
    assert_eq!(status.active.as_deref(), Some("1.0.0"));
    assert_eq!(status.versions.len(), 1);
    assert!(status.versions[0].active);
    assert_eq!(
        store.active_dir("dictionary.test").unwrap(),
        outcome.resource.path
    );
}

#[test]
fn 安装新版本不夺取激活位() {
    let (root, store) = store();
    let v1 = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let v2 = build_package(root.path(), PackageSpec::v2("dictionary.test", "2.0.0"));

    store.install_from_dir(&v1).unwrap();
    let outcome = store.install_from_dir(&v2).unwrap();

    assert!(!outcome.activated, "P2.2 不做隐式热切换");
    assert_eq!(outcome.previous_active.as_deref(), Some("1.0.0"));
    let status = store.inspect("dictionary.test").unwrap();
    assert_eq!(
        status.active.as_deref(),
        Some("1.0.0"),
        "激活版本必须保持不变"
    );
    assert_eq!(status.versions.len(), 2);
}

#[test]
fn 激活与回滚保留历史() {
    let (root, store) = store();
    let v1 = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let v2 = build_package(root.path(), PackageSpec::v2("dictionary.test", "2.0.0"));
    store.install_from_dir(&v1).unwrap();
    store.install_from_dir(&v2).unwrap();

    store.activate("dictionary.test", "2.0.0").unwrap();
    let status = store.inspect("dictionary.test").unwrap();
    assert_eq!(status.active.as_deref(), Some("2.0.0"));
    assert_eq!(status.history, vec!["1.0.0".to_string()]);

    let back = store.rollback("dictionary.test").unwrap();
    assert_eq!(back, "1.0.0");
    let status = store.inspect("dictionary.test").unwrap();
    assert_eq!(status.active.as_deref(), Some("1.0.0"));
    assert_eq!(status.history, vec!["2.0.0".to_string()]);

    // 两个版本互相成为历史：再回滚一次回到 2.0.0
    let again = store.rollback("dictionary.test").unwrap();
    assert_eq!(again, "2.0.0");
}

#[test]
fn 没有历史版本时回滚报错() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();
    let err = store.rollback("dictionary.test").unwrap_err();
    assert!(matches!(err, ResourceError::NotFound(_)), "实际：{err}");
}

#[test]
fn 激活不存在的版本报错() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();
    assert!(matches!(
        store.activate("dictionary.test", "9.9.9"),
        Err(ResourceError::NotInstalled { .. })
    ));
    assert!(matches!(
        store.activate("nope", "1.0.0"),
        Err(ResourceError::NotFound(_))
    ));
}

// ---------------- 校验失败不影响旧版本 ----------------

#[test]
fn 内容被篡改时拒绝安装且旧版本仍可用() {
    let (root, store) = store();
    let good = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&good).unwrap();

    let mut spec = PackageSpec::v2("dictionary.test", "2.0.0");
    spec.tamper = true;
    let bad = build_package(root.path(), spec);

    let err = store.install_from_dir(&bad).unwrap_err();
    assert!(
        matches!(err, ResourceError::IntegrityFailure { .. }),
        "实际：{err}"
    );

    let status = store.inspect("dictionary.test").unwrap();
    assert_eq!(
        status.active.as_deref(),
        Some("1.0.0"),
        "旧版本必须仍可激活"
    );
    assert_eq!(status.versions.len(), 1, "失败安装不得留下记录");
    assert!(store.active_dir("dictionary.test").unwrap().exists());
}

#[test]
fn 缺文件时拒绝安装() {
    let (root, store) = store();
    let mut spec = PackageSpec::v2("dictionary.test", "1.0.0");
    spec.omit_file = Some("entries.sqlite");
    let pkg = build_package(root.path(), spec);

    let err = store.install_from_dir(&pkg).unwrap_err();
    assert!(
        matches!(err, ResourceError::IntegrityFailure { .. }),
        "实际：{err}"
    );
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn 清单路径穿越时拒绝安装() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    // 直接改清单，让 files[].path 指向上级目录
    let path = pkg.join("manifest.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("index.fst", "../index.fst")).unwrap();

    let err = store.install_from_dir(pkg).unwrap_err();
    assert!(matches!(err, ResourceError::UnsafePath(_)), "实际：{err}");
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn 不支持的协议版本被拒收() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let path = pkg.join("manifest.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace(
            "\"manifest_schema_version\": 2",
            "\"manifest_schema_version\": 7",
        ),
    )
    .unwrap();

    let err = store.install_from_dir(pkg).unwrap_err();
    assert!(
        matches!(err, ResourceError::UnsupportedManifestSchema(7)),
        "实际：{err}"
    );
}

#[test]
fn 不支持的payload版本被拒收() {
    let (root, store) = store();
    let mut spec = PackageSpec::v2("dictionary.test", "1.0.0");
    spec.payload = "dictionary.sqlite.v9";
    let pkg = build_package(root.path(), spec);

    let err = store.install_from_dir(&pkg).unwrap_err();
    assert!(
        matches!(err, ResourceError::UnsupportedPayloadSchema(ref v) if v == "dictionary.sqlite.v9"),
        "实际：{err}"
    );
}

#[test]
fn 缺依赖时拒收() {
    let (root, store) = store();
    let mut spec = PackageSpec::v2("dictionary.test", "1.0.0");
    spec.extra_fields = "\"dependencies\": [{ \"id\": \"rules.base\", \"version\": \"1.0.0\" }]";
    let pkg = build_package(root.path(), spec);

    let err = store.install_from_dir(&pkg).unwrap_err();
    assert!(
        matches!(err, ResourceError::MissingDependency { ref id, .. } if id == "rules.base"),
        "实际：{err}"
    );
}

// ---------------- v1 兼容 ----------------

#[test]
fn v1旧包可以直接安装() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v1("dictionary.legacy", "0.1.0"));

    // 先确认它被解析成 v1 + 推断 payload
    let manifest =
        ResourceManifest::parse(&std::fs::read_to_string(pkg.join("manifest.json")).unwrap())
            .unwrap();
    assert!(manifest.is_v1());
    assert_eq!(manifest.payload_schema, PayloadSchema::DictionarySqliteV1);

    let outcome = store.install_from_dir(&pkg).expect("v1 包必须可安装");
    assert!(outcome.activated);
    let status = store.inspect("dictionary.legacy").unwrap();
    assert_eq!(status.active.as_deref(), Some("0.1.0"));
    assert_eq!(status.versions[0].payload_schema, "dictionary.sqlite.v1");
}

#[test]
fn 真实v1词典包若存在则必须被识别() {
    // resources/zh-en 是 P1 生成的完整 CC-CEDICT 包（gitignore，不一定存在）
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/zh-en");
    if !root.join("manifest.json").exists() {
        println!("[跳过] 未找到 {root:?}");
        return;
    }
    let manifest = translator_resource::read_manifest_from(&root).unwrap();
    assert!(manifest.is_v1(), "P1 生成的是 v1 清单");
    assert_eq!(manifest.payload_schema, PayloadSchema::DictionarySqliteV1);
    assert_eq!(manifest.kind, ResourceKind::Dictionary);
    let report = check(&manifest, &HostSupport::current(), &BTreeMap::new());
    assert!(report.is_compatible(), "{:?}", report.issues);
    // 逐文件校验必须通过（与词典包自身的 sha256 一致）
    translator_resource::verify_files(&root, &manifest).expect("真实包应通过校验");
}

// ---------------- 删除与清理 ----------------

#[test]
fn 激活版本不可删除() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();

    let err = store.uninstall("dictionary.test", "1.0.0").unwrap_err();
    assert!(
        matches!(err, ResourceError::ResourceInUse { .. }),
        "实际：{err}"
    );
    assert!(store.active_dir("dictionary.test").unwrap().exists());
}

#[test]
fn 非激活版本可以卸载() {
    let (root, store) = store();
    let v1 = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let v2 = build_package(root.path(), PackageSpec::v2("dictionary.test", "2.0.0"));
    store.install_from_dir(&v1).unwrap();
    store.install_from_dir(&v2).unwrap();

    store.uninstall("dictionary.test", "2.0.0").unwrap();
    let status = store.inspect("dictionary.test").unwrap();
    assert_eq!(status.versions.len(), 1);
    assert!(!status.versions.iter().any(|v| v.version == "2.0.0"));
}

#[test]
fn cleanup只删超出保留数的旧版本() {
    let (root, store) = store();
    for version in ["1.0.0", "2.0.0", "3.0.0", "4.0.0"] {
        let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", version));
        store.install_from_dir(&pkg).unwrap();
        store.activate("dictionary.test", version).unwrap();
    }
    // 现在 active=4.0.0，history=[3.0.0, 2.0.0, 1.0.0]
    let report = store.cleanup("dictionary.test", 1).unwrap();
    assert_eq!(report.removed, vec!["1.0.0", "2.0.0"]);
    assert!(report.kept_leased.is_empty());

    let status = store.inspect("dictionary.test").unwrap();
    assert_eq!(status.active.as_deref(), Some("4.0.0"));
    assert_eq!(status.versions.len(), 2, "保留激活版本 + 1 个历史版本");
}

#[test]
fn 卸载最后一个旧版本后资源记录消失() {
    let (root, store) = store();
    let v1 = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let v2 = build_package(root.path(), PackageSpec::v2("dictionary.test", "2.0.0"));
    store.install_from_dir(&v1).unwrap();
    store.install_from_dir(&v2).unwrap();
    store.activate("dictionary.test", "2.0.0").unwrap();

    store.uninstall("dictionary.test", "1.0.0").unwrap();
    let status = store.inspect("dictionary.test").unwrap();
    assert_eq!(status.versions.len(), 1);
    assert_eq!(status.active.as_deref(), Some("2.0.0"));
}

// ---------------- 崩溃恢复与状态 ----------------

#[test]
fn recover清理遗留staging目录() {
    let (root, store) = store();
    let staging = root.path().join("staging").join("leftover-1.0.0-123-456");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("partial.bin"), b"x").unwrap();

    let report = store.recover().unwrap();
    assert_eq!(report.removed_staging, 1);
    assert!(!staging.exists());
}

#[test]
fn 状态文件损坏时启动即报错() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("state.json"), "{ not json").unwrap();
    let err = ResourceStore::open(dir.path()).unwrap_err();
    assert!(matches!(err, ResourceError::CorruptState(_)), "实际：{err}");
}

#[test]
fn 状态文件未知schema时拒绝加载() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("state.json"),
        r#"{ "schema": 99, "resources": {} }"#,
    )
    .unwrap();
    let err = ResourceStore::open(dir.path()).unwrap_err();
    assert!(matches!(err, ResourceError::CorruptState(_)), "实际：{err}");
}

#[test]
fn 重复安装同一版本被拒() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();
    let err = store.install_from_dir(&pkg).unwrap_err();
    assert!(
        matches!(err, ResourceError::AlreadyInstalled { .. }),
        "实际：{err}"
    );
}

#[test]
fn 非法版本号不能当目录名() {
    let (root, store) = store();
    let mut spec = PackageSpec::v2("dictionary.test", "../evil");
    spec.version = "../evil";
    let pkg = build_package(root.path(), spec);
    let err = store.install_from_dir(&pkg).unwrap_err();
    assert!(
        matches!(err, ResourceError::InvalidVersion(_)),
        "实际：{err}"
    );
}

#[cfg(unix)]
#[test]
fn 符号链接被拒绝() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    std::os::unix::fs::symlink("/etc/passwd", pkg.join("link")).unwrap();

    let err = store.install_from_dir(&pkg).unwrap_err();
    assert!(
        matches!(err, ResourceError::SymlinkNotAllowed(_)),
        "实际：{err}"
    );
}

#[test]
fn 依赖满足时可以安装() {
    let (root, store) = store();
    // 先装 rules.base@1.0.0
    let mut base = PackageSpec::v2("rules.base", "1.0.0");
    base.kind = "rules";
    base.payload = "rules.json.v1";
    let base_pkg = build_package(root.path(), base);
    store.install_from_dir(&base_pkg).unwrap();

    let mut spec = PackageSpec::v2("dictionary.test", "1.0.0");
    spec.extra_fields = "\"dependencies\": [{ \"id\": \"rules.base\", \"version\": \"1.0.0\" }]";
    let pkg = build_package(root.path(), spec);
    store.install_from_dir(&pkg).expect("依赖已满足应能安装");
}

#[test]
fn 平台不匹配时拒收() {
    let (root, store) = store();
    let mut spec = PackageSpec::v2("dictionary.test", "1.0.0");
    spec.extra_fields = "\"dependencies\": [], \"platforms\": [\"plan9-mips\"]";
    let pkg = build_package(root.path(), spec);

    let err = store.install_from_dir(&pkg).unwrap_err();
    assert!(
        matches!(err, ResourceError::UnsupportedPlatform(_)),
        "实际：{err}"
    );
}

#[test]
fn 兼容性报告能同时报出多个问题() {
    let (root, _store) = store();
    let mut spec = PackageSpec::v2("dictionary.test", "1.0.0");
    spec.payload = "dictionary.sqlite.v9";
    spec.extra_fields =
        "\"dependencies\": [{ \"id\": \"rules.base\" }], \"platforms\": [\"plan9-mips\"]";
    let pkg = build_package(root.path(), spec);
    let manifest = translator_resource::read_manifest_from(&pkg).unwrap();

    let report = check(&manifest, &HostSupport::current(), &BTreeMap::new());
    assert!(report.issues.len() >= 3, "{:?}", report.issues);
    assert!(report
        .issues
        .iter()
        .any(|i| matches!(i, CompatibilityIssue::UnsupportedPayloadSchema(_))));
    assert!(report
        .issues
        .iter()
        .any(|i| matches!(i, CompatibilityIssue::MissingDependency { .. })));
}

// ============================================================
// P2.3：租约
// ============================================================

#[test]
fn 租约保护非激活版本不被卸载() {
    let (root, store) = store();
    let v1 = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let v2 = build_package(root.path(), PackageSpec::v2("dictionary.test", "2.0.0"));
    store.install_from_dir(&v1).unwrap();
    store.install_from_dir(&v2).unwrap();
    store.activate("dictionary.test", "2.0.0").unwrap();

    let lease = store
        .acquire_lease("dictionary.test", "1.0.0", "test")
        .unwrap();
    let err = store.uninstall("dictionary.test", "1.0.0").unwrap_err();
    match &err {
        ResourceError::ResourceLeased { holders, .. } => {
            assert_eq!(holders.len(), 1);
            assert!(holders[0].contains("holder=test"), "实际：{holders:?}");
        }
        other => panic!("期望 ResourceLeased，实际 {other:?}"),
    }
    let _ = err;

    // 释放后就可以删了
    lease.release().unwrap();
    store.uninstall("dictionary.test", "1.0.0").unwrap();
}

#[test]
fn 租约文件随drop消失并清理空目录() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();

    {
        let _lease = store
            .acquire_lease("dictionary.test", "1.0.0", "test")
            .unwrap();
        assert_eq!(store.all_leases().unwrap().len(), 1);
        assert!(root.path().join("leases/dictionary.test/1.0.0").exists());
    }
    assert!(store.all_leases().unwrap().is_empty());
    assert!(
        !root.path().join("leases/dictionary.test").exists(),
        "空目录应被顺手清掉"
    );
}

#[test]
fn 租约不阻止安装与激活() {
    let (root, store) = store();
    let v1 = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let v2 = build_package(root.path(), PackageSpec::v2("dictionary.test", "2.0.0"));
    store.install_from_dir(&v1).unwrap();
    let _lease = store
        .acquire_lease("dictionary.test", "1.0.0", "test")
        .unwrap();

    // 装新版本、切换激活都不该受旧版本租约影响
    store.install_from_dir(&v2).unwrap();
    store.activate("dictionary.test", "2.0.0").unwrap();
    assert_eq!(
        store.inspect("dictionary.test").unwrap().active.as_deref(),
        Some("2.0.0")
    );
    // 旧版本仍然可读
    assert!(store
        .version_dir("dictionary.test", "1.0.0")
        .unwrap()
        .exists());
}

#[test]
fn cleanup保留被租用的版本并如实报告() {
    let (root, store) = store();
    for version in ["1.0.0", "2.0.0", "3.0.0", "4.0.0"] {
        let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", version));
        store.install_from_dir(&pkg).unwrap();
        store.activate("dictionary.test", version).unwrap();
    }
    let _lease = store
        .acquire_lease("dictionary.test", "1.0.0", "test")
        .unwrap();

    // active=4.0.0，history=[3.0.0, 2.0.0, 1.0.0]；keep=1 时 1.0.0/2.0.0 是候选
    let report = store.cleanup("dictionary.test", 1).unwrap();
    assert_eq!(report.removed, vec!["2.0.0"]);
    assert_eq!(report.kept_leased.len(), 1);
    assert_eq!(report.kept_leased[0].0, "1.0.0");
    assert!(store
        .version_dir("dictionary.test", "1.0.0")
        .unwrap()
        .exists());
}

#[test]
fn 未安装的版本不能登记租约() {
    let (_root, store) = store();
    let err = store
        .acquire_lease("dictionary.test", "9.9.9", "test")
        .unwrap_err();
    assert!(
        matches!(err, ResourceError::NotInstalled { .. }),
        "实际：{err}"
    );
}

#[test]
fn recover回收死进程的租约() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();

    // 手工造一个"上个进程崩溃留下"的租约
    let dir = root.path().join("leases/dictionary.test/1.0.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("4194303-1.json"),
        r#"{"pid":4194303,"holder":"ghost","acquired_at_unix":0}"#,
    )
    .unwrap();

    #[cfg(target_os = "linux")]
    {
        let report = store.recover().unwrap();
        assert_eq!(report.reclaimed_leases, 1);
        assert!(store.all_leases().unwrap().is_empty());
    }
    #[cfg(not(target_os = "linux"))]
    {
        // 非 Linux 无法判定存活：必须保留，绝不猜
        let report = store.recover().unwrap();
        assert_eq!(report.reclaimed_leases, 0);
        assert_eq!(store.all_leases().unwrap().len(), 1);
    }
}

#[test]
fn 多个租约按pid稳定排序() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();

    let a = store
        .acquire_lease("dictionary.test", "1.0.0", "a")
        .unwrap();
    let b = store
        .acquire_lease("dictionary.test", "1.0.0", "b")
        .unwrap();
    let leases = store.leases("dictionary.test", "1.0.0").unwrap();
    assert_eq!(leases.len(), 2);
    assert!(leases.iter().all(|l| l.is_active()));
    assert_eq!(leases[0].pid, leases[1].pid);
    // 同 pid 时按 holder 名字排（a 在 b 前）
    assert_eq!(leases[0].holder, "a");
    drop((a, b));
}

// ============================================================
// P2.3：跨进程锁
// ============================================================

#[test]
fn 另一个持有者占着状态锁时安装会超时且不留残骸() {
    let (root, store) = store();
    let held = store.lock_state("外部备份工具").unwrap();

    let other = ResourceStore::open_with_timeout(root.path(), std::time::Duration::from_millis(80))
        .unwrap();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let err = other.install_from_dir(&pkg).unwrap_err();
    match err {
        ResourceError::LockTimeout { holder, .. } => {
            assert!(holder.contains("外部备份工具"), "实际：{holder}");
        }
        other => panic!("期望 LockTimeout，实际 {other:?}"),
    }

    // 失败不得留下 staging 残骸或半个安装
    let staging = std::fs::read_dir(root.path().join("staging"))
        .unwrap()
        .count();
    assert_eq!(staging, 0, "失败后 staging 必须清空");
    assert!(!root.path().join("installed/dictionary.test/1.0.0").exists());

    drop(held);
    // 锁释放后同一个包可以正常安装
    other.install_from_dir(&pkg).unwrap();
}

#[test]
fn 状态锁释放后可被下一个持有者获得() {
    let (_root, store) = store();
    let first = store.lock_state("first").unwrap();
    let path = first.path().to_path_buf();
    assert!(path.exists());
    drop(first);
    assert!(!path.exists());
    let _second = store.lock_state("second").unwrap();
}

#[test]
fn 并发安装同一版本只有一次成功() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    let s1 = ResourceStore::open(root.path()).unwrap();
    let s2 = ResourceStore::open(root.path()).unwrap();
    let p1 = pkg.clone();
    let p2 = pkg.clone();
    let t1 = std::thread::spawn(move || s1.install_from_dir(&p1).is_ok());
    let t2 = std::thread::spawn(move || s2.install_from_dir(&p2).is_ok());
    let (a, b) = (t1.join().unwrap(), t2.join().unwrap());
    assert!(a ^ b, "必须恰好一次成功，实际 a={a} b={b}");
    assert!(store.inspect("dictionary.test").unwrap().versions.len() == 1);
}

// ============================================================
// P2-S / S-05：三方对账（audit）与显式清理
//
// 对账的定位是"给用户一个能自查的入口"，所以这里断言的重点不是"能修"，
// 而是"报得准、并且不擅自替用户修"。
// ============================================================

#[test]
fn 新建仓库对账是干净的() {
    let (_root, store) = store();
    let audit = store.audit().unwrap();
    assert!(audit.is_clean(), "{}", audit.summary());
    assert!(!audit.needs_attention());
    assert_eq!(audit.lease_files, 0);
    assert!(audit.summary().contains("一致"));
}

#[test]
fn 孤儿目录被报出并可显式清理() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();

    // 手工造一个"目录在、状态里没有"的孤儿（模拟安装到一半被中断 / 状态回滚）
    let orphan = root.path().join("installed/dictionary.test/9.9.9");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("index.fst"), b"leftover").unwrap();

    let audit = store.audit().unwrap();
    assert!(!audit.is_clean());
    assert!(audit.needs_attention(), "孤儿目录需要人确认");
    assert_eq!(audit.orphan_dirs.len(), 1, "{}", audit.summary());
    assert_eq!(audit.orphan_dirs[0].version, "9.9.9");
    assert!(audit.summary().contains("孤儿"));

    // 只对账不修：再问一次还是同样的结果
    assert_eq!(store.audit().unwrap().orphan_dirs.len(), 1);

    let removed = store.prune_orphans().unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].version, "9.9.9");
    assert!(!orphan.exists());
    assert!(store.audit().unwrap().is_clean());
    // 正常版本一点没动
    assert!(store
        .version_dir("dictionary.test", "1.0.0")
        .unwrap()
        .exists());
}

#[test]
fn 目录丢失被报成缺失且不会自动删记录() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();

    // 模拟"人手工删了目录"或"文件系统丢了目录项"
    std::fs::remove_dir_all(root.path().join("installed/dictionary.test/1.0.0")).unwrap();

    let audit = store.audit().unwrap();
    assert!(!audit.is_clean());
    assert!(audit.needs_attention());
    assert_eq!(audit.missing_dirs.len(), 1);
    assert_eq!(audit.missing_dirs[0].version, "1.0.0");
    assert_eq!(
        audit.active_missing,
        vec!["dictionary.test@1.0.0".to_string()],
        "激活版本目录缺失必须单独报出来"
    );
    assert!(
        audit.summary().contains("激活版本目录缺失"),
        "{}",
        audit.summary()
    );

    // 缺失方向不自动修：状态记录还在（用户可以选择重新安装或手工清理）
    assert_eq!(store.inspect("dictionary.test").unwrap().versions.len(), 1);
    assert!(
        store.prune_orphans().unwrap().is_empty(),
        "别把缺失当孤儿删"
    );
    assert_eq!(store.audit().unwrap().missing_dirs.len(), 1);
}

#[test]
fn 半成品残留可被对账发现并自动回收() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();

    // staging 半成品 + 写状态时崩溃残留的临时文件
    let staging = root.path().join("staging/dictionary.test-2.0.0-4242-abc");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("manifest.json"), b"{}").unwrap();
    let tmp = root.path().join("state.json.tmp-4242-abc");
    std::fs::write(&tmp, b"{}").unwrap();

    let audit = store.audit().unwrap();
    assert_eq!(audit.staging_dirs.len(), 1);
    assert_eq!(audit.stale_tmp_files.len(), 1);
    assert!(!audit.is_clean());
    assert!(
        !audit.needs_attention(),
        "半成品是自动可清的，不该要求人工介入"
    );

    let report = store.recover().unwrap();
    assert_eq!(report.removed_staging, 1);
    assert_eq!(report.removed_temp_files, 1);
    assert!(!staging.exists());
    assert!(!tmp.exists());
    assert!(store.audit().unwrap().is_clean());
}

#[test]
fn 被租用的孤儿目录不会被清理() {
    let (root, store) = store();
    let pkg = build_package(root.path(), PackageSpec::v2("dictionary.test", "1.0.0"));
    store.install_from_dir(&pkg).unwrap();

    let orphan = root.path().join("installed/dictionary.test/9.9.9");
    std::fs::create_dir_all(&orphan).unwrap();
    // 孤儿目录也可能正被别人 mmap：手写一条"本进程"的租约（本进程一定活着）
    let lease_dir = root.path().join("leases/dictionary.test/9.9.9");
    std::fs::create_dir_all(&lease_dir).unwrap();
    std::fs::write(
        lease_dir.join(format!("{}-1.json", std::process::id())),
        format!(
            r#"{{"pid":{},"holder":"other-process","acquired_at_unix":0}}"#,
            std::process::id()
        ),
    )
    .unwrap();

    let audit = store.audit().unwrap();
    assert_eq!(audit.orphan_dirs.len(), 1);
    assert_eq!(audit.lease_files, 1);
    assert_eq!(audit.stale_leases, 0, "活进程的租约不算失效");

    let removed = store.prune_orphans().unwrap();
    assert!(removed.is_empty(), "被租用的目录不得删除");
    assert!(orphan.exists(), "目录必须还在");
    // 租约本身也留着，由持有人自己释放
    assert_eq!(store.all_leases().unwrap().len(), 1);
}
