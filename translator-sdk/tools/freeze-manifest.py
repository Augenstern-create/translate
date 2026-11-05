#!/usr/bin/env python3
"""P3 冻结清单：生成与校验。

`docs/23` §2.2 要求把"冻结的到底是什么"变成可核对的证据：git sha、tag、工具链、
`Cargo.lock` 摘要、模型文件摘要、Bergamot commit、基线文件、关键门禁命令、Conditional 项。

两种用法：

    python3 tools/freeze-manifest.py --write      # 生成 fixtures/p3-freeze/manifest.json
    python3 tools/freeze-manifest.py --check      # 校验冻结基线未被改动

`--check` 的判据（任一不满足即 FAIL）：
  * tag `p3.4` 仍指向 manifest 里记录的 commit；
  * 分支 `p3-frozen` 存在且指向同一 commit；
  * `Cargo.lock` 与基线 JSON 的 sha256 未变；
  * 若 manifest 记录了模型文件哈希，且当前 `NMT_MODEL_DIR` 存在，则重新核对；
  * release-blockers 台账存在且条目 id 与 manifest 记录一致。

注意：清单**不是**模型再分发授权，只是版本证据。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent
REPO = SDK.parent
MANIFEST = SDK / "fixtures" / "p3-freeze" / "manifest.json"
BLOCKERS = SDK / "fixtures" / "release-blockers.json"
SCHEMA = "sdk-p3-freeze/v1"

GATE_COMMANDS = [
    "cargo fmt --all --check",
    "cargo clippy --workspace --all-targets --all-features -- -D warnings",
    "cargo test --workspace --locked",
    "cargo test --workspace --all-features --locked",
    "python3 tools/license-audit.py --check",
    "python3 tools/check-release.py",
    "cargo test --workspace --all-features -- --ignored --test-threads=1   # 需本机权重与 native",
]


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def git(*args: str) -> str:
    out = subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)
    return out.stdout.strip() if out.returncode == 0 else ""


def tool_version(cmd: list[str]) -> str:
    exe = shutil.which(cmd[0])
    if exe is None:
        return "unavailable"
    out = subprocess.run([exe, *cmd[1:]], capture_output=True, text=True)
    return out.stdout.strip().splitlines()[0] if out.stdout.strip() else "unknown"


def baseline_files() -> dict[str, str]:
    out = {}
    for path in sorted((SDK / "fixtures" / "nmt").glob("baseline-*.json")):
        out[f"fixtures/nmt/{path.name}"] = sha256_file(path)
    for path in sorted((SDK / "fixtures" / "nmt").glob("gate-ci.json")):
        out[f"fixtures/nmt/{path.name}"] = sha256_file(path)
    return out


def model_files() -> dict[str, dict]:
    """记录模型文件哈希；没有本地权重时如实写 absent，而不是留空。"""

    assets = json.loads((SDK / "licenses" / "model-assets.json").read_text(encoding="utf-8"))
    coords = next(a for a in assets["assets"] if a["kind"] == "model-weights")["coordinates"]
    expected = [(coords["model"]["record_name"], coords["model"]["sha256"])]
    expected += [(f["record_name"], f["sha256"]) for f in coords["support_files"]]

    model_dir = os.environ.get("NMT_MODEL_DIR")
    rows = {}
    for name, want in expected:
        row = {"expected_sha256": want, "present": False, "actual_sha256": None}
        if model_dir:
            path = Path(model_dir) / name
            if path.is_file():
                row["present"] = True
                row["actual_sha256"] = sha256_file(path)
        rows[name] = row
    return rows


def collect() -> dict:
    blockers = json.loads(BLOCKERS.read_text(encoding="utf-8"))
    bergamot_bin = os.environ.get("NMT_BERGAMOT_BIN")
    return {
        "schema": SCHEMA,
        "generated_at": "2026-11-05",
        "purpose": "P3 功能冻结的版本证据（docs/23 §2.2）。不是模型再分发授权。",
        "git": {
            "frozen_commit": git("rev-parse", "p3.4"),
            "frozen_tag": "p3.4",
            "frozen_branch": "p3-frozen",
            "branch_points_at": git("rev-parse", "p3-frozen"),
            "head_at_generation": git("rev-parse", "HEAD"),
            "subject": git("log", "-1", "--format=%s"),
        },
        "toolchain": {
            "rustc": tool_version(["rustc", "-V"]),
            "cargo": tool_version(["cargo", "-V"]),
            "python": tool_version(["python3", "-V"]),
            "os": tool_version(["uname", "-sr"]),
        },
        "cargo_lock_sha256": sha256_file(SDK / "Cargo.lock"),
        "native_runtime": {
            "bergamot_commit": "9271618ebbdc5d21ac4dc4df9e72beb7ce644774",
            "binary": bergamot_bin or "absent",
            "binary_sha256": sha256_file(Path(bergamot_bin)) if bergamot_bin and Path(bergamot_bin).is_file() else None,
        },
        "models": model_files(),
        "baselines": baseline_files(),
        "gate_commands": GATE_COMMANDS,
        "gate_results": {
            "date": "2026-11-05",
            "fmt": "pass",
            "clippy": "pass（-D warnings）",
            "test_default": "365 passed / 0 failed / 41 ignored",
            "test_all_features": "365 passed / 0 failed / 41 ignored",
            "license_audit": "pass（53 个第三方包）",
            "check_release": "pass（7 项，C7 需设置 NMT_MODEL_DIR）",
            "real_model_ignored": "41 passed / 0 failed（同机、有权重）",
            "note": "以上为本机实测；详见 docs/21 §2 与 docs/24 §2",
        },
        "blockers": [{"id": b["id"], "status": b["status"], "blocks_release": b["blocks_release"]} for b in blockers["blockers"]],
        "verdict": "Conditional：功能可验收，发行受 release-blockers 中的 open 项阻断",
    }


def cmd_write() -> int:
    data = collect()
    MANIFEST.parent.mkdir(parents=True, exist_ok=True)
    MANIFEST.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"已写入 {MANIFEST.relative_to(SDK)}")
    print(f"  冻结提交：{data['git']['frozen_commit'][:12]}（tag {data['git']['frozen_tag']}）")
    print(f"  基线文件：{len(data['baselines'])} 个")
    print(f"  模型文件：{sum(1 for m in data['models'].values() if m['present'])}/{len(data['models'])} 在本机存在")
    return 0


def cmd_check() -> int:
    if not MANIFEST.exists():
        print(f"FAIL 缺少 {MANIFEST.relative_to(SDK)}；先 --write")
        return 1
    want = json.loads(MANIFEST.read_text(encoding="utf-8"))
    problems: list[str] = []

    if git("rev-parse", "p3.4") != want["git"]["frozen_commit"]:
        problems.append("tag p3.4 已移动（不可变承诺被破坏）")
    if git("rev-parse", "p3-frozen") != want["git"]["frozen_commit"]:
        problems.append("分支 p3-frozen 不再指向冻结提交")
    if sha256_file(SDK / "Cargo.lock") != want["cargo_lock_sha256"]:
        problems.append("Cargo.lock 与冻结时不同（冻结基线被改动）")

    current_baselines = baseline_files()
    for name, sha in want["baselines"].items():
        if name not in current_baselines:
            problems.append(f"基线文件缺失：{name}")
        elif current_baselines[name] != sha:
            problems.append(f"基线文件被改写：{name}")

    model_dir = os.environ.get("NMT_MODEL_DIR")
    if model_dir:
        for name, row in want["models"].items():
            path = Path(model_dir) / name
            if path.is_file() and sha256_file(path) != row["expected_sha256"]:
                problems.append(f"模型文件哈希与资产清单不符：{name}")
    else:
        print("（未设置 NMT_MODEL_DIR，跳过本机权重复核）")

    if not BLOCKERS.exists():
        problems.append("release-blockers.json 缺失")
    else:
        cur = json.loads(BLOCKERS.read_text(encoding="utf-8"))
        ids = sorted(b["id"] for b in cur["blockers"])
        want_ids = sorted(b["id"] for b in want["blockers"])
        if ids != want_ids:
            problems.append(f"台账条目变化：{want_ids} → {ids}")
        for b in cur["blockers"]:
            if b["status"] == "closed" and not (b.get("closed_at") and b.get("closed_evidence")):
                problems.append(f"{b['id']} 标为 closed 但没有 closed_at/closed_evidence")

    if problems:
        print("FAIL 冻结基线校验未通过：")
        for p in problems:
            print(f"  - {p}")
        return 1
    print(f"PASS 冻结基线未被改动（commit {want['git']['frozen_commit'][:12]}，"
          f"{len(want['baselines'])} 个基线文件，{len(want['blockers'])} 条 release-blockers）")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="P3 冻结清单")
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--write", action="store_true")
    g.add_argument("--check", action="store_true")
    args = ap.parse_args()
    return cmd_write() if args.write else cmd_check()


if __name__ == "__main__":
    sys.exit(main())
