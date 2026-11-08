#!/usr/bin/env python3
"""生成 P12-Q 四类补充样本（docs/62 §9.2）。

- word-phrases-v1.jsonl：100 条（90 条项目补充词/短语 + 10 个多义词）
- sentence-v1.jsonl：100 条（40 条新写 + 60 条从 P11 金样本留出的验证句）
- paragraph-v1.jsonl：50 段（由已验证句子组合，保留来源 id）
- document-v1.jsonl：10 篇 Markdown（结构、代码块、列表、术语一致性）

这些是**新增测试集目标**的实现；参考译文仍为项目自编，review_status=pending。
"""
from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIX = ROOT / "fixtures/quality"

NEW_SENTENCES = [
    ("I did not expect the server to fail during the update.", "我没想到服务器会在更新期间发生故障。", "negation"),
    ("She has never seen such a large dataset.", "她从未见过这么大的数据集。", "negation"),
    ("The API does not return errors for empty input.", "API 对空输入不返回错误。", "negation"),
    ("We could not reproduce the issue on Windows.", "我们无法在 Windows 上复现该问题。", "negation"),
    ("Neither the cache nor the database was cleared.", "缓存和数据库都没有被清除。", "negation"),
    ("The package is not available in this region.", "该软件包在此地区不可用。", "negation"),
    ("He rarely uses the command line.", "他很少使用命令行。", "negation"),
    ("No data was lost during the migration.", "迁移过程中没有丢失任何数据。", "negation"),
    ("If the token expires, the client must log in again.", "如果令牌过期，客户端必须重新登录。", "conditional"),
    ("Unless the disk is full, the job will continue.", "除非磁盘已满，否则任务会继续。", "conditional"),
    ("The service will restart if the health check fails.", "如果健康检查失败，服务将重启。", "conditional"),
    ("If you change the schema, update the migration script.", "如果修改了模式，请更新迁移脚本。", "conditional"),
    ("The request is rejected unless the signature is valid.", "除非签名有效，否则请求会被拒绝。", "conditional"),
    ("If the cache is warm, the response is much faster.", "如果缓存已预热，响应会快得多。", "conditional"),
    ("The build fails when a test times out.", "当测试超时时，构建会失败。", "conditional"),
    ("If I had known the cost, I would have chosen another model.", "如果早知道成本，我会选择另一个模型。", "conditional"),
    ("The report covers 1,200 users in 15 countries.", "报告涵盖15个国家的1,200名用户。", "numbers_units"),
    ("The job finished in 2 hours and 30 minutes.", "任务在2小时30分钟内完成。", "numbers_units"),
    ("The error code 0x80004005 indicates access denied.", "错误代码0x80004005表示拒绝访问。", "numbers_units"),
    ("Please retry after 30 seconds.", "请在30秒后重试。", "numbers_units"),
    ("The discount is 20% for orders over 500 yuan.", "订单超过500元可享受20%的折扣。", "numbers_units"),
    ("The server has 128 GB of memory and 32 cores.", "服务器有128 GB内存和32个核心。", "numbers_units"),
    ("The release is scheduled for 2027-01-15.", "发布计划定于2027-01-15。", "numbers_units"),
    ("The file is 4.5 GB, which exceeds the limit.", "文件为4.5 GB，超过了限制。", "numbers_units"),
    ("The bank approved the loan after reviewing the documents.", "银行在审查文件后批准了贷款。", "polysemy"),
    ("We sat on the river bank and watched the boats.", "我们坐在河岸上看船。", "polysemy"),
    ("The spring in the mattress broke last night.", "床垫里的弹簧昨晚断了。", "polysemy"),
    ("The hot spring is a popular tourist attraction.", "这处温泉是热门旅游景点。", "polysemy"),
    ("She will charge the battery before the trip.", "她会在旅行前给电池充电。", "polysemy"),
    ("The hotel charges 300 yuan per night.", "酒店每晚收费300元。", "polysemy"),
    ("The program runs on both Windows and Linux.", "该程序在 Windows 和 Linux 上都能运行。", "polysemy"),
    ("He runs a small software company.", "他经营一家小型软件公司。", "polysemy"),
    ("Please close the window before you leave.", "离开前请关上窗户。", "polysemy"),
    ("She is a close friend of the family.", "她是这个家庭的亲密朋友。", "polysemy"),
    ("The light in the room is too bright.", "房间里的灯太亮了。", "polysemy"),
    ("This laptop is light enough to carry all day.", "这台笔记本电脑很轻，可以整天携带。", "polysemy"),
    ("The match ended in a draw.", "比赛以平局结束。", "polysemy"),
    ("These colors match the logo.", "这些颜色与标志相配。", "polysemy"),
    ("The present was wrapped in blue paper.", "礼物用蓝色纸包着。", "polysemy"),
    ("All members were present at the meeting.", "所有成员都出席了会议。", "polysemy"),
]


