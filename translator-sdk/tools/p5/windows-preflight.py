#!/usr/bin/env python3
"""windows-preflight.py -- judge a `dsh-win-recon/v1` capture against the
deployment checklist in `docs/31`.

Why a separate tool: the recon script only *reports*. Turning "did I deploy it
right?" into a pass/fail list that cannot be argued with is the same discipline
as `check-release.py` (ADR-056) -- whoever deploys runs one command and gets
PASS/FAIL per requirement instead of eyeballing a JSON blob.

Usage:
    tools/p5/windows-preflight.py win-recon.json
    tools/p5/windows-preflight.py win-recon.json --json > preflight.json

Exit codes: 0 = no FAIL (WARN allowed), 1 = at least one FAIL, 2 = bad input.

Requirement ids are stable and referenced from `docs/31`; do not renumber.
"""

from __future__ import annotations

import argparse
import json
import re
import sys

# Windows 控制台默认是 ANSI 代码页（如 cp936），打印 UTF-8 内容（中文、U+FFFD 替换符）
# 会 UnicodeEncodeError。输出流统一改成 UTF-8 + replace：宁可显示"?"，也不要因为打印崩掉。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:  # noqa: BLE001 - 老版本 Python 或非 TTY 时不支持 reconfigure
    pass


SCHEMA_IN = "dsh-win-recon/v1"
SCHEMA_OUT = "dsh-win-preflight/v1"

# Deployment targets (docs/31 §1 / §2). Kept here as the single source so the
# doc can quote them instead of hard-coding numbers in prose.
MIN_BUILD = 17763          # Windows 10 1809: OpenSSH Server + LongPaths + winget-era
DISK_FAIL_GIB = 15.0       # below this the build cannot finish
DISK_WARN_GIB = 25.0       # comfortable: MSVC + SDK + toolchain + target/
RAM_FAIL_GIB = 6.0
RAM_WARN_GIB = 14.0
MIN_PY = (3, 9)            # tools/*.py uses str.removeprefix
EXPECTED_RUSTC = "1.99.1"  # same as the Linux verification host (docs/22)

RESULTS: list[dict] = []


def rec(rid: str, status: str, detail: str) -> None:
    assert status in ("PASS", "FAIL", "WARN", "INFO"), status
    RESULTS.append({"id": rid, "status": status, "detail": detail})


def get(doc: dict, *path, default=None):
    cur = doc
    for key in path:
        if not isinstance(cur, dict) or key not in cur:
            return default
        cur = cur[key]
    return cur


def parse_version(text: str | None) -> tuple[int, ...] | None:
    if not text:
        return None
    m = re.search(r"(\d+)\.(\d+)(?:\.(\d+))?", text)
    if not m:
        return None
    return tuple(int(g) for g in m.groups() if g is not None)


