# Token-Less Unified Build System
# Builds tokenless (schema/response compression), rtk (command rewriting), and toon (JSON encoding)

INSTALL_PROFILE ?= user
DESTDIR ?=

ifeq ($(INSTALL_PROFILE),user)
PREFIX ?= $(HOME)/.local
COSH_EXTENSION_DIR ?= $(HOME)/.copilot-shell/extensions/tokenless
else
PREFIX ?= /usr
COSH_EXTENSION_DIR ?= $(PREFIX)/share/anolisa/extensions/tokenless
endif

BINDIR ?= $(PREFIX)/bin
LIBEXECDIR ?= $(PREFIX)/libexec/anolisa/tokenless
DATADIR ?= $(PREFIX)/share
SHARE_DIR ?= $(DATADIR)/anolisa/adapters/tokenless
BIN_DIR ?= $(BINDIR)
LIB_DIR ?= $(LIBEXECDIR)
ADAPTER_SRC_DIR := adapters/tokenless
OPENCLAW_PLUGIN_SRC_DIR := $(ADAPTER_SRC_DIR)/openclaw
ADAPTER_TEMPLATES := \
    $(ADAPTER_SRC_DIR)/manifest.json \
    $(ADAPTER_SRC_DIR)/openclaw/package.json \
    $(ADAPTER_SRC_DIR)/openclaw/openclaw.plugin.json \
    $(ADAPTER_SRC_DIR)/hermes/plugin.yaml \
    $(ADAPTER_SRC_DIR)/qoder/.qoder-plugin/plugin.json \
    $(ADAPTER_SRC_DIR)/claude-code/.claude-plugin/plugin.json \
    $(ADAPTER_SRC_DIR)/codex/.codex-plugin/plugin.json \
    $(ADAPTER_SRC_DIR)/qwencode/qwen-extension.json
# Component contract (publishing/plugin metadata) — kept separate from the
# adapter payload tree so the contract is not mixed with adapter resources.
# Shipped to %{_datadir}/anolisa/components/tokenless/component.toml.
COMPONENT_CONTRACT := .anolisa/component.toml
TOON_VER := 0.4.6
VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')

.PHONY: build build-tokenless build-toon build-openclaw-plugin \
        stamp-adapter-templates \
        install uninstall test lint clean \
        install-binaries install-helpers install-adapter-resources install-cosh-extension \
        adapter-install adapter-uninstall adapter-scan \
        cosh-extension-install cosh-extension-uninstall \
        openclaw-install openclaw-uninstall \
        hermes-install hermes-uninstall \
        qoder-install qoder-uninstall \
        claude-code-install claude-code-uninstall \
        codex-install codex-uninstall \
        qwencode-install qwencode-uninstall \
        setup help \
        test-hooks

# Default target
all: build

# Build both projects
build: build-tokenless build-toon build-openclaw-plugin

build-tokenless:
	@echo "==> Building tokenless + rtk..."
	just setup-rtk
	cargo build --release
	cargo build --release --manifest-path third_party/rtk/Cargo.toml

build-toon:
	@echo "==> Installing toon binary (v$(TOON_VER))..."
	cargo install toon-format --version $(TOON_VER) --locked

# Generate adapter config files from .in templates (VERSION from Cargo.toml).
stamp-adapter-templates: $(ADAPTER_TEMPLATES) $(COMPONENT_CONTRACT)

$(ADAPTER_SRC_DIR)/manifest.json: $(ADAPTER_SRC_DIR)/manifest.json.in Cargo.toml
	@echo "==> Generating manifest.json (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

$(COMPONENT_CONTRACT): $(COMPONENT_CONTRACT).in Cargo.toml
	@echo "==> Generating component contract (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

$(ADAPTER_SRC_DIR)/openclaw/package.json: $(ADAPTER_SRC_DIR)/openclaw/package.json.in Cargo.toml
	@echo "==> Generating openclaw/package.json (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

$(ADAPTER_SRC_DIR)/openclaw/openclaw.plugin.json: $(ADAPTER_SRC_DIR)/openclaw/openclaw.plugin.json.in Cargo.toml
	@echo "==> Generating openclaw/openclaw.plugin.json (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

$(ADAPTER_SRC_DIR)/hermes/plugin.yaml: $(ADAPTER_SRC_DIR)/hermes/plugin.yaml.in Cargo.toml
	@echo "==> Generating hermes/plugin.yaml (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

