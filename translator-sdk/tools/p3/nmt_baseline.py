#!/usr/bin/env python3
"""P3.0 原生参考链路基线采集（Bergamot CLI）。

只做一件事：把"真实模型到底能不能用、快不快、占多少内存"变成可复现的数字。
不依赖任何第三方 Python 包（stdlib only），也不联网。

前置：参考 CLI 需要先打一个本地补丁，让它支持「按行模式」并逐条报告耗时——
原版 CLI 把整个 stdin 当成一段文本，只能测"一次翻译"，测不出热翻译 p50/p95。
补丁见 `tools/p3/bergamot-cli-lines.patch`（只改 I/O 循环，翻译路径未动），
打上后设置环境变量 `BERGAMOT_STDIN_LINES=1` 生效。

采集四组数据：

1. **golden**：把 `fixtures/nmt/golden-v1.jsonl` 的样本逐条送进原生 CLI，
   记录译文、逐条延迟，并校验约束（`must_contain` / `must_contain_any`）。
2. **cold**：新进程从启动到"第一条译文出现"的墙钟时间（含权重读盘 + 初始化）。
3. **warm**：同一进程内连续 N 次翻译同一句子的 p50/p95/p99。
4. **memory**：从 `/proc/<pid>/status` 读 `VmHWM`（峰值 RSS）与 `VmRSS`。

诚实边界（写进报告时必须带上）：

- 这是**原生参考链路**的观测，不是 `translator-sdk` 的观测；
- `VmHWM` 是内核记账的进程峰值 RSS，含页缓存映射，不等于"模型权重常驻量"；
- CLI 进程退出即释放，"卸载后 RSS"无法用 CLI 测（进程都没了），需要 P3.1 的
  Rust API 才能测——本脚本不假装测过；
- 页面缓存没清（无 root，不能 drop_caches），所以 cold 是"热页缓存下的冷进程"；
- 延迟是**本机**数字（共享 CPU），不是跨平台承诺。

用法：

    python3 tools/p3/nmt_baseline.py \\
        --bergamot /tmp/p3/bergamot-translator/build/app/bergamot \\
        --config /tmp/p3/models/config.bergamot.yml \\
        --samples fixtures/nmt/golden-v1.jsonl \\
        --warm-iters 100 --threads 2 \\
        --out /tmp/p3/baseline.json
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import shlex
import statistics
import subprocess
import sys
import time


def read_status(pid: int) -> dict[str, int]:
    """读 /proc/<pid>/status 里的内存字段（KiB）。读不到就返回空。"""
    out: dict[str, int] = {}
    try:
        text = pathlib.Path(f"/proc/{pid}/status").read_text()
    except OSError:
        return out
    for line in text.splitlines():
        for key in ("VmRSS:", "VmHWM:", "VmPeak:", "VmSize:"):
            if line.startswith(key):
                parts = line.split()
                if len(parts) >= 2 and parts[1].isdigit():
                    out[key.rstrip(":")] = int(parts[1])
    return out


class Bergamot:
    """一个常驻的 bergamot CLI 进程（按行模式）：逐行喂输入、逐行收 JSON。"""

    def __init__(self, binary: str, config: str, threads: int) -> None:
        env = dict(os.environ)
        env["BERGAMOT_STDIN_LINES"] = "1"
        self.command = [
            binary,
            "--model-config-paths",
            config,
            "--cpu-threads",
            str(threads),
            "--log-level",
            "critical",
        ]
        self.proc = subprocess.Popen(
            self.command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            env=env,
        )
        # 第一行是加载耗时，由补丁输出
        first = self._read_json()
        self.load_ms = float(first.get("load_ms", float("nan")))
        self.peak = read_status(self.proc.pid)

    def _read_json(self) -> dict:
        assert self.proc.stdout is not None
        line = self.proc.stdout.readline()
        if not line:
            err = ""
            if self.proc.poll() is not None and self.proc.stderr is not None:
                err = self.proc.stderr.read()[-4000:]
            raise RuntimeError(f"CLI 没有输出（退出码 {self.proc.poll()}）：{err}")
        try:
            return json.loads(line)
        except json.JSONDecodeError as exc:  # pragma: no cover - 只在补丁没打时出现
            raise RuntimeError(f"无法解析 CLI 输出 {line!r}：{exc}") from exc

    def translate(self, text: str) -> tuple[str, float, float, dict[str, int]]:
        """送一句，等一句；返回 (译文, CLI 计时 ms, 墙钟 ms, 内存快照)。"""
        assert self.proc.stdin is not None
        started = time.perf_counter()
        # 空行会被 CLI 当成"空请求"，这里用单个空格避免歧义
        self.proc.stdin.write((text if text.strip() else " ") + "\n")
        self.proc.stdin.flush()
        payload = self._read_json()
        wall = (time.perf_counter() - started) * 1000.0
        snap = self.memory()
        return payload.get("text", ""), float(payload.get("ms", 0.0)), wall, snap

    def memory(self) -> dict[str, int]:
        snap = read_status(self.proc.pid)
        for key in ("VmHWM", "VmRSS"):
            if key in snap:
                self.peak[key] = max(self.peak.get(key, 0), snap[key])
        return {**snap, "VmHWM_peak": self.peak.get("VmHWM", 0)}

    def close(self) -> str:
        """关掉 stdin 让 CLI 正常退出，返回 stderr（用于记录告警）。"""
        err = ""
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
            if self.proc.stdout:
                self.proc.stdout.close()
            if self.proc.stderr:
                err = self.proc.stderr.read()
                self.proc.stderr.close()
        finally:
            try:
                self.proc.wait(timeout=60)
            except subprocess.TimeoutExpired:  # pragma: no cover
                self.proc.kill()
        return err.strip()


def percentile(values: list[float], p: float) -> float:
    if not values:
        return float("nan")
    ordered = sorted(values)
    k = max(0, min(len(ordered) - 1, int(round((p / 100.0) * (len(ordered) - 1)))))
    return ordered[k]


def check_constraints(text: str, constraints: dict) -> list[str]:
    """返回不满足的约束描述（空列表 = 通过）。"""
    problems: list[str] = []
    for must in constraints.get("must_contain", []):
        if must not in text:
            problems.append(f"缺少 {must!r}")
    any_of = constraints.get("must_contain_any", [])
    if any_of and not any(m in text for m in any_of):
        problems.append(f"应当包含 {' / '.join(repr(m) for m in any_of)} 之一")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bergamot", required=True)
    ap.add_argument("--config", required=True)
    ap.add_argument("--samples", required=True)
    ap.add_argument("--warm-iters", type=int, default=100)
    ap.add_argument("--warm-text", default="The library opens at nine in the morning.")
    ap.add_argument("--threads", type=int, default=2)
    ap.add_argument("--beam-size", type=int, default=1, help="仅记录（配置里固定）")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    samples = [
        json.loads(line)
        for line in pathlib.Path(args.samples).read_text().splitlines()
        if line.strip()
    ]
    report: dict = {
        "status": "measured",
        "tool": "tools/p3/nmt_baseline.py",
        "command": shlex.join([args.bergamot, "--model-config-paths", args.config,
                               "--cpu-threads", str(args.threads)]),
        "samples": args.samples,
        "threads": args.threads,
        "beam_size": args.beam_size,
        "warm_iters": args.warm_iters,
        "golden": [],
        "cold": None,
        "warm": None,
        "note": "原生参考链路（Bergamot CLI + 按行补丁）观测，不是 translator-sdk 的观测；"
                "页面缓存未清，cold 指'热页缓存下的冷进程'",
    }

    # ---- cold：新进程，一条请求 ----
    first = next((s for s in samples if s["text"].strip()), None)
    if first is not None:
        proc = Bergamot(args.bergamot, args.config, args.threads)
        try:
            text, cli_ms, wall_ms, snap = proc.translate(first["text"])
            load_ms = proc.load_ms
        finally:
            err = proc.close()
        report["cold"] = {
            "text": first["text"],
            "output": text,
            "load_ms": round(load_ms, 1),
            "first_request_ms": round(cli_ms, 1),
            "cold_to_first_line_ms": round(load_ms + cli_ms, 1),
            "wall_to_first_line_ms": round(wall_ms, 1),
            "vm_hwm_mib": round(snap.get("VmHWM_peak", 0) / 1024.0, 1),
            "vm_rss_mib": round(snap.get("VmRSS", 0) / 1024.0, 1),
            "stderr_tail": err[-2000:],
        }

    # ---- golden：逐条译文 + 约束 ----
    proc = Bergamot(args.bergamot, args.config, args.threads)
    failures = 0
    try:
        for s in samples:
            text = s["text"]
            out, cli_ms, wall_ms, snap = proc.translate(text)
            problems = check_constraints(out, s.get("constraints", {}))
            if text.strip() and not out.strip():
                problems.append("译文为空")
            if not text.strip() and out.strip():
                problems.append("空输入却返回了非空译文")
            report["golden"].append(
                {
                    "id": s["id"],
                    "text": text,
                    "output": out,
                    "ms": round(cli_ms, 1),
                    "wall_ms": round(wall_ms, 1),
                    "ok": not problems,
                    "problems": problems,
                    "vm_hwm_mib": round(snap.get("VmHWM_peak", 0) / 1024.0, 1),
                }
            )
            failures += 0 if not problems else 1

        # ---- warm：连续 N 次同一句子 ----
        for _ in range(3):
            proc.translate(args.warm_text)
        lat: list[float] = []
        for _ in range(args.warm_iters):
            _, cli_ms, _, _ = proc.translate(args.warm_text)
            lat.append(cli_ms)
        snap = proc.memory()
        report["warm"] = {
            "text": args.warm_text,
            "iters": args.warm_iters,
            "p50_ms": round(percentile(lat, 50), 2),
            "p95_ms": round(percentile(lat, 95), 2),
            "p99_ms": round(percentile(lat, 99), 2),
            "min_ms": round(min(lat), 2),
            "max_ms": round(max(lat), 2),
            "mean_ms": round(statistics.fmean(lat), 2),
            "stdev_ms": round(statistics.pstdev(lat), 2),
            "vm_hwm_mib": round(snap.get("VmHWM_peak", 0) / 1024.0, 1),
            "vm_rss_mib": round(snap.get("VmRSS", 0) / 1024.0, 1),
        }
    finally:
        err = proc.close()

    report["golden_total"] = len(report["golden"])
    report["golden_failed"] = failures
    report["stderr_tail"] = err[-2000:]
    pathlib.Path(args.out).write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(
        f"golden {len(report['golden']) - failures}/{len(report['golden'])} ok"
        f" · cold {report['cold']['cold_to_first_line_ms']} ms"
        f" · warm p50 {report['warm']['p50_ms']} ms / p95 {report['warm']['p95_ms']} ms"
        f" · peak {report['warm']['vm_hwm_mib']} MiB"
        f" → {args.out}"
    )
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
