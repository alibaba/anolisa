# =============================================================================
# BUILD
# =============================================================================

LIBBPF_SYS_LIBRARY_PATH ?= $(shell pkg-config --variable=libdir libbpf 2>/dev/null || echo /usr/lib64:/usr/lib)
NPM_REGISTRY ?= https://registry.npmmirror.com
NPM_REPLACE_REGISTRY_HOST ?= always

.PHONY: build
build: ## Build agentsight binary
	env -u DESTDIR -u MAKEFLAGS -u MFLAGS -u MAKEOVERRIDES \
		LIBBPF_SYS_LIBRARY_PATH="$(LIBBPF_SYS_LIBRARY_PATH)" \
		cargo build --release

.PHONY: build-frontend
build-frontend: ## Build and embed frontend into frontend-dist/
	cd dashboard && npm install \
		--registry="$(NPM_REGISTRY)" \
		--replace-registry-host="$(NPM_REPLACE_REGISTRY_HOST)" \
		&& npm run build:embed

.PHONY: build-all
build-all: build-frontend build ## Build frontend then Rust binary (with embedded UI)

.PHONY: example
example: build ## Build C FFI example (requires libagentsight)
	gcc -o target/release/agentsight_example examples/agentsight_example.c \
		-Iinclude -Ltarget/release -lagentsight -lpthread -ldl -lm

# =============================================================================
# INSTALL
# =============================================================================

INSTALL_PROFILE ?= system
DESTDIR ?=
ifeq ($(INSTALL_PROFILE),user)
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
INSTALL_SYSTEMD ?= 0
else
PREFIX ?= /usr
BINDIR ?= /usr/local/bin
INSTALL_SYSTEMD ?= 1
endif
LIBDIR ?= $(PREFIX)/lib
SYSTEMD_SYSTEM_DIR ?= $(LIBDIR)/systemd/system
SERVICE_BINDIR ?= $(BINDIR)
SETCAP ?= auto

.PHONY: install uninstall
install: build-all ## Build and install agentsight binary and set BPF capabilities
	install -d -m 0755 $(DESTDIR)$(BINDIR)
	install -p -m 0755 target/release/agentsight $(DESTDIR)$(BINDIR)/
	@if [ "$(SETCAP)" = "1" ] || { [ "$(SETCAP)" = "auto" ] && [ "$(INSTALL_PROFILE)" = "system" ] && [ -z "$(DESTDIR)" ] && command -v setcap >/dev/null 2>&1; }; then \
		setcap cap_bpf,cap_perfmon=ep $(DESTDIR)$(BINDIR)/agentsight; \
	else \
		echo "Skipping setcap for $(DESTDIR)$(BINDIR)/agentsight"; \
	fi
	@if [ "$(INSTALL_SYSTEMD)" = "1" ]; then \
		install -d -m 0755 $(DESTDIR)$(SYSTEMD_SYSTEM_DIR); \
		install -p -m 0755 scripts/agentsight-start.sh $(DESTDIR)$(BINDIR)/agentsight-start; \
		sed 's|/usr/local/bin/agentsight-start|$(SERVICE_BINDIR)/agentsight-start|g' \
			scripts/agentsight.service > $(DESTDIR)$(SYSTEMD_SYSTEM_DIR)/agentsight.service; \
		chmod 0644 $(DESTDIR)$(SYSTEMD_SYSTEM_DIR)/agentsight.service; \
	fi

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/agentsight
	rm -f $(DESTDIR)$(BINDIR)/agentsight-start
	rm -f $(DESTDIR)$(SYSTEMD_SYSTEM_DIR)/agentsight.service

# =============================================================================
# TEST & QUALITY
# =============================================================================

.PHONY: test
test: ## Run unit tests (fast, no coverage)
	cd $(CURDIR) && cargo test

.PHONY: test-coverage
test-coverage: ## Run unit tests with coverage (same as CI)
	cd $(CURDIR) && cargo llvm-cov --cobertura --output-path coverage.xml \
		--ignore-filename-regex '(\.skel\.rs|target/debug/build|target/release/build|src/probes/)'

.PHONY: coverage
coverage: ## Generate HTML coverage report (opens browser)
	cd $(CURDIR) && cargo llvm-cov --html --output-dir coverage-html --open \
		--ignore-filename-regex '(\.skel\.rs|target/debug/build|target/release/build|src/probes/)'

.PHONY: help
help: ## Show this help message
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := build-all
