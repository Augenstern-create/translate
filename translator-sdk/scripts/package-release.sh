#!/usr/bin/env bash
# 打发布包（Linux）：release 二进制 + 必要文件 + SHA256SUMS。
#
# 用法：bash scripts/package-release.sh [输出目录]
# 产物：augenstern-translate-linux-x64.tar.gz 与 SHA256SUMS.txt
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/dist}"
version="$(grep -m1 '^version' "$root/Cargo.toml" | sed -E 's/.*"(.*)".*/\1/')"
name="augenstern-translate-linux-x64"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

cd "$root"
echo "==> 构建 release（--locked）"
cargo build --locked --release -p translator-cli

mkdir -p "$stage/$name"
cp target/release/translator-cli "$stage/$name/"
cp README.md LICENSE-MIT LICENSE-APACHE THIRD_PARTY_NOTICES.md translate.example.toml "$stage/$name/"
cp "$root/../docs/release.md" "$stage/$name/RELEASE.md" 2>/dev/null || true

echo "==> 打包"
mkdir -p "$out"
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"

echo "==> 生成 SHA-256"
(
  cd "$out"
  sha256sum "$name.tar.gz" > SHA256SUMS.txt
  cat SHA256SUMS.txt
)
echo "产物目录：$out"
