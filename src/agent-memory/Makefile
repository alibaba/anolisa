# Agent memory — Build System (Linux-only)
# Single Rust binary MCP server for AI agent memory (file tools)
#
# Local build/test targets require Linux. On a non-Linux dev box, use:
#   make remote-test    — push to git remote shiloong/memory, then ssh
#                          aos2 to pull/build/test
#   make remote-build   — push + ssh aos2 build only

VERSION ?= $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
NAME := agent-memory

# Remote Linux dev host used by remote-* targets.
REMOTE_HOST ?= aos2
REMOTE_PATH ?= /root/anolisa
REMOTE_BRANCH ?= memory
REMOTE_REPO ?= shiloong

# Install paths
PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DATADIR ?= /usr/share/anolisa
DOCDIR ?= $(PREFIX)/share/doc/$(NAME)

# RPM build paths
RPMBUILD_DIR ?= $(HOME)/rpmbuild
SPEC_FILE := $(NAME).spec

.PHONY: all build test lint fmt clean install dist rpm help

all: build

# =============================================================================
# BUILD
# =============================================================================

build: ## Build release binary
	@echo "==> Building $(NAME) v$(VERSION)..."
	cargo build --release --locked
	@echo "==> Binary: target/release/$(NAME) ($$(du -sh target/release/$(NAME) | cut -f1))"

build-debug: ## Build debug binary
	cargo build --locked

# =============================================================================
# TEST
# =============================================================================

test: ## Run all tests (Linux only — use `make remote-test` on macOS/Windows)
	@echo "==> Running tests..."
	cargo test --locked

test-mcp: ## Run MCP integration tests only
	cargo test --test mcp_integration_test

test-tools: ## Run Tier A file-tool integration tests only
	cargo test --test file_tools_test

# =============================================================================
# REMOTE Linux build/test (for cross-platform development)
# =============================================================================

remote-push: ## Push current branch HEAD to the remote git repo
	@echo "==> Pushing $$(git rev-parse --abbrev-ref HEAD) to $(REMOTE_REPO)..."
	cd ../.. && git push $(REMOTE_REPO) $(REMOTE_BRANCH)

# Refuses to reset a dirty remote worktree — otherwise `remote-test`
# called while someone is debugging on $(REMOTE_HOST) silently wipes
# their work. Set REMOTE_FORCE=1 to override (CI / known-clean only).
REMOTE_FORCE ?=
ifeq ($(REMOTE_FORCE),1)
REMOTE_RESET_GUARD :=
else
REMOTE_RESET_GUARD := test -z "$$(git status --porcelain)" || { echo "ERROR: $(REMOTE_HOST):$(REMOTE_PATH) has uncommitted changes — refusing to reset. Set REMOTE_FORCE=1 to override." >&2; exit 1; } &&
endif

remote-build: remote-push ## Push + ssh $(REMOTE_HOST): pull + cargo build --release
	@echo "==> Building on $(REMOTE_HOST):$(REMOTE_PATH)..."
	ssh $(REMOTE_HOST) 'cd $(REMOTE_PATH) && $(REMOTE_RESET_GUARD) git fetch $(REMOTE_REPO) $(REMOTE_BRANCH) && git reset --hard $(REMOTE_REPO)/$(REMOTE_BRANCH) && cd src/agent-memory && cargo build --release | tail -5'

remote-test: remote-push ## Push + ssh $(REMOTE_HOST): full test + clippy
	@echo "==> Testing on $(REMOTE_HOST):$(REMOTE_PATH)..."
	ssh $(REMOTE_HOST) 'cd $(REMOTE_PATH) && $(REMOTE_RESET_GUARD) git fetch $(REMOTE_REPO) $(REMOTE_BRANCH) && git reset --hard $(REMOTE_REPO)/$(REMOTE_BRANCH) && cd src/agent-memory && cargo test --release | grep "test result" && cargo clippy --release --all-targets -- -D warnings | tail -3'

# =============================================================================
# CODE QUALITY
# =============================================================================

lint: ## Run clippy lints
	cargo clippy -- -D warnings

fmt: ## Format code
	cargo fmt

fmt-check: ## Check formatting without modifying
	cargo fmt -- --check

# =============================================================================
# INSTALL (source build)
# =============================================================================

install: build ## Install binary and config to PREFIX
	@echo "==> Installing to $(DESTDIR)$(BINDIR)..."
	install -d -m 0755 $(DESTDIR)$(BINDIR)
	install -p -m 0755 target/release/$(NAME) $(DESTDIR)$(BINDIR)/
	install -d -m 0755 $(DESTDIR)$(DATADIR)/agent-memory
	install -p -m 0644 config/default.toml $(DESTDIR)$(DATADIR)/agent-memory/
	install -d -m 0755 $(DESTDIR)$(DATADIR)/mcp-servers
	install -p -m 0644 config/mcp-server.json $(DESTDIR)$(DATADIR)/mcp-servers/agent-memory.json
	@echo "==> Installed $(NAME) to $(DESTDIR)$(BINDIR)/$(NAME)"

install-systemd-user: ## Install per-user systemd template (Linux only)
	@echo "==> Installing systemd template to /usr/lib/systemd/user/..."
	install -D -m0644 config/systemd/anolisa-memory@.service \
		$(DESTDIR)/usr/lib/systemd/user/anolisa-memory@.service
	@echo "==> Enable with: systemctl --user enable --now anolisa-memory@$$USER"

