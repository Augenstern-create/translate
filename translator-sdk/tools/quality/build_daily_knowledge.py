#!/usr/bin/env python3
"""P18-L: 从本地 ECDICT + Google 词表构建日常知识测试集与词典补充。

本工具不联网。它只读取：
  - .build/quality-data/ecdict.csv（本地 ECDICT 快照）
  - fixtures/quality/google-10000-english-usa.txt
  - fixtures/dictionary/en-zh.supplement.jsonl（项目已有技术/固定表达）

输出：
  - fixtures/quality/daily-knowledge-v1.jsonl（10,000 条，含 split）
  - fixtures/quality/daily-knowledge-v1.construction.jsonl
  - fixtures/quality/daily-knowledge-v1.frozen.jsonl
  - fixtures/quality/daily-knowledge-v1.lock.json
  - fixtures/dictionary/en-zh.daily-supplement.jsonl（构建新词典用）

口径说明：
  - 知识单元是完整词头/短语/搭配/表达，不拆分；
  - 词形回退由查询工具另行记录，不在此假装命中；
  - split 只由 `sha256(category\\0unit)` 决定，避免按类别手工挑选验收集；
  - 本测试集是**源内覆盖基准**，不是独立第三方外部基准。
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import re
from collections import Counter, OrderedDict, defaultdict
from pathlib import Path

VERBS = set(
    "get take make have go come look give put turn set run keep bring call carry hold "
    "pick break cut drop fall fill find move pass pay play pull push send show shut sit "
    "stand stay talk tell think throw try wake walk work write".split()
)
PREPS = set(
    "in on at by for with without about after before under over from to of up down out off".split()
)
ADJS = set(
    "good bad big small new old young happy sad easy hard difficult long short high low hot "
    "cold warm cool fast slow early late right wrong beautiful nice great little large "
    "important different same free full empty clean dirty quiet loud strong weak rich poor "
    "safe busy tired hungry thirsty ready sorry sure".split()
)
NOUNS = set(
    "time day night morning afternoon evening week month year hour minute second home house "
    "room door window table chair bed car bus train plane road street city town school work "
    "office shop store market food water coffee tea milk bread rice meat fish fruit apple "
    "book phone computer email message question answer problem idea plan money price name "
    "number address weather rain snow wind sun family mother father parent child children "
    "friend people man woman boy girl".split()
)
STARTERS = set(
    "good thank please sorry excuse how what where when why who i you we they he she it this "
    "that there here let can could would may shall should will do does did have has had be "
    "am is are was were welcome congratulations happy merry nice see take keep mind no not "
    "right well too very all any some".split()
)
# 这些词出现在短语里通常说明它不是日常表达（或是说明书/软件 UI）。
TECH_BLOCK = set(
    "clipboard viewer editor program manager pif equation menu dialog command system computer "
    "software file files disk printer window windows document documents data database network "
    "internet web email browser server client folder directory settings setup install installation "
    "update version password user account log login logout click button icon screen keyboard "
    "mouse monitor api sdk cpu gpu ram formatting copies".split()
)
# 日常介词短语白名单；不用全部 2-4 词介词组合（很多是术语/残片）。
PREP_PHRASES = set(
    "in fact at least at most at home at work at school at night in time on time "
    "by bus by car by train on foot in front of next to because of instead of due to "
    "according to as well as in order to for example such as in the morning in the afternoon "
    "in the evening on the way in a hurry in common in general in public in private "
    "at first at last at once at present at the moment for a while for now from now on "
    "in addition in advance in case in charge in detail in effect in person in practice "
    "in short in summary in total on average on purpose on sale on the other hand "
    "out of date out of order out of work up to date".split()
)
BAD_REF_MARKERS = ("（", "）", "组织", "城镇", "电影", "书名", "人名", "地名", "球队", "商标")
PLACEHOLDER = set(
    "sth sb someone something oneself yourself himself herself themselves myself ourselves".split()
)
POS_RE = re.compile(
    r"^(?:n|v|vt|vi|adj|a|adv|ad|prep|conj|pron|num|art|int|interj|aux|abbr)\.\s*",
    re.IGNORECASE,
)
WORD_RE = re.compile(r"^[a-z][a-z'\- ]*$")


def sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def clean_translation(raw: str) -> tuple[str, list[str]]:
    """返回 (展示用中文, 全部有效中文义项)。"""
    senses: list[str] = []
    for line in (raw or "").replace("\\n", "\n").splitlines():
        line = line.strip()
        if not line or not any("\u4e00" <= c <= "\u9fff" for c in line):
            continue
        # 网络义/领域义不适合作为日常基础释义
        if "网络" in line or "[" in line:
            continue
        line = POS_RE.sub("", line).strip(" ;；,，")
        if not line or not any("\u4e00" <= c <= "\u9fff" for c in line):
            continue
        if "..." in line or "…" in line:
            continue
        if line not in senses:
            senses.append(line)
    return "；".join(senses[:2]), senses


def normalized_unit(raw: str) -> str:
    return " ".join(raw.strip().lower().split())


def phrase_ok(unit: str) -> bool:
    words = unit.split()
    if not (2 <= len(words) <= 4):
        return False
    if not WORD_RE.fullmatch(unit) or len(unit) > 42:
        return False
    if any(w in PLACEHOLDER or w in TECH_BLOCK for w in words):
        return False
    return True


def expression_ok(unit: str) -> bool:
    words = unit.split()
    if not (2 <= len(words) <= 4):
        return False
    if not WORD_RE.fullmatch(unit) or len(unit) > 48:
        return False
    if any(w in PLACEHOLDER or w in TECH_BLOCK for w in words):
        return False
    return words[0] in STARTERS


def split_of(category: str, unit: str) -> str:
    # 5 取模近似 80/20；按类别+单元计算，避免按类别整批分到同一侧。
    digest = hashlib.sha256(f"{category}\0{unit}".encode("utf-8")).digest()
    return "frozen" if digest[0] % 5 == 0 else "construction"


def load_ecdict(path: Path):
    words: dict[str, dict] = {}
    with path.open(newline="", encoding="utf-8") as fh:
        for row in csv.DictReader(fh):
            unit = normalized_unit(row.get("word") or "")
            if not unit:
                continue
            display, senses = clean_translation(row.get("translation") or "")
            if not senses:
                continue
            words[unit] = {
                "unit": unit,
                "display": display or senses[0],
                "senses": senses,
                "raw": row,
            }
    return words


def pick_round_robin(pool: list[dict], limit: int) -> list[dict]:
    """按首词分组轮转，避免 2,500 条短语全被 have 占满。"""
    groups: dict[str, list[dict]] = defaultdict(list)
    for item in pool:
        groups[item["unit"].split()[0]].append(item)
    for group in groups.values():
        group.sort(key=lambda x: (len(x["unit"]), x["unit"]))
    firsts = sorted(groups, key=lambda x: (len(groups[x]), x))
    out: list[dict] = []
    cursor = 0
    while len(out) < limit:
        added = False
        for first in firsts:
            if cursor < len(groups[first]):
                out.append(groups[first][cursor])
                added = True
                if len(out) >= limit:
                    break
        if not added:
            break
        cursor += 1
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ecdict", default=".build/quality-data/ecdict.csv")
    ap.add_argument("--wordlist", default="fixtures/quality/google-10000-english-usa.txt")
    ap.add_argument("--existing-supplement", default="fixtures/dictionary/en-zh.supplement.jsonl")
    ap.add_argument("--out-dir", default="fixtures/quality")
    ap.add_argument("--dict-out", default="fixtures/dictionary/en-zh.daily-supplement.jsonl")
    ap.add_argument("--limit", type=int, default=10000)
    args = ap.parse_args()

    ecdict_path = Path(args.ecdict)
    if not ecdict_path.exists():
        raise SystemExit(
            f"缺少本地 ECDICT：{ecdict_path}（本工具不联网；请放入已有快照）"
        )
    ecdict_sha = sha256_file(ecdict_path)
    wordlist_path = Path(args.wordlist)
    google = [
        normalized_unit(line)
        for line in wordlist_path.read_text(encoding="utf-8").splitlines()
        if normalized_unit(line) and " " not in normalized_unit(line)
    ]
    google = list(dict.fromkeys(google))

    lexicon = load_ecdict(ecdict_path)
    print(f"ECDICT 可用词条：{len(lexicon)}")

    # 1) 4,000 个高频单词（Google 词表优先，必须已有有效中文义）。
    word_units: list[dict] = []
    used: set[str] = set()
    for word in google:
        if word in used or word not in lexicon:
            continue
        if len(word.split()) != 1:
            continue
        entry = lexicon[word]
        word_units.append({**entry, "category": "word"})
        used.add(word)
        if len(word_units) >= 4000:
            break
    if len(word_units) < 4000:
        for word, entry in lexicon.items():
            if len(word.split()) == 1 and word not in used:
                word_units.append({**entry, "category": "word"})
                used.add(word)
                if len(word_units) >= 4000:
                    break

    # 2) 500 个高频多义词：Google 剩余词中义项 >=2。
    polysemy: list[dict] = []
    for word in google:
        if len(polysemy) >= 500:
            break
        if word in used or word not in lexicon:
            continue
        entry = lexicon[word]
        if len(entry["senses"]) < 2:
            continue
        polysemy.append({**entry, "category": "polysemy", "min_senses": 2})
        used.add(word)

    # 3) 短语/搭配/表达候选池。
    phrase_pool: list[dict] = []
    collocation_pool: list[dict] = []
    expression_pool: list[dict] = []
    for unit, entry in lexicon.items():
        if unit in used:
            continue
        first = unit.split()[0]
        if any(marker in entry["display"] for marker in BAD_REF_MARKERS):
            continue
        if phrase_ok(unit) and (first in VERBS or (first in PREPS and unit in PREP_PHRASES)):
            phrase_pool.append({**entry, "category": "phrase"})
        elif phrase_ok(unit) and (first in ADJS or first in NOUNS):
            collocation_pool.append({**entry, "category": "collocation"})
        elif expression_ok(unit):
            expression_pool.append({**entry, "category": "expression"})

    # 表达优先已知问候/礼貌/疑问；其余轮转，避免全是 be...
    greeting_priority = {
        "good morning", "good afternoon", "good evening", "good night", "good bye",
        "goodbye", "thank you", "thanks a lot", "excuse me", "sorry", "please",
        "how are you", "nice to meet you", "see you later", "see you tomorrow",
        "what is your name", "where are you from", "how much is it",
    }
    expression_pool.sort(key=lambda x: (0 if x["unit"] in greeting_priority else 1, len(x["unit"]), x["unit"]))
    phrase_units = pick_round_robin(phrase_pool, 2500)
    collocation_units = pick_round_robin(collocation_pool, 1500)
    expression_units = pick_round_robin(expression_pool, 1500)

    all_units = word_units + phrase_units + collocation_units + expression_units + polysemy
    # 去重（按 unit）；类别优先级：word > phrase > collocation > expression > polysemy
    dedup: OrderedDict[str, dict] = OrderedDict()
    for item in all_units:
        dedup.setdefault(item["unit"], item)
    all_units = list(dedup.values())

    # 若不足 10,000，用尚未使用的 Google 单词补足（并标注 category=word_fill）。
    if len(all_units) < args.limit:
        for word in google:
            if len(all_units) >= args.limit:
                break
            if word in dedup or word not in lexicon:
                continue
            item = {**lexicon[word], "category": "word_fill"}
            dedup[word] = item
            all_units.append(item)
    if len(all_units) < args.limit:
        raise SystemExit(f"只能构建 {len(all_units)} 条，少于目标 {args.limit}；请检查数据源")
    all_units = all_units[: args.limit]

    for idx, item in enumerate(all_units, start=1):
        item["id"] = f"daily-{idx:05d}"
        item["split"] = split_of(item["category"], item["unit"])
        item["source"] = "ECDICT+google-10000-english-usa"
        item["license"] = "MIT (ECDICT); wordlist local-research"

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    all_path = out_dir / "daily-knowledge-v1.jsonl"
    with all_path.open("w", encoding="utf-8") as fh:
        for item in all_units:
            fh.write(json.dumps({
                "id": item["id"],
                "unit": item["unit"],
                "category": item["category"],
                "split": item["split"],
                "reference": item["display"],
                "min_senses": item.get("min_senses", 1),
                "source": item["source"],
                "license": item["license"],
            }, ensure_ascii=False, separators=(",", ":")) + "\n")

    units_path = out_dir / "daily-knowledge-v1.units.txt"
    units_path.write_text("".join(item["unit"] + "\n" for item in all_units), encoding="utf-8")

    for split in ("construction", "frozen"):
        path = out_dir / f"daily-knowledge-v1.{split}.jsonl"
        with path.open("w", encoding="utf-8") as fh:
            for item in all_units:
                if item["split"] != split:
                    continue
                fh.write(json.dumps({
                    "id": item["id"],
                    "unit": item["unit"],
                    "category": item["category"],
                    "split": item["split"],
                    "reference": item["display"],
                    "min_senses": item.get("min_senses", 1),
                    "source": item["source"],
                    "license": item["license"],
                }, ensure_ascii=False, separators=(",", ":")) + "\n")

    # 构建补充：所有通过日常过滤的完整短语/搭配/表达，不按测试集成员挑选。
    universe: dict[str, dict] = {}
    for pool in (phrase_pool, collocation_pool, expression_pool):
        for item in pool:
            universe.setdefault(item["unit"], item)
    # 保留已有技术/固定表达补充（它们不应被日常过滤覆盖）。
    existing_path = Path(args.existing_supplement)
    supplement: OrderedDict[str, dict] = OrderedDict()
    if existing_path.exists():
        for line in existing_path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            raw = json.loads(line)
            key = normalized_unit(raw["headword"])
            supplement[key] = raw
    for unit, item in universe.items():
        if unit in supplement:
            continue
        supplement[unit] = {
            "headword": unit,
            "pos": None,
            "definitions": item["senses"][:3],
            "source": "ECDICT",
        }
    dict_out = Path(args.dict_out)
    dict_out.parent.mkdir(parents=True, exist_ok=True)
    with dict_out.open("w", encoding="utf-8") as fh:
        for raw in supplement.values():
            fh.write(json.dumps(raw, ensure_ascii=False, separators=(",", ":")) + "\n")

    counts = Counter(i["category"] for i in all_units)
    split_counts = Counter(i["split"] for i in all_units)
    lock = {
        "schema": "daily-knowledge-lock/v1",
        "generated_at": "2026-11-09",
        "unit_total": len(all_units),
        "category_counts": dict(sorted(counts.items())),
        "split_counts": dict(sorted(split_counts.items())),
        "sources": [
            {
                "name": "ECDICT",
                "path": str(ecdict_path),
                "sha256": ecdict_sha,
                "license": "MIT (repository); upstream gloss provenance not independently audited",
                "usage": "local-research/dictionary-candidates",
            },
            {
                "name": "Google 10000 English (USA)",
                "path": str(wordlist_path),
                "sha256": sha256_file(wordlist_path),
                "license": "no separate declaration; local-research coverage target",
                "usage": "word-selection-only",
            },
        ],
        "selection_rules": [
            "words: Google list single words with valid ECDICT Chinese senses",
            "phrases: ECDICT 2-4 word entries starting with common verb/preposition, no domain/network senses",
            "collocations: ECDICT 2-4 word entries starting with common adjective/noun",
            "expressions: ECDICT 2-4 word entries starting with daily expression starters",
            "split: sha256(category\\0unit) % 5 == 0 -> frozen (~20%)",
            "dictionary supplement: all filtered daily phrases/collocations/expressions, not membership-targeted",
        ],
        "limitations": [
            "source-internal coverage benchmark; not an independent third-party semantic benchmark",
            "phrase translations inherit ECDICT upstream quality; manual review pending",
            "test set coverage != contextual sense selection accuracy",
        ],
        "files": {
            "daily": str(all_path),
            "construction": str(out_dir / "daily-knowledge-v1.construction.jsonl"),
            "frozen": str(out_dir / "daily-knowledge-v1.frozen.jsonl"),
            "dictionary_supplement": str(dict_out),
        },
    }
    lock_path = out_dir / "daily-knowledge-v1.lock.json"
    lock_path.write_text(json.dumps(lock, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    print(json.dumps({
        "units": len(all_units),
        "categories": dict(counts),
        "splits": dict(split_counts),
        "dictionary_supplement": len(supplement),
    }, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
