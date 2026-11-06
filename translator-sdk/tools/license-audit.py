#!/usr/bin/env python3
"""第三方依赖许可证清单的生成与校验（P3.4）。

两件事：
  1. `--write`  从 `cargo metadata` 重新生成 `licenses/third-party.json`；
  2. `--check`  校验清单与当前依赖树一致，并按策略判定每个许可证是否可接受。

设计取舍：
  * 清单**入库**（而不是每次现算），这样"发行时到底带了哪些许可"是可 diff、可审计的；
  * 判定策略只看 SPDX 表达式，不联网、不解析许可证正文——本工具**不是**法律意见，
    它只保证"没有未审阅的许可证混进来"；
  * 表达式求值：`OR` 取任一、`AND` 取全部。`r-efi` 这类 `MIT OR Apache-2.0 OR LGPL-2.1-or-later`
    因此可以选 MIT 通过；而单独的 `LGPL-2.1-or-later` 会被拒。
  * 只解析一层括号（`(A OR B) AND C`）。更复杂的表达式直接报错要求人工处理，
    而不是猜——猜错方向比报错更危险。

用法：
    python3 tools/license-audit.py --write
    python3 tools/license-audit.py --check
"""

# Windows: 子进程输出默认按 ANSI 代码页(如 cp936)解码，cargo/git 吐 UTF-8 会直接
# UnicodeDecodeError。所有文本模式的 subprocess 调用都必须显式指定 encoding。
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys

# Windows 控制台默认是 ANSI 代码页（如 cp936），打印 UTF-8 内容（中文、U+FFFD 替换符）
# 会 UnicodeEncodeError。输出流统一改成 UTF-8 + replace：宁可显示"?"，也不要因为打印崩掉。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:  # noqa: BLE001 - 老版本 Python 或非 TTY 时不支持 reconfigure
    pass

from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "licenses" / "third-party.json"
SCHEMA = "sdk-third-party-licenses/v1"

# 允许的许可证原子。只放**宽松**许可；copyleft 一律不在允许集合里，
# 除非它出现在 OR 里、且存在宽松分支（由表达式求值自然处理）。
ALLOWED = {
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "0BSD",
    "Unlicense",
    "Zlib",
    "CC0-1.0",
    "Unicode-3.0",
    "MPL-2.0",  # 文件级 copyleft，允许在动态链接/独立进程场景使用；需保留声明
    # CDLA-Permissive-2.0：数据许可（不是代码许可），OSI 认可、无 copyleft、只要求保留声明。
    # P4 引入：`ort-sys` 的 **build-dependency** `ureq` 需要根证书数据（`webpki-root-certs`）。
    # 该依赖**只在构建期**用于下载 ONNX Runtime 预编译包，不进发行产物；详见 docs/25 §3。
    "CDLA-Permissive-2.0",
}

# 明确拒绝（即使出现在 OR 里也不作为唯一出路时需要人工复核）
DENIED = {"GPL-2.0", "GPL-3.0", "AGPL-3.0", "SSPL-1.0", "BUSL-1.1", "Commons-Clause"}


class LicenseParseError(Exception):
    pass


def tokenize(expr: str) -> list[str]:
    out: list[str] = []
    i = 0
    while i < len(expr):
        ch = expr[i]
        if ch.isspace():
            i += 1
            continue
        if ch in "()":
            out.append(ch)
            i += 1
            continue
        j = i
        while j < len(expr) and not expr[j].isspace() and expr[j] not in "()":
            j += 1
        word = expr[i:j]
        out.append(word)
        i = j
    return out


def normalize(expr: str) -> tuple[str, bool]:
    """把历史写法归一化，返回 `(表达式, 是否用了非 SPDX 写法)`。

    `MIT/Apache-2.0` 这种以 `/` 分隔的写法不是合法 SPDX，但在老 crate 的
    `license` 字段里很常见，历代工具的约定含义是"任选其一"。本工具按 OR 处理，
    但把这件事**记进清单**（`legacy_notation`），而不是悄悄改掉——
    审计时需要能看出"这条判定建立在一次解释之上"。
    """

    if "/" in expr:
        parts = [p.strip() for p in expr.split("/") if p.strip()]
        return (" OR ".join(parts), True)
    return (expr, False)


def parse_and_eval(expr: str) -> bool:
    """把 SPDX 表达式求值为"是否存在一条全宽松的许可选择路径"。"""

    expr, _ = normalize(expr)
    tokens = tokenize(expr)

    pos = 0

    def peek() -> str | None:
        return tokens[pos] if pos < len(tokens) else None

    def take() -> str:
        nonlocal pos
        tok = tokens[pos]
        pos += 1
        return tok

    def atom() -> bool:
        nonlocal pos
        tok = peek()
        if tok is None:
            raise LicenseParseError("表达式意外结束")
        if tok == "(":
            take()
            value = disjunction()
            if peek() != ")":
                raise LicenseParseError("括号不匹配")
            take()
            return value
        if tok in ("AND", "OR", ")", "WITH"):
            raise LicenseParseError(f"意外的记号 {tok}")
        name = take()
        if peek() == "WITH":
            take()
            exception = take()
            name = f"{name} WITH {exception}"
        if name in DENIED:
            return False
        return name in ALLOWED

    def conjunction() -> bool:
        nonlocal pos
        value = atom()
        while peek() == "AND":
            take()
            right = atom()
            value = value and right
        return value

    def disjunction() -> bool:
        nonlocal pos
        value = conjunction()
        while peek() == "OR":
            take()
            right = conjunction()
            value = value or right
        return value

    result = disjunction()
    if pos != len(tokens):
        raise LicenseParseError(f"无法解析的尾部记号：{tokens[pos:]}")
    return result


