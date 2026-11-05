#!/usr/bin/env bash
# P3.0：按 models.lock.json 下载并校验 en→zh-Hans 参考模型，然后生成 Bergamot 配置。
#
# 为什么要有这个脚本：模型权重不入库（体积 + 许可待确认），所以"复现"必须靠
# 「记录里的 CDN 位置 + decompressedHash」而不是靠仓库里的二进制。
# 脚本做三件事：
#   1. 从 Firefox Remote Settings CDN 下载 4 个 .zst（model / srcvocab / trgvocab / lex）；
#   2. 解压后逐个比对 models.lock.json 里的 sha256，不一致立刻退出；
#   3. 用 fixtures/nmt/config.enzh.bergamot.yml.example 展开出真实 config。
#
# 用法：
#   ./tools/p3/prepare-model.sh [模型目录]      # 默认 ./models/enzh
#
# 需要：curl、zstd。约 40 MB 下载（压缩后），解压约 51 MB。

set -euo pipefail

DEST="${1:-$PWD/models/enzh}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LOCK="$REPO_ROOT/fixtures/nmt/models.lock.json"
TEMPLATE="$REPO_ROOT/fixtures/nmt/config.enzh.bergamot.yml.example"
BASE="https://firefox-settings-attachments.cdn.mozilla.net"

mkdir -p "$DEST"

# 记录 → (location, 文件名, sha256)：从 lock 文件里读，避免脚本和文档各写一份真相
python3 - "$LOCK" <<'PY' > /tmp/p3-download-plan.txt
import json, sys
lock = json.load(open(sys.argv[1]))
files = [lock["model"]] + lock["support_files"]
for f in files:
    print(f"{f['attachment_location']}\t{f['record_name']}\t{f['verified_sha256']}")
PY

echo "==> 下载与校验（目标目录 $DEST）"
while IFS=$'\t' read -r location name sha; do
  zst="$DEST/$name.zst"
  if [ ! -f "$DEST/$name" ]; then
    echo "  - $name"
    curl -sSL --fail -m 900 "$BASE/$location" -o "$zst"
    zstd -q -d -f "$zst" -o "$DEST/$name"
    rm -f "$zst"
  fi
  actual="$(sha256sum "$DEST/$name" | cut -d' ' -f1)"
  if [ "$actual" != "$sha" ]; then
    echo "!! sha256 不匹配：$name" >&2
    echo "   期望 $sha" >&2
    echo "   实际 $actual" >&2
    exit 1
  fi
  echo "    sha256 ok  $name"
done < /tmp/p3-download-plan.txt

# ssplit 的前缀表来自 ssplit-cpp 仓库（很小，随源码一起取）
if [ ! -f "$DEST/nonbreaking_prefix.en" ]; then
  for candidate in "$PWD/p3-native/bergamot-translator" "$PWD/bergamot-translator"; do
    if [ -f "$candidate/3rd_party/ssplit-cpp/nonbreaking_prefixes/nonbreaking_prefix.en" ]; then
      cp "$candidate/3rd_party/ssplit-cpp/nonbreaking_prefixes/nonbreaking_prefix.en" "$DEST/"
      break
    fi
  done
fi
if [ ! -f "$DEST/nonbreaking_prefix.en" ]; then
  curl -sSL --fail -m 120 \
    "https://raw.githubusercontent.com/ugermann/ssplit-cpp/master/nonbreaking_prefixes/nonbreaking_prefix.en" \
    -o "$DEST/nonbreaking_prefix.en"
fi

echo "==> 生成配置（$DEST/config.bergamot.yml）"
sed "s#\${MODEL_DIR}#$DEST#g" "$TEMPLATE" > "$DEST/config.bergamot.yml"
echo "    配置已写入；用原生 CLI 试一句："
echo "    build/app/bergamot --model-config-paths $DEST/config.bergamot.yml --cpu-threads 2 <<< 'Hello world!'"
