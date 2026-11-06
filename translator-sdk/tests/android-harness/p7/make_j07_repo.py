#!/usr/bin/env python3
"""构造 J-07 证据仓库：先装好 v1，再逐个尝试坏包并断言被拒绝。

输出：
  /tmp/p7repo                 推送到模拟器的仓库（只含合法的 v1）
  tests/android-harness/out/j07-install.log  每条命令与退出码
"""
from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parents[3]
OUT = SDK / "tests/android-harness/out"
WORK = Path("/tmp/p7-j07-work")
REPO = Path("/tmp/p7repo")
CLI = SDK / "target/release/translator-cli"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_pkg(dir: Path, ident: str, version: str, files: dict[str, bytes],
              manifest_schema: int = 2, payload_schema: str = "dictionary.sqlite.v1",
              file_paths: dict[str, str] | None = None) -> Path:
    dir.mkdir(parents=True, exist_ok=True)
    entries = []
    for name, content in files.items():
        path = dir / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        entries.append({
            "path": (file_paths or {}).get(name, name),
            "sha256": sha256(path),
            "bytes": len(content),
        })
    (dir / "LICENSE.txt").write_bytes(b"CC0")
    manifest = {
        "manifest_schema_version": manifest_schema,
        "payload_schema_version": payload_schema,
        "id": ident,
        "version": version,
        "kind": "dictionary",
        "language": "zh-Hans",
        "target_languages": ["en"],
        "entry_count": 1,
        "provenance": {
            "name": "P7-J07", "source_url": "https://example.invalid",
            "revision": "r1", "license_spdx": "CC0-1.0",
            "license_file": "LICENSE.txt", "retrieved_at": "2026-11-07",
        },
        "files": entries,
        "dependencies": [],
        "platforms": ["*"],
    }
    (dir / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    return dir


def run(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(args, text=True, capture_output=True, encoding="utf-8", errors="replace")


def main() -> int:
    if not CLI.exists():
        print(f"缺少 {CLI}，先 cargo build --release -p translator-cli", file=sys.stderr)
        return 2
    shutil.rmtree(WORK, ignore_errors=True)
    shutil.rmtree(REPO, ignore_errors=True)
    WORK.mkdir(parents=True)
    REPO.mkdir(parents=True)
    OUT.mkdir(parents=True, exist_ok=True)

    log: list[str] = []

    def install(pkg: Path, expect_ok: bool, label: str) -> None:
        proc = run([str(CLI), "--store", str(REPO), "resource", "install", str(pkg)])
        ok = proc.returncode == 0
        log.append(f"$ translator-cli --store {REPO} resource install {pkg}")
        log.append(f"[{label}] exit={proc.returncode} expect_ok={expect_ok}")
        log.append((proc.stdout + proc.stderr).strip())
        if ok != expect_ok:
            raise SystemExit(f"{label}: 退出码 {proc.returncode} 与期望不符\n{proc.stdout}\n{proc.stderr}")

    good = write_pkg(WORK / "good", "dictionary.p7.base", "1.0.0",
                     {"index.fst": b"p7-fst", "entries.sqlite": b"p7-sqlite"})
    install(good, True, "J-07 baseline v1")

    # a) 改一个字节（保持大小）
    tampered = write_pkg(WORK / "tampered", "dictionary.p7.base", "2.0.0",
                         {"index.fst": b"p7-fst-v2", "entries.sqlite": b"p7-sqlite-v2"})
    data = bytearray((tampered / "entries.sqlite").read_bytes())
    data[-1] ^= 0xFF
    (tampered / "entries.sqlite").write_bytes(bytes(data))
    install(tampered, False, "J-07a tampered sha256")

    # b) 缺文件：先按含 entries.sqlite 生成清单，再删掉
    missing = write_pkg(WORK / "missing", "dictionary.p7.base", "2.0.0",
                        {"index.fst": b"p7-fst-v2", "entries.sqlite": b"p7-sqlite-v2"})
    (missing / "entries.sqlite").unlink()
    install(missing, False, "J-07b missing file")

    # c) 未知 schema
    unknown_m = write_pkg(WORK / "unknown-manifest", "dictionary.p7.schema", "1.0.0",
                          {"entries.sqlite": b"x"}, manifest_schema=999)
    install(unknown_m, False, "J-07c unknown manifest schema")
    unknown_p = write_pkg(WORK / "unknown-payload", "dictionary.p7.payload", "1.0.0",
                          {"entries.sqlite": b"x"}, payload_schema="dictionary.sqlite.v9")
    install(unknown_p, False, "J-07c unknown payload schema")

    # d) 路径穿越：id 与包内路径
    bad_id = write_pkg(WORK / "bad-id", "../evil", "1.0.0", {"entries.sqlite": b"x"})
    install(bad_id, False, "J-07d traversal id")
    bad_file = write_pkg(WORK / "bad-file", "dictionary.p7.traversal", "1.0.0",
                         {"entries.sqlite": b"x"}, file_paths={"entries.sqlite": "../evil"})
    install(bad_file, False, "J-07d traversal file path")

    # 最终仓库必须只有 v1 且 active=1.0.0，仓库外不能出现 evil
    listing = run([str(CLI), "--store", str(REPO), "resource", "list"])
    log.append("$ translator-cli --store {REPO} resource list")
    log.append(listing.stdout.strip() + listing.stderr.strip())
    if listing.returncode != 0 or "dictionary.p7.base" not in listing.stdout:
        raise SystemExit("仓库清单里缺少合法 v1")
    if "evil" in listing.stdout or "traversal" in listing.stdout:
        raise SystemExit("坏包进入了仓库")
    if Path("/tmp/evil").exists() or (REPO.parent / "evil").exists():
        raise SystemExit("仓库外出现了 evil 路径")

    (OUT / "j07-install.log").write_text("\n".join(log) + "\n", encoding="utf-8")
    print(f"OK repo={REPO} log={OUT / 'j07-install.log'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
