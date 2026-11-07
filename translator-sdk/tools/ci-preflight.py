#!/usr/bin/env python3
"""G11 上传范围预检：在把仓库交给外部 CI 之前，先证明"会传什么、不会传什么"。

只读 `git ls-files`，不联网、不改文件。检查项：

  P1  跟踪文件里没有权重 / native 二进制 / 大文件
  P2  没有常见凭据文件名（.env / *.pem / *.key / id_rsa / keystore ...）
  P3  `.gitignore` 覆盖 resources / models / target / dist
  P4  workflow 含 `permissions: contents: read`，且不引用 `secrets.`
  P5  workflow 不含 `pull_request_target`（避免 fork PR 拿到写权限）
  P6  上传工件路径不含权重后缀

用法：
    python3 translator-sdk/tools/ci-preflight.py --check
    python3 translator-sdk/tools/ci-preflight.py --list
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent
REPO = SDK.parent
WORKFLOWS = REPO / ".github" / "workflows"

WEIGHT_SUFFIXES = {
    ".bin", ".spm", ".onnx", ".npz", ".gguf", ".safetensors",
    ".pt", ".pth", ".marian", ".so", ".dylib", ".dll", ".a",
}
MAX_BYTES = 5 * 1024 * 1024
CREDENTIAL_PATTERNS = [
    re.compile(r"(^|/)\.env(\.|$)", re.I),
    re.compile(r"(^|/)id_(rsa|ed25519|ecdsa|dsa)(\.|$)", re.I),
    re.compile(r"\.(pem|key|p12|pfx|keystore|jks)$", re.I),
    re.compile(r"(^|/)(secrets?|credentials?)(\.|/|$)", re.I),
    re.compile(r"(^|/)\.npmrc$", re.I),
    re.compile(r"(^|/)\.pypirc$", re.I),
]


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True, text=True,
        encoding="utf-8", errors="replace",
    ).stdout


def tracked() -> list[Path]:
    raw = git("ls-files", "-z")
    return [REPO / p for p in raw.split("\0") if p]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args()

    files = tracked()
    problems: list[str] = []
    if args.list:
        for p in files:
            print(p.relative_to(REPO))
        print(f"# {len(files)} tracked files")

    total = 0
    large: list[tuple[int, Path]] = []
    weights: list[Path] = []
    creds: list[Path] = []
    for p in files:
        rel = p.relative_to(REPO)
        if p.is_file():
            size = p.stat().st_size
            total += size
            if size > MAX_BYTES:
                large.append((size, rel))
        if p.suffix.lower() in WEIGHT_SUFFIXES:
            weights.append(rel)
        text = str(rel).replace("\\", "/")
        if any(rx.search(text) for rx in CREDENTIAL_PATTERNS):
            creds.append(rel)

    if large:
        problems.append("大文件： " + ", ".join(f"{p}({s} B)" for s, p in large))
    if weights:
        problems.append("权重/二进制： " + ", ".join(str(p) for p in weights))
    if creds:
        problems.append("疑似凭据文件： " + ", ".join(str(p) for p in creds))

    ignore = (REPO / ".gitignore").read_text(encoding="utf-8")
    for entry in ("translator-sdk/resources/", "translator-sdk/models/", "translator-sdk/target/", "translator-sdk/dist/"):
        if entry not in ignore:
            problems.append(f".gitignore 缺少 {entry}")

    workflows = sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml"))
    if not workflows:
        problems.append("没有找到 .github/workflows/*.yml")
    for wf in workflows:
        text = wf.read_text(encoding="utf-8")
        name = wf.name
        if "permissions:" in text and "contents: read" not in text:
            problems.append(f"{name}: permissions 不是 contents: read")
        if "permissions:" not in text:
            problems.append(f"{name}: 未显式声明 permissions")
        if "secrets." in text:
            problems.append(f"{name}: 引用了 secrets.*（需要单独授权）")
        if "pull_request_target" in text:
            problems.append(f"{name}: 使用 pull_request_target（fork PR 可拿写权限）")
        # 上传工件块里不能出现权重后缀
        for block in re.findall(r"uses:\s*actions/upload-artifact@[\s\S]*?(?=\n\s*-\s+name:|\Z)", text):
            if re.search(r"\.(onnx|bin|spm|npz|gguf|safetensors|pt|pth|so|dll)\b", block):
                problems.append(f"{name}: upload-artifact 可能包含权重/二进制")

    print(f"tracked_files={len(files)} total_bytes={total}")
    print(f"workflows={[w.name for w in workflows]}")
    if large:
        print("largest:")
        for s, p in sorted(large, reverse=True)[:5]:
            print(f"  {s:>10}  {p}")
    if problems:
        print("\nFAIL:")
        for p in problems:
            print(f"  - {p}")
        return 1
    print("PASS: 上传范围预检通过（无权重/凭据/大文件；workflow 只读权限）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
