#!/usr/bin/env bash
# 安装 Rust 工具链到 /opt/rust（CARGO_HOME=/opt/rust/cargo, RUSTUP_HOME=/opt/rust/rustup）
# 幂等：已安装的部分会跳过
set -euo pipefail

export RUSTUP_HOME=/opt/rust/rustup
export CARGO_HOME=/opt/rust/cargo
export PATH="$CARGO_HOME/bin:$PATH"

echo "=== [1/6] 准备目录 ==="
mkdir -p "$RUSTUP_HOME" "$CARGO_HOME"
ls -ld /opt/rust "$RUSTUP_HOME" "$CARGO_HOME"

echo "=== [2/6] 下载 rustup-init ==="
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
echo "脚本大小: $(wc -c < /tmp/rustup-init.sh) 字节"

echo "=== [3/6] 安装 stable 工具链（minimal profile，不改动 shell 配置）==="
if [ -x "$CARGO_HOME/bin/rustup" ]; then
  echo "rustup 已存在，执行更新"
  rustup update stable --no-self-update || true
else
  sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain stable
fi

echo "=== [4/6] 安装组件 ==="
for c in rustfmt clippy llvm-tools-preview; do
  rustup component add "$c" || echo "!! 组件 $c 安装失败"
done
# rust-analyzer 组件在部分镜像源上可能缺失，失败不阻断
rustup component add rust-analyzer || echo "!! rust-analyzer 组件不可用（可后续 cargo install --locked rust-analyzer）"

echo "=== [5/6] 添加交叉编译目标 ==="
for t in aarch64-unknown-linux-gnu x86_64-pc-windows-gnu aarch64-linux-android; do
  rustup target add "$t" || echo "!! 目标 $t 添加失败"
done

echo "=== [6/6] 生成环境脚本 ==="
cat > /opt/rust/env.sh <<'ENVEOF'
# Rust 工具链环境（由 DeepSeek Harness 于 2026-11-05 生成）
export RUSTUP_HOME=/opt/rust/rustup
export CARGO_HOME=/opt/rust/cargo
case ":$PATH:" in
  *":$CARGO_HOME/bin:"*) ;;
  *) export PATH="$CARGO_HOME/bin:$PATH" ;;
esac
ENVEOF
chmod 644 /opt/rust/env.sh

echo
echo "=== 安装结果 ==="
rustc --version
cargo --version
rustup --version
echo "--- 组件 ---"
rustup component list --installed
echo "--- 目标平台 ---"
rustup target list --installed
echo "--- 占用 ---"
du -sh /opt/rust/rustup /opt/rust/cargo 2>/dev/null

echo
echo "=== 冒烟测试（编译并运行 hello world）==="
rm -rf /tmp/_rusthello
cargo new --quiet /tmp/_rusthello
cd /tmp/_rusthello
cargo build --quiet
./target/debug/_rusthello
cd /
rm -rf /tmp/_rusthello
echo "=== 全部完成 ==="
