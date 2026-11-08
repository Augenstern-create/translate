#!/usr/bin/env python3
"""ECDICT CSV → 规范化词典 JSONL（T1 / docs/57 §5 T1）。

只读取本地 CSV，不联网。输出一行一个对象，供：

    pack-dictionary <out.jsonl> <package_dir> --format jsonl ...

使用。字段与 `tools/pack-dictionary` 的 `JsonEntry` 对齐：

    {"headword": ..., "pos": ..., "definitions": [...]}

设计取舍
--------
1. **不把 ECDICT 的整份 CSV 直接塞进包**（770k 行、63 MB）。首版目标是
   “高频 5k–10k + 技术/短语”，按 BNC/FRQ 词频、Collins/Oxford/考试标签
   和一份独立高频词表选词；后续可以扩大 `--max-entries`。
2. **每条中文释义保留为独立 sense**，不合并成一段。多义词必须能查到多个义项
   （docs/57 T1.4），整句翻译也不允许机械取第一义。
3. 中文释义里的 `n.` / `vt.` / `[计]` 前缀不进入译文，词性单独进 `pos`；
   领域标记 `[计]` 等保留为 sense 前缀，因为“计算机义”与日常义确实不同。
4. 过滤 `[网络]` 释义：它们在 ECDICT 里质量参差（机翻/网友补充），
   只有在没有其它释义时才作为兜底保留。
5. 同一 headword 大小写变体合并（英语查询本身大小写不敏感）。

输出统计写到 `--stats`，用于质量报告；不写工作区外路径。
"""
from __future__ import annotations

import argparse
import csv
import json
import re
import sys
from collections import Counter, OrderedDict
from pathlib import Path

# 常见词性前缀（ECDICT 用缩写，部分条目用完整写法）
POS_PREFIXES = [
    ("vt.", "verb"),
    ("vi.", "verb"),
    ("v.", "verb"),
    ("n.", "noun"),
    ("adj.", "adjective"),
    ("a.", "adjective"),
    ("adv.", "adverb"),
    ("ad.", "adverb"),
    ("prep.", "preposition"),
    ("conj.", "conjunction"),
    ("pron.", "pronoun"),
    ("num.", "numeral"),
    ("art.", "article"),
    ("int.", "interjection"),
    ("interj.", "interjection"),
    ("aux.", "auxiliary"),
    ("abbr.", "abbreviation"),
    ("suf.", "suffix"),
    ("suff.", "suffix"),
    ("pref.", "prefix"),
    ("pl.", "plural"),
]

# 领域标记：保留，但排在普通释义之后
DOMAIN_RE = re.compile(r"^\[([^\]]{1,12})\]\s*")

# 明显不是“英文词头”的行（标点/纯符号/过长）
HEADWORD_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9'’.\- ]*$")

TECH_SEEDS = """
memory leak|thread pool|garbage collection|race condition|deadlock|stack overflow
buffer overflow|null pointer|dangling pointer|use after free|source code|open source
pull request|code review|unit test|integration test|regression test|command line
operating system|file system|database|query language|machine learning|deep learning
neural network|artificial intelligence|large language model|natural language processing
computer vision|cloud computing|edge computing|distributed system|load balancer
microservice|virtual machine|compiler|interpreter|runtime|garbage collector|heap
stack|cache|latency|throughput|bandwidth|application programming interface
software development kit|command-line interface|graphical user interface
central processing unit|graphics processing unit|random access memory|solid-state drive
hard disk drive|universal serial bus|hypertext transfer protocol|domain name system
javascript|typescript|python|rust|java|kotlin|swift|golang|scala|haskell|sql
json|xml|yaml|html|css|http|https|tcp|udp|dns|api|sdk|cli|gui|cpu|gpu|ram|rom
ssd|hdd|usb|url|uri|uuid|utf-8|unicode|regex|regular expression|hash table
linked list|binary tree|red-black tree|priority queue|dynamic programming
time complexity|space complexity|big o notation|design pattern|dependency injection
continuous integration|continuous delivery|version control|git commit|merge conflict
build system|package manager|dependency graph|static analysis|linter|formatter
debugger|profiler|benchmark|fuzz testing|property testing|end-to-end test
as soon as possible|in order to|by the way|at least|at most|for example|such as
instead of|according to|due to|in spite of|as well as|on behalf of|take care of
look forward to|make sure|find out|give up|carry out|point out|come up with
turn on|turn off|set up|shut down|log in|log out|sign in|sign up|back up
break down|bring up|call back|check out|come across|cut down|drop off|figure out
fill in|get along|get rid of|go over|grow up|hang out|hold on|keep up|look after
look into|pick up|put off|run out|show up|take off|try out|work out|write down
""".replace("\n", "|").split("|")


