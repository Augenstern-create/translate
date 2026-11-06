#!/usr/bin/env python3
"""P4 有条件验收台账的校验（`docs/27` R-02 / R-06 / R-07）。

`fixtures/p4/acceptance.json` 把"工作包交付 / 门禁闭合 / 候选可用"三件事拆开记录。
这个脚本只做一件事：**不让台账自相矛盾**，也不让它和发行阻断台账分叉。

判据（任一不满足即 FAIL）：

* schema 正确；门禁 id 唯一且都在 `P4-G01…G08` 范围内；
* `status=pass` 的门禁**不得**带 `open_items`（"通过"了却还挂着未闭合项，是最容易糊弄人的写法）；
* `status=partial` 的门禁**必须**带 `open_items`，且每一项都能在 `carry_over` 里找到，
  并带 `priority` / `owner` / `exit_evidence`（没有退出证据的承接项等于没承接）；
* `carry_over` 的 `from_gate` 必须指向存在的门禁；
* `release_blockers_reference.ids` 必须与 `fixtures/release-blockers.json` **完全一致**，
  且这些项在台账里仍是 `open` —— 不允许在验收文件里"顺手"把它们写闭（`docs/27` R-01）。

用法：
    python3 tools/p4-acceptance.py --check
"""

from __future__ import annotations

import argparse
import json
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
ACCEPT = ROOT / "fixtures" / "p4" / "acceptance.json"
BLOCKERS = ROOT / "fixtures" / "release-blockers.json"
SCHEMA = "sdk-p4-acceptance/v1"
GATE_IDS = [f"P4-G{i:02d}" for i in range(1, 9)]
PRIORITIES = {"P0", "P1", "P2"}
STATUSES = {"pass", "partial", "expected-unsupported", "open"}


def fail(msg: str) -> None:
    print(f"FAIL {msg}")
    sys.exit(1)


def _open_blocker_count() -> int:
    """引用集合里仍 open 的条数（关项不算 open，避免打印误导）。"""
    ledger = json.loads(BLOCKERS.read_text(encoding="utf-8"))
    return sum(1 for b in ledger.get("blockers", []) if b.get("status") == "open")


def main() -> int:
    ap = argparse.ArgumentParser(description="P4 有条件验收台账校验")
    ap.add_argument("--check", action="store_true", required=True)
    ap.parse_args()

    if not ACCEPT.exists():
        fail(f"缺少 {ACCEPT.relative_to(ROOT)}")
    data = json.loads(ACCEPT.read_text(encoding="utf-8"))
    if data.get("schema") != SCHEMA:
        fail(f"schema 应为 {SCHEMA}，实际 {data.get('schema')!r}")

    problems: list[str] = []

    # ---- 门禁 ----
    gates = data.get("gates", [])
    seen: set[str] = set()
    for g in gates:
        gid = g.get("id", "")
        if gid in seen:
            problems.append(f"门禁 id 重复：{gid}")
        seen.add(gid)
        if gid not in GATE_IDS:
            problems.append(f"未知门禁 id：{gid}")
        status = g.get("status")
        if status not in STATUSES:
            problems.append(f"{gid} 的 status 非法：{status!r}")
        opens = g.get("open_items", [])
        if status == "pass" and opens:
            problems.append(f"{gid} 标为 pass 却还挂着未闭合项：{opens}")
        if status == "partial" and not opens:
            problems.append(f"{gid} 标为 partial 但没写 open_items（未闭合项去哪了？）")
        if status in {"partial", "expected-unsupported", "open"} and not g.get("evidence"):
            problems.append(f"{gid} 状态是 {status}，必须给出 evidence 指向")

    for needed in ("P4-01", "P4-07"):
        if needed not in {w.get("id") for w in data.get("work_packages", [])}:
            problems.append(f"工作包 {needed} 缺失")

    missing = [g for g in GATE_IDS if g not in seen]
    if missing:
        problems.append(f"门禁缺失（不允许静默消失）：{missing}")

    # 承接项可以挂到门禁，也可以挂到工作包（性能长稳、质量金样本来自 P4-05/06 而不是门禁）
    wp_ids = {w.get("id") for w in data.get("work_packages", [])}

    # ---- 承接台账 ----
    carry = {c.get("id"): c for c in data.get("carry_over", [])}
    if len(carry) != len(data.get("carry_over", [])):
        problems.append("carry_over 里有重复 id")
    for cid, c in carry.items():
        for field in ("priority", "owner", "from_gate", "title", "exit_evidence"):
            if not c.get(field):
                problems.append(f"承接项 {cid} 缺 {field}")
        if c.get("priority") and c["priority"] not in PRIORITIES:
            problems.append(f"承接项 {cid} 的 priority 非法：{c['priority']}")
        src_gate = c.get("from_gate")
        if src_gate and src_gate not in seen and src_gate not in wp_ids:
            problems.append(f"承接项 {cid} 指向不存在的门禁/工作包 {src_gate}")

    for g in gates:
        for item in g.get("open_items", []):
            if item not in carry:
                problems.append(f"{g['id']} 的未闭合项 {item} 没有进入承接台账（等于没人接）")

    # ---- 发行阻断台账（单一事实来源） ----
    if not BLOCKERS.exists():
        problems.append("fixtures/release-blockers.json 缺失")
    else:
        ledger = json.loads(BLOCKERS.read_text(encoding="utf-8"))
        ledger_ids = sorted(b["id"] for b in ledger.get("blockers", []))
        ref = data.get("release_blockers_reference", {})
        if sorted(ref.get("ids", [])) != ledger_ids:
            problems.append(
                f"验收文件引用的阻断项与台账不一致：{sorted(ref.get('ids', []))} vs {ledger_ids}"
            )
        # 关项是允许的，但必须带证据：closed_at 与 closed_evidence 都不能为空。
        # 规则演进（2026-11-07，D-46 关项时）：原规则"引用项必须全部 open"会让
        # "已按证据关闭"变成不可表达的状态——门禁要么永远关不掉，要么得改台账口径。
        # 现在改成：引用集合必须等于台账全集；其中每一项要么 open，要么已带证据关闭。
        bad_closed = [
            b["id"]
            for b in ledger.get("blockers", [])
            if b.get("status") != "open"
            and not (b.get("closed_at") and b.get("closed_evidence"))
        ]
        if bad_closed:
            problems.append(
                f"台账里这些项已关闭但缺证据（closed_at/closed_evidence 不能为空）：{bad_closed}"
            )

    if problems:
        print("FAIL P4 验收台账自相矛盾：")
        for p in problems:
            print(f"  - {p}")
        return 1

    partial = [g["id"] for g in gates if g.get("status") == "partial"]
    print(
        f"PASS P4 台账一致（{len(gates)} 个门禁，部分满足 {partial or '无'}，"
        f"{len(carry)} 条承接项，阻断项 {_open_blocker_count()} 条仍 open）"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
