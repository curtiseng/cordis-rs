#!/usr/bin/env bash
#
# 把 guests/ 下的每个 crate 编成原生可执行文件，产物放到 target/guests/。
set -euo pipefail

cd "$(dirname "$0")/.."
OUT="$(pwd)/target/guests"
mkdir -p "$OUT"

BUILD="$OUT/build"

for dir in guests/*/; do
    name="$(basename "$dir")"
    echo "==> 编 $name"
    (cd "$dir" && CARGO_TARGET_DIR="$BUILD" cargo build --release)
    cp "$BUILD/release/$name" "$OUT/$name"
done

echo
echo "产物在 ${OUT}"
ls -la "$OUT"