$(ADAPTER_SRC_DIR)/qoder/.qoder-plugin/plugin.json: $(ADAPTER_SRC_DIR)/qoder/.qoder-plugin/plugin.json.in Cargo.toml
	@echo "==> Generating qoder/.qoder-plugin/plugin.json (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

$(ADAPTER_SRC_DIR)/claude-code/.claude-plugin/plugin.json: \
    $(ADAPTER_SRC_DIR)/claude-code/.claude-plugin/plugin.json.in Cargo.toml
	@echo "==> Generating claude-code/.claude-plugin/plugin.json (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

$(ADAPTER_SRC_DIR)/codex/.codex-plugin/plugin.json: $(ADAPTER_SRC_DIR)/codex/.codex-plugin/plugin.json.in Cargo.toml
	@echo "==> Generating codex/.codex-plugin/plugin.json (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

$(ADAPTER_SRC_DIR)/qwencode/qwen-extension.json: \
    $(ADAPTER_SRC_DIR)/qwencode/qwen-extension.json.in Cargo.toml
	@echo "==> Generating qwencode/qwen-extension.json (version=$(VERSION))..."
	sed 's/@VERSION@/$(VERSION)/g' $< > $@

# Build the tokenless OpenClaw plugin (TypeScript -> dist/index.js).
# Produces $(OPENCLAW_PLUGIN_SRC_DIR)/dist/index.js, which install-adapter-resources
# then copies into $(SHARE_DIR)/openclaw/dist/index.js. This file is referenced by
# the plugin's package.json ("main" / "openclaw.extensions") and must be present
# for `openclaw plugins install` to succeed.
build-openclaw-plugin: stamp-adapter-templates
	@echo "==> Building tokenless OpenClaw plugin..."
	cd $(OPENCLAW_PLUGIN_SRC_DIR) && npm install --legacy-peer-deps --no-audit --no-fund --package-lock=false
	cd $(OPENCLAW_PLUGIN_SRC_DIR) && npm run build
	@test -f $(OPENCLAW_PLUGIN_SRC_DIR)/dist/index.js \
	    || { echo "ERROR: $(OPENCLAW_PLUGIN_SRC_DIR)/dist/index.js was not produced"; exit 1; }

# Install binaries + adapter resources per FHS spec.
install: build install-binaries install-helpers install-adapter-resources install-cosh-extension

install-binaries:
	@echo "==> Installing tokenless to $(DESTDIR)$(BINDIR)..."
	install -d -m 0755 $(DESTDIR)$(BINDIR)
	install -p -m 0755 target/release/tokenless $(DESTDIR)$(BINDIR)/

install-helpers:
	@echo "==> Installing helpers to $(DESTDIR)$(LIBEXECDIR)..."
	install -d -m 0755 $(DESTDIR)$(LIBEXECDIR)
	install -p -m 0755 third_party/rtk/target/release/rtk $(DESTDIR)$(LIBEXECDIR)/
	install -p -m 0755 $(HOME)/.cargo/bin/toon $(DESTDIR)$(LIBEXECDIR)/
	ln -sf $(LIBEXECDIR)/rtk $(DESTDIR)$(BINDIR)/rtk
	ln -sf $(LIBEXECDIR)/toon $(DESTDIR)$(BINDIR)/toon

install-adapter-resources: build-openclaw-plugin
	@echo "==> Installing adapter resources to $(DESTDIR)$(SHARE_DIR)..."
	rm -rf $(DESTDIR)$(SHARE_DIR)
	install -d -m 0755 $(DESTDIR)$(SHARE_DIR)
	cp -pr $(ADAPTER_SRC_DIR)/. $(DESTDIR)$(SHARE_DIR)/
	# Strip build-only artifacts from the installed plugin tree.
	rm -rf $(DESTDIR)$(SHARE_DIR)/openclaw/node_modules
	rm -f  $(DESTDIR)$(SHARE_DIR)/openclaw/package-lock.json
	find $(DESTDIR)$(SHARE_DIR) -name '*.in' -delete
	find $(DESTDIR)$(SHARE_DIR) -type f \( -name '*.py' -o -name '*.sh' \) -exec chmod 0755 {} +
	@test -f $(DESTDIR)$(SHARE_DIR)/openclaw/dist/index.js \
	    || { echo "ERROR: $(DESTDIR)$(SHARE_DIR)/openclaw/dist/index.js missing after install-adapter-resources"; exit 1; }

