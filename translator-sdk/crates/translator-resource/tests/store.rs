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
    let removed = store.cleanup("dictionary.test", 1).unwrap();
    assert_eq!(removed, vec!["1.0.0", "2.0.0"]);

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
