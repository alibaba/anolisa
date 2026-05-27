#!/bin/bash

set -euo pipefail

PLUGIN_DST="${HOME}/.hermes/plugins/ws-ckpt"
SKILL_DST="${HOME}/.hermes/skills/ws-ckpt"

# 1. Remove plugin symlink
if [ -L "$PLUGIN_DST" ] || [ -d "$PLUGIN_DST" ]; then
    rm -rf "$PLUGIN_DST"
    echo "plugin removed: $PLUGIN_DST"
fi

# 2. Remove ws-ckpt config from ~/.hermes/config.yaml
HERMES_CONFIG="${HOME}/.hermes/config.yaml"
if [ -f "$HERMES_CONFIG" ]; then
    python3 -c "
import sys, re

path = sys.argv[1]
with open(path) as f:
    lines = f.readlines()

out = []
in_plugins = False
plugins_indent = -1
skip_indent = -1

for line in lines:
    stripped = line.strip()
    indent = len(line) - len(line.lstrip()) if stripped else 0

    # Track whether we're inside the plugins: block
    if re.match(r'^plugins:\s*$', line):
        in_plugins = True
        plugins_indent = indent
        out.append(line)
        continue
    if in_plugins and stripped and indent <= plugins_indent:
        in_plugins = False

    # Still skipping children of ws-ckpt: block
    if skip_indent >= 0:
        if not stripped:
            out.append(line)
            continue
        if indent > skip_indent:
            continue
        skip_indent = -1

    # Only act inside plugins: block
    if in_plugins:
        if re.match(r'^\s*- ws-ckpt\s*$', line):
            continue
        m = re.match(r'^(\s*)ws-ckpt:\s*$', line)
        if m:
            skip_indent = len(m.group(1))
            continue

    out.append(line)

with open(path, 'w') as f:
    f.writelines(out)
" "$HERMES_CONFIG" && echo "ws-ckpt config removed from $HERMES_CONFIG"
fi

# 3. Remove skill if exists
if [ -d "$SKILL_DST" ]; then
    rm -rf "$SKILL_DST"
    echo "skill removed: $SKILL_DST"
fi