def has_cjk(text: str) -> bool:
    return any(
        "\u4e00" <= c <= "\u9fff"
        or "\u3400" <= c <= "\u4dbf"
        or "\uf900" <= c <= "\ufaff"
        for c in text
    )


def parse_freq(raw: str) -> int:
    try:
        return int((raw or "0").strip() or 0)
    except ValueError:
        return 0


def parse_translation(text: str) -> tuple[list[str], list[str]]:
    """把 ECDICT translation 拆成 (中文释义, 词性)。

    - 每个非空行是一个 sense；
    - 行首词性去掉，词性写入 pos；
    - 行首领域标记保留（`[计] 计算机`）；
    - 同一行内用 `\\n` 分隔的换行已切分；
    - 去掉纯引用/空释义。
    """
    senses: list[str] = []
    pos: list[str] = []
    for raw_line in (text or "").splitlines():
        line = raw_line.strip()
        if not line:
            continue
        # 有些条目用 \n 字面量而不是真换行
        line = line.replace("\\n", "\n")
        if "\n" in line:
            parts = [p.strip() for p in line.splitlines() if p.strip()]
        else:
            parts = [line]
        for part in parts:
            if not has_cjk(part):
                continue
            # 词性前缀
            lowered = part.lower()
            for prefix, canonical in POS_PREFIXES:
                if lowered.startswith(prefix):
                    part = part[len(prefix):].strip()
                    if canonical not in pos:
                        pos.append(canonical)
                    break
            # 领域标记
            domain = None
            m = DOMAIN_RE.match(part)
            if m:
                domain = m.group(1).strip()
                part = part[m.end():].strip()
            # 去掉引用/占位
            part = part.strip(" ;；,，")
            if not part or not has_cjk(part):
                continue
            if part in {"...", "…"}:
                continue
            if domain:
                part = f"[{domain}] {part}"
            if part not in senses:
                senses.append(part)
    return senses, pos