def cargo_metadata() -> dict:
    cargo = shutil.which("cargo")
    if cargo is None:
        sys.exit("找不到 cargo；先 `. /opt/rust/env.sh`")
    out = subprocess.run(
        [cargo, "metadata", "--format-version", "1", "--all-features", "--locked"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if out.returncode != 0:
        sys.stderr.write(out.stderr)
        sys.exit("cargo metadata 失败")
    return json.loads(out.stdout)


def collect(meta: dict) -> list[dict]:
    members = set(meta["workspace_members"])
    rows = []
    for pkg in meta["packages"]:
        if pkg["id"] in members:
            continue
        raw_license = pkg.get("license") or ""
        normalized, legacy = normalize(raw_license)
        rows.append(
            {
                "name": pkg["name"],
                "version": pkg["version"],
                "license": raw_license,
                "license_normalized": normalized,
                "legacy_notation": legacy,
                "source": pkg.get("source") or "workspace",
                "checksum": pkg.get("checksum") or "",
            }
        )
    rows.sort(key=lambda r: (r["name"], r["version"]))
    return rows


def toolchain() -> str:
    cargo = shutil.which("cargo") or "cargo"
    out = subprocess.run([cargo, "--version"], capture_output=True, text=True, encoding="utf-8", errors="replace")
    return out.stdout.strip() or "unknown"


def build_manifest(rows: list[dict]) -> dict:
    return {
        "schema": SCHEMA,
        "generated_by": "tools/license-audit.py --write",
        "generated_at": "2026-11-05",
        "toolchain": toolchain(),
        "feature_set": "--all-features (含 mocks / dictionary / rules / resource / nmt-bergamot)",
        "policy": {
            "allowed_atoms": sorted(ALLOWED),
            "denied_atoms": sorted(DENIED),
            "note": "判定只针对 SPDX 表达式，不构成法律意见；权重的许可另行记录在 licenses/model-weights.json",
        },
        "packages": rows,
        "count": len(rows),
    }


def cmd_write() -> int:
    rows = collect(cargo_metadata())
    MANIFEST.parent.mkdir(parents=True, exist_ok=True)
    MANIFEST.write_text(
        json.dumps(build_manifest(rows), ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"已写入 {MANIFEST.relative_to(ROOT)}：{len(rows)} 个第三方包")
    return 0


def cmd_check() -> int:
    if not MANIFEST.exists():
        print(f"FAIL 缺少 {MANIFEST.relative_to(ROOT)}；先运行 --write")
        return 1
    committed = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if committed.get("schema") != SCHEMA:
        print(f"FAIL 清单 schema 不是 {SCHEMA}")
        return 1

    rows = collect(cargo_metadata())
    old = {(r["name"], r["version"]): r for r in committed.get("packages", [])}
    new = {(r["name"], r["version"]): r for r in rows}

    failed = False
    added = sorted(set(new) - set(old))
    removed = sorted(set(old) - set(new))
    if added:
        failed = True
        print("FAIL 有未登记的第三方包（清单过期，需要重新 --write 并审阅）：")
        for name, version in added:
            print(f"       + {name} {version}  {new[(name, version)]['license'] or 'UNKNOWN'}")
    if removed:
        failed = True
        print("FAIL 清单里有已经消失的包（清单过期）：")
        for name, version in removed:
            print(f"       - {name} {version}")

    # 即便清单没变，也重新按当前策略判定一遍（策略可能收紧了）
    for (name, version), row in sorted(new.items()):
        expr = row["license"]
        if not expr:
            failed = True
            print(f"FAIL {name} {version} 没有声明许可证")
            continue
        try:
            ok = parse_and_eval(expr)
        except LicenseParseError as exc:
            failed = True
            print(f"FAIL {name} {version} 的表达式无法解析：{expr!r}（{exc}）")
            continue
        if not ok:
            failed = True
            print(f"FAIL {name} {version} 的许可证不在允许集合：{expr}")

    if failed:
        return 1
    print(f"PASS 第三方许可清单与依赖树一致（{len(rows)} 个包，全部通过策略）")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="第三方依赖许可证清单")
    group = ap.add_mutually_exclusive_group(required=True)
    group.add_argument("--write", action="store_true", help="重新生成 licenses/third-party.json")
    group.add_argument("--check", action="store_true", help="校验清单与策略")
    args = ap.parse_args()
    return cmd_write() if args.write else cmd_check()


if __name__ == "__main__":
    sys.exit(main())