install-cosh-extension:
	@echo "==> Installing tokenless cosh extension to $(DESTDIR)$(COSH_EXTENSION_DIR)..."
	rm -rf $(DESTDIR)$(COSH_EXTENSION_DIR)
	install -d -m 0755 $(DESTDIR)$(COSH_EXTENSION_DIR)/hooks $(DESTDIR)$(COSH_EXTENSION_DIR)/commands
	cp -pr $(ADAPTER_SRC_DIR)/common/hooks/. $(DESTDIR)$(COSH_EXTENSION_DIR)/hooks/
	cp -pr $(ADAPTER_SRC_DIR)/common/commands/. $(DESTDIR)$(COSH_EXTENSION_DIR)/commands/
	install -p -m 0644 $(ADAPTER_SRC_DIR)/common/cosh-extension.json $(DESTDIR)$(COSH_EXTENSION_DIR)/
	find $(DESTDIR)$(COSH_EXTENSION_DIR) -type f \( -name '*.py' -o -name '*.sh' \) -exec chmod 0755 {} +

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/tokenless
	rm -f $(DESTDIR)$(LIBEXECDIR)/rtk
	rm -f $(DESTDIR)$(LIBEXECDIR)/toon
	rm -rf $(DESTDIR)$(SHARE_DIR)
	rm -rf $(DESTDIR)$(COSH_EXTENSION_DIR)

# Run tests
test: test-tokenless test-hooks

test-tokenless:
	@echo "==> Testing tokenless..."
	cargo test -p tokenless-cli -p tokenless-schema -p tokenless-stats

test-hooks:
	@echo "==> Testing copilot-shell hooks..."
	bash tests/run-all-tests.sh

# Lint (rtk excluded — vendored third-party source)
lint:
	@echo "==> Linting tokenless..."
	cargo clippy -p tokenless-cli -p tokenless-schema -p tokenless-stats -- -D warnings

# Format
fmt:
	@echo "==> Formatting..."
	cargo fmt -p tokenless-cli -p tokenless-schema -p tokenless-stats

# Clean (rtk excluded from workspace — needs explicit clean)
clean:
	@echo "==> Cleaning..."
	cargo clean
	cargo clean --release --manifest-path third_party/rtk/Cargo.toml 2>/dev/null || true
	rm -rf $(OPENCLAW_PLUGIN_SRC_DIR)/dist $(OPENCLAW_PLUGIN_SRC_DIR)/node_modules
	rm -f  $(OPENCLAW_PLUGIN_SRC_DIR)/package-lock.json
	rm -f  $(ADAPTER_TEMPLATES) $(COMPONENT_CONTRACT)

# Create source tarball (excludes build artifacts)
dist: clean
	@echo "==> Creating tarball (with pre-patched rtk source)..."
	just setup-rtk
	tar czf ../tokenless-$(VERSION).tar.gz \
	    --exclude='target' \
	    --exclude='.git' \
	    -C .. tokenless
	@echo "==> Tarball: ../tokenless-$(VERSION).tar.gz"

# Adapter management — invokes detect/install/uninstall action scripts per the
# ANOLISA FHS adapter spec.  Environment variables point to installed FHS paths.
ADAPTER_ENV = ANOLISA_PREFIX=$(HOME)/.local \
              ANOLISA_ADAPTER_DIR=$(SHARE_DIR) \
              ANOLISA_COMPONENT=tokenless \
              ANOLISA_VERSION=$(VERSION)

adapter-install: cosh-extension-install openclaw-install hermes-install qoder-install claude-code-install codex-install qwencode-install

adapter-uninstall: cosh-extension-uninstall openclaw-uninstall hermes-uninstall qoder-uninstall claude-code-uninstall codex-uninstall qwencode-uninstall

adapter-scan:
	@echo "=== Tokenless Adapter Manifest ==="
	@python3 -c "import json; m=json.load(open('$(SHARE_DIR)/manifest.json')); print(f\"  component: {m['component']} v{m['version']}\"); [print(f\"  {t:12s} {c['compatibleVersions']:12s} \" + (', '.join(f'{len(v)} {k}' for k,v in c.get('capabilities',{}).items()) or 'no caps')) for t,c in m['targets'].items()]"