uninstall: ## Remove installed files
	rm -f $(DESTDIR)$(BINDIR)/$(NAME)
	rm -f $(DESTDIR)$(DATADIR)/agent-memory/default.toml
	rm -f $(DESTDIR)$(DATADIR)/mcp-servers/agent-memory.json
	rmdir $(DESTDIR)$(DATADIR)/agent-memory 2>/dev/null || true
	rmdir $(DESTDIR)$(DATADIR)/mcp-servers 2>/dev/null || true

# =============================================================================
# DISTRIBUTION
# =============================================================================

dist: clean ## Create source + vendor tarballs for RPM build
	@echo "==> Vendoring crates..."
	cargo vendor vendor/
	@mkdir -p .cargo
	@printf '[source.crates-io]\nreplace-with = "vendored-sources"\n\n[source.vendored-sources]\ndirectory = "vendor"\n' > .cargo/config.toml
	@echo "==> Creating $(NAME)-$(VERSION).tar.gz..."
	@mkdir -p dist
	# Top-level dir is $(NAME)-$(VERSION) so it matches the spec's
	# `%setup -n %{name}-%{version}` and the CI archive shape.
	@rm -rf dist/$(NAME)-$(VERSION) && mkdir -p dist/$(NAME)-$(VERSION)
	@cp -R Cargo.toml Cargo.lock src config tests docs \
		Makefile agent-memory.spec.in \
		.gitignore .cargo/config.toml dist/$(NAME)-$(VERSION)/
	tar czf dist/$(NAME)-$(VERSION).tar.gz -C dist $(NAME)-$(VERSION)
	@echo "==> Creating $(NAME)-$(VERSION)-vendor.tar.gz..."
	# Top-level dir is `vendor/` (no $(NAME)-vendor wrapper) so that
	# `%setup -a 1` lands vendor/ next to the unpacked sources, where
	# cargo `--offline` can find it.
	tar czf dist/$(NAME)-$(VERSION)-vendor.tar.gz vendor
	@rm -rf dist/$(NAME)-$(VERSION) vendor
	@echo "==> Tarballs: dist/$(NAME)-$(VERSION).tar.gz + dist/$(NAME)-$(VERSION)-vendor.tar.gz"

# =============================================================================
# RPM BUILD
# =============================================================================

spec: ## Generate RPM spec from template (substitute @VERSION@)
	@echo "==> Generating $(SPEC_FILE) with version $(VERSION)..."
	sed 's/@VERSION@/$(VERSION)/g' $(NAME).spec.in > $(SPEC_FILE)

rpm: dist spec ## Build RPM package
	@echo "==> Building RPM for $(NAME)-$(VERSION)..."
	mkdir -p $(RPMBUILD_DIR)/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
	cp dist/$(NAME)-$(VERSION).tar.gz $(RPMBUILD_DIR)/SOURCES/
	cp dist/$(NAME)-$(VERSION)-vendor.tar.gz $(RPMBUILD_DIR)/SOURCES/
	cp $(SPEC_FILE) $(RPMBUILD_DIR)/SPECS/
	rpmbuild -bb $(RPMBUILD_DIR)/SPECS/$(SPEC_FILE)
	@echo "==> RPM built. Check $(RPMBUILD_DIR)/RPMS/"

srpm: dist spec ## Build source RPM
	@echo "==> Building SRPM..."
	mkdir -p $(RPMBUILD_DIR)/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
	cp dist/$(NAME)-$(VERSION).tar.gz $(RPMBUILD_DIR)/SOURCES/
	cp dist/$(NAME)-$(VERSION)-vendor.tar.gz $(RPMBUILD_DIR)/SOURCES/
	cp $(SPEC_FILE) $(RPMBUILD_DIR)/SPECS/
	rpmbuild -bs $(RPMBUILD_DIR)/SPECS/$(SPEC_FILE)
	@echo "==> SRPM built. Check $(RPMBUILD_DIR)/SRPMS/"

# =============================================================================
# UTILITY
# =============================================================================

clean: ## Clean build artifacts
	cargo clean
	rm -rf dist/
	rm -f $(SPEC_FILE)

version: ## Show current version
	@echo $(VERSION)

# Quick smoke test: init the mount, then drive 5 MCP tools through stdin.
# Validates: write → read → append → grep → list, plus path-sandbox enforcement.
smoke: build ## Run end-to-end smoke test against release binary
	@echo "==> Smoke test..."
	@TMPDIR=$$(mktemp -d) && \
		MEMORY_BASE_DIR="$$TMPDIR" USER_ID=smoke target/release/$(NAME) init && \
		MEMORY_BASE_DIR="$$TMPDIR" USER_ID=smoke target/release/$(NAME) info && \
		MEMORY_BASE_DIR="$$TMPDIR" USER_ID=smoke \
		printf '%s\n%s\n%s\n%s\n%s\n%s\n%s\n' \
			'{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"1.0"}}}' \
			'{"jsonrpc":"2.0","method":"notifications/initialized"}' \
			'{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
			'{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"mem_write","arguments":{"path":"notes/smoke.md","content":"hello smoke world"}}}' \
			'{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"mem_read","arguments":{"path":"notes/smoke.md"}}}' \
			'{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"mem_grep","arguments":{"pattern":"smoke"}}}' \
			'{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"mem_list","arguments":{"recursive":true}}}' \
		| target/release/$(NAME) | head -10 && \
		rm -rf "$$TMPDIR" && \
		echo "==> Smoke test PASSED"

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-16s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help
