#!/usr/bin/env python3
"""发行红线检查（P3.4）。

`docs/17` §7.2 的 P3-G12 是**发布阻断项**：Mozilla 模型权重的再分发许可未确认。
口头承诺不算数，本脚本把它变成一条每次提交都会跑的检查：

  C1  已跟踪文件里没有权重 / native 二进制 / 大文件
  C2  第三方许可清单与依赖树一致（调用 tools/license-audit.py --check）
  C3  资产清单里的模型坐标与 models.lock.json、性能基线三者一致
  C4  `cargo package` 的文件列表里没有权重，且所有成员 crate 都禁止误发布
  C5  `.gitignore` 覆盖 resources / models / target
  C6  红线文案与清单文件确实存在（避免"删掉文档就算通过"）
  C7  若设置了 NMT_MODEL_DIR，逐个校验模型文件哈希（未设置则 SKIP）

退出码：0 全部 PASS/SKIP；1 有 FAIL。

用法：
    python3 tools/check-release.py
    python3 tools/check-release.py --verbose
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent
REPO = SDK.parent

WEIGHT_SUFFIXES = (
    ".bin",
    ".spm",
    ".onnx",
    ".npz",
    ".gguf",
    ".safetensors",
    ".pt",
    ".pth",
    ".marian",
    ".so",
    ".dylib",
    ".dll",
    ".a",
)
MAX_TRACKED_BYTES = 5 * 1024 * 1024  # 单文件 5 MiB：本仓库最大的正常文件是文档/基线 JSON

results: list[tuple[str, str, str]] = []  # (检查项, 状态, 详情)


def record(name: str, status: str, detail: str) -> None:
    results.append((name, status, detail))


def git(*args: str) -> str:
    out = subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} 失败：{out.stderr.strip()}")
    return out.stdout


def tracked_files() -> list[Path]:
    out = git("ls-files", "-z")
    return [REPO / p for p in out.split("\0") if p]


def check_c1() -> None:
    bad: list[str] = []
    for path in tracked_files():
        rel = path.relative_to(REPO)
        if path.suffix.lower() in WEIGHT_SUFFIXES:
            bad.append(f"{rel}（扩展名像权重/二进制）")
            continue
        if path.is_file() and path.stat().st_size > MAX_TRACKED_BYTES:
            bad.append(f"{rel}（{path.stat().st_size} 字节 > 5 MiB）")
    if bad:
        record("C1 已跟踪文件", "FAIL", "；".join(bad))
    else:
        record("C1 已跟踪文件", "PASS", f"{len(tracked_files())} 个文件，无权重/二进制/大文件")


def check_c2() -> None:
    if shutil.which("cargo") is None:
        record("C2 第三方许可", "SKIP", "找不到 cargo（先 . /opt/rust/env.sh）")
        return
    out = subprocess.run(
        [sys.executable, str(SDK / "tools" / "license-audit.py"), "--check"],
        cwd=SDK,
        capture_output=True,
        text=True,
    )
    text = (out.stdout + out.stderr).strip().splitlines()
    summary = text[-1] if text else "(无输出)"
    record("C2 第三方许可", "PASS" if out.returncode == 0 else "FAIL", summary)


def check_c3() -> None:
    assets = json.loads((SDK / "licenses" / "model-assets.json").read_text(encoding="utf-8"))
    lock = json.loads((SDK / "fixtures" / "nmt" / "models.lock.json").read_text(encoding="utf-8"))
    load = json.loads(
        (SDK / "fixtures" / "nmt" / "baseline-p33-load.json").read_text(encoding="utf-8")
    )

    asset = next(a for a in assets["assets"] if a["kind"] == "model-weights")
    coords = asset["coordinates"]
    problems: list[str] = []

    if coords["model"]["sha256"] != lock["model"]["verified_sha256"]:
        problems.append("model-assets.json 的模型 sha256 与 models.lock.json 不一致")

    lock_support = {f["record_name"]: f["verified_sha256"] for f in lock["support_files"]}
    for f in coords["support_files"]:
        if f["record_name"] in lock_support and lock_support[f["record_name"]] != f["sha256"]:
            problems.append(f"{f['record_name']} 的 sha256 与 lock 不一致")

    if coords["total_decompressed_bytes"] != load["model_bytes"]:
        problems.append(
            f"资产清单总字节 {coords['total_decompressed_bytes']} != 基线 model_bytes {load['model_bytes']}"
        )

    revision = load.get("model_revision") or ""
    prefix = coords["model"]["sha256"][:16]
    if prefix not in revision:
        problems.append(f"基线 model_revision（{revision}）未包含模型摘要前缀 {prefix}")

    if asset["license"]["redistribution"] != "prohibited-until-confirmed":
        problems.append("权重再分发状态不是 prohibited-until-confirmed（红线被改动？）")
    if asset["license"]["vendored_in_repo"] or asset["license"]["shipped_in_artifacts"]:
        problems.append("清单声称权重已入库或已随包分发，与红线冲突")

    if problems:
        record("C3 模型坐标一致性", "FAIL", "；".join(problems))
    else:
        record(
            "C3 模型坐标一致性",
            "PASS",
            f"模型 {prefix}… @ {coords['total_decompressed_bytes']} 字节，三处一致",
        )


def check_c4() -> None:
    cargo = shutil.which("cargo")
    if cargo is None:
        record("C4 打包清单", "SKIP", "找不到 cargo")
        return

    problems: list[str] = []
    packaged: list[str] = []
    path_blocked: list[str] = []
    for manifest in sorted(SDK.glob("crates/*/Cargo.toml")) + sorted(SDK.glob("tools/*/Cargo.toml")) + sorted(
        SDK.glob("apps/*/Cargo.toml")
    ):
        text = manifest.read_text(encoding="utf-8")
        name = re.search(r'^name\s*=\s*"([^"]+)"', text, re.M)
        crate = name.group(1) if name else manifest.parent.name
        if "publish" not in text:
            problems.append(f"{crate} 的 Cargo.toml 没有 publish 设置（可能被误发布）")
        # 只有不依赖 path 依赖的 crate 才能 package；失败的记录为"不可打包"而不是失败
        # `--list` 只列文件，不做依赖校验；真正的打包可行性用后面那次 --no-verify 判
        out = subprocess.run(
            [cargo, "package", "--list", "--allow-dirty", "-p", crate],
            cwd=SDK,
            capture_output=True,
            text=True,
        )
        if out.returncode != 0:
            problems.append(f"{crate} 的文件列表都拿不到：{out.stderr.strip()[:200]}")
            continue
        files = [line.strip() for line in out.stdout.splitlines() if line.strip()]
        real = subprocess.run(
            [cargo, "package", "--no-verify", "--allow-dirty", "-p", crate],
            cwd=SDK,
            capture_output=True,
            text=True,
        )
        if real.returncode == 0:
            packaged.append(crate)
        elif "must have a version requirement" in real.stderr:
            # 预期行为：内部依赖是 path-only，cargo 无法打包（ADR-041）。
            path_blocked.append(crate)
        else:
            problems.append(f"{crate} 打包失败（非预期原因）：{real.stderr.strip().splitlines()[-1][:200]}")
        for f in files:
            if Path(f).suffix.lower() in WEIGHT_SUFFIXES:
                problems.append(f"{crate} 的包清单包含可疑文件 {f}")
        for f in files:
            path = SDK / f
            if path.is_file() and path.stat().st_size > MAX_TRACKED_BYTES:
                problems.append(f"{crate} 的包清单包含大文件 {f}")
    if problems:
        record("C4 打包清单", "FAIL", "；".join(problems))
    else:
        detail = (
            f"可真正打包：{', '.join(packaged) if packaged else '（无）'}；"
            f"因 path-only 依赖按设计不可打包：{len(path_blocked)} 个；"
            "所有包清单均不含权重"
        )
        record("C4 打包清单", "PASS", detail)


def check_c5() -> None:
    text = (SDK / ".gitignore").read_text(encoding="utf-8")
    missing = [p for p in ("/resources", "/models", "/target") if p not in text]
    if missing:
        record("C5 .gitignore", "FAIL", f"缺少条目：{missing}")
    else:
        record("C5 .gitignore", "PASS", "resources / models / target 均被忽略")


def check_c6() -> None:
    required = {
        SDK / "THIRD_PARTY_NOTICES.md": ["不入库", "不分发"],
        SDK / "licenses" / "model-assets.json": ["prohibited-until-confirmed", "red_line"],
        SDK / "README.md": ["权重"],
    }
    problems: list[str] = []
    for path, markers in required.items():
        if not path.exists():
            problems.append(f"缺少 {path.relative_to(SDK)}")
            continue
        text = path.read_text(encoding="utf-8")
        for marker in markers:
            if marker not in text:
                problems.append(f"{path.relative_to(SDK)} 缺少关键表述 {marker!r}")
    if problems:
        record("C6 红线文案", "FAIL", "；".join(problems))
    else:
        record("C6 红线文案", "PASS", "清单与文档中的红线表述存在")


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def check_c7(verbose: bool) -> None:
    import os

    model_dir = os.environ.get("NMT_MODEL_DIR")
    if not model_dir:
        record("C7 权重哈希", "SKIP", "未设置 NMT_MODEL_DIR")
        return
    root = Path(model_dir)
    if not root.is_dir():
        record("C7 权重哈希", "FAIL", f"NMT_MODEL_DIR={root} 不存在")
        return
    assets = json.loads((SDK / "licenses" / "model-assets.json").read_text(encoding="utf-8"))
    coords = next(a for a in assets["assets"] if a["kind"] == "model-weights")["coordinates"]
    expected = [(coords["model"]["record_name"], coords["model"]["sha256"])]
    expected += [(f["record_name"], f["sha256"]) for f in coords["support_files"]]

    problems: list[str] = []
    checked = 0
    for name, want in expected:
        path = root / name
        if not path.is_file():
            problems.append(f"{name} 缺失")
            continue
        got = sha256_of(path)
        checked += 1
        if got != want:
            problems.append(f"{name} 哈希不符（实际 {got[:16]}…）")
        elif verbose:
            print(f"    {name} -> {got[:16]}… OK")
    if problems:
        record("C7 权重哈希", "FAIL", "；".join(problems))
    else:
        record("C7 权重哈希", "PASS", f"{root} 下 {checked} 个文件哈希全部匹配")


def main() -> int:
    ap = argparse.ArgumentParser(description="发行红线检查")
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()

    print(f"仓库根：{REPO}")
    check_c1()
    check_c2()
    check_c3()
    check_c4()
    check_c5()
    check_c6()
    check_c7(args.verbose)

    width = max(len(name) for name, _, _ in results)
    print()
    for name, status, detail in results:
        print(f"{name.ljust(width)}  {status:4}  {detail}")

    failed = [r for r in results if r[1] == "FAIL"]
    print()
    if failed:
        print(f"结论：FAIL（{len(failed)} 项）—— 发行红线未满足")
        return 1
    skipped = [r for r in results if r[1] == "SKIP"]
    print(f"结论：PASS（{len(results)} 项，{len(skipped)} 项 SKIP）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