def main() -> int:
    ap = argparse.ArgumentParser(description="judge a dsh-win-recon/v1 capture")
    ap.add_argument("recon", help="JSON produced by tools/p5/windows-recon.sh")
    ap.add_argument("--json", action="store_true", help="emit dsh-win-preflight/v1 JSON instead of a table")
    args = ap.parse_args()

    try:
        with open(args.recon, encoding="utf-8") as fh:
            doc = json.load(fh)
    except Exception as exc:  # noqa: BLE001 - any read/parse error is a bad input
        print(f"FAIL: cannot read {args.recon}: {exc}", file=sys.stderr)
        return 2

    if doc.get("schema") != SCHEMA_IN:
        print(f"FAIL: expected schema {SCHEMA_IN}, got {doc.get('schema')!r}", file=sys.stderr)
        return 2

    # ---- R1 OS version ----------------------------------------------------
    build = int(get(doc, "os", "build", default=0) or 0)
    caption = get(doc, "os", "caption") or "?"
    if build == 0:
        rec("R1", "FAIL", f"读不到 build（caption={caption}）")
    elif build < MIN_BUILD:
        rec("R1", "FAIL", f"build {build} < {MIN_BUILD}（Windows 10 1809 起才自带 OpenSSH/长路径/直接装 winget）")
    elif build < 22000:
        rec("R1", "WARN", f"Windows 10 build {build}：可用，但 Win11 上才验过的组合更少")
    else:
        rec("R1", "PASS", f"{caption} build {build}")

    arch = (get(doc, "os", "architecture") or "")
    rec("R2", "PASS" if "64" in arch else "FAIL", f"OS 架构 {arch or '未知'}（必须 x64）")

    cores = int(get(doc, "cpu", "logical", default=0) or 0)
    cores_phys = int(get(doc, "cpu", "cores", default=0) or 0)
    cpu_name = get(doc, "cpu", "name") or "?"
    if cores == 0:
        rec("R3", "FAIL", "读不到 CPU 逻辑核数")
    elif cores < 4:
        rec("R3", "WARN", f"{cpu_name} {cores_phys}C/{cores}T：能编但很慢")
    else:
        rec("R3", "PASS", f"{cpu_name} {cores_phys}C/{cores}T")

    ram = float(get(doc, "memory", "total_gib", default=0) or 0)
    if ram < RAM_FAIL_GIB:
        rec("R4", "FAIL", f"{ram} GiB：低于 {RAM_FAIL_GIB} GiB，链接 ORT 时容易 OOM")
    elif ram < RAM_WARN_GIB:
        rec("R4", "WARN", f"{ram} GiB：够用，但建议 16 GiB")
    else:
        rec("R4", "PASS", f"{ram} GiB")

    # ---- R5 disk ----------------------------------------------------------
    vols = get(doc, "volumes", default=[]) or []
    work = next((v for v in vols if str(v.get("drive", "")).upper().startswith("C:")), None) or (vols[0] if vols else None)
    if not work:
        rec("R5", "FAIL", "读不到任何固定磁盘卷")
    else:
        free = float(work.get("free_gib") or 0)
        drive = work.get("drive")
        fs = work.get("fs")
        need = "MSVC+SDK ~6 GB / toolchain ~1.5 GB / registry ~1 GB / target ~9 GB"
        if free < DISK_FAIL_GIB:
            rec("R5", "FAIL", f"{drive} 仅剩 {free} GiB（< {DISK_FAIL_GIB}），装不下：{need}")
        elif free < DISK_WARN_GIB:
            rec("R5", "WARN", f"{drive} 剩 {free} GiB（建议 ≥{DISK_WARN_GIB}）：{need}")
        elif str(fs).upper() not in ("NTFS", "REFS"):
            rec("R5", "WARN", f"{drive} 剩 {free} GiB，但文件系统是 {fs}（需要 NTFS：长路径 + 大小写不敏感假设）")
        else:
            rec("R5", "PASS", f"{drive} {fs} 剩 {free} GiB")

    # ---- R6 rust host must be MSVC ---------------------------------------
    host = get(doc, "rust", "host")
    rustc_ver = get(doc, "rust", "rustc_version")
    if not rustc_ver:
        rec("R6", "FAIL", "未装 Rust（找不到 rustc）")
    elif not host:
        rec("R6", "FAIL", f"rustc 版本={rustc_ver} 但读不到 host（`rustc -vV` 失败）")
    elif host != "x86_64-pc-windows-msvc":
        rec("R6", "FAIL", f"host={host}：必须 x86_64-pc-windows-msvc（GNU 组合与 link.exe/ort DLL 都对不上）")
    else:
        rec("R6", "PASS", f"{rustc_ver}（host {host}）")

    # ---- R7 rustfmt + clippy ---------------------------------------------
    comps = get(doc, "rust", "components", default=[]) or []
    flat = " ".join(str(c) for c in comps)
    missing = [c for c in ("rustfmt", "clippy") if c not in flat]
    if not comps:
        rec("R7", "FAIL", "读不到 `rustup component list --installed`")
    elif missing:
        rec("R7", "FAIL", f"缺组件 {missing}：`cargo fmt --check` / `clippy -D warnings` 跑不了")
    else:
        rec("R7", "PASS", "rustfmt + clippy 已装")

    # ---- R8 python --------------------------------------------------------
    py = get(doc, "tools", "python")
    pyv = parse_version(py)
    if not pyv:
        rec("R8", "FAIL", f"未装 Python（{py!r}）；tools/*.py 需要 ≥{MIN_PY[0]}.{MIN_PY[1]}")
    elif pyv[:2] < MIN_PY:
        rec("R8", "FAIL", f"Python {'.'.join(map(str, pyv))} < {MIN_PY[0]}.{MIN_PY[1]}（用了 str.removeprefix）")
    else:
        rec("R8", "PASS", f"Python {'.'.join(map(str, pyv))}")

    # ---- R9 git -----------------------------------------------------------
    git = get(doc, "tools", "git")
    if not git:
        rec("R9", "FAIL", "未装 Git（freeze-manifest.py 要 `git show`/`rev-parse`，且仓库要靠它取）")
    else:
        rec("R9", "PASS", str(git))

    # ---- R10 sshd ---------------------------------------------------------
    svc = get(doc, "ssh", "sshd_service") or {}
    status = str(svc.get("status", "absent"))
    start = str(svc.get("startType", "?"))
    listen = get(doc, "ssh", "listening_22", default=[]) or []
    if status == "absent":
        rec("R10", "FAIL", "sshd 服务不存在：Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0")
    elif status != "Running":
        rec("R10", "FAIL", f"sshd 状态 {status}（需要 Running）")
    elif start.lower() != "automatic":
        rec("R10", "WARN", f"sshd 在跑但 StartType={start}（重启后不自动起来）")
    elif not listen:
        rec("R10", "WARN", "服务在跑但 22 端口没有 LISTEN 记录（防火墙或端口被改？）")
    else:
        rec("R10", "PASS", f"Running/Automatic，监听 {', '.join(str(x) for x in listen)}")

    # ---- R11 long paths ---------------------------------------------------
    lp = get(doc, "host", "long_paths_enabled")
    if lp in (1, "1", True):
        rec("R11", "PASS", "LongPathsEnabled=1")
    else:
        rec("R11", "FAIL", f"LongPathsEnabled={lp!r}：cargo target 路径会超 260，必须设 1")

    # ---- R12 MSVC toolset -------------------------------------------------
    vc = get(doc, "vs", "vc_tools_version")
    if not vc:
        rec("R12", "FAIL", "vswhere 查不到 VC.Tools.x86.x64：libsqlite3-sys(bundled) 与链接都过不去")
    else:
        sdk = get(doc, "windows_sdk_lib_versions", default=[]) or []
        rec("R12", "PASS", f"MSVC toolset {vc}（SDK Lib: {', '.join(str(s) for s in sdk) or '未检出'}）")

    # ---- R13 toolchain version drift (fmt) --------------------------------
    if rustc_ver:
        shown = str(rustc_ver).removeprefix("rustc ").strip()
        drift = EXPECTED_RUSTC not in str(rustc_ver)
        rec("R13", "WARN" if drift else "PASS",
            f"{shown}（Linux 侧 {EXPECTED_RUSTC}）" +
            ("：rustfmt 版本差会让 `cargo fmt --check` 出现与代码无关的差异，报告要写成工具链差异" if drift else ""))

    # ---- R14 elevation (informational) ------------------------------------
    elev = get(doc, "host", "elevated")
    rec("R14", "INFO", f"当前会话是否提权：{elev}（只读勘察不需要；设 Defender 排除/装组件才需要）")

    # ---- R15 defender exclusions (informational) --------------------------
    excl = get(doc, "defender", "exclusions_path", default=[]) or []
    rec("R15", "INFO", f"Defender 排除路径 {len(excl)} 条" + ("：建议把仓库与 %USERPROFILE%\\.cargo 排除掉，否则编译慢一倍" if not excl else ""))

    fails = [r for r in RESULTS if r["status"] == "FAIL"]
    warns = [r for r in RESULTS if r["status"] == "WARN"]

    if args.json:
        out = {
            "schema": SCHEMA_OUT,
            "source": args.recon,
            "summary": {
                "pass": sum(1 for r in RESULTS if r["status"] == "PASS"),
                "fail": len(fails),
                "warn": len(warns),
                "info": sum(1 for r in RESULTS if r["status"] == "INFO"),
            },
            "verdict": "FAIL" if fails else "PASS",
            "results": RESULTS,
        }
        print(json.dumps(out, ensure_ascii=False, indent=2))
    else:
        width = max(len(r["id"]) for r in RESULTS)
        for r in RESULTS:
            print(f"{r['id']:<{width}}  {r['status']:<4}  {r['detail']}")
        print()
        print(f"结论：{'FAIL' if fails else 'PASS'}（PASS {out_summary(RESULTS, 'PASS')} / "
              f"FAIL {len(fails)} / WARN {len(warns)} / INFO {out_summary(RESULTS, 'INFO')}）"
              + ("—— 先按 FAIL 项修，再让 DSH 连上来" if fails else "—— 可以给 DSH 连了"))

    return 1 if fails else 0


def out_summary(results: list[dict], status: str) -> int:
    return sum(1 for r in results if r["status"] == status)


if __name__ == "__main__":
    sys.exit(main())
