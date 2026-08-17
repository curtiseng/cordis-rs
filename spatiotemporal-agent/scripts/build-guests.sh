#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="$(pwd)/target/guests"
mkdir -p "$OUT"

if ! rustup target list --installed | grep -q wasm32-wasip2; then
    echo "缺 wasm32-wasip2，先跑：rustup target add wasm32-wasip2" >&2
    exit 1
fi

BUILD="$OUT/build"
echo "==> 编 outline"
(cd guests/outline && CARGO_TARGET_DIR="$BUILD" cargo build --release --target wasm32-wasip2)
cp "$BUILD/wasm32-wasip2/release/outline.wasm" "$OUT/outline.wasm"
echo "产物在 ${OUT}"
ls -la "$OUT"
