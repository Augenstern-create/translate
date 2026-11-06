#!/usr/bin/env python3
"""生成最小 CycloneDX 1.5 SBOM（依赖 + 许可证），不联网。

数据源：`cargo metadata --format-version 1 --locked`。
只输出机器可核对字段：name/version/license/repository/source。
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent


def main() -> int:
    out_path = Path(sys.argv[1]) if len(sys.argv) > 1 else SDK / "dist/p7/sbom.json"
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=SDK,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        ).stdout
    )
    components = []
    for pkg in sorted(meta["packages"], key=lambda p: (p["name"], p["version"])):
        components.append(
            {
                "type": "library",
                "name": pkg["name"],
                "version": pkg["version"],
                "bom-ref": f"pkg:cargo/{pkg['name']}@{pkg['version']}",
                "licenses": (
                    [{"license": {"id": pkg["license"]}}] if pkg.get("license") else []
                ),
                "externalReferences": (
                    [{"type": "vcs", "url": pkg["repository"]}]
                    if pkg.get("repository")
                    else []
                ),
                "properties": [
                    {"name": "cargo.source", "value": pkg.get("source") or "workspace"}
                ],
            }
        )
    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "translator-sdk",
                "version": "0.1.0",
            },
            "tools": [{"name": "tools/make-sbom.py"}],
        },
        "components": components,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(sbom, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"SBOM: {out_path}（{len(components)} 个组件）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
