#!/usr/bin/env bash
# 把 release 二进制与运行时资源打成 tar.gz / zip，供 GitHub Release 分发。
set -euo pipefail

AGENT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_ROOT="$(cd "$AGENT_DIR/.." && pwd)"
cd "$AGENT_DIR"

VERSION="${1:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')}"
TARGET="${2:-$(rustc -vV | sed -n 's/^host: //p')}"
BIN="${3:-$REPO_ROOT/target/release/spatiotemporal-agent}"

if [[ ! -f "$BIN" ]]; then
    echo "找不到二进制：$BIN" >&2
    echo "用法：$0 [version] [target-triple] [path/to/spatiotemporal-agent]" >&2
    exit 1
fi

if [[ ! -f target/guests/outline.wasm ]]; then
    echo "缺 outline.wasm，先跑：./scripts/build-guests.sh" >&2
    exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

PKG="spatiotemporal-agent-${VERSION}-${TARGET}"
ROOT="$STAGE/$PKG"
mkdir -p "$ROOT/target/guests" "$ROOT/plugins/generated"

cp "$BIN" "$ROOT/"
cp target/guests/outline.wasm "$ROOT/target/guests/"
cp cordis.yml cordis.coding.yml cordis.creation.yml cordis.smoke.yml cordis.patch.example.yml "$ROOT/"
cp -R assets plugins "$ROOT/"
cp plugins/generated/.gitkeep "$ROOT/plugins/generated/" 2>/dev/null || true

cat >"$ROOT/INSTALL.md" <<EOF
# Spatiotemporal Agent ${VERSION} (${TARGET})

## 快速开始

1. 解压本包到任意目录。
2. 设置 API Key（不要写进 cordis.yml）：

   \`\`\`bash
   export DEEPSEEK_API_KEY=sk-你的key
   \`\`\`

3. 在**你的工作区目录**启动 agent（fs/bash 沙箱根 = 启动时的 cwd）：

   \`\`\`bash
   cd /path/to/your/project
   /path/to/spatiotemporal-agent-${TARGET}/spatiotemporal-agent
   \`\`\`

   编码模式：追加 \`--coding\`；创造模式：\`--creation\`；CI 自检：\`--smoke\`。

4. 浏览器打开 http://127.0.0.1:8787

## 可选环境变量

| 变量 | 作用 |
|---|---|
| \`DEEPSEEK_API_KEY\` | DeepSeek Bearer token |
| \`DEEPSEEK_BASE_URL\` | API 网关（默认 \`https://api.deepseek.com\`） |
| \`DEEPSEEK_MODEL\` | 模型名（默认 \`deepseek-chat\`） |
| \`PORT\` | Web 端口（默认 8787，或 cordis.yml \`ui.config.port\`） |
| \`WORKSPACE\` | 覆盖启动时的工作区根 |
| \`SPATIOTEMPORAL_AGENT_HOME\` | 覆盖本包内 cordis/assets/plugins 所在目录 |

文档：https://github.com/zifeng-yang/spatiotemporal/tree/main/spatiotemporal-agent
EOF

DIST="$AGENT_DIR/dist"
mkdir -p "$DIST"
ARCHIVE="$DIST/${PKG}"

case "$TARGET" in
    *windows*)
        if command -v zip >/dev/null 2>&1; then
            (cd "$STAGE" && zip -rq "${ARCHIVE}.zip" "$PKG")
        else
            tar -caf "${ARCHIVE}.zip" -C "$STAGE" "$PKG"
        fi
        echo "产物：${ARCHIVE}.zip"
        ;;
    *)
        tar -C "$STAGE" -czf "${ARCHIVE}.tar.gz" "$PKG"
        echo "产物：${ARCHIVE}.tar.gz"
        ;;
esac

ls -la "$DIST"
