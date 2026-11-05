#!/usr/bin/env python3
"""CI 性能门禁：只判"有没有坏掉"，不判"快了多少"。

为什么不是"与基线比较百分比"：
  * 本项目的基线来自一台共享的 i5-8250U，CI runner 的 CPU 完全不同；
  * 跨机器比较延迟数字没有意义（`docs/20` D-41），拿它做门禁只会产生假警报；
  * 于是这里用**宽松的绝对上限**：只要没有出现数量级退化（比如线程超订导致 20× 变慢）、
    没有把内存翻几倍，就算通过。真正的性能验收仍在固定机器上按 `fixtures/nmt/baseline-p33-*.json` 做。

用法：
    python3 tools/bench-gate.py --case empty --actual /tmp/empty.json
    python3 tools/bench-gate.py --case all --actual-dir /tmp/gate
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent
CEILINGS = SDK / "fixtures" / "nmt" / "gate-ci.json"


def pick(data: dict, path: str) -> float:
    """按 `a.b.c` 取数值；取不到就报错（门禁缺数据必须显性失败）。"""

    cur: object = data
    for part in path.split("."):
        if not isinstance(cur, dict) or part not in cur:
            raise KeyError(path)
        cur = cur[part]
    if not isinstance(cur, (int, float)):
        raise TypeError(f"{path} 不是数值：{cur!r}")
    return float(cur)


def check(case: str, actual_path: Path, ceilings: dict) -> list[str]:
    spec = ceilings["cases"].get(case)
    if spec is None:
        return [f"gate-ci.json 里没有 case {case}"]
    if not actual_path.is_file():
        return [f"缺少实测文件 {actual_path}"]
    data = json.loads(actual_path.read_text(encoding="utf-8"))
    problems: list[str] = []
    for item in spec["limits"]:
        metric, limit = item["metric"], float(item["max"])
        try:
            value = pick(data, metric)
        except (KeyError, TypeError) as exc:
            problems.append(f"{case}: 取不到指标 {metric}（{exc}）")
            continue
        status = "ok" if value <= limit else "超限"
        print(f"  {case:12} {metric:32} {value:>10.2f} <= {limit:>8.2f}  {status}")
        if value > limit:
            problems.append(f"{case}: {metric} = {value:.2f} > 上限 {limit:.2f}")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description="CI 性能门禁")
    ap.add_argument("--case", required=True, help="empty / dictionary / worker / all")
    ap.add_argument("--actual", help="单个 case 的 JSON")
    ap.add_argument("--actual-dir", help="`all` 时的目录（含 <case>.json）")
    args = ap.parse_args()

    ceilings = json.loads(CEILINGS.read_text(encoding="utf-8"))
    cases = list(ceilings["cases"]) if args.case == "all" else [args.case]

    problems: list[str] = []
    for case in cases:
        if args.case == "all":
            path = Path(args.actual_dir or ".") / f"{case}.json"
        else:
            if not args.actual:
                return print("--case 单个时必须给 --actual") or 2
            path = Path(args.actual)
        problems += check(case, path, ceilings)

    print()
    if problems:
        print("结论：FAIL")
        for p in problems:
            print(f"  - {p}")
        return 1
    print(f"结论：PASS（{len(cases)} 个 case，全部在宽松上限内）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
