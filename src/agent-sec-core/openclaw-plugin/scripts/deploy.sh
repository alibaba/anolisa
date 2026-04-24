#!/usr/bin/env bash
# =============================================================================
# Deploy agent-sec OpenClaw plugin
#
# Usage:
#   ./scripts/deploy.sh [PLUGIN_DIR]
#
# Supports:
#   - Fresh install
#   - Upgrade (using openclaw plugins install --force)
#   - Multi-plugin coexistence
#
# NOTE: This script ONLY registers the plugin with openclaw config.
#       It does NOT start/stop openclaw-gateway. Use systemd or manually
#       restart the service after deployment.
# =============================================================================

set -euo pipefail

PLUGIN_DIR="${1:-/opt/agent-sec/openclaw-plugin}"

# Convert to absolute path if relative
PLUGIN_DIR="$(cd "$PLUGIN_DIR" && pwd)"

# 1. 前置检查
command -v openclaw >/dev/null 2>&1 || { echo "ERROR: openclaw 不在 PATH 中"; exit 1; }
command -v agent-sec-cli >/dev/null 2>&1 || { echo "ERROR: agent-sec-cli 不在 PATH 中"; exit 1; }
[[ -f "$PLUGIN_DIR/openclaw.plugin.json" ]] || { echo "ERROR: 清单文件不存在: $PLUGIN_DIR/openclaw.plugin.json"; exit 1; }
[[ -d "$PLUGIN_DIR/dist" ]] || { echo "ERROR: dist/ 不存在,请先运行 npm run build"; exit 1; }

PLUGIN_VERSION=$(jq -r '.version' "$PLUGIN_DIR/openclaw.plugin.json")
echo "部署插件: agent-sec v${PLUGIN_VERSION}"
echo "  路径: $PLUGIN_DIR"

# 2. 使用官方命令安装插件
echo ""
echo "安装插件..."
openclaw plugins install "$PLUGIN_DIR" --force --dangerously-force-unsafe-install

echo "  ✓ 插件已安装/更新"

echo ""
echo "提示: 请重启 OpenClaw gateway 以加载插件"
echo "  openclaw gateway restart"
echo ""
echo "拦截 prompt 注入风险请求"
echo "  openclaw config set plugins.entries.agent-sec.config.promptScanBlock true"