def sense_priority(sense: str) -> tuple[int, int]:
    """释义排序：普通义在前，领域义其次，网络义最后；同组保持原顺序。"""
    if sense.startswith("[网络]"):
        return (2, 0)
    if sense.startswith("["):
        return (1, 0)
    return (0, 0)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("csv", help="ECDICT ecdict.csv")
    ap.add_argument("output", help="输出 JSONL")
    ap.add_argument("--wordlist", default=None, help="独立高频词表（每行一个词），用于保证覆盖")
    ap.add_argument("--max-frequency-rank", type=int, default=20000)
    ap.add_argument("--max-entries", type=int, default=30000)
    ap.add_argument("--include-phrases", action="store_true", default=True)
    ap.add_argument("--supplement", default=None, help="项目自编补充 JSONL（覆盖同 headword）")
    ap.add_argument("--stats", default=None)
    args = ap.parse_args()

    required: set[str] = set()
    if args.wordlist:
        for line in Path(args.wordlist).read_text(encoding="utf-8").splitlines():
            word = line.strip().lower()
            if word:
                required.add(word)

    seeds = {s.strip().lower() for s in TECH_SEEDS if s.strip()}

    stats = Counter()
    entries: "OrderedDict[str, dict]" = OrderedDict()

    with open(args.csv, newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            stats["total_rows"] += 1
            word = (row.get("word") or "").strip()
            if not word:
                stats["skipped_empty_headword"] += 1
                continue
            if len(word) > 60 or not HEADWORD_RE.match(word):
                stats["skipped_invalid_headword"] += 1
                continue
            key = word.lower()
            translation = row.get("translation") or ""
            senses, pos = parse_translation(translation)
            if not senses:
                stats["skipped_no_chinese"] += 1
                continue

            bnc = parse_freq(row.get("bnc", ""))
            frq = parse_freq(row.get("frq", ""))
            collins = parse_freq(row.get("collins", ""))
            oxford = (row.get("oxford") or "").strip() == "1"
            tags = (row.get("tag") or "").strip()
            words = key.split()
            is_phrase = len(words) > 1

            selected = False
            if key in required or key in seeds:
                selected = True
                stats["selected_by_wordlist_or_seed"] += 1
            elif bnc and bnc <= args.max_frequency_rank:
                selected = True
                stats["selected_by_bnc"] += 1
            elif frq and frq <= args.max_frequency_rank:
                selected = True
                stats["selected_by_frq"] += 1
            elif collins or oxford or tags:
                selected = True
                stats["selected_by_exam_or_collins"] += 1
            elif args.include_phrases and is_phrase and len(words) <= 4 and (bnc or frq):
                selected = True
                stats["selected_phrase"] += 1

            if not selected:
                stats["skipped_low_frequency"] += 1
                continue

            # 网络义只在没有其它义时兜底
            general = [s for s in senses if not s.startswith("[网络]")]
            if general:
                network_count = len([s for s in senses if s.startswith("[网络]")])
                senses = [s for s in senses if not s.startswith("[网络]")]
                stats["dropped_network_senses"] += network_count
            senses = sorted(senses, key=sense_priority)
            # 去掉完全重复（排序后仍可能重复）
            deduped: list[str] = []
            for sense in senses:
                if sense not in deduped:
                    deduped.append(sense)
            senses = deduped[:12]
            if not senses:
                stats["skipped_no_usable_sense"] += 1
                continue

            if key in entries:
                stats["merged_duplicates"] += 1
                old = entries[key]
                for sense in senses:
                    if sense not in old["definitions"]:
                        old["definitions"].append(sense)
                for p in pos:
                    if p not in old["pos"]:
                        old["pos"].append(p)
                continue

            entries[key] = {
                "headword": word,
                "pos": "|".join(pos) if pos else None,
                "definitions": senses,
                "_bnc": bnc,
                "_frq": frq,
                "_is_phrase": is_phrase,
            }

    # 项目自编补充条目覆盖同 headword 的 ECDICT 释义（补齐 ECDICT 缺失的
    # 技术短语，并纠正 garbage collection 之类的旧译法）。
    if args.supplement:
        supplement_count = 0
        for line in Path(args.supplement).read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line:
                continue
            raw = json.loads(line)
            key = raw["headword"].lower()
            definitions = [d.strip() for d in raw.get("definitions", []) if d.strip()]
            if not definitions:
                continue
            entries[key] = {
                "headword": raw["headword"],
                "pos": raw.get("pos"),
                "definitions": definitions,
                "_bnc": 0,
                "_frq": 0,
                "_is_phrase": " " in key,
                "_supplement": True,
            }
            supplement_count += 1
        stats["supplement_entries"] = supplement_count

    # 按“更常用优先”稳定排序后截断；required/seed 词必须保留，不能因为截断丢失。
    def rank(item: tuple[str, dict]) -> tuple[int, int, int, str]:
        key, entry = item
        supplement_flag = 0 if entry.get("_supplement") else 1
        required_flag = 0 if (key in required or key in seeds) else 1
        best = min([x for x in (entry["_bnc"], entry["_frq"]) if x > 0] or [10**9])
        return (supplement_flag, required_flag, best, key)

    ordered = sorted(entries.items(), key=rank)
    if args.max_entries and len(ordered) > args.max_entries:
        stats["truncated_entries"] = len(ordered) - args.max_entries
        ordered = ordered[: args.max_entries]

    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as fh:
        for key, entry in ordered:
            record = {
                "headword": entry["headword"],
                "pos": entry["pos"],
                "definitions": entry["definitions"],
            }
            fh.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n")

    stats["output_entries"] = len(ordered)
    stats["output_senses"] = sum(len(e["definitions"]) for _, e in ordered)
    stats["output_phrases"] = sum(1 for _, e in ordered if e["_is_phrase"])
    stats["required_wordlist"] = len(required)
    covered = sum(1 for w in required if w in {k for k, _ in ordered})
    stats["required_covered"] = covered
    stats["required_coverage"] = round(covered / len(required), 4) if required else None
    stats["selected_seed_present"] = sum(1 for s in seeds if s in {k for k, _ in ordered})
    stats["selected_seed_total"] = len(seeds)

    if args.stats:
        Path(args.stats).write_text(
            json.dumps(stats, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
    print(json.dumps(stats, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
