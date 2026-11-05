#!/usr/bin/env bash
# P3.0：构建 Bergamot 原生参考链路（Ubuntu x86_64，无需 root）。
#
# 为什么要有这个脚本：报告里的"原生 CLI 能真实翻译"必须能被别人原样重跑，
# 否则就只是一段自述。脚本把 commit、子模块、cmake 开关、编译器都钉死。
#
# 用法：
#   ./tools/p3/build-bergamot.sh [/path/to/workdir]
#
# 默认工作目录：$PWD/p3-native（三处产物：bergamot-translator/、build/、models/）
#
# 依赖（apt 包名，运行时/构建时）：
#   git cmake ninja-build g++ python3 curl zstd
# 本机没有 root 时的两个变通（脚本会自动识别）：
#   - pcre2：用 -DSSPLIT_USE_INTERNAL_PCRE2=ON，让 ssplit 自己下载编译 pcre2 10.39；
#   - BLAS：bergamot 的 marian-lite 走 intgemm，不需要 MKL/OpenBLAS（cmake 会提示
#     "Could NOT find BLAS"，这不是错误）。

set -euo pipefail

WORKDIR="${1:-$PWD/p3-native}"
BERGAMOT_COMMIT="9271618ebbdc5d21ac4dc4df9e72beb7ce644774"
SRC="$WORKDIR/bergamot-translator"

mkdir -p "$WORKDIR"

if [ ! -d "$SRC/.git" ]; then
  echo "==> clone bergamot-translator @ $BERGAMOT_COMMIT"
  git clone https://github.com/browsermt/bergamot-translator.git "$SRC"
  git -C "$SRC" checkout "$BERGAMOT_COMMIT"
  # 子模块固定到父仓库记录的 revision（marian-dev / ssplit-cpp / intgemm / sentencepiece）
  git -C "$SRC" submodule update --init --recursive --depth 1
else
  echo "==> 复用已有源码 $SRC"
  git -C "$SRC" checkout "$BERGAMOT_COMMIT"
  git -C "$SRC" submodule update --init --recursive --depth 1
fi

echo "==> 实际 commit：$(git -C "$SRC" rev-parse HEAD)"
echo "==> 子模块："
git -C "$SRC" submodule status --recursive | sed -n '1,8p'

echo "==> configure"
cmake -S "$SRC" -B "$SRC/build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DUSE_WASM_COMPATIBLE_SOURCE=off \
  -DSSPLIT_USE_INTERNAL_PCRE2=ON

echo "==> build（8 并行，约 15–25 分钟；首次要编译 marian + sentencepiece + intgemm + pcre2）"
ninja -C "$SRC/build" app/bergamot

echo "==> 产物"
ls -l "$SRC/build/app/bergamot"
echo "==> 运行库依赖（确认没有意外的重型依赖）"
ldd "$SRC/build/app/bergamot" | sed 's/^/    /'
