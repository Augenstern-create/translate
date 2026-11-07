#!/usr/bin/env python3
"""P9.0 语言资源锁文件校验（只读，不下载）。

用法：
    python3 tools/check-language-resources.py --check
    python3 tools/check-language-resources.py --list
    python3 tools/check-language-resources.py --self-test
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent
LOCK = SDK / "fixtures" / "p9" / "resource-lock.json"

KINDS = {"dictionary", "language-identification", "morphology", "model", "runtime"}
STATUSES = {"not-acquired", "sample-acquired", "full-acquired", "installed-locally", "blocked"}
LICENSE_STATUSES = {
    "pending-review",
    "existing-package-license-present",
    "reviewed-allowed",
    "reviewed-denied",
    "not-required",
    "internal-only-reviewed",
}
REDISTRIBUTION = {"pending-review", "allowed", "allowed-with-attribution", "prohibited"}
REQUIRED = {
    "id", "kind", "language_pair", "format", "official_url", "source_revision",
    "source_sha256", "size_bytes", "license_spdx", "license_status",
    "redistribution_status", "converter_version", "status", "notes",
}
SHA256 = re.compile(r"^[0-9a-f]{64}$")


def validate(data: dict) -> list[str]:
    problems: list[str] = []
    if data.get("schema") != "sdk-language-resource-lock/v1":
        problems.append("schema 不是 sdk-language-resource-lock/v1")
    resources = data.get("resources")
    if not isinstance(resources, list) or not resources:
        return problems + ["resources 必须是非空数组"]
    seen: set[str] = set()
    for i, r in enumerate(resources):
        where = f"resources[{i}]"
        if not isinstance(r, dict):
            problems.append(f"{where} 不是对象")
            continue
        missing = REQUIRED - set(r)
        if missing:
            problems.append(f"{where} 缺字段：{sorted(missing)}")
        rid = r.get("id")
        if not isinstance(rid, str) or not rid:
            problems.append(f"{where}.id 为空")
        elif rid in seen:
            problems.append(f"重复 id：{rid}")
        seen.add(rid)
        if r.get("kind") not in KINDS:
            problems.append(f"{rid}: kind 非法 {r.get('kind')!r}")
        if r.get("status") not in STATUSES:
            problems.append(f"{rid}: status 非法 {r.get('status')!r}")
        if r.get("license_status") not in LICENSE_STATUSES:
            problems.append(f"{rid}: license_status 非法 {r.get('license_status')!r}")
        if r.get("redistribution_status") not in REDISTRIBUTION:
            problems.append(f"{rid}: redistribution_status 非法 {r.get('redistribution_status')!r}")
        for field in ("source_sha256", "source_revision", "size_bytes", "language_pair"):
            if field not in r:
                continue
        if r.get("status") == "not-acquired":
            if r.get("source_sha256") is not None or r.get("size_bytes") is not None:
                problems.append(f"{rid}: not-acquired 不得填哈希/大小")
        if r.get("status") in {"sample-acquired", "full-acquired"}:
            if not SHA256.match(str(r.get("source_sha256") or "")):
                problems.append(f"{rid}: acquired 状态必须有 64 位 source_sha256")
            if not isinstance(r.get("size_bytes"), int) or r.get("size_bytes", 0) <= 0:
                problems.append(f"{rid}: acquired 状态必须有正 size_bytes")
            if r.get("license_spdx") == "pending-review":
                problems.append(f"{rid}: acquired 状态不得仍为 license pending-review")
        if r.get("status") == "installed-locally" and r.get("source_sha256") is not None:
            if not SHA256.match(str(r["source_sha256"])):
                problems.append(f"{rid}: source_sha256 不是 64 位十六进制")
        if r.get("license_status") == "reviewed-allowed" and r.get("license_spdx") == "pending-review":
            problems.append(f"{rid}: reviewed-allowed 必须有具体 license_spdx")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    data = json.loads(LOCK.read_text(encoding="utf-8"))
    problems = validate(data)
    if args.self_test:
        bad = json.loads(json.dumps(data))
        bad["resources"][0]["status"] = "not-acquired"
        bad["resources"][0]["source_sha256"] = "0" * 64
        if not validate(bad):
            problems.append("self-test: 非法占位未被拒绝")
        bad2 = json.loads(json.dumps(data))
        bad2["resources"].append(bad2["resources"][0])
        if not validate(bad2):
            problems.append("self-test: 重复 id 未被拒绝")
    if args.list:
        for r in data.get("resources", []):
            print(f"{r['id']}\t{r['status']}\t{r['license_status']}\t{r['redistribution_status']}")
    print(f"resources={len(data.get('resources', []))}")
    if problems:
        print("FAIL:")
        for p in problems:
            print(f"  - {p}")
        return 1
    print("PASS: 语言资源锁文件合法（无虚构哈希；acquired 状态才要求真实哈希）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
