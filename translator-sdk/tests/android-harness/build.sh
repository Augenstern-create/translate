#!/usr/bin/env bash
# P6.1 Android Harness 构建 + 部署 + 运行（仅模拟器，无 Gradle）。
#
# 为什么不用 Gradle：它会把依赖和缓存写到 ~/.gradle（工作区外），
# 按 ADR-057 需要逐项审批。这里只用 cmdline-tools 自带的 javac/d8
# 与模拟器内的 /data/local/tmp，宿主侧不写工作区外任何文件（docs/39 D-134）。
#
# 用法：
#   tests/android-harness/build.sh                 # 构建 + 部署 + 运行
#   tests/android-harness/build.sh --build-only
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
sdk="${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}"
adb="$sdk/platform-tools/adb"
d8="$sdk/cmdline-tools/latest/bin/d8"
out="$here/out"
devdir="/data/local/tmp/p7"
lib="$repo/target/x86_64-linux-android/release/libtranslator_jni.so"

mode="${1:-all}"

if [ ! -f "$lib" ]; then
  echo "缺少 $lib：先跑 cargo build -p translator-jni --target x86_64-linux-android --release" >&2
  exit 3
fi

# 只清理构建产物，保留 out/ 下的证据日志（p71.log、j07-install.log 等）。
rm -rf "$out/classes" "$out/dex"
mkdir -p "$out/classes" "$out/dex"

echo "== javac"
javac -encoding UTF-8 -source 8 -target 8 -d "$out/classes" \
  "$here/src/ai/ridge/translator/NativeBridge.java" \
  "$here/src/ai/ridge/translator/Harness.java" \
  "$here/src/ai/ridge/translator/PerfHarness.java"

echo "== d8 → dex"
"$d8" --min-api 21 --output "$out/dex" \
  $(find "$out/classes" -name '*.class') 2>&1 | tail -3
ls -l "$out/dex/classes.dex"

if [ "$mode" = "--build-only" ]; then
  echo "built: $out"
  exit 0
fi

echo "== adb push"
"$adb" shell "mkdir -p $devdir" >/dev/null
"$adb" push "$lib" "$devdir/libtranslator_jni.so" >/dev/null
"$adb" push "$out/dex/classes.dex" "$devdir/classes.dex" >/dev/null
if [ -d /tmp/dictpkg ]; then
  "$adb" push /tmp/dictpkg "$devdir/dictpkg" >/dev/null
fi
if [ -d /tmp/p9-jmdict ]; then
  "$adb" push /tmp/p9-jmdict "$devdir/p9dict" >/dev/null
fi
if [ -d /tmp/p7repo ]; then
  "$adb" shell "rm -rf $devdir/repo && mkdir -p $devdir/repo" >/dev/null
  "$adb" push /tmp/p7repo/. "$devdir/repo/" >/dev/null
  # 模拟器内路径与宿主脚本约定一致
  "$adb" shell "rm -rf /data/local/tmp/p7repo && mv $devdir/repo /data/local/tmp/p7repo"
fi

if [ "$mode" = "--perf" ]; then
  echo "== run J-14"
  "$adb" shell "cd $devdir && app_process -Djava.class.path=$devdir/classes.dex \
    -Djava.library.path=$devdir /system/bin ai.ridge.translator.PerfHarness $devdir/dictpkg ${2:-35}"
  exit 0
fi

echo "== run"
"$adb" shell "cd $devdir && app_process -Djava.class.path=$devdir/classes.dex \
  -Djava.library.path=$devdir /system/bin ai.ridge.translator.Harness $devdir/dictpkg $devdir/p9dict"
