#!/usr/bin/env python3
"""把 opus-mt-en-zh 的 `source.spm` + `vocab.json` 合成一个 `tokenizer.json`。

## 为什么需要这个工具（P4-02 踩过的坑）

这份 ONNX 导出的 id 空间**不是** SentencePiece 的 piece id，而是 `vocab.json` 里那份
65001 条的**联合词表**（源语言 + 目标语言合并，`</s>`=0、`<unk>`=1、`<pad>`=65000）。
直接用 `source.spm` 的 piece id 喂模型**不会报错**，只会输出通顺但完全错误的中文
（门禁阶段实测：`Hello world!` → `和 大说`）。所以 SDK 侧必须用按联合词表构造的分词器。

## 产出结构与依据

- `model`: Unigram，词表顺序 = `vocab.json` 的 id 顺序；
  分数取 `source.spm` 里同名词条的 score（两份词表都是 metaspace `▁` 记法，可以直接对齐），
  `</s>` / `<unk>` 记 0.0，`vocab.json` 里有、`source.spm` 里没有的记 -10.0。
- `pre_tokenizer` / `decoder`: Metaspace（`▁`，`prepend_scheme=always`，`split=true`）。
- 不加 `added_tokens`、不做 truncation/padding：这些都是宿主策略，不该在模型资产里固化。

## 用法

```bash
# 生成
python3 tools/p4/build-tokenizer.py \
    --source-spm /path/to/source.spm \
    --vocab-json /path/to/vocab.json \
    --out /path/to/tokenizer.json

# 只校验（用已知参考答案核对 id 空间，不写文件）
python3 tools/p4/build-tokenizer.py --source-spm ... --vocab-json ... --check
```

`--check` 失败即表示"分词器合成错了"，而不是"模型的译文不好"。
"""

import argparse
import json
import sys

# 门禁阶段用 Python 参考实现核对过的 id（docs/24 §1.4 第 1 条）。
# 这几个数字是"id 空间对不对"的判据，改动分词器必须重跑。
REFERENCE = {
    "Hello world!": [3828, 431, 50],
}

SPECIAL_ZERO = ("</s>", "<unk>")
DEFAULT_SCORE = -10.0


def load_spm_scores(source_spm: str) -> dict:
    try:
        import sentencepiece as spm  # type: ignore
    except ImportError:  # pragma: no cover - 环境问题要说清楚
        sys.exit(
            "缺少 sentencepiece：pip install sentencepiece\n"
            "（只在**离线准备资产**时需要，SDK 运行时不需要 Python）"
        )
    proc = spm.SentencePieceProcessor(model_file=source_spm)
    return {proc.id_to_piece(i): proc.get_score(i) for i in range(proc.get_piece_size())}


def build(source_spm: str, vocab_json: str) -> dict:
    scores = load_spm_scores(source_spm)
    with open(vocab_json, encoding="utf-8") as fh:
        vocab = json.load(fh)
    if not isinstance(vocab, dict):
        sys.exit(f"{vocab_json} 应当是 piece → id 的字典（onnx 导出里就是这种形状）")

    # dict 的插入顺序就是 id 顺序：导出文件按 id 递增写出，这里显式校验一遍
    ordered = sorted(vocab.items(), key=lambda kv: kv[1])
    if [i for _, i in ordered] != list(range(len(ordered))):
        sys.exit(f"{vocab_json} 的 id 不是从 0 开始的连续整数，拒绝合成")

    pieces = []
    for piece, _id in ordered:
        if piece in SPECIAL_ZERO:
            score = 0.0
        else:
            score = scores.get(piece, DEFAULT_SCORE)
        pieces.append([piece, float(score)])

    return {
        "version": "1.0",
        "truncation": None,
        "padding": None,
        "added_tokens": [],
        "normalizer": None,
        "pre_tokenizer": {
            "type": "Metaspace",
            "replacement": "\u2581",
            "prepend_scheme": "always",
            "split": True,
        },
        "post_processor": None,
        "decoder": {
            "type": "Metaspace",
            "replacement": "\u2581",
            "prepend_scheme": "always",
            "split": True,
        },
        "model": {
            "type": "Unigram",
            # unk_id 必须是 1（`<unk>`），不能是 None：
            # None 会让分词器**静默丢掉**词表外的字符（emoji、生僻字），
            # 让"只剩 emoji 的片段"变成空 token 序列 → 上层报 InvalidRequest，
            # 更糟的是含 emoji 的正常句子会悄悄少字符。P4-05 的 sp10 就是这么被打回的。
            "unk_id": 1,
            "byte_fallback": False,
            "vocab": pieces,
        },
    }


def encode_pieces(source_spm: str, vocab_json: str, text: str) -> list:
    """参考实现：piece → 联合词表 id（**两步**，不是直接拿 spm 的 id）。"""
    import sentencepiece as spm  # type: ignore

    proc = spm.SentencePieceProcessor(model_file=source_spm)
    with open(vocab_json, encoding="utf-8") as fh:
        vocab = json.load(fh)
    out = []
    for piece_id in proc.encode(text, out_type=int):
        piece = proc.id_to_piece(piece_id)
        if piece not in vocab:
            sys.exit(f"piece {piece!r} 不在联合词表里：这份 vocab.json 与 spm 不是同一套？")
        out.append(vocab[piece])
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--source-spm", required=True)
    ap.add_argument("--vocab-json", required=True)
    ap.add_argument("--out")
    ap.add_argument("--check", action="store_true", help="只校验 id 空间，不写文件")
    args = ap.parse_args()

    ok = True
    for text, expected in REFERENCE.items():
        got = encode_pieces(args.source_spm, args.vocab_json, text)
        status = "ok" if got == expected else "MISMATCH"
        if got != expected:
            ok = False
        print(f"{text!r}: {got} (期望 {expected}) [{status}]")
        # 也核对一下"直接拿 spm id"的错误做法确实不同，提醒后来者
        import sentencepiece as spm  # type: ignore

        proc = spm.SentencePieceProcessor(model_file=args.source_spm)
        naive = proc.encode(text, out_type=int)
        if naive == expected:
            print(f"  注意：{text!r} 上 spm 原始 id 恰好等于联合 id，不要据此判断实现正确")

    if not ok:
        print("id 空间不一致：分词器合成有误，先修这个再谈译文质量", file=sys.stderr)
        return 1
    if args.check:
        print("仅校验模式：未写文件")
        return 0
    if not args.out:
        print("需要 --out（或加 --check 只校验）", file=sys.stderr)
        return 2

    data = build(args.source_spm, args.vocab_json)
    with open(args.out, "w", encoding="utf-8") as fh:
        json.dump(data, fh, ensure_ascii=False, separators=(",", ":"))
    print(f"已写出 {args.out}（{len(data['model']['vocab'])} 个词条）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
