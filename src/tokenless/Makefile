# Token-Less Unified Build System
# Builds both tokenless (schema compression) and rtk (command rewriting)

INSTALL_DIR ?= /usr/share/tokenless/bin
OPENCLAW_DIR ?= /usr/share/tokenless/openclaw
COPILOT_SHELL_HOOK_DIR ?= /usr/share/tokenless/hooks/copilot-shell
RTK_DIR := third_party/rtk

.PHONY: build build-tokenless build-rtk install test lint clean dist \
        openclaw-install openclaw-uninstall \
        copilot-shell-install copilot-shell-uninstall setup help

# Default target
all: build

# Build both projects
build: build-tokenless build-rtk

build-tokenless:
	@echo "==> Building tokenless..."
	cargo build --release

build-rtk:
	@echo "==> Building rtk..."
	cargo build --release --manifest-path $(RTK_DIR)/Cargo.toml

# Install binaries
install: build
	@echo "==> Installing binaries to $(INSTALL_DIR)..."
	@mkdir -p $(INSTALL_DIR)
	cp target/release/tokenless $(INSTALL_DIR)/
	cp $(RTK_DIR)/target/release/rtk $(INSTALL_DIR)/
	@echo "==> Installed tokenless and rtk to $(INSTALL_DIR)"

# Run tests
test: test-tokenless test-rtk

test-tokenless:
	@echo "==> Testing tokenless..."
	cargo test --workspace

test-rtk:
	@echo "==> Testing rtk build..."
	cargo check --manifest-path $(RTK_DIR)/Cargo.toml

# Lint
lint:
	@echo "==> Linting tokenless..."
	cargo clippy --workspace -- -D warnings

# Format
fmt:
	@echo "==> Formatting..."
	cargo fmt --all

# Clean
clean:
	@echo "==> Cleaning..."
	cargo clean
	cargo clean --manifest-path $(RTK_DIR)/Cargo.toml

# Create source tarball (excludes build artifacts)
dist: clean
	@echo "==> Creating tarball..."
	tar czf ../tokenless-0.1.0.tar.gz \
	    --exclude='target' \
	    --exclude='third_party/rtk/target' \
	    --exclude='.git' \
	    -C .. tokenless
	@echo "==> Tarball: ../tokenless-0.1.0.tar.gz"

# OpenClaw plugin management
openclaw-install:
	@echo "==> Installing OpenClaw plugin..."
	@mkdir -p $(OPENCLAW_DIR)
	cp openclaw/index.ts $(OPENCLAW_DIR)/
	cp openclaw/openclaw.plugin.json $(OPENCLAW_DIR)/
	@# Strip TypeScript type annotations to produce a plain JS file
	@if command -v npx >/dev/null 2>&1; then \
		npx --yes esbuild openclaw/index.ts --bundle --platform=node --format=esm --outfile=$(OPENCLAW_DIR)/index.js 2>/dev/null \
		&& echo "==> Compiled index.ts -> index.js (esbuild)" \
		|| (sed 's/: any//g; s/: string//g; s/: boolean | null/: any/g; s/: Record<string, unknown>//g; s/: { [^}]*}//g' openclaw/index.ts > $(OPENCLAW_DIR)/index.js \
		&& echo "==> Compiled index.ts -> index.js (sed fallback)"); \
	else \
		sed 's/: any//g; s/: string//g; s/: boolean | null/: any/g; s/: Record<string, unknown>//g; s/: { [^}]*}//g' openclaw/index.ts > $(OPENCLAW_DIR)/index.js \
		&& echo "==> Compiled index.ts -> index.js (sed fallback)"; \
	fi
	@# Configure plugins.allow in OpenClaw config
	@OPENCLAW_CONFIG=$(HOME)/.openclaw/openclaw.json; \
	if [ -f "$$OPENCLAW_CONFIG" ] && command -v python3 >/dev/null 2>&1; then \
		python3 -c "\
import json, sys; \
cfg=json.load(open('$$OPENCLAW_CONFIG')); \
p=cfg.setdefault('plugins',{}); \
a=set(p.get('allow',[])); \
a.add('tokenless-openclaw'); \
p['allow']=sorted(a); \
json.dump(cfg,open('$$OPENCLAW_CONFIG','w'),indent=2)" \
		&& echo "==> Added tokenless-openclaw to plugins.allow"; \
	fi
	@echo "==> OpenClaw plugin installed to $(OPENCLAW_DIR)"
	@echo "    Run 'openclaw gateway restart' to activate"

openclaw-uninstall:
	@echo "==> Uninstalling OpenClaw plugin..."
	rm -rf $(OPENCLAW_DIR)
	@echo "==> OpenClaw plugin removed"

# copilot-shell hook management
copilot-shell-install:
	@echo "==> Installing copilot-shell hooks..."
	@mkdir -p $(COPILOT_SHELL_HOOK_DIR)
	cp hooks/copilot-shell/tokenless-*.sh $(COPILOT_SHELL_HOOK_DIR)/
	chmod +x $(COPILOT_SHELL_HOOK_DIR)/tokenless-*.sh
	@echo "==> copilot-shell hooks installed to $(COPILOT_SHELL_HOOK_DIR)"
	@echo "    Add hook configs to your copilot-shell settings.json."
	@echo "    See hooks/copilot-shell/README.md for the full settings.json template."

copilot-shell-uninstall:
	@echo "==> Removing copilot-shell hooks..."
	rm -f $(COPILOT_SHELL_HOOK_DIR)/tokenless-*.sh
	@echo "==> copilot-shell hooks removed"
	@echo "    Remember to remove the hook configs from your copilot-shell settings.json"

# One-step setup: build + install + openclaw plugin + copilot-shell hook
setup: install openclaw-install copilot-shell-install
	@echo ""
	@echo "============================================"
	@echo "  Token-Less setup complete!"
	@echo "  - tokenless: $(INSTALL_DIR)/tokenless"
	@echo "  - rtk:       $(INSTALL_DIR)/rtk"
	@echo "  - OpenClaw:  $(OPENCLAW_DIR)/"
	@echo "  - Hooks:     $(COPILOT_SHELL_HOOK_DIR)/tokenless-*.sh"
	@echo "============================================"
	@echo ""
	@echo "Verify installation:"
	@echo "  tokenless --version"
	@echo "  rtk --version"

# Help
help:
	@echo "Token-Less Build System"
	@echo ""
	@echo "Targets:"
	@echo "  build             Build tokenless + rtk (release mode)"
	@echo "  build-tokenless   Build tokenless only"
	@echo "  build-rtk         Build rtk only"
	@echo "  install           Install binaries to INSTALL_DIR (default: /usr/share/tokenless/bin)"
	@echo "  test              Run all tests"
	@echo "  lint              Run clippy checks"
	@echo "  fmt               Format code"
	@echo "  clean             Clean build artifacts"
	@echo "  openclaw-install  Install OpenClaw plugin"
	@echo "  openclaw-uninstall Remove OpenClaw plugin"
	@echo "  copilot-shell-install   Install copilot-shell hooks"
	@echo "  copilot-shell-uninstall Remove copilot-shell hooks"
	@echo "  setup             Full setup: build + install + openclaw + hook"
	@echo "  help              Show this help"
	@echo ""
	@echo "Variables:"
	@echo "  INSTALL_DIR       Binary install path (default: /usr/share/tokenless/bin)"
	@echo "  OPENCLAW_DIR      OpenClaw plugin path (default: /usr/share/tokenless/openclaw)"
	@echo "  COPILOT_SHELL_HOOK_DIR  Hook install path (default: /usr/share/tokenless/hooks/copilot-shell)"
