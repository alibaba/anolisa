# Token-Less Unified Build System
# Builds tokenless (schema/response compression), rtk (command rewriting), and toon (JSON encoding)

SHARE_DIR ?= $(HOME)/.local/share/tokenless
BIN_DIR ?= $(HOME)/.local/bin
OPENCLAW_DIR ?= $(SHARE_DIR)/adapters/openclaw
COSH_EXTENSION_DIR ?= $(HOME)/.copilot-shell/extensions/tokenless
RTK_DIR := third_party/rtk
TOON_DIR := third_party/toon

.PHONY: build build-tokenless build-rtk build-toon install test lint clean \
        openclaw-install openclaw-uninstall \
        cosh-install cosh-uninstall setup help

# Default target
all: build

# Build both projects
build: build-tokenless build-rtk build-toon

build-tokenless:
	@echo "==> Building tokenless..."
	cargo build --release

build-rtk:
	@echo "==> Building rtk..."
	cargo build --release --manifest-path $(RTK_DIR)/Cargo.toml

build-toon:
	@echo "==> Building toon..."
	cargo build --release --manifest-path $(TOON_DIR)/Cargo.toml --features cli

# Install binaries
install: build
	@echo "==> Installing binaries to $(BIN_DIR)..."
	@mkdir -p $(BIN_DIR)
	cp target/release/tokenless $(BIN_DIR)/
	cp $(RTK_DIR)/target/release/rtk $(BIN_DIR)/
	cp $(TOON_DIR)/target/release/toon $(BIN_DIR)/
	@echo "==> Installed tokenless, rtk, and toon to $(BIN_DIR)"

# Run tests
test: test-tokenless test-rtk test-toon

test-tokenless:
	@echo "==> Testing tokenless..."
	cargo test --workspace

test-rtk:
	@echo "==> Testing rtk build..."
	cargo check --manifest-path $(RTK_DIR)/Cargo.toml

test-toon:
	@echo "==> Testing toon build..."
	cargo check --manifest-path $(TOON_DIR)/Cargo.toml

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
	cargo clean --manifest-path $(TOON_DIR)/Cargo.toml

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

# copilot-shell extension management
cosh-install:
	@echo "==> Installing copilot-shell extension..."
	@rm -rf $(COSH_EXTENSION_DIR) 2>/dev/null || true
	@mkdir -p $(COSH_EXTENSION_DIR)
	cp -r cosh-extension/hooks $(COSH_EXTENSION_DIR)/
	cp -r cosh-extension/commands $(COSH_EXTENSION_DIR)/
	cp cosh-extension/cosh-extension.json $(COSH_EXTENSION_DIR)/
	cp cosh-extension/README.md $(COSH_EXTENSION_DIR)/
	chmod +x $(COSH_EXTENSION_DIR)/hooks/*.py
	@echo "==> copilot-shell extension installed to $(COSH_EXTENSION_DIR)"

cosh-uninstall:
	@echo "==> Uninstalling copilot-shell extension..."
	rm -rf $(COSH_EXTENSION_DIR)
	@echo "==> copilot-shell extension removed"

# One-step setup: build + install + openclaw plugin + cosh extension
setup: install openclaw-install cosh-install
	@echo ""
	@echo "============================================"
	@echo "  Token-Less setup complete!"
	@echo "  - tokenless: $(BIN_DIR)/tokenless"
	@echo "  - rtk:       $(BIN_DIR)/rtk"
	@echo "  - toon:      $(BIN_DIR)/toon"
	@echo "  - OpenClaw:  $(OPENCLAW_DIR)/"
	@echo "  - Cosh Ext:  $(COSH_EXTENSION_DIR)/"
	@echo "============================================"
	@echo ""
	@echo "Verify installation:"
	@echo "  tokenless --version"
	@echo "  rtk --version"
	@echo "  toon --version"

# Help
help:
	@echo "Token-Less Build System"
	@echo ""
	@echo "Targets:"
	@echo "  build             Build tokenless + rtk + toon (release mode)"
	@echo "  build-tokenless   Build tokenless only"
	@echo "  build-rtk         Build rtk only"
	@echo "  build-toon        Build toon only"
	@echo "  install           Install binaries to BIN_DIR (default: ~/.local/bin)"
	@echo "  test              Run all tests"
	@echo "  lint              Run clippy checks"
	@echo "  fmt               Format code"
	@echo "  clean             Clean build artifacts"
	@echo "  openclaw-install  Install OpenClaw plugin"
	@echo "  openclaw-uninstall Remove OpenClaw plugin"
	@echo "  cosh-install         Install copilot-shell extension"
	@echo "  cosh-uninstall       Uninstall copilot-shell extension"
	@echo "  setup                Full setup: build + install + openclaw + cosh extension"
	@echo "  help              Show this help"
	@echo ""
	@echo "Variables:"
	@echo "  SHARE_DIR         Data install root (default: ~/.local/share/tokenless)"
	@echo "  BIN_DIR           Binary install path (default: ~/.local/bin)"
	@echo "  OPENCLAW_DIR      OpenClaw plugin path (default: ~/.local/share/tokenless/adapters/openclaw)"
	@echo "  COSH_EXTENSION_DIR  Cosh extension install path (default: ~/.copilot-shell/extensions/tokenless)"
