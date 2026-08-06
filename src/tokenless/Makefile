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
ANOLISA_COMPONENT_MANIFEST := ../anolisa/manifests/components/tokenless/component.toml
TOON_VER := 0.5.0
VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
# Architecture suffix for release tarball naming (derived from the Rust host
# triple — more accurate than uname -m under Rosetta). Cross-architecture
# builds are NOT supported: set up a native build environment for the target
# arch instead, or pass CARGO_TARGET_TRIPLE and a matching --target to cargo.
ARCH ?= $(shell rustc -vV 2>/dev/null | sed -n 's/^host: *\([^ -]*\).*/\1/p' || uname -m)

.PHONY: build build-tokenless build-toon build-openclaw-plugin \
        stamp-adapter-templates check-component-contract \
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
        test-hooks \
        release release-install-script release-check-cargo-config \
        npm-package npm-package-all npm-publish

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

check-component-contract:
	@tmp=$$(mktemp); \
		cp "$(COMPONENT_CONTRACT)" "$$tmp"; \
		trap 'cp "$$tmp" "$(COMPONENT_CONTRACT)"; rm -f "$$tmp"' EXIT; \
		$(MAKE) -B "$(COMPONENT_CONTRACT)"; \
		cmp -s "$$tmp" "$(COMPONENT_CONTRACT)" || { \
			diff -u "$$tmp" "$(COMPONENT_CONTRACT)"; \
			exit 1; \
		}
	@catalog_version=$$(sed -n 's/^version = "\([^"]*\)"/\1/p' \
		"$(ANOLISA_COMPONENT_MANIFEST)" | head -1); \
		[ "$$catalog_version" = "$(VERSION)" ] || { \
			echo "ANOLISA tokenless manifest version '$$catalog_version' does not match $(VERSION)"; \
			exit 1; \
		}

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
test: test-tokenless test-hooks test-integration test-adapters

test-tokenless:
	@echo "==> Testing tokenless..."
	cargo test --workspace -- --test-threads=1

test-hooks:
	@echo "==> Testing copilot-shell hooks..."
	bash tests/test-run-hook-install-scope.sh
	bash tests/test-tool-ready-readable-fixer.sh
	bash tests/run-all-tests.sh

test-integration:
	@echo "==> Testing hook integration (Python)..."
	python3 tests/test_compress_response_hook.py
	python3 tests/test_compress_schema_hook.py
	python3 tests/test_rewrite_hook.py
	python3 tests/test_resolve_agent_id.py

test-adapters:
	@echo "==> Testing qoder adapter installer..."
	bash tests/test-qoder-adapter-install.sh

# Lint (rtk excluded — vendored third-party source)
lint:
	@echo "==> Linting tokenless..."
	cargo clippy --workspace -- -D warnings

# Format
fmt:
	@echo "==> Formatting..."
	cargo fmt --all

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
	    --exclude='tokenless/dist' \
	    --exclude='node_modules' \
	    --exclude='__pycache__' \
	    -C .. tokenless
	@echo "==> Tarball: ../tokenless-$(VERSION).tar.gz"