def load_golden() -> list[dict]:
    return [json.loads(line) for line in (FIX / "golden-v1.jsonl").read_text(encoding="utf-8").splitlines() if line.strip()]


def build_words() -> int:
    supplement = [
        json.loads(line)
        for line in (ROOT / "fixtures/dictionary/en-zh.supplement.jsonl").read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    selected = supplement[:90]
    rows = []
    for idx, item in enumerate(selected, 1):
        rows.append(
            {
                "id": f"word-v1-{idx:04d}",
                "source": item["headword"],
                "reference": item["definitions"][0],
                "category": "word_phrase",
                "critical_constraints": ["dictionary_contains", "target_language_zh"],
                "review_status": "pending",
                "split": "validation",
            }
        )
    ambiguous = [
        ("bank", "银行"),
        ("spring", "春天"),
        ("charge", "指控"),
        ("run", "跑"),
        ("match", "比赛"),
        ("right", "权利"),
        ("book", "书"),
        ("present", "现在"),
        ("light", "光"),
        ("close", "关闭"),
    ]
    for offset, (word, expected) in enumerate(ambiguous, 1):
        rows.append(
            {
                "id": f"word-v1-{90 + offset:04d}",
                "source": word,
                "reference": expected,
                "category": "word_polysemy",
                "critical_constraints": ["dictionary_contains", "target_language_zh"],
                "review_status": "pending",
                "split": "validation",
            }
        )
    write("word-phrases-v1.jsonl", rows)
    return len(rows)


def build_sentences(golden: list[dict]) -> int:
    rows = []
    for idx, (source, reference, category) in enumerate(NEW_SENTENCES, 1):
        rows.append(
            {
                "id": f"sentence-v1-{idx:04d}",
                "source": source,
                "reference": reference,
                "category": category,
                "critical_constraints": ["target_language_zh"],
                "review_status": "pending",
                "split": "validation-new",
            }
        )
    # 从 P11 金样本留出 60 条（不参与规则/术语调参）：negation/conditional/numbers/technical 各 15
    by_cat: dict[str, list[dict]] = {}
    for row in golden:
        by_cat.setdefault(row["category"], []).append(row)
    picks = []
    for category in ["negation", "conditional", "numbers_units", "technical"]:
        picks.extend(by_cat[category][:15])
    for idx, row in enumerate(picks, len(rows) + 1):
        cloned = dict(row)
        cloned["id"] = f"sentence-v1-{idx:04d}"
        cloned["split"] = "validation-heldout"
        cloned["source_id"] = row["id"]
        rows.append(cloned)
    write("sentence-v1.jsonl", rows)
    return len(rows)


def build_paragraphs(golden: list[dict]) -> int:
    by_cat: dict[str, list[dict]] = {}
    for row in golden:
        by_cat.setdefault(row["category"], []).append(row)
    plans = [
        ("technical", 16, "paragraph-technical"),
        ("daily", 13, "paragraph-daily"),
        ("long", 8, "paragraph-mixed"),
    ]
    rows = []

    def add_group(items: list[dict], out_cat: str) -> None:
        source = " ".join(item["source"] for item in items)
        refs = [item["reference"].rstrip("。！？.!?") for item in items[:-1]]
        reference = "。".join(refs + [items[-1]["reference"]])
        rows.append(
            {
                "id": f"paragraph-v1-{len(rows) + 1:04d}",
                "source": source,
                "reference": reference,
                "category": out_cat,
                "critical_constraints": ["target_language_zh", "paragraph_complete"],
                "review_status": "pending",
                "split": "validation-constructed",
                "source_ids": [item["id"] for item in items],
            }
        )

    for category, count, out_cat in plans:
        pool = by_cat[category]
        made = 0
        i = 0
        while made < count and i + 2 < len(pool):
            add_group(pool[i : i + 3], out_cat)
            i += 3
            made += 1

    # 补足到 50 段（三句一组，句群与整段/逐句才会产生可比较的差异）。
    for category in ["basic", "software", "proper_noun", "polysemy"]:
        pool = by_cat.get(category, [])
        i = 0
        while len(rows) < 50 and i + 2 < len(pool):
            add_group(pool[i : i + 3], "paragraph-mixed")
            i += 3
    write("paragraph-v1.jsonl", rows)
    return len(rows)

DOCUMENTS = [
    (
        "# System status\n\nThe server is running. However, the disk is almost full. Please clean the logs.\n\n- CPU: 50%\n- Memory: 80%\n\n```bash\nsystemctl status app\n```\n",
        "# 系统状态\n\n服务器正在运行。但是磁盘几乎已满。请清理日志。\n\n- CPU：50%\n- 内存：80%\n\n```bash\nsystemctl status app\n```\n",
    ),
    (
        "# Release notes\n\nVersion 2.4.1 fixes a memory leak. It also improves startup time.\n\n- Fixed the login bug\n- Updated the API documentation\n\n```text\nnpm install app@2.4.1\n```\n",
        "# 发布说明\n\n版本 2.4.1 修复了内存泄漏，还缩短了启动时间。\n\n- 修复登录错误\n- 更新 API 文档\n\n```text\nnpm install app@2.4.1\n```\n",
    ),
    (
        "# API guide\n\nSend a POST request to `/api/v1/users`. The server returns JSON.\n\n> All requests require an API key.\n\n```http\nPOST /api/v1/users\n```\n",
        "# API 指南\n\n向 `/api/v1/users` 发送 POST 请求。服务器返回 JSON。\n\n> 所有请求都需要 API 密钥。\n\n```http\nPOST /api/v1/users\n```\n",
    ),
    (
        "# Database migration\n\nThe migration may take 30 minutes. Do not stop the service during this time.\n\n1. Back up the database.\n2. Run the migration script.\n3. Verify the schema.\n",
        "# 数据库迁移\n\n迁移可能需要 30 分钟。在此期间不要停止服务。\n\n1. 备份数据库。\n2. 运行迁移脚本。\n3. 验证模式。\n",
    ),
    (
        "# Security notice\n\nThe password must contain at least 8 characters. Never share your token.\n\n- Use HTTPS\n- Rotate keys every 90 days\n",
        "# 安全公告\n\n密码必须至少包含 8 个字符。切勿分享你的令牌。\n\n- 使用 HTTPS\n- 每 90 天轮换密钥\n",
    ),
    (
        "# Performance report\n\nThroughput increased by 15% after the cache change. Latency dropped from 300 ms to 120 ms.\n\n| Metric | Before | After |\n|---|---|---|\n| Latency | 300 ms | 120 ms |\n",
        "# 性能报告\n\n缓存改动后吞吐量提高了 15%。延迟从 300 毫秒降到 120 毫秒。\n\n| 指标 | 之前 | 之后 |\n|---|---|---|\n| 延迟 | 300 毫秒 | 120 毫秒 |\n",
    ),
    (
        "# Troubleshooting\n\nIf the service does not start, check the log file. The error code 0x80070005 means access denied.\n\n```bash\ntail -n 100 app.log\n```\n",
        "# 故障排查\n\n如果服务无法启动，请检查日志文件。错误代码 0x80070005 表示拒绝访问。\n\n```bash\ntail -n 100 app.log\n```\n",
    ),
    (
        "# Model deployment\n\nDownload the model and verify its checksum. Then restart the worker.\n\n> The model is about 4.5 GB.\n\n```bash\nsha256sum model.onnx\n```\n",
        "# 模型部署\n\n下载模型并验证其校验和，然后重启工作进程。\n\n> 模型约为 4.5 GB。\n\n```bash\nsha256sum model.onnx\n```\n",
    ),
    (
        "# Configuration\n\nSet the cache size to 512 MB. The value must be a positive integer.\n\n```yaml\ncache:\n  size_mb: 512\n```\n",
        "# 配置\n\n将缓存大小设置为 512 MB。该值必须是正整数。\n\n```yaml\ncache:\n  size_mb: 512\n```\n",
    ),
    (
        "# Incident review\n\nAt 09:15 the API stopped responding. The team restarted the service at 09:40.\n\n- Root cause: memory leak\n- Fix: upgrade to 2.4.1\n",
        "# 事故复盘\n\n09:15 时 API 停止响应。团队在 09:40 重启了服务。\n\n- 根因：内存泄漏\n- 修复：升级到 2.4.1\n",
    ),
]


def build_documents() -> int:
    rows = []
    for idx, (source, reference) in enumerate(DOCUMENTS, 1):
        rows.append(
            {
                "id": f"document-v1-{idx:04d}",
                "source": source,
                "reference": reference,
                "category": "document",
                "critical_constraints": ["target_language_zh", "document_structure"],
                "review_status": "pending",
                "split": "validation-document",
            }
        )
    write("document-v1.jsonl", rows)
    return len(rows)


def write(name: str, rows: list[dict]) -> None:
    (FIX / name).write_text(
        "\n".join(json.dumps(row, ensure_ascii=False, separators=(",", ":")) for row in rows) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    golden = load_golden()
    counts = {
        "word": build_words(),
        "sentence": build_sentences(golden),
        "paragraph": build_paragraphs(golden),
        "document": build_documents(),
    }
    print(counts)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
