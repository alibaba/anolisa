#!/bin/bash
# copilot-shell wrapper — locates Node.js and launches cli.js
# @@LIBDIR@@ is replaced at install time by `make install`.

LIBDIR="@@LIBDIR@@"

# ── Resolve Node.js when not already in PATH (e.g. nvm-managed installs) ──
if ! command -v node >/dev/null 2>&1; then
  nvm_dir="${NVM_DIR:-$HOME/.nvm}"
  if [ -d "$nvm_dir/versions/node" ]; then
    latest_dir=$(ls -d "$nvm_dir/versions/node/"v* 2>/dev/null | sort -V | tail -1)
    if [ -n "$latest_dir" ] && [ -x "$latest_dir/bin/node" ]; then
      export PATH="$latest_dir/bin:$PATH"
    fi
  fi
fi

if ! command -v node >/dev/null 2>&1; then
  echo "Error: Node.js not found. Install Node.js >= 20 or configure NVM_DIR." >&2
  exit 1
fi

exec node "$LIBDIR/cli.js" "$@"
