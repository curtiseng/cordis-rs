#!/usr/bin/env bash
#
# 把 guests/ 下的每个 crate 编成 wasm 组件，产物放到 target/guests/。
#
# 为什么要显式跑这一步，而不是塞进 build.rs：guest 编到另一个 target、有自己的
# workspace 和 lockfile，让 build.rs 去 shell out 会把这些藏起来，出错时信息也
# 难看。测试找不到产物会直接失败并让你回来跑这个脚本——总比「测试绿了但其实
# 什么都没测」好。
set -euo pipefail

cd "$(dirname "$0")/.."
OUT="$(pwd)/target/guests"
mkdir -p "$OUT"

if ! rustup target list --installed | grep -q wasm32-wasip2; then
    echo "缺 wasm32-wasip2，先跑：rustup target add wasm32-wasip2" >&2
    exit 1
fi

# 显式指定输出目录，而不是猜 `$dir/target`：CARGO_TARGET_DIR 可能已经被外面
# 设过（沙箱、共享缓存、CI 缓存都会这么干），猜路径就会扑空。
BUILD="$OUT/build"

for dir in guests/*/; do
    name="$(basename "$dir")"
    echo "==> 编 $name"
    (cd "$dir" && CARGO_TARGET_DIR="$BUILD" cargo build --release --target wasm32-wasip2)
    cp "$BUILD/wasm32-wasip2/release/$name.wasm" "$OUT/$name.wasm"
done

echo
# 花括号是必须的：紧跟变量名的全角标点会被 bash 当成标识符的一部分。
echo "产物在 ${OUT}"
ls -la "$OUT"
