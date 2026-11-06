//! P7.1 / J-07：坏资源包与路径穿越。
//!
//! 判据（`docs/43` §1.5）：受控拒绝、激活版本不受影响、不在仓库外产生文件。
//! 所有坏包都在 `staging/` 阶段失败；`recover()` 后仓库必须干净。

use std::path::{Path, PathBuf};

use translator_resource::store::{sha256_file, ResourceStore};
use translator_resource::ResourceError;

fn pkg_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 写一个合法 v2 词典包；`files` 为 (相对路径, 内容)。
fn write_package(dir: &Path, id: &str, version: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let mut entries = Vec::new();
    for (name, content) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        entries.push(format!(
            r#"{{ "path": "{name}", "sha256": "{}", "bytes": {} }}"#,
            sha256_file(&path).unwrap(),
            content.len()
        ));
    }
    std::fs::write(dir.join("LICENSE.txt"), b"CC0").unwrap();
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
            "dependencies": [],
            "platforms": ["*"]
        }}"#,
        entries.join(", ")
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    dir.to_path_buf()
}

fn store(root: &Path) -> ResourceStore {
    ResourceStore::open(root).unwrap()
}

fn valid_pkg(root: &Path) -> PathBuf {
    write_package(
        &pkg_dir(root, "pkg-good"),
        "dictionary.p7.base",
        "1.0.0",
        &[
            ("index.fst", b"fake-fst-v1"),
            ("entries.sqlite", b"fake-sqlite-v1"),
        ],
    )
}

fn assert_repo_still_has_active_v1(s: &ResourceStore) {
    let status = s.inspect("dictionary.p7.base").unwrap();
    assert_eq!(
        status.active.as_deref(),
        Some("1.0.0"),
        "激活版本被坏包改坏"
    );
    let dir = s
        .version_dir("dictionary.p7.base", "1.0.0")
        .expect("旧版本目录应存在");
    assert!(dir.join("entries.sqlite").exists());
    assert_eq!(
        std::fs::read(dir.join("entries.sqlite")).unwrap(),
        b"fake-sqlite-v1"
    );
    assert!(s.audit().unwrap().is_clean(), "坏包失败后仓库必须干净");
}

#[test]
fn j07a_篡改一个字节必须被拒绝且旧版本不受影响() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let s = store(root);
    s.install_from_dir(valid_pkg(root)).unwrap();

    let bad = pkg_dir(root, "pkg-tampered");
    let good = valid_pkg(&pkg_dir(root, "pkg-good2"));
    // 复制一个合法包再改内容（保持文件大小，专门触发 sha256 不符而不是长度不符）。
    for name in [
        "manifest.json",
        "index.fst",
        "entries.sqlite",
        "LICENSE.txt",
    ] {
        std::fs::copy(good.join(name), bad.join(name)).unwrap();
    }
    let text = std::fs::read_to_string(bad.join("manifest.json")).unwrap();
    std::fs::write(
        bad.join("manifest.json"),
        text.replace("\"version\": \"1.0.0\"", "\"version\": \"2.0.0\""),
    )
    .unwrap();
    let mut bytes = std::fs::read(bad.join("entries.sqlite")).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(bad.join("entries.sqlite"), bytes).unwrap();

    let err = s.install_from_dir(&bad).unwrap_err();
    assert!(
        matches!(err, ResourceError::IntegrityFailure { .. }),
        "期望 IntegrityFailure，实际 {err:?}"
    );
    assert_repo_still_has_active_v1(&s);
    // 坏包没有落位成 v2
    assert!(!root.join("installed/dictionary.p7.base/2.0.0").exists());
}

#[test]
fn j07b_缺文件必须被拒绝() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let s = store(root);
    s.install_from_dir(valid_pkg(root)).unwrap();

    let bad = pkg_dir(root, "pkg-missing");
    let good = valid_pkg(&pkg_dir(root, "pkg-good3"));
    for name in ["manifest.json", "index.fst", "LICENSE.txt"] {
        std::fs::copy(good.join(name), bad.join(name)).unwrap();
    }
    // 故意不复制 entries.sqlite：清单仍声明它。换成未安装的版本号，
    // 否则会先在 precheck 报 AlreadyInstalled，测不到完整性校验。
    let text = std::fs::read_to_string(bad.join("manifest.json")).unwrap();
    std::fs::write(
        bad.join("manifest.json"),
        text.replace("\"version\": \"1.0.0\"", "\"version\": \"2.0.0\""),
    )
    .unwrap();
    let err = s.install_from_dir(&bad).unwrap_err();
    assert!(
        matches!(err, ResourceError::IntegrityFailure { .. }),
        "期望 IntegrityFailure（缺文件），实际 {err:?}"
    );
    assert_repo_still_has_active_v1(&s);
}

