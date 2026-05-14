# Token-Less Unified Build System
# Builds tokenless (schema/response compression), rtk (command rewriting), and toon (JSON encoding)

SHARE_DIR ?= $(HOME)/.local/share/anolisa/adapters/tokenless
BIN_DIR ?= $(HOME)/.local/bin
LIB_DIR ?= $(HOME)/.local/lib/anolisa/tokenless
ADAPTER_DIR := adapters/tokenless
RTK_DIR := third_party/rtk
TOON_DIR := third_party/toon

.PHONY: build build-tokenless build-rtk build-toon install test lint clean \
        adapter-install adapter-uninstall adapter-scan \
        cosh-install cosh-uninstall \
        openclaw-install openclaw-uninstall \
        setup help \
        test-hooks

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

# Install binaries + adapter resources per FHS spec
install: build
	@echo "==> Installing binaries..."
	@mkdir -p $(BIN_DIR) $(LIB_DIR)
	cp target/release/tokenless $(BIN_DIR)/
	cp $(RTK_DIR)/target/release/rtk $(LIB_DIR)/
	cp $(TOON_DIR)/target/release/toon $(LIB_DIR)/
	ln -sf $(LIB_DIR)/rtk $(BIN_DIR)/rtk
	ln -sf $(LIB_DIR)/toon $(BIN_DIR)/toon
	@echo "==> Installed tokenless to $(BIN_DIR), rtk/toon to $(LIB_DIR) (symlinked to $(BIN_DIR))"
	@echo "==> Installing adapter resources to $(SHARE_DIR)..."
	@mkdir -p $(SHARE_DIR)/common/hooks $(SHARE_DIR)/common/commands \
	           $(SHARE_DIR)/cosh/scripts $(SHARE_DIR)/openclaw/scripts
	cp $(ADAPTER_DIR)/manifest.json $(SHARE_DIR)/
	cp $(ADAPTER_DIR)/common/tool-ready-spec.json $(SHARE_DIR)/common/
	cp $(ADAPTER_DIR)/common/tokenless-env-fix.sh $(SHARE_DIR)/common/
	cp $(ADAPTER_DIR)/common/hooks/*.py $(SHARE_DIR)/common/hooks/
	cp $(ADAPTER_DIR)/common/hooks/*.sh $(SHARE_DIR)/common/hooks/
	cp $(ADAPTER_DIR)/common/commands/*.toml $(SHARE_DIR)/common/commands/
	cp $(ADAPTER_DIR)/cosh/scripts/detect.sh $(SHARE_DIR)/cosh/scripts/
	cp $(ADAPTER_DIR)/cosh/scripts/install.sh $(SHARE_DIR)/cosh/scripts/
	cp $(ADAPTER_DIR)/cosh/scripts/uninstall.sh $(SHARE_DIR)/cosh/scripts/
	cp $(ADAPTER_DIR)/openclaw/scripts/detect.sh $(SHARE_DIR)/openclaw/scripts/
	cp $(ADAPTER_DIR)/openclaw/scripts/install.sh $(SHARE_DIR)/openclaw/scripts/
	cp $(ADAPTER_DIR)/openclaw/scripts/uninstall.sh $(SHARE_DIR)/openclaw/scripts/
	cp $(ADAPTER_DIR)/openclaw/openclaw.plugin.json $(SHARE_DIR)/openclaw/
	cp $(ADAPTER_DIR)/openclaw/package.json $(SHARE_DIR)/openclaw/
	@echo "==> Adapter resources installed to $(SHARE_DIR)"

# Run tests
test: test-tokenless test-rtk test-toon test-hooks

test-tokenless:
	@echo "==> Testing tokenless..."
	cargo test --workspace

test-rtk:
	@echo "==> Testing rtk build..."
	cargo check --manifest-path $(RTK_DIR)/Cargo.toml

test-toon:
	@echo "==> Testing toon build..."
	cargo check --manifest-path $(TOON_DIR)/Cargo.toml

test-hooks:
	@echo "==> Testing copilot-shell hooks..."
	bash tests/run-all-tests.sh

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

# Adapter management — invokes detect/install/uninstall action scripts per the
# ANOLISA FHS adapter spec.  Environment variables point to installed FHS paths.
ADAPTER_ENV = ANOLISA_PREFIX=$(HOME)/.local \
              ANOLISA_ADAPTER_DIR=$(SHARE_DIR) \
              ANOLISA_COMPONENT=tokenless \
              ANOLISA_VERSION=0.3.2

adapter-install: cosh-install openclaw-install

adapter-uninstall: cosh-uninstall openclaw-uninstall

adapter-scan:
	@echo "=== Tokenless Adapter Manifest ==="
	@python3 -c "import json; m=json.load(open('$(SHARE_DIR)/manifest.json')); print(f\"  component: {m['component']} v{m['version']}\"); [print(f\"  {t:12s} {c['compatibleVersions']:12s} \" + (', '.join(f'{len(v)} {k}' for k,v in c.get('capabilities',{}).items()) or 'no caps')) for t,c in m['targets'].items()]"

# --- copilot-shell (cosh) ---

cosh-install:
	@echo "==> Installing tokenless cosh extension..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=cosh bash $(SHARE_DIR)/cosh/scripts/install.sh

cosh-uninstall:
	@echo "==> Uninstalling tokenless cosh extension..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=cosh bash $(SHARE_DIR)/cosh/scripts/uninstall.sh

# --- OpenClaw ---

openclaw-install:
	@echo "==> Installing tokenless OpenClaw plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=openclaw bash $(SHARE_DIR)/openclaw/scripts/detect.sh
	@$(ADAPTER_ENV) ANOLISA_TARGET=openclaw bash $(SHARE_DIR)/openclaw/scripts/install.sh

openclaw-uninstall:
	@echo "==> Uninstalling tokenless OpenClaw plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=openclaw bash $(SHARE_DIR)/openclaw/scripts/uninstall.sh

# One-step setup: build + install + all adapters
setup: install adapter-install
	@echo ""
	@echo "============================================"
	@echo "  Token-Less setup complete!"
	@echo "============================================"
	@echo "  Binaries (FHS):"
	@echo "    tokenless -> $(BIN_DIR)/tokenless"
	@echo "    rtk       -> $(LIB_DIR)/rtk -> $(BIN_DIR)/rtk"
	@echo "    toon      -> $(LIB_DIR)/toon -> $(BIN_DIR)/toon"
	@echo "  Adapter (FHS):"
	@echo "    $(SHARE_DIR)/"
	@echo "  Registered:"
	@echo "    cosh, openclaw"
	@echo ""
	@echo "Verify:"
	@echo "  tokenless --version"
	@echo "  rtk --version"
	@echo "  toon --version"
	@echo "  make adapter-scan"

# Help
help:
	@echo "Token-Less Build System"
	@echo ""
	@echo "Targets:"
	@echo "  build             Build tokenless + rtk + toon (release mode)"
	@echo "  build-tokenless   Build tokenless only"
	@echo "  build-rtk         Build rtk only"
	@echo "  build-toon        Build toon only"
	@echo "  install           Install binaries + adapter to FHS paths"
	@echo "  test              Run all tests"
	@echo "  lint              Run clippy checks"
	@echo "  fmt               Format code"
	@echo "  clean             Clean build artifacts"
	@echo "  adapter-scan      List registered adapter capabilities"
	@echo "  adapter-install   Register all adapters (cosh+openclaw)"
	@echo "  adapter-uninstall Unregister all adapters"
	@echo "  cosh-install      Register copilot-shell extension"
	@echo "  cosh-uninstall    Unregister copilot-shell extension"
	@echo "  openclaw-install  Register OpenClaw plugin"
	@echo "  openclaw-uninstall Unregister OpenClaw plugin"
	@echo "  setup             Full setup: build + install + register adapters"
	@echo "  help              Show this help"
	@echo ""
	@echo "Variables:"
	@echo "  BIN_DIR    User commands (default: ~/.local/bin)"
	@echo "  LIB_DIR    Helper binaries (default: ~/.local/lib/anolisa/tokenless)"
	@echo "  SHARE_DIR  Adapter resources (default: ~/.local/share/anolisa/adapters/tokenless)"