# Path variables used by the embedded install.sh.
# IMPORTANT — keep these in sync with the Makefile definitions above:
#   BINDIR            ?= $(PREFIX)/bin
#   LIBEXECDIR        ?= $(PREFIX)/libexec/anolisa/tokenless
#   SHARE_DIR         ?= $(PREFIX)/share/anolisa/adapters/tokenless
#   components        :  $(PREFIX)/share/anolisa/components/tokenless
#   COSH_EXTENSION_DIR?= $(PREFIX)/share/anolisa/extensions/tokenless
# The install.sh defines these same paths relative to its own PREFIX default,
# so any future layout change (e.g. libexec restructuring, new adapter
# subdirectories) must be updated in BOTH the Makefile variables and the
# install.sh variable definitions below to prevent path drift between
# `make install` and the release tarball installer.
#
# Overwrite behavior: the install.sh performs destructive operations —
#   - replaces rtk/toon symlinks (or binaries) in $BINDIR
#   - rm -rf + reinstall of the entire $SHARE_DIR directory
#   - replaces the cosh extension directory
# All destructive actions emit warnings before execution. This is by design
# for a self-contained release installer. For environments requiring
# non-destructive or staged installs, use `make install` directly.
#
# Pre-flight guards: the release target refuses to build on non-Linux
# platforms and rejects Cargo configuration overrides of [build] target or
# target-dir — in $CARGO_HOME/config{,.toml}, in any .cargo/config{,.toml}
# from the working directory up to the filesystem root (all locations Cargo
# merges), and via CARGO_BUILD_TARGET / CARGO_TARGET_DIR /
# CARGO_BUILD_TARGET_DIR environment variables — since those would change
# the artifact paths the packaging step relies on.
#
# Build all binaries and package into a release tarball.
# Produces dist/tokenless-$(VERSION)-$(ARCH).tar.gz containing:
#   bin/tokenless        - main CLI binary
#   libexec/rtk          - command rewriting proxy
#   libexec/toon         - TOON encoder/decoder (built fresh via cargo install)
#   share/adapters/      - adapter plugins (openclaw, hermes, qoder, etc.)
#   share/component.toml - component contract
#   docs/                - user manuals
#   LICENSE
#   install.sh           - standalone deployment script
release:
	@# --- Pre-flight guards ---
	@# [Guard] Linux-only: tokenless release artifacts are Linux-only.
	@if [ "$$(uname -s)" != "Linux" ]; then \
	    echo "ERROR: make release is only supported on Linux."; \
	    echo "       Current platform: $$(uname -s)."; \
	    echo "       Use a Linux build environment."; \
	    exit 1; \
	fi
	@# [Guard] Cargo config files can override target/target-dir via config
	@# rather than environment variables. Cargo merges config from
	@# $CARGO_HOME plus every .cargo/config{,.toml} from the CWD up to the
	@# filesystem root, so all those locations are scanned (target_dir, the
	@# underscore spelling cargo also accepts, is rejected as well).
	@CFG_FILES=""; \
	CARGO_HOME_DIR="$${CARGO_HOME:-$$HOME/.cargo}"; \
	for f in "$$CARGO_HOME_DIR/config.toml" "$$CARGO_HOME_DIR/config"; do \
	    [ -f "$$f" ] && CFG_FILES="$$CFG_FILES$$f "; \
	done; \
	d=$$(pwd); \
	while :; do \
	    for f in "$$d/.cargo/config.toml" "$$d/.cargo/config"; do \
	        [ -f "$$f" ] && CFG_FILES="$$CFG_FILES$$f "; \
	    done; \
	    case "$$d" in /) break ;; esac; \
	    d=$${d%/*}; [ -n "$$d" ] || d=/; \
	done; \
	for cfg in $$CFG_FILES; do \
	    if grep -qE '^[[:space:]]*target[[:space:]]*=' "$$cfg" 2>/dev/null; then \
	        echo "ERROR: Cargo config $$cfg sets [build] target."; \
	        echo "       release builds require the default target. Remove or rename $$cfg."; \
	        exit 1; \
	    fi; \
	    if grep -qE '^[[:space:]]*target[-_]dir[[:space:]]*=' "$$cfg" 2>/dev/null; then \
	        echo "ERROR: Cargo config $$cfg sets [build] target-dir."; \
	        echo "       release builds require the default target-dir. Remove or rename $$cfg."; \
	        exit 1; \
	    fi; \
	done
	@# [Guard] Environment variable checks
	@HOST_ARCH="$$(rustc -vV 2>/dev/null | sed -n 's/^host: *\([^ -]*\).*/\1/p' || uname -m)"; \
	if [ -n "$$CARGO_BUILD_TARGET" ]; then \
	    echo "ERROR: CARGO_BUILD_TARGET is set to $$CARGO_BUILD_TARGET."; \
	    echo "       release builds always use target/release/ — cross-compilation is not supported."; \
	    echo "       Unset CARGO_BUILD_TARGET and build on a native $(ARCH) machine."; \
	    exit 1; \
	fi; \
	if [ -n "$$CARGO_TARGET_DIR" ]; then \
	    echo "ERROR: CARGO_TARGET_DIR is set to $$CARGO_TARGET_DIR."; \
	    echo "       release builds always read from target/release/ — custom target dirs are not supported."; \
	    echo "       Unset CARGO_TARGET_DIR before running make release."; \
	    exit 1; \
	fi; \
	if [ -n "$$CARGO_BUILD_TARGET_DIR" ]; then \
	    echo "ERROR: CARGO_BUILD_TARGET_DIR is set to $$CARGO_BUILD_TARGET_DIR."; \
	    echo "       release builds always read from target/release/ — custom target dirs are not supported."; \
	    echo "       Unset CARGO_BUILD_TARGET_DIR before running make release."; \
	    exit 1; \
	fi; \
	if [ "$(ARCH)" != "$$HOST_ARCH" ]; then \
	    echo "ERROR: ARCH=$(ARCH) does not match build host architecture ($$HOST_ARCH)."; \
	    echo "       Cross-architecture release builds are not supported."; \
	    echo "       Build on a native $(ARCH) machine instead."; \
	    exit 1; \
	fi
	@# --- Build ---
	$(MAKE) --no-print-directory build
	@# --- Package ---
	@echo "==> Packaging release tarball for tokenless v$(VERSION)..."
	@rm -rf dist/tokenless-$(VERSION)
	@mkdir -p dist/tokenless-$(VERSION)/bin \
	    dist/tokenless-$(VERSION)/libexec \
	    dist/tokenless-$(VERSION)/share/adapters/tokenless \
	    dist/tokenless-$(VERSION)/docs
	install -m 0755 target/release/tokenless dist/tokenless-$(VERSION)/bin/
	install -m 0755 third_party/rtk/target/release/rtk dist/tokenless-$(VERSION)/libexec/
	@# [Toon] Build toon into a private staging dir to guarantee we package
	@# the binary from THIS build, not an arbitrary toon from the environment.
	@STAGING=$$(mktemp -d); \
	trap 'rm -rf "$$STAGING"' EXIT; \
	if ! cargo install toon-format --version $(TOON_VER) --locked --root "$$STAGING" 2>"$$STAGING/install.err"; then \
	    echo "ERROR: toon install failed — a release tarball must bundle the toon from this build."; \
	    cat "$$STAGING/install.err" >&2; \
	    exit 1; \
	elif [ ! -x "$$STAGING/bin/toon" ]; then \
	    echo "ERROR: toon binary not found at staging path $$STAGING/bin/toon — refusing to package."; \
	    exit 1; \
	else \
	    install -m 0755 "$$STAGING/bin/toon" dist/tokenless-$(VERSION)/libexec/toon; \
	    echo "==> Bundled toon $$("$$STAGING/bin/toon" --version 2>/dev/null || echo $(TOON_VER))"; \
	fi
	tar -cf - -C $(ADAPTER_SRC_DIR) \
	    --exclude='node_modules' \
	    --exclude='.tsbuildinfo' \
	    --exclude='*.in' \
	    --exclude='tests' \
	    . | tar -xf - -C dist/tokenless-$(VERSION)/share/adapters/tokenless/
	find dist/tokenless-$(VERSION)/share/adapters/tokenless \
	    -type f \( -name '*.py' -o -name '*.sh' \) -exec chmod 0755 {} +
	install -m 0644 $(COMPONENT_CONTRACT) dist/tokenless-$(VERSION)/share/component.toml
	install -m 0644 ../../docs/user-guide/en/token-saving/tokenless/user-manual.md dist/tokenless-$(VERSION)/docs/tokenless-user-manual-en.md
	install -m 0644 ../../docs/user-guide/zh/token-saving/tokenless/user-manual.md dist/tokenless-$(VERSION)/docs/tokenless-user-manual-zh.md
	install -m 0644 docs/response-compression.md dist/tokenless-$(VERSION)/docs/
	install -m 0644 LICENSE dist/tokenless-$(VERSION)/
	$(MAKE) --no-print-directory release-install-script
	@rm -f dist/tokenless-$(VERSION)-$(ARCH).tar.gz
	tar czf dist/tokenless-$(VERSION)-$(ARCH).tar.gz -C dist tokenless-$(VERSION)
	@rm -rf dist/tokenless-$(VERSION)
	@echo ""
	@echo "==> Release tarball: dist/tokenless-$(VERSION)-$(ARCH).tar.gz $$(du -sh dist/tokenless-$(VERSION)-$(ARCH).tar.gz | cut -f1)"
	@echo "==> Extract and run ./install.sh to deploy."

# Helper target: verify no Cargo config overrides are active.
# Can be called independently in CI.
release-check-cargo-config:
	@# Scan every location Cargo merges: $CARGO_HOME plus all ancestor
	@# .cargo/config{,.toml} from the CWD up to the filesystem root.
	@CFG_FILES=""; \
	CARGO_HOME_DIR="$${CARGO_HOME:-$$HOME/.cargo}"; \
	for f in "$$CARGO_HOME_DIR/config.toml" "$$CARGO_HOME_DIR/config"; do \
	    [ -f "$$f" ] && CFG_FILES="$$CFG_FILES$$f "; \
	done; \
	d=$$(pwd); \
	while :; do \
	    for f in "$$d/.cargo/config.toml" "$$d/.cargo/config"; do \
	        [ -f "$$f" ] && CFG_FILES="$$CFG_FILES$$f "; \
	    done; \
	    case "$$d" in /) break ;; esac; \
	    d=$${d%/*}; [ -n "$$d" ] || d=/; \
	done; \
	FAIL=0; \
	for cfg in $$CFG_FILES; do \
	    if grep -qE '^[[:space:]]*target[[:space:]]*=' "$$cfg" 2>/dev/null; then \
	        echo "FAIL: $$cfg sets [build] target"; FAIL=1; \
	    fi; \
	    if grep -qE '^[[:space:]]*target[-_]dir[[:space:]]*=' "$$cfg" 2>/dev/null; then \
	        echo "FAIL: $$cfg sets [build] target-dir"; FAIL=1; \
	    fi; \
	done; \
	if [ "$$FAIL" -ne 0 ]; then exit 1; fi; \
	echo "OK: no Cargo config overrides detected"

# Write the embedded install script into the release staging directory.
# Invoked as a dependency of `release` (not called directly).
#
# The path variables below mirror the Makefile defaults. If the Makefile layout
# changes, update these definitions to prevent path drift.
#
# Generated install.sh features:
#   - PREFIX must be an absolute path (rejects relative paths)
#   - Preflight checks for path conflicts before any file writes
#   - Uses `cp -r` (not `cp -pr`) to avoid preserving source file ownership
#   - Deploys cosh extension (hooks/commands/cosh-extension.json)
#   - Safe toon symlink cleanup that preserves external symlinks
release-install-script:
	@mkdir -p dist/tokenless-$(VERSION)
	@echo '#!/usr/bin/env bash' > dist/tokenless-$(VERSION)/install.sh
	@echo 'set -euo pipefail' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'SCRIPT_DIR="$$(cd "$$(dirname "$$0")" && pwd)"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'VERSION="$(VERSION)"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'PREFIX="$${PREFIX:-/usr/local}"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# --- Preflight: PREFIX must be an absolute path ---' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'case "$$PREFIX" in' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    /*) ;;' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    *)  echo "ERROR: PREFIX must be an absolute path (got: $$PREFIX)"; exit 1 ;;' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'esac' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# --- Preflight: ensure BINDIR parent is a directory ---' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -e "$$PREFIX/bin" ] && [ ! -d "$$PREFIX/bin" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    echo "ERROR: $$PREFIX/bin exists but is not a directory"; exit 1' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# --- Preflight: rtk/toon entries must be replaceable by symlinks ---' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# ln -sf cannot overwrite a directory; reject before writing anything.' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'for entry in "$$PREFIX/bin/rtk" "$$PREFIX/bin/toon"; do' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    if [ -d "$$entry" ] && [ ! -L "$$entry" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        echo "ERROR: $$entry is a directory — remove it before installing."; exit 1' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'done' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# --- Preflight: an existing PREFIX must be writable ---' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -d "$$PREFIX" ] && [ ! -w "$$PREFIX" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    echo "ERROR: $$PREFIX is not writable — re-run with sudo or choose a writable PREFIX"; exit 1' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# Path layout — keep in sync with Makefile' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'BINDIR="$$PREFIX/bin"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'LIBEXECDIR="$$PREFIX/libexec/anolisa/tokenless"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'SHARE_DIR="$$PREFIX/share/anolisa/adapters/tokenless"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'COMPONENTS_DIR="$$PREFIX/share/anolisa/components/tokenless"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'COSH_EXT_DIR="$$PREFIX/share/anolisa/extensions/tokenless"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "==> tokenless $$VERSION installer"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "    Install prefix : $$PREFIX"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "    Binaries       : $$BINDIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "    Libexec        : $$LIBEXECDIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "    Adapters       : $$SHARE_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "    Cosh extension : $$COSH_EXT_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "    Note: this script will OVERWRITE rtk/toon symlinks in $$BINDIR,"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "          replace all files under $$SHARE_DIR,"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "          and replace the cosh extension at $$COSH_EXT_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "==> Installing tokenless $$VERSION from $$SCRIPT_DIR ..."' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo ""' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# --- Install binaries ---' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'install -d -m 0755 "$$BINDIR" "$$LIBEXECDIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'install -m 0755 "$$SCRIPT_DIR/bin/tokenless" "$$BINDIR/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'install -m 0755 "$$SCRIPT_DIR/libexec/rtk" "$$LIBEXECDIR/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# rtk symlink' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'TARGET="$$LIBEXECDIR/rtk"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -L "$$BINDIR/rtk" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    RTK_LINK=$$(readlink "$$BINDIR/rtk" || true)' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    case "$$RTK_LINK" in' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        *"$$PREFIX"*) ;;' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        *) echo "WARNING: $$BINDIR/rtk is an existing symlink pointing to $$RTK_LINK — will be overwritten" ;;' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    esac' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'elif [ -e "$$BINDIR/rtk" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    echo "WARNING: $$BINDIR/rtk already exists — will be overwritten"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'ln -sf "$$TARGET" "$$BINDIR/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# toon (conditional)' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -f "$$SCRIPT_DIR/libexec/toon" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    install -m 0755 "$$SCRIPT_DIR/libexec/toon" "$$LIBEXECDIR/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    TARGET="$$LIBEXECDIR/toon"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    if [ -L "$$BINDIR/toon" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        TOON_LINK=$$(readlink "$$BINDIR/toon" || true)' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        case "$$TOON_LINK" in' >> dist/tokenless-$(VERSION)/install.sh
	@echo '            *"$$PREFIX"*) ;;' >> dist/tokenless-$(VERSION)/install.sh
	@echo '            *) echo "WARNING: $$BINDIR/toon is an existing symlink pointing to $$TOON_LINK — will be overwritten" ;;' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        esac' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    elif [ -e "$$BINDIR/toon" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        echo "WARNING: $$BINDIR/toon already exists — will be overwritten"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    ln -sf "$$TARGET" "$$BINDIR/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'else' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    # Clean up toon from a previous install when this release does not bundle it.' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    if [ -e "$$LIBEXECDIR/toon" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        rm -f "$$LIBEXECDIR/toon"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    if [ -L "$$BINDIR/toon" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        TOON_LINK=$$(readlink "$$BINDIR/toon" 2>/dev/null || true)' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        TOON_REAL=$$(cd "$$BINDIR" && realpath -m "$$TOON_LINK" 2>/dev/null || echo "$$TOON_LINK")' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        TOON_EXPECT="$$(realpath -m "$$LIBEXECDIR/toon" 2>/dev/null || echo "$$LIBEXECDIR/toon")"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        if [ "$$TOON_REAL" = "$$TOON_EXPECT" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '            rm -f "$$BINDIR/toon"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '            echo "NOTE: removed $$BINDIR/toon symlink (no longer bundled)"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        else' >> dist/tokenless-$(VERSION)/install.sh
	@echo '            echo "NOTE: $$BINDIR/toon is an external symlink ($$TOON_LINK) — left untouched"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '        fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# --- Adapter resources ---' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# Use cp -r (NOT cp -pr) to avoid preserving source file ownership.' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -d "$$SHARE_DIR" ] && [ "$$(ls -A "$$SHARE_DIR" 2>/dev/null)" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    echo "WARNING: $$SHARE_DIR is not empty — all contents will be replaced"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'rm -rf "$$SHARE_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'install -d -m 0755 "$$SHARE_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'cp -r "$$SCRIPT_DIR/share/adapters/tokenless/." "$$SHARE_DIR/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'find "$$SHARE_DIR" -type f \( -name '\''*.py'\'' -o -name '\''*.sh'\'' \) -exec chmod 0755 {} +' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# --- Component contract ---' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -f "$$SCRIPT_DIR/share/component.toml" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    install -d -m 0755 "$$COMPONENTS_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    install -m 0644 "$$SCRIPT_DIR/share/component.toml" "$$COMPONENTS_DIR/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo '# --- Cosh extension ---' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -d "$$COSH_EXT_DIR" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    echo "NOTE: replacing existing cosh extension at $$COSH_EXT_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'rm -rf "$$COSH_EXT_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'install -d -m 0755 "$$COSH_EXT_DIR/hooks" "$$COSH_EXT_DIR/commands"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -d "$$SHARE_DIR/common/hooks" ] && [ "$$(ls -A "$$SHARE_DIR/common/hooks" 2>/dev/null)" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    cp -r "$$SHARE_DIR/common/hooks/." "$$COSH_EXT_DIR/hooks/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -d "$$SHARE_DIR/common/commands" ] && [ "$$(ls -A "$$SHARE_DIR/common/commands" 2>/dev/null)" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    cp -r "$$SHARE_DIR/common/commands/." "$$COSH_EXT_DIR/commands/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'if [ -f "$$SHARE_DIR/common/cosh-extension.json" ]; then' >> dist/tokenless-$(VERSION)/install.sh
	@echo '    install -m 0644 "$$SHARE_DIR/common/cosh-extension.json" "$$COSH_EXT_DIR/"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'fi' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'find "$$COSH_EXT_DIR" -type f \( -name '\''*.py'\'' -o -name '\''*.sh'\'' \) -exec chmod 0755 {} +' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "==> cosh extension installed to $$COSH_EXT_DIR"' >> dist/tokenless-$(VERSION)/install.sh
	@echo '' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo ""' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "==> Done. Run '\''tokenless --version'\'' to verify."' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "==> Cosh extension deployed to $$COSH_EXT_DIR."' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "==> If cosh does not scan $$PREFIX/share/anolisa/extensions by default,"' >> dist/tokenless-$(VERSION)/install.sh
	@echo 'echo "    you may need to configure cosh to include this extension path."' >> dist/tokenless-$(VERSION)/install.sh
	@chmod +x dist/tokenless-$(VERSION)/install.sh

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

# --- npm Packaging ---
# Package for current platform only
npm-package:
	@echo "==> Packaging anolisa-tokenless for npm (current platform)..."
	node npm/scripts/package-npm.js

# Package for all supported targets (requires cross-compilation toolchain)
npm-package-all:
	@echo "==> Packaging anolisa-tokenless for npm (all platforms)..."
	node npm/scripts/package-npm.js --all

# Publish all packages to npm registry
# IMPORTANT: Platform packages must be published BEFORE the root package,
# because the root package declares them as optionalDependencies.
# If the root package is published first and a platform package fails,
# npm will have a package referencing non-existent dependencies.
#
# The registry is pinned explicitly: the nested npm/dist/* package roots do
# NOT inherit npm/.npmrc, so without --registry a user-level registry
# override could silently win. Already-published name@version pairs are
# skipped, so a partially failed publish run can simply be re-executed
# (npm forbids republishing the same version).
NPM_REGISTRY ?= https://registry.npmjs.org/

npm-publish: npm-package-all
	@echo "==> Publishing anolisa-tokenless packages to $(NPM_REGISTRY) (platform packages first, root last)..."
	@set -e; \
	for pkg in tokenless-linux-x64 tokenless-linux-arm64 tokenless-darwin-x64 tokenless-darwin-arm64 tokenless; do \
		dir=npm/dist/$$pkg; \
		name=$$(node -p "require('./$$dir/package.json').name"); \
		ver=$$(node -p "require('./$$dir/package.json').version"); \
		if npm view "$$name@$$ver" version --registry=$(NPM_REGISTRY) >/dev/null 2>&1; then \
			echo "  $$name@$$ver already published — skipping"; \
		else \
			echo "  publishing $$name@$$ver..."; \
			(cd $$dir && npm publish --access public --registry=$(NPM_REGISTRY)); \
		fi; \
	done

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
	@echo "  release           Build all + package release tarball"
	@echo "  npm-package       Package for npm (current platform)"
	@echo "  npm-package-all   Package for npm (all platforms)"
	@echo "  npm-publish       Build and publish all npm packages"
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