# --- Copilot Shell (cosh) — extension format ---
# cosh uses cosh-extension.json (not adapter scripts) for auto-discovery.
# The extension directory deploys hooks/commands/spec from adapter resources.

cosh-extension-install:
	@echo "==> Installing tokenless cosh extension..."
	@rm -rf $(COSH_EXTENSION_DIR)
	@install -d -m 0755 $(COSH_EXTENSION_DIR)/hooks $(COSH_EXTENSION_DIR)/commands
	cp -pr $(SHARE_DIR)/common/hooks/. $(COSH_EXTENSION_DIR)/hooks/
	cp -pr $(SHARE_DIR)/common/commands/. $(COSH_EXTENSION_DIR)/commands/
	install -p -m 0644 $(SHARE_DIR)/common/cosh-extension.json $(COSH_EXTENSION_DIR)/
	find $(COSH_EXTENSION_DIR) -type f \( -name '*.py' -o -name '*.sh' \) -exec chmod 0755 {} +
	@echo "==> tokenless cosh extension installed to $(COSH_EXTENSION_DIR)"

cosh-extension-uninstall:
	@echo "==> Uninstalling tokenless cosh extension..."
	rm -rf $(COSH_EXTENSION_DIR)
	@echo "==> tokenless cosh extension removed from $(COSH_EXTENSION_DIR)"

# --- OpenClaw ---

openclaw-install:
	@echo "==> Installing tokenless OpenClaw plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=openclaw bash $(SHARE_DIR)/openclaw/scripts/detect.sh
	@$(ADAPTER_ENV) ANOLISA_TARGET=openclaw bash $(SHARE_DIR)/openclaw/scripts/install.sh

openclaw-uninstall:
	@echo "==> Uninstalling tokenless OpenClaw plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=openclaw bash $(SHARE_DIR)/openclaw/scripts/uninstall.sh

# --- Hermes Agent ---
HERMES_ADAPTER_DIR = $(SHARE_DIR)/hermes

hermes-install:
	@echo "==> Installing Hermes Agent plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=hermes bash $(HERMES_ADAPTER_DIR)/scripts/detect.sh || true
	@$(ADAPTER_ENV) ANOLISA_TARGET=hermes bash $(HERMES_ADAPTER_DIR)/scripts/install.sh

hermes-uninstall:
	@echo "==> Uninstalling Hermes Agent plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=hermes bash $(HERMES_ADAPTER_DIR)/scripts/uninstall.sh

# --- Qoder CLI ---
QODER_ADAPTER_DIR = $(SHARE_DIR)/qoder

qoder-install:
	@echo "==> Installing tokenless Qoder CLI plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=qoder bash $(QODER_ADAPTER_DIR)/scripts/detect.sh || true
	@$(ADAPTER_ENV) ANOLISA_TARGET=qoder bash $(QODER_ADAPTER_DIR)/scripts/install.sh

qoder-uninstall:
	@echo "==> Uninstalling tokenless Qoder CLI plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=qoder bash $(QODER_ADAPTER_DIR)/scripts/uninstall.sh

# --- Claude Code ---
# Plugin is sourced from SHARE_DIR (FHS-installed adapter resources). When
# claude is not available the scripts gracefully no-op so `make adapter-install`
# stays usable on machines without Claude Code.
CLAUDE_CODE_ADAPTER_DIR = $(SHARE_DIR)/claude-code

claude-code-install:
	@echo "==> Installing Claude Code plugin..."
	@# detect.sh exits 1 (installable) or 2 (missing prereq); we tolerate both
	@# and let install.sh make the final call (it gracefully no-ops without claude CLI).
	@$(ADAPTER_ENV) ANOLISA_TARGET=claude-code bash $(CLAUDE_CODE_ADAPTER_DIR)/scripts/detect.sh || true
	@$(ADAPTER_ENV) ANOLISA_TARGET=claude-code bash $(CLAUDE_CODE_ADAPTER_DIR)/scripts/install.sh

claude-code-uninstall:
	@echo "==> Uninstalling Claude Code plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=claude-code bash $(CLAUDE_CODE_ADAPTER_DIR)/scripts/uninstall.sh

# --- Codex ---
CODEX_ADAPTER_DIR = $(SHARE_DIR)/codex