#[test]
fn j07c_未知schema必须稳定拒绝() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let s = store(root);
    s.install_from_dir(valid_pkg(root)).unwrap();

    // manifest schema 999
    let bad = pkg_dir(root, "pkg-schema");
    write_package(
        &bad,
        "dictionary.p7.schema",
        "1.0.0",
        &[("entries.sqlite", b"x")],
    );
    let text = std::fs::read_to_string(bad.join("manifest.json")).unwrap();
    std::fs::write(
        bad.join("manifest.json"),
        text.replace(
            "\"manifest_schema_version\": 2",
            "\"manifest_schema_version\": 999",
        ),
    )
    .unwrap();
    let err = s.install_from_dir(&bad).unwrap_err();
    assert!(
        matches!(err, ResourceError::UnsupportedManifestSchema(999)),
        "期望 UnsupportedManifestSchema(999)，实际 {err:?}"
    );

    // payload schema 未知
    let bad2 = pkg_dir(root, "pkg-payload");
    write_package(
        &bad2,
        "dictionary.p7.payload",
        "1.0.0",
        &[("entries.sqlite", b"x")],
    );
    let text = std::fs::read_to_string(bad2.join("manifest.json")).unwrap();
    std::fs::write(
        bad2.join("manifest.json"),
        text.replace("dictionary.sqlite.v1", "dictionary.sqlite.v9"),
    )
    .unwrap();
    let err = s.install_from_dir(&bad2).unwrap_err();
    assert!(
        matches!(err, ResourceError::UnsupportedPayloadSchema(_)),
        "期望 UnsupportedPayloadSchema，实际 {err:?}"
    );
    assert_repo_still_has_active_v1(&s);
}

#[test]
fn j07d_路径穿越不产生仓库外文件() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let s = store(root);
    s.install_from_dir(valid_pkg(root)).unwrap();

    let outside = root.parent().unwrap().join("evil");
    let _ = std::fs::remove_dir_all(&outside);

    // id = ../evil → 目录名非法
    let bad_id = pkg_dir(root, "pkg-bad-id");
    write_package(&bad_id, "../evil", "1.0.0", &[("entries.sqlite", b"x")]);
    let err = s.install_from_dir(&bad_id).unwrap_err();
    assert!(
        matches!(err, ResourceError::InvalidVersion(_)),
        "id 穿越应报 InvalidVersion，实际 {err:?}"
    );

    // 清单里声明 ../x → UnsafePath
    let bad_file = pkg_dir(root, "pkg-bad-file");
    std::fs::write(bad_file.join("entries.sqlite"), b"x").unwrap();
    std::fs::write(bad_file.join("LICENSE.txt"), b"CC0").unwrap();
    let manifest = format!(
        r#"{{
            "manifest_schema_version": 2, "payload_schema_version": "dictionary.sqlite.v1",
            "id": "dictionary.p7.traversal", "version": "1.0.0", "kind": "dictionary",
            "language": "zh-Hans", "target_languages": ["en"], "entry_count": 1,
            "provenance": {{ "name": "T", "source_url": "https://example.invalid",
              "revision": "r1", "license_spdx": "CC0-1.0",
              "license_file": "LICENSE.txt", "retrieved_at": "2026-11-05" }},
            "files": [
              {{ "path": "../evil", "sha256": "{}", "bytes": 1 }},
              {{ "path": "entries.sqlite", "sha256": "{}", "bytes": 1 }}
            ],
            "dependencies": [], "platforms": ["*"]
        }}"#,
        sha256_file(bad_file.join("entries.sqlite")).unwrap(),
        sha256_file(bad_file.join("entries.sqlite")).unwrap(),
    );
    std::fs::write(bad_file.join("manifest.json"), manifest).unwrap();
    let err = s.install_from_dir(&bad_file).unwrap_err();
    assert!(
        matches!(err, ResourceError::UnsafePath(_)),
        "清单穿越应报 UnsafePath，实际 {err:?}"
    );

    // 绝对路径
    let bad_abs = pkg_dir(root, "pkg-bad-abs");
    std::fs::write(bad_abs.join("entries.sqlite"), b"x").unwrap();
    std::fs::write(bad_abs.join("LICENSE.txt"), b"CC0").unwrap();
    let manifest = format!(
        r#"{{
            "manifest_schema_version": 2, "payload_schema_version": "dictionary.sqlite.v1",
            "id": "dictionary.p7.abs", "version": "1.0.0", "kind": "dictionary",
            "language": "zh-Hans", "target_languages": ["en"], "entry_count": 1,
            "provenance": {{ "name": "T", "source_url": "https://example.invalid",
              "revision": "r1", "license_spdx": "CC0-1.0",
              "license_file": "LICENSE.txt", "retrieved_at": "2026-11-05" }},
            "files": [
              {{ "path": "/tmp/evil", "sha256": "{}", "bytes": 1 }},
              {{ "path": "entries.sqlite", "sha256": "{}", "bytes": 1 }}
            ],
            "dependencies": [], "platforms": ["*"]
        }}"#,
        sha256_file(bad_abs.join("entries.sqlite")).unwrap(),
        sha256_file(bad_abs.join("entries.sqlite")).unwrap(),
    );
    std::fs::write(bad_abs.join("manifest.json"), manifest).unwrap();
    let err = s.install_from_dir(&bad_abs).unwrap_err();
    assert!(
        matches!(err, ResourceError::UnsafePath(_)),
        "绝对路径应报 UnsafePath，实际 {err:?}"
    );

    assert!(!outside.exists(), "仓库外出现了文件：{}", outside.display());
    assert_repo_still_has_active_v1(&s);
}

#[cfg(unix)]
#[test]
fn j07d_符号链接被拒绝() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let s = store(root);
    s.install_from_dir(valid_pkg(root)).unwrap();

    let bad = pkg_dir(root, "pkg-symlink");
    write_package(
        &bad,
        "dictionary.p7.symlink",
        "1.0.0",
        &[("entries.sqlite", b"x")],
    );
    symlink("/etc/passwd", bad.join("leak")).unwrap();
    let err = s.install_from_dir(&bad).unwrap_err();
    assert!(
        matches!(err, ResourceError::SymlinkNotAllowed(_)),
        "期望 SymlinkNotAllowed，实际 {err:?}"
    );
    assert_repo_still_has_active_v1(&s);
}