codex-install:
	@echo "==> Installing Codex plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=codex bash $(CODEX_ADAPTER_DIR)/scripts/detect.sh || true
	@$(ADAPTER_ENV) ANOLISA_TARGET=codex bash $(CODEX_ADAPTER_DIR)/scripts/install.sh

codex-uninstall:
	@echo "==> Uninstalling Codex plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=codex bash $(CODEX_ADAPTER_DIR)/scripts/uninstall.sh

# --- Qwen Code ---
QWENCODE_ADAPTER_DIR = $(SHARE_DIR)/qwencode

qwencode-install:
	@echo "==> Installing Qwen Code plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=qwencode bash $(QWENCODE_ADAPTER_DIR)/scripts/detect.sh || true
	@$(ADAPTER_ENV) ANOLISA_TARGET=qwencode bash $(QWENCODE_ADAPTER_DIR)/scripts/install.sh

qwencode-uninstall:
	@echo "==> Uninstalling Qwen Code plugin..."
	@$(ADAPTER_ENV) ANOLISA_TARGET=qwencode bash $(QWENCODE_ADAPTER_DIR)/scripts/uninstall.sh

# One-step setup: build + install + all adapters
setup: install adapter-install
	@echo ""
	@echo "============================================"
	@echo "  Token-Less setup complete!"
	@echo "============================================"
	@echo "  Binaries (FHS):"
	@echo "    tokenless -> $(BIN_DIR)/tokenless"
	@echo "    rtk       -> $(LIB_DIR)/rtk"
	@echo "    toon      -> $(LIB_DIR)/toon"
	@echo "  Extension (cosh):"
	@echo "    $(COSH_EXTENSION_DIR)/"
	@echo "  Adapter (FHS):"
	@echo "    $(SHARE_DIR)/"
	@echo "  Registered:"
	@echo "    cosh, openclaw, hermes, qoder, claude-code, codex, qwencode"
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
	@echo "  build             Build tokenless + rtk + toon + openclaw plugin"
	@echo "  build-tokenless   Build tokenless + rtk (via justfile)"
	@echo "  build-toon        Install toon binary via cargo install toon-format"
	@echo "  build-openclaw-plugin  Compile OpenClaw plugin TS -> dist/index.js"
	@echo "  install           Install binaries + adapter to FHS paths"
	@echo "  uninstall         Remove installed files"
	@echo "  test              Run all tests"
	@echo "  lint              Run clippy checks"
	@echo "  fmt               Format code"
	@echo "  clean             Clean build artifacts"
	@echo "  adapter-scan      List registered adapter capabilities"
	@echo "  adapter-install   Register all adapters (cosh+openclaw+hermes+qoder+claude-code+codex+qwencode)"
	@echo "  adapter-uninstall Unregister all adapters"
	@echo "  cosh-extension-install  Install cosh extension"
	@echo "  cosh-extension-uninstall Uninstall cosh extension"
	@echo "  openclaw-install  Register OpenClaw plugin"
	@echo "  openclaw-uninstall Unregister OpenClaw plugin"
	@echo "  hermes-install    Install Hermes Agent plugin"
	@echo "  hermes-uninstall  Uninstall Hermes Agent plugin"
	@echo "  qoder-install     Register Qoder CLI plugin"
	@echo "  qoder-uninstall   Unregister Qoder CLI plugin"
	@echo "  claude-code-install   Register Claude Code plugin via claude CLI"
	@echo "  claude-code-uninstall Unregister Claude Code plugin"
	@echo "  codex-install     Register Codex plugin via codex CLI"
	@echo "  codex-uninstall   Unregister Codex plugin"
	@echo "  qwencode-install  Register Qwen Code plugin via qwen CLI"
	@echo "  qwencode-uninstall Unregister Qwen Code plugin"
	@echo "  setup             Full setup: build + install + register adapters"
	@echo "  help              Show this help"
	@echo ""
	@echo "Variables:"
	@echo "  INSTALL_PROFILE  user or system (default: user)"
	@echo "  DESTDIR          Staging root for package builds"
	@echo "  PREFIX           Install prefix (default: ~/.local or /usr)"
	@echo "  BINDIR           Binary install path"
	@echo "  LIBEXECDIR       Helper binary install path"
	@echo "  SHARE_DIR        Adapter resources path"
	@echo "  COSH_EXTENSION_DIR  Cosh extension path